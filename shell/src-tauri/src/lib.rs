//! bellwether's shell: the strip of sheep. A pure client of the brain:
//! it renders, does click-through, and owns the per-OS window work. It knows nothing
//! about hooks.

mod brain;
mod clickthrough;
mod ax;
mod dock;
mod locator;
mod log;
mod placement;
mod privacy;
mod signals;
mod settings;
mod strips;
mod tray;

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Manager, RunEvent};

use brain::Brain;
use clickthrough::{Hitboxes, Rect};
use log::log;
use settings::Settings;

const MAX_PET_BYTES: u64 = 16 * 1024 * 1024;
const PET_LOCK: &str = include_str!("../../../pets.lock.json");

/// Checks a pet file against its pinned sha256 from pets.lock.json.
fn verify_pet(folder: &str, bytes: &[u8]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let lock: Value = serde_json::from_str(PET_LOCK).map_err(|e| format!("pets.lock.json: {e}"))?;
    let want = lock
        .pointer(&format!("/pets/{folder}/sha256"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{folder} is not a pinned pet (pets.lock.json); only pinned pets are loaded"))?;
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err(format!("data/pets/{folder}/animations.xml doesn't match its pinned sha256 (expected {want}, got {got}); run `bellwether pets fetch`"));
    }
    Ok(())
}

/// Pet files never need a DTD; refusing DOCTYPE and ENTITY outright rules out entity
/// expansion ("billion laughs") and external entities before any parser sees the file.
fn reject_doctype(xml: &str) -> Result<(), String> {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return Err("the pet file declares a DOCTYPE or ENTITY, which pet files never need".into());
    }
    Ok(())
}
const MAX_HITBOXES: usize = 512;

/// Where each sheep lives (Milestone 8): kept by the placement thread, read by
/// `get_snapshot`.
struct Places {
    primary: String,
    placed: std::collections::HashMap<String, placement::Placed>,
    book: std::collections::HashMap<String, (String, i64)>,
}

struct Shared {
    agent_dir: PathBuf,
    data_dir: PathBuf,
    config: Value,
    selftest: bool,
    /// Debug only: where `--snapshot` writes the strip's canvas. None in normal use,
    /// and then `debug_snapshot` refuses.
    snapshot: Option<(String, f64)>,
    /// Debug only: also snapshot every N seconds (--snapshot-every N).
    snapshot_every: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PetPayload {
    folder: String,
    xml: String,
    animation_map: Value,
    display: Value,
}

/// The pet's animations.xml from data/pets/<folder>/, plus what the webview needs to
/// draw it. The folder name comes from config and must be a plain name: no path.
#[tauri::command]
fn get_pet(shared: tauri::State<'_, Shared>) -> Result<PetPayload, String> {
    let folder = shared.config.pointer("/pets/default").and_then(Value::as_str).unwrap_or("esheep64").to_string();
    if folder.is_empty() || !folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(format!("pets.default {folder:?} must be a plain folder name"));
    }
    let path = shared.data_dir.join("pets").join(&folder).join("animations.xml");
    let meta = std::fs::metadata(&path).map_err(|_| format!("{} is missing: run `bellwether pets fetch`", path.display()))?;
    if meta.len() > MAX_PET_BYTES {
        return Err(format!("{} is larger than {} MB", path.display(), MAX_PET_BYTES / 1024 / 1024));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // The same pins `bellwether pets fetch` uses (pets.lock.json, compiled in): a pet that
    // isn't pinned, or whose file doesn't match, is refused.
    verify_pet(&folder, &bytes).map_err(|e| {
        log(&format!("error: refusing to load pet {folder}: {e}"));
        e
    })?;
    let xml = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    reject_doctype(&xml).map_err(|e| {
        log(&format!("error: refusing to load pet {folder}: {e}"));
        e
    })?;
    let animation_map = shared.config.pointer(&format!("/pets/animationMap/{folder}")).cloned().unwrap_or(Value::Null);
    let num = |p: &str, d: f64| shared.config.pointer(p).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d);
    let palette = sheep_palette(shared.config.pointer("/pets/palette"));
    let display = serde_json::json!({
        "palette": palette,
        "scale": num("/pets/scale", 1.5),
        "lambScale": num("/pets/lambScale", 0.6),
        // "horns" (default): only the horns take the session colour; "whole": the old tint.
        "tint": match shared.config.pointer("/pets/tint").and_then(Value::as_str) { Some("whole") => "whole", _ => "horns" },
        "slotWidthPx": num("/strip/slotWidthPx", 110.0),
        "homeWanderPx": num("/strip/homeWanderPx", 120.0),
        "fadeOpacity": shared.config.pointer("/fade/opacity").and_then(Value::as_f64).filter(|v| (0.0..=1.0).contains(v)).unwrap_or(0.25),
        "selftest": shared.selftest,
        "trace": crate::settings::env_os("SHELL_TRACE").is_some(),
        "snapshotAfterMs": shared.snapshot.as_ref().map(|(_, secs)| secs * 1000.0),
        "snapshotEveryMs": shared.snapshot.as_ref().and(shared.snapshot_every).map(|secs| secs * 1000.0),
    });
    Ok(PetPayload { folder, xml, animation_map, display })
}

/// Long-poll for the next snapshot (rev > after), at most 25 s.
#[tauri::command]
async fn get_snapshot(
    after: i64,
    shell_after: Option<i64>,
    window: tauri::WebviewWindow,
    brain: tauri::State<'_, Arc<Brain>>,
    places: tauri::State<'_, Arc<std::sync::Mutex<Places>>>,
    strips: tauri::State<'_, strips::Strips>,
) -> Result<Option<Value>, String> {
    let brain = brain.inner().clone();
    let b2 = brain.clone();
    let snap = tauri::async_runtime::spawn_blocking(move || brain.wait_for(after, shell_after.unwrap_or(i64::MAX), Duration::from_secs(25)))
        .await
        .map_err(|e| e.to_string())?;
    // Milestone 8: each strip window sees only the sheep living on its monitor.
    let brain_view = b2.view();
    let snap = snap.map(|s| {
        let p = places.lock().unwrap();
        let mine = strips.monitor_of(window.label()).unwrap_or_else(|| p.primary.clone());
        let open: std::collections::HashSet<String> = strips.by_monitor.lock().unwrap().keys().cloned().collect();
        let mut v = placement::view_for(&s, &mine, &p.primary, &p.placed, &open, &p.book);
        if v.get("shell").is_some() {
            v["shell"] = brain_view.json_for(window.label());
        }
        v
    });
    if crate::settings::env_os("SHELL_TRACE").is_some() {
        let rev = snap.as_ref().and_then(|s| s.get("rev")).and_then(Value::as_i64).unwrap_or(-1);
        log(&format!("get_snapshot(after {after}) -> rev {rev}"));
    }
    Ok(snap)
}

/// Where this window's sprites are, in the window's logical pixels.
#[tauri::command]
fn report_hitboxes(window: tauri::WebviewWindow, boxes: Vec<Rect>, hitboxes: tauri::State<'_, Hitboxes>) {
    let boxes: Vec<Rect> = boxes.into_iter().filter(|r| [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0).take(MAX_HITBOXES).collect();
    if crate::settings::env_os("SHELL_TRACE").is_some() {
        let list: Vec<String> = boxes.iter().map(|r| format!("{:.0},{:.0} {:.0}x{:.0}", r.x, r.y, r.w, r.h)).collect();
        log(&format!("hitboxes {}: {}", window.label(), list.join(" | ")));
    }
    hitboxes.by_window.lock().unwrap().insert(window.label().to_string(), boxes);
}

/// A sheep (or lamb) was clicked: the webview plays the hop, and the shell brings that
/// session's terminal to the front (Milestone 8), on its own thread.
#[tauri::command]
fn sprite_clicked(id: String, brain: tauri::State<'_, Arc<Brain>>, locator: tauri::State<'_, Arc<locator::Locator>>) {
    let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ':' || *c == '.' || *c == '_' || *c == '@').take(128).collect();
    log(&format!("sprite clicked: {id}"));
    let Some(snap) = brain.current() else { return };
    // A lamb's click goes to its parent's terminal.
    let terminal = snap.get("flock").and_then(Value::as_array).and_then(|f| {
        f.iter()
            .find(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str()) || s.get("lambs").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.get("id").and_then(Value::as_str) == Some(id.as_str()))))
            .map(|s| locator::Terminal::from_json(s.get("terminal").unwrap_or(&Value::Null)))
    });
    let Some(terminal) = terminal else { return };
    // One focus at a time: rapid clicks while one is under way are dropped.
    static FOCUSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if FOCUSING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        log(&format!("click on {id}: ignored, still bringing the last one forward"));
        return;
    }
    /// Clears FOCUSING when dropped, so even a panic in `focus` can't leave clicks
    /// ignored until a restart.
    struct Focusing;
    impl Drop for Focusing {
        fn drop(&mut self) {
            FOCUSING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let guard = Focusing;
    let locator = locator.inner().clone();
    std::thread::spawn(move || {
        let _guard = guard;
        let result = locator.focus(&terminal);
        log(&format!("click on {id}: {result:?}"));
    });
}

/// One line from the webview into the shell's log (pet warnings, sheep arriving).
#[tauri::command]
fn shell_log(line: String, app: tauri::AppHandle, shared: tauri::State<'_, Shared>) {
    let line: String = line.chars().filter(|c| !c.is_control()).take(500).collect();
    log(&format!("webview: {line}"));
    if shared.selftest {
        println!("{line}");
        if line.starts_with("selftest: done") {
            app.exit(if line.contains("FAIL") { 1 } else { 0 });
        }
    }
}

/// Debug only (`--snapshot <png>`): writes what the strip's canvas shows, as a PNG
/// with its transparency. `screencapture` would need the Screen Recording permission,
/// which bellwether never asks for, and on macOS 26 an in-process CoreGraphics capture
/// returns an empty image without it (measured), so the webview hands over its own
/// pixels instead. Refused unless the shell was started with --snapshot.
#[tauri::command]
fn debug_snapshot(png: String, suffix: String, shared: tauri::State<'_, Shared>) -> Result<String, String> {
    let Some((path, _)) = shared.snapshot.as_ref() else { return Err("not started with --snapshot".into()) };
    let suffix: String = suffix.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(32).collect();
    let target = if suffix.is_empty() { path.clone() } else { path.replace(".png", &format!("-{suffix}.png")) };
    let bytes = decode_base64(png.strip_prefix("data:image/png;base64,").unwrap_or(&png)).ok_or("not base64")?;
    if bytes.len() > 64 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG") {
        return Err("not a PNG".into());
    }
    std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
    log(&format!("snapshot written: {target} ({} bytes)", bytes.len()));
    Ok(target)
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

struct Flags {
    snapshot: Option<String>,
    snapshot_after: f64,
    snapshot_every: Option<f64>,
    quit_after: Option<f64>,
    spawn_brain: bool,
    selftest: bool,
    demo: bool,
    /// Debug: locate one terminal (the snapshot's `terminal` object as JSON), print
    /// the result and quit. Opens no window.
    locate: Option<String>,
}

fn flags() -> Flags {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    Flags {
        snapshot: value("--snapshot"),
        snapshot_after: value("--snapshot-after").and_then(|v| v.parse().ok()).unwrap_or(6.0),
        snapshot_every: value("--snapshot-every").and_then(|v| v.parse::<f64>().ok()).filter(|v| *v >= 0.5),
        quit_after: value("--quit-after").and_then(|v| v.parse().ok()),
        spawn_brain: !args.iter().any(|a| a == "--no-spawn-brain"),
        selftest: args.iter().any(|a| a == "--selftest"),
        demo: args.iter().any(|a| a == "--demo"),
        locate: value("--locate"),
    }
}

pub fn run() {
    // `bellwether shell dock-permission`: ask macOS for Accessibility for this very binary
    // (the one the LaunchAgent runs), then quit. Only ever on the user's request.
    if std::env::args().any(|a| a == "--ax-prompt") {
        let granted = ax::prompt();
        println!("{}", if granted {
            "Accessibility is already granted to this binary: sheep stand on the Dock's top edge (restart the strip if it was just granted: `bellwether shell restart`)"
        } else {
            "macOS is showing its Accessibility prompt for bellwether-shell. Turn it on in System Settings > Privacy & Security > Accessibility, then `bellwether shell restart`. After every rebuild of the shell this must be given again (the binary is ad-hoc signed)."
        });
        std::process::exit(0);
    }
    let flags = flags();
    let settings = match Settings::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bellwether-shell: {e}");
            std::process::exit(3);
        }
    };
    log::init(settings.data_dir.join("shell.log"));
    log(&format!("starting: agent dir {}, brain on 127.0.0.1:{}", settings.agent_dir.display(), settings.port));

    let brain = Brain::new();
    let strip_height = settings.f64_at("/strip/heightPx", 140.0);
    let brain_cfg = brain::BrainConfig {
        port: settings.port,
        token: settings.token.clone(),
        node_path: settings.node_path.clone(),
        agent_dir: settings.agent_dir.clone(),
        data_dir: settings.data_dir.clone(),
        spawn: flags.spawn_brain && !flags.selftest,
    };
    let shared = Shared {
        agent_dir: settings.agent_dir.clone(),
        data_dir: settings.data_dir.clone(),
        config: settings.config.clone(),
        selftest: flags.selftest,
        snapshot: flags.snapshot.clone().map(|p| (p, flags.snapshot_after)),
        snapshot_every: flags.snapshot_every,
    };
    let _ = &shared.agent_dir;
    let capture_exclusion = settings.config.pointer("/privacy/captureExclusion").and_then(Value::as_bool).unwrap_or(true);
    let hide_when_sharing = settings.config.pointer("/privacy/hideWhenSharing").and_then(Value::as_bool).unwrap_or(true);
    let sharing_names = if hide_when_sharing { privacy::wanted_names(&settings.config) } else { Vec::new() };
    log(&format!(
        "privacy: capture exclusion {}, hide while sharing {} ({})",
        if capture_exclusion { "on" } else { "off" },
        if hide_when_sharing { "on" } else { "off" },
        if sharing_names.is_empty() { "no process names".to_string() } else { sharing_names.join(", ") }
    ));
    for w in privacy::sharing_warnings(&settings.config) {
        log(&format!("warning: {w}; the strip won't hide for it while you share"));
    }
    let fade_radius = settings.f64_at("/fade/radiusPx", 90.0);
    let locator = Arc::new(make_locator(&settings.config));
    let relocate = Duration::from_secs_f64(settings.f64_at("/strip/relocateSec", 60.0));
    let (data_dir, port, token) = (settings.data_dir.clone(), settings.port, settings.token.clone());
    let token_for_places = settings.token.clone();

    let brain_for_setup = brain.clone();
    let mut builder = tauri::Builder::default();
    // One strip at a time. Only `--locate` skips this: it opens no window and quits
    // after one lookup. `--selftest` keeps it, because it does open a window (the pet is
    // parsed by the webview's own XML parser), so it is refused while a strip runs.
    if flags.locate.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {
            log("another bellwether shell tried to start; this one keeps running");
        }));
    }
    let app = builder
        .manage(brain.clone())
        .manage(Hitboxes::default())
        .manage(strips::Strips::default())
        .manage(locator.clone())
        .manage(Arc::new(std::sync::Mutex::new(Places { primary: String::new(), placed: Default::default(), book: Default::default() })))
        .manage(shared)
        .invoke_handler(tauri::generate_handler![get_pet, get_snapshot, report_hitboxes, sprite_clicked, shell_log, debug_snapshot])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            let monitor = app.primary_monitor()?.ok_or("no primary monitor")?;
            app.state::<Arc<std::sync::Mutex<Places>>>().lock().unwrap().primary = strips::monitor_name(&monitor);
            if let Some(json) = &flags.locate {
                // --locate: one lookup, printed; no window, no tray, no brain.
                let t = locator::Terminal::from_json(&serde_json::from_str(json).unwrap_or(Value::Null));
                let started = std::time::Instant::now();
                let found = locator.locate(&t);
                let mons = monitors_of(&handle);
                let primary = strips::monitor_name(&monitor);
                println!("locate: {found:?}");
                let d = dock::read_prefs();
                let r = strips::strip_rect(&monitor, strip_height, d);
                println!("dock: autohide {}, {:?}; the strip would be x {} y {} {}x{}", d.autohide, d.side, r.x, r.y, r.width, r.height);
                println!("monitor: {} (of {:?}) in {} ms", placement::monitor_of(&found, &mons, &primary), mons.iter().map(|m| &m.name).collect::<Vec<_>>(), started.elapsed().as_millis());
                handle.exit(0);
                return Ok(());
            }
            // Milestone 7: the tray, and the thread that hides the strip while sharing
            // or when the tray says so. The tray switches start as they were left, so a
            // strip hidden from the tray opens hidden (never flashing up first).
            let mut kept = privacy::load_kept(&data_dir);
            app.manage(tray::Switches { kept: std::sync::Mutex::new(kept), data_dir: data_dir.clone(), port, token: token.clone() });
            if !flags.selftest {
                if let Err(e) = tray::build(&handle, kept, false) {
                    log(&format!("warning: no tray icon ({e}); use `bellwether mute` / `bellwether show` instead"));
                    if kept.hidden {
                        // Fail open: with no tray, a strip saved hidden would be lost.
                        log("ERROR: the strip was saved hidden and there is no tray to show it again; showing it (`bellwether hide` hides it)");
                        kept.hidden = false;
                        *app.state::<tray::Switches>().kept.lock().unwrap() = kept;
                        let _ = privacy::save_kept(&data_dir, kept);
                    }
                }
            }
            brain_for_setup.update_view(|v| v.show_labels = kept.show_labels);
            let index = app.state::<strips::Strips>().next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            strips::open(&handle, &monitor, strip_height, index, capture_exclusion, kept.hidden)?;
            if !flags.demo && !flags.selftest {
                let (h, b, l) = (handle.clone(), brain_for_setup.clone(), locator.clone());
                std::thread::spawn(move || placement_loop(h, b, l, relocate, strip_height, capture_exclusion, port, token_for_places));
            }
            let (h, b) = (handle.clone(), brain_for_setup.clone());
            std::thread::spawn(move || privacy_loop(h, b, sharing_names, strip_height));

            let b = brain_for_setup.clone();
            if flags.demo {
                // Debug: one sheep per behaviour, without a brain, to check the
                // animation map by eye. No hooks, no brain, nothing persisted.
                log("--demo: showing a synthetic flock; the brain is not contacted");
                b.set_demo(demo_snapshot());
            } else {
                std::thread::spawn(move || brain::run(b, brain_cfg));
            }
            let h = handle.clone();
            std::thread::spawn(move || clickthrough::run(h, fade_radius));

            if flags.selftest {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(30));
                    println!("selftest: done FAIL (no answer from the webview in 30 s)");
                    h.exit(1);
                });
            }
            if let Some(secs) = flags.quit_after {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs_f64(secs));
                    log("--quit-after reached; quitting");
                    h.exit(0);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the bellwether shell");

    // SIGINT (Ctrl-C), SIGTERM and SIGHUP go through the same shutdown as a normal
    // quit, so the brain the shell started is stopped (verification M3 verification D1).
    signals::install();
    let handle = app.handle().clone();
    let brain_for_signals = brain.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(sig) = signals::received() {
            log(&format!("received signal {sig}; quitting"));
            brain_for_signals.stop_child();
            handle.exit(0);
            return;
        }
    });

    app.run(move |_handle, event| {
        if let RunEvent::Exit = event {
            brain.stop_child();
            log("quit");
        }
    });
}

/**
 * Every 250 ms: whether the strip should be hidden (the tray's Hide; a sharing process,
 * read every 500 ms, debounced 2 s on the way back), applied to every strip window,
 * and the tray's Mute check kept in step with the brain.
 */
fn privacy_loop(app: tauri::AppHandle, brain: Arc<Brain>, sharing_names: Vec<String>, strip_height: f64) {
    use std::time::Instant;
    let mut debounce = privacy::Debounce::new();
    let mut sharing: Option<String> = None;
    let mut processes = privacy::ProcessNames::default();
    let mut last_poll = Instant::now() - privacy::SHARING_POLL;
    let mut shown: Option<bool> = None;
    let mut synced: Option<(bool, privacy::Kept)> = None;
    let data_dir = app.state::<tray::Switches>().data_dir.clone();
    let mut stamp = privacy::kept_stamp(&data_dir);
    // The Dock (auto-hide, side): read every 2 s; strips move when it changes, and are
    // checked against their monitor anyway every 10 s (a work area can change too).
    let mut dock = dock::read_prefs();
    let mut last_dock_check = Instant::now();
    let mut last_follow = Instant::now();
    // Dock-width confinement: only with Accessibility already granted (never prompted
    // from here). Re-checked every 10 s, so a grant made while running is picked up.
    let mut trusted = ax::trusted();
    log(&if trusted {
        "dock: Accessibility granted to this binary: sheep stand on the Dock's top edge, within its width".to_string()
    } else {
        "dock: Accessibility not granted to this binary: the full-width strip is used (`bellwether shell dock-permission` asks for it)".to_string()
    });
    let mut last_trust_check = Instant::now();
    let mut steady = ax::Steady::default();
    // The AX reader: its own thread, every 2 s, only while trusted (short AX timeouts).
    let dock_seen: Arc<std::sync::Mutex<Option<dock::Frame>>> = Arc::default();
    {
        let seen = dock_seen.clone();
        std::thread::spawn(move || loop {
            let (trusted, dock) = (ax::trusted(), dock::read_prefs());
            let frame = if trusted && !dock.autohide && dock.side == dock::Side::Bottom { ax::dock_pid().and_then(ax::dock_frame) } else { None };
            *seen.lock().unwrap() = frame;
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
    }
    let mut last_stamp_check = Instant::now();
    loop {
        let now = Instant::now();
        // `bellwether show` / `bellwether hide` (or the file edited by hand) while running.
        if now.duration_since(last_stamp_check) >= std::time::Duration::from_secs(1) {
            last_stamp_check = now;
            let s = privacy::kept_stamp(&data_dir);
            if s != stamp {
                stamp = s;
                let fresh = privacy::load_kept(&data_dir);
                let switches = app.state::<tray::Switches>();
                let mut k = switches.kept.lock().unwrap();
                if *k != fresh {
                    log(&format!("shell-state.json changed: hide {} labels {}", fresh.hidden, fresh.show_labels));
                    *k = fresh;
                    brain.update_view(|v| v.show_labels = fresh.show_labels);
                }
            }
        }
        if !sharing_names.is_empty() && now.duration_since(last_poll) >= privacy::SHARING_POLL {
            last_poll = now;
            let found = privacy::sharing_match(&processes.read(now), &sharing_names);
            if found != sharing {
                match &found {
                    Some(name) => log(&format!("privacy: {name} is running (screen sharing); hiding the strip")),
                    None => log("privacy: the sharing process has gone; the strip shows again in 2 s"),
                }
                sharing = found;
            }
        }
        if now.duration_since(last_trust_check) >= std::time::Duration::from_secs(10) {
            last_trust_check = now;
            let t = ax::trusted();
            if t != trusted {
                log(&format!("dock: Accessibility {} while running", if t { "granted" } else { "withdrawn" }));
                trusted = t;
            }
        }
        if now.duration_since(last_dock_check) >= std::time::Duration::from_secs(2) {
            last_dock_check = now;
            // The Dock's frame, as the AX reader thread last saw it (this loop never
            // waits on Accessibility: it hides the strip while sharing).
            let frame = if trusted && !dock.autohide && dock.side == dock::Side::Bottom { *dock_seen.lock().unwrap() } else { None };
            steady.feed(frame);
            let mut extents = Vec::new();
            for (_, label) in app.state::<strips::Strips>().by_monitor.lock().unwrap().clone() {
                let Some(w) = app.get_webview_window(&label) else { continue };
                let (Ok(p), Ok(sz), Ok(sc)) = (w.outer_position(), w.outer_size(), w.scale_factor()) else { continue };
                let strip = dock::Frame { x: p.x as f64 / sc, y: p.y as f64 / sc, w: sz.width as f64 / sc, h: sz.height as f64 / sc };
                if let Some((lo, hi)) = ax::dock_extent(strip, steady.accepted, dock) {
                    extents.push((label, lo, hi));
                }
            }
            if brain.view().dock != extents {
                log(&format!("dock: {}", if extents.is_empty() { "sheep use the full strip".to_string() } else { format!("sheep stand on the Dock: {:?}", extents) }));
                brain.update_view(|v| v.dock = extents);
            }
            let fresh = dock::read_prefs();
            if fresh != dock || now.duration_since(last_follow) >= std::time::Duration::from_secs(10) {
                if fresh != dock {
                    log(&format!("dock settings changed: autohide {}, {:?}", fresh.autohide, fresh.side));
                }
                dock = fresh;
                last_follow = now;
                let (h, height) = (app.clone(), strip_height);
                let _ = app.run_on_main_thread(move || strips::follow_dock(&h, height, fresh));
            }
        }
        let kept = *app.state::<tray::Switches>().kept.lock().unwrap();
        let hidden = debounce.update(sharing.is_some(), now) || kept.hidden;
        if shown != Some(!hidden) {
            for (_, w) in tauri::Manager::webview_windows(&app) {
                let _ = if hidden { w.hide() } else { w.show() };
            }
            log(&format!(
                "strip {}{}",
                if hidden { "hidden" } else { "shown" },
                if kept.hidden {
                    " (Hide the strip is on; to show it again: the tray's Hide the strip, or `bellwether show`)"
                } else if hidden {
                    " (sharing)"
                } else {
                    ""
                }
            ));
            shown = Some(!hidden);
            brain.update_view(|v| v.hidden = hidden);
        }
        let muted = brain.muted();
        if synced != Some((muted, kept)) {
            tray::sync(&app, muted, kept);
            synced = Some((muted, kept));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Session colours (on the horns by default): the eight brain/defaults.mjs explains, in the
/// same order. Keep the two lists the same.
pub const DEFAULT_PALETTE: [&str; 8] = ["#FF8C00", "#1F5BFF", "#202020", "#E8202A", "#00C2D4", "#8B4513", "#12A84A", "#FF4FB0"];
/// The old default (untinted white first): a config.json copied from the old example
/// still has it verbatim, and that means "the default", so the new one is used.
const OLD_DEFAULT_PALETTE: [&str; 8] = ["#ffffff", "#7fb3ff", "#ffb36b", "#9be37d", "#f28bd0", "#c9a2ff", "#ffe066", "#ff7b7b"];

fn sheep_palette(configured: Option<&Value>) -> Value {
    let new = || serde_json::json!(DEFAULT_PALETTE);
    let Some(list) = configured.and_then(Value::as_array).filter(|a| !a.is_empty()) else { return new() };
    let old = list.len() == OLD_DEFAULT_PALETTE.len() && list.iter().zip(OLD_DEFAULT_PALETTE).all(|(v, o)| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(o)));
    if old {
        new()
    } else {
        Value::Array(list.clone())
    }
}

/// The locator from config: `locator.strategies`, `locator.herdrPath`,
/// `locator.appleScript` (false: never send AppleScript, so never ask for Automation).
fn make_locator(config: &Value) -> locator::Locator {
    let strategies: Vec<String> = config
        .pointer("/locator/strategies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_else(|| ["herdr", "iterm2", "terminal-app", "vscode", "process-tree", "app-activate"].iter().map(|s| s.to_string()).collect());
    let herdr_path = config.pointer("/locator/herdrPath").and_then(Value::as_str).unwrap_or("herdr").to_string();
    // A bare name is looked up where the CLI installs by default too: the shell may be
    // started without the user's PATH (a login item, Finder).
    let herdr_path = if herdr_path.contains('/') {
        herdr_path
    } else {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/bin").join(&herdr_path))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(herdr_path)
    };
    let apple_script = config.pointer("/locator/appleScript").and_then(Value::as_bool).unwrap_or(true);
    log(&format!("locator: {} (herdr at {herdr_path}; AppleScript {})", strategies.join(" > "), if apple_script { "on: iTerm2 and Terminal may ask for Automation permission" } else { "off" }));
    locator::Locator::new(
        Arc::new(locator::RealRunner),
        Arc::new(locator::RealWindows),
        locator::Settings { strategies, herdr_path, apple_script, uid: unsafe { libc::getuid() } },
        Box::new(|m| log(m)),
    )
}

/// At most this many terminals are looked up at once (scoped threads, per pass).
const LOCATE_WORKERS: usize = 4;

/// Locates each sheep's terminal, `LOCATE_WORKERS` at a time; results in input order.
fn locate_all(locator: &locator::Locator, todo: &[&placement::Sheep]) -> Vec<locator::Located> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let out: std::sync::Mutex<Vec<Option<locator::Located>>> = std::sync::Mutex::new(vec![None; todo.len()]);
    std::thread::scope(|scope| {
        for _ in 0..LOCATE_WORKERS.min(todo.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(s) = todo.get(i) else { break };
                let found = locator.locate(&locator::Terminal::from_json(&s.terminal));
                out.lock().unwrap()[i] = Some(found);
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|f| f.unwrap_or_else(|| locator::Located { strategy: "none".into(), bounds: None, confidence: locator::Confidence::None })).collect()
}

/// The monitors, read on the main thread: from any other thread macOS reports none
/// (measured: an empty list from the placement thread).
fn monitors_main(app: &tauri::AppHandle) -> Vec<locator::Monitor> {
    let (tx, rx) = std::sync::mpsc::channel();
    let h = app.clone();
    if app.run_on_main_thread(move || {
        if crate::settings::env_os("SHELL_TRACE").is_some() {
            let a = h.available_monitors();
            let w = h.webview_windows().into_values().next().map(|w| w.available_monitors().map(|v| v.len()));
            log(&format!("trace: available_monitors app {:?} window {:?} primary {:?}", a.as_ref().map(|v| v.len()), w, h.primary_monitor().map(|m| m.map(|m| strips::monitor_name(&m)))));
        }
        let _ = tx.send(monitors_of(&h));
    }).is_err() {
        return Vec::new();
    }
    rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default()
}

/// The connected monitors, in global points (top-left origin), as the window list
/// uses. Main thread only (see `monitors_main`).
fn monitors_of(app: &tauri::AppHandle) -> Vec<locator::Monitor> {
    let primary = app.primary_monitor().ok().flatten().map(|m| strips::monitor_name(&m));
    let all = app.available_monitors().unwrap_or_else(|e| {
        log(&format!("warning: could not list the monitors ({e})"));
        Vec::new()
    });
    all
        .iter()
        .map(|m| {
            let sc = m.scale_factor();
            let (p, s) = (m.position(), m.size());
            let name = strips::monitor_name(m);
            locator::Monitor {
                primary: Some(&name) == primary.as_ref(),
                rect: locator::Rect { x: p.x as f64 / sc, y: p.y as f64 / sc, w: s.width as f64 / sc, h: s.height as f64 / sc },
                name,
            }
        })
        .collect()
}

/**
 * Milestone 8: every 500 ms, (re)locate the sheep that need it (new, a new alert,
 * every strip.relocateSec, displays changed), open and close strips to match, keep
 * slots on the other monitors, and wake the strips when anything moved.
 */
#[allow(clippy::too_many_arguments)]
fn placement_loop(app: tauri::AppHandle, brain: Arc<Brain>, locator: Arc<locator::Locator>, relocate: Duration, strip_height: f64, capture_exclusion: bool, port: u16, token: Option<String>) {
    use std::collections::HashSet;
    use std::time::Instant;
    let places = app.state::<Arc<std::sync::Mutex<Places>>>().inner().clone();
    let mut last_monitors: Vec<locator::Monitor> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let monitors = monitors_main(&app);
        if monitors.is_empty() {
            continue;
        }
        if last_monitors.is_empty() {
            log(&format!("monitors: {}", monitors.iter().map(|m| format!("{:?} {}x{} at {},{}{}", m.name, m.rect.w, m.rect.h, m.rect.x, m.rect.y, if m.primary { " (primary)" } else { "" })).collect::<Vec<_>>().join("; ")));
        }
        let displays_changed = !last_monitors.is_empty() && monitors != last_monitors;
        if displays_changed {
            log(&format!("displays changed: {:?}; locating every sheep again", monitors.iter().map(|m| &m.name).collect::<Vec<_>>()));
        }
        last_monitors = monitors.clone();
        let Some(snap) = brain.current() else { continue };
        let sheep = placement::sheep_of(&snap);
        let now = Instant::now();
        let primary = places.lock().unwrap().primary.clone();
        let mut changed = false;
        let present: HashSet<String> = sheep.iter().map(|s| s.identity.clone()).collect();
        // Who needs a lookup, done at most LOCATE_WORKERS at a time, so one slow terminal
        // (a herdr socket that hangs until the 4 s kill) doesn't hold up the others.
        let todo: Vec<&placement::Sheep> = sheep
            .iter()
            .filter(|s| {
                let prev = places.lock().unwrap().placed.get(&s.identity).cloned();
                placement::needs_locate(prev.as_ref(), s, now, relocate, displays_changed)
            })
            .collect();
        let found = locate_all(&locator, &todo);
        for (s, found) in todo.iter().zip(found) {
            let prev = places.lock().unwrap().placed.get(&s.identity).cloned();
            let monitor = placement::monitor_of(&found, &monitors, &primary);
            if prev.as_ref().map(|p| &p.monitor) != Some(&monitor) {
                log(&format!("placement: {} -> {monitor:?} ({}, {:?})", s.label, found.strategy, found.confidence));
                changed = true;
            }
            places.lock().unwrap().placed.insert(s.identity.clone(), placement::Placed { monitor, at: now, alert: s.alert.clone() });
        }
        {
            let mut p = places.lock().unwrap();
            let before = p.placed.len();
            p.placed.retain(|id, _| present.contains(id));
            changed |= p.placed.len() != before;
        }
        // Strips to match.
        let needed = {
            let p = places.lock().unwrap();
            placement::strips_needed(&primary, &monitors, &p.placed, &present)
        };
        let open: std::collections::BTreeSet<String> = app.state::<strips::Strips>().by_monitor.lock().unwrap().keys().cloned().collect();
        for m in needed.difference(&open) {
            let hidden = brain.view().hidden;
            let index = app.state::<strips::Strips>().next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (h, m) = (app.clone(), m.clone());
            let _ = app.run_on_main_thread(move || {
                let Some(mon) = h.available_monitors().unwrap_or_default().into_iter().find(|x| strips::monitor_name(x) == m) else { return };
                if let Err(e) = strips::open(&h, &mon, strip_height, index, capture_exclusion, hidden) {
                    log(&format!("warning: could not open a strip on {m:?}: {e}"));
                }
            });
            changed = true;
        }
        for m in open.difference(&needed) {
            if *m != primary {
                let (h, m) = (app.clone(), m.clone());
                let _ = app.run_on_main_thread(move || strips::close(&h, &m));
                changed = true;
            }
        }
        // Slots on the other monitors, reported to the brain so they're kept.
        let report = {
            let mut p = places.lock().unwrap();
            let (book, report) = placement::assign_slots(&sheep, &p.placed, &primary, &p.book);
            changed |= book != p.book;
            p.book = book;
            report
        };
        for (identity, monitor, slot) in report {
            let msg = serde_json::json!({"action": "placed", "identityKey": identity, "monitor": monitor, "slot": slot});
            if let Err(e) = brain::control_json(port, token.as_deref(), &msg) {
                log(&format!("warning: could not report a placement to the brain ({e})"));
            }
        }
        if changed {
            brain.update_view(|v| v.placement += 1);
        }
    }
}

fn demo_snapshot() -> Value {
    let states: [(&str, Option<&str>, Option<&str>); 9] = [
        ("busy", Some("running"), None),
        ("busy", Some("reading"), None),
        ("idle", None, None),
        ("sleeping", None, None),
        ("done", None, None),
        ("compacting", None, None),
        ("error", Some("rate_limit"), None),
        ("busy", Some("thinking"), Some("Bash: git push origin main?")),
        ("arriving", None, None),
    ];
    let flock: Vec<Value> = states
        .iter()
        .enumerate()
        .map(|(i, (state, flavour, alert))| {
            let mut v = serde_json::json!({
                "id": format!("s-demo{i}"),
                "identityKey": format!("demo:{i}"),
                "label": format!("{state}{}", flavour.map(|f| format!(" {f}")).unwrap_or_default()),
                "colourIndex": i % 8,
                "slot": i,
                "state": state,
                "since": 0,
                "terminal": {},
                "lambs": [],
                "hiddenLambCount": 0,
            });
            if let Some(f) = flavour {
                v["flavour"] = Value::from(*f);
            }
            // Lambs on two of the sheep: three busy ones and "+2", and one waiting on a prompt.
            if i == 1 {
                v["lambs"] = serde_json::json!(["Explore", "Plan", "general-purpose"]
                    .iter()
                    .enumerate()
                    .map(|(k, t)| serde_json::json!({"id": format!("l-demo1{k}"), "agentType": t, "label": t, "state": "busy", "flavour": "reading"}))
                    .collect::<Vec<_>>());
                v["hiddenLambCount"] = Value::from(2);
            }
            if i == 4 {
                v["lambs"] = serde_json::json!([{"id": "l-demo40", "agentType": "Explore", "label": "Explore", "state": "busy", "flavour": "running",
                    "alert": {"kind": "permission", "text": "Bash: npm test?", "since": 0, "confirmed": false}}]);
                v["lambAlert"] = Value::from(true);
            }
            if let Some(text) = alert {
                v["alert"] = serde_json::json!({"kind": "permission", "text": text, "since": 0, "confirmed": false});
            }
            v
        })
        .collect();
    serde_json::json!({"rev": 1, "at": 0, "muted": false, "flock": flock})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every herdr call takes 300 ms; counts how many run at once.
    struct SlowRunner {
        now: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }
    impl locator::Runner for SlowRunner {
        fn run(&self, _p: &str, _a: &[&str], _e: &[(&str, &str)]) -> Result<String, locator::RunError> {
            use std::sync::atomic::Ordering::SeqCst;
            let n = self.now.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(n, SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            self.now.fetch_sub(1, SeqCst);
            Err(locator::RunError::Failed("slow".into()))
        }
    }

    #[test]
    fn sheep_are_located_concurrently_but_never_more_than_four_at_once() {
        let runner = Arc::new(SlowRunner { now: Default::default(), peak: Default::default() });
        let mut loc = locator::Locator::new(
            runner.clone(),
            Arc::new(locator::tests::FakeWindows(vec![])),
            locator::Settings { strategies: vec!["herdr".into()], herdr_path: "herdr".into(), apple_script: false, uid: 501 },
            Box::new(|_| {}),
        );
        loc.socket_check = Box::new(|_| Ok(()));
        let sheep: Vec<placement::Sheep> = (0..8)
            .map(|i| placement::Sheep { identity: format!("s{i}"), label: String::new(), alert: String::new(), terminal: serde_json::json!({"herdrPane": format!("w1:p{i}"), "herdrSocket": format!("/s/{i}")}), slots: Default::default() })
            .collect();
        let todo: Vec<&placement::Sheep> = sheep.iter().collect();
        let t = std::time::Instant::now();
        let found = locate_all(&loc, &todo);
        let took = t.elapsed();
        assert_eq!(found.len(), 8);
        assert_eq!(runner.peak.load(std::sync::atomic::Ordering::SeqCst), LOCATE_WORKERS);
        assert!(took < Duration::from_millis(1000), "8 × 300 ms in {took:?}: not one at a time");
        assert!(locate_all(&loc, &[]).is_empty());
    }

    #[test]
    fn the_old_default_palette_means_the_new_default() {
        let old = serde_json::json!(["#FFFFFF", "#7fb3ff", "#ffb36b", "#9be37d", "#f28bd0", "#c9a2ff", "#ffe066", "#ff7b7b"]);
        assert_eq!(sheep_palette(Some(&old)), serde_json::json!(DEFAULT_PALETTE));
        let mine = serde_json::json!(["#123456", "#654321"]);
        assert_eq!(sheep_palette(Some(&mine)), mine, "a palette of the user's own is kept");
        assert_eq!(sheep_palette(None), serde_json::json!(DEFAULT_PALETTE));
        assert!(!DEFAULT_PALETTE.iter().any(|c| c.eq_ignore_ascii_case("#ffffff")), "no untinted sheep");
    }

    #[test]
    fn pet_pins_and_doctype() {
        assert!(verify_pet("esheep64", b"tampered").unwrap_err().contains("doesn't match its pinned sha256"));
        assert!(verify_pet("notpinned", b"x").unwrap_err().contains("not a pinned pet"));
        assert!(verify_pet("../esheep64", b"x").is_err());
        assert!(reject_doctype("<?xml version=\"1.0\"?><!DOCTYPE a [<!ENTITY x \"y\">]><animations/>").is_err());
        assert!(reject_doctype("<animations><!ENTITY x></animations>").is_err());
        assert!(reject_doctype("<animations><header/></animations>").is_ok());
    }

    #[test]
    fn the_real_pin_matches_the_downloaded_file_when_present() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/pets/esheep64/animations.xml");
        if let Ok(bytes) = std::fs::read(file) {
            verify_pet("esheep64", &bytes).unwrap();
            reject_doctype(std::str::from_utf8(&bytes).unwrap()).unwrap();
        }
    }
}
