//! bellwether's shell: the strip of sheep. A pure client of the brain:
//! it renders, does click-through, and owns the per-OS window work. It knows nothing
//! about hooks.

pub mod actions;
mod agent;
mod brain;
pub mod cli;
mod clickthrough;
// Hidden `__` subcommands for the conformance harness; never in a release build.
#[cfg(feature = "conformance")]
pub mod conformance;
#[cfg(feature = "conformance")]
mod conformance_session;
pub mod dispatch;
pub mod identity;
pub mod login_items;
pub mod migration;
pub mod onboarding;
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
mod updater;

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

/// A pet's pin in the pets.lock.json compiled into this binary: a bundled pet's under
/// "bundled" (plan D8, built by scripts/build-pet.mjs), a fetched one's under "pets".
fn pin(folder: &str) -> Result<(Value, bool), String> {
    let lock: Value = serde_json::from_str(PET_LOCK).map_err(|e| format!("pets.lock.json: {e}"))?;
    let plain = bellwether_brain::pets::valid_name(folder);
    if let Some(p) = lock.get("bundled").and_then(|b| b.get(folder)).filter(|_| plain) {
        return Ok((p.clone(), true));
    }
    match lock.get("pets").and_then(|b| b.get(folder)).filter(|_| plain) {
        Some(p) => Ok((p.clone(), false)),
        None => Err(format!("{folder} is not a pinned pet (pets.lock.json); only pinned pets are loaded")),
    }
}

/// Checks a pet file against its pinned sha256 from pets.lock.json.
fn verify_pet(folder: &str, bytes: &[u8]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let (entry, bundled) = pin(folder)?;
    let want = entry.get("sha256").and_then(Value::as_str).ok_or_else(|| format!("{folder}'s pin in pets.lock.json has no sha256"))?;
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        let fix = if bundled { "it is one of Bellwether's own pets: rebuild it with `node scripts/build-pet.mjs build` (a development build), or reinstall the app" } else { "fetch it again with `node scripts/fetch-pets.mjs` (a development build; see docs/DEVELOPMENT.md)" };
        return Err(format!("pets/{folder}/animations.xml doesn't match its pinned sha256 (expected {want}, got {got}); {fix}"));
    }
    Ok(())
}

/// A bundled pet's behaviour map, from its pin (plan D8): what `get_pet` uses when
/// config.json has no `pets.animationMap.<name>`.
fn bundled_map(folder: &str) -> Option<Value> {
    pin(folder).ok().filter(|(_, bundled)| *bundled).and_then(|(p, _)| p.get("animationMap").cloned())
}

/// A bundled pet's overlays, from its pin (deviation 125): images the player draws over
/// some frames and never mirrors (the sheep's sleeping "z"). Other pets have none.
fn bundled_overlays(folder: &str) -> Value {
    pin(folder).ok().filter(|(_, bundled)| *bundled).and_then(|(p, _)| p.get("overlays").cloned()).unwrap_or(Value::Null)
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
    /// Sheep put down by hand (D10, milestone 12), by identity key. In memory only.
    dropped: std::collections::HashMap<String, placement::Dropped>,
    /// The last drop's number.
    seq: u64,
}

struct Shared {
    pets: PetPlace,
    /// The config the strip uses; the onboarding window's pet choice changes `pets` in it.
    config: std::sync::RwLock<Value>,
    selftest: bool,
    /// Debug only: where `--snapshot` writes the strip's canvas. None in normal use,
    /// and then `debug_snapshot` refuses.
    snapshot: Option<(String, f64)>,
    /// Debug only: also snapshot every N seconds (--snapshot-every N).
    snapshot_every: Option<f64>,
    /// Debug only (`--demo --demo-drag`): the strip runs a scripted drag (D10).
    demo_drag: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PetPayload {
    folder: String,
    xml: String,
    animation_map: Value,
    overlays: Value,
    display: Value,
}

/// Where pets come from (distribution plan D8, milestone 5).
enum PetPlace {
    /// A repository: a bundled pet from the checkout's `pets/<name>/animations.xml`, else
    /// `data/pets/<name>/animations.xml`, both pinned in pets.lock.json.
    Repository { data_dir: PathBuf, repo: Option<PathBuf> },
    /// The app: a bundled pet (`Contents/Resources/pets/<name>/animations.xml`, pinned in
    /// the pets.lock.json compiled into this binary), else an imported one
    /// (`<home>/pets/<name>/animations.xml`, pinned in `<home>/pets.user.lock.json` by
    /// `bellwether pets import`).
    App { resources: Option<PathBuf>, home: PathBuf },
}

/// The pet's file, checked against its pin, as text. `name` is already a plain name.
fn pet_xml(place: &PetPlace, name: &str) -> Result<String, String> {
    let read_size = |path: &std::path::Path, missing: &str| -> Result<(), String> {
        let meta = std::fs::metadata(path).map_err(|_| missing.to_string())?;
        if meta.len() > MAX_PET_BYTES {
            return Err(format!("{} is larger than {} MB", path.display(), MAX_PET_BYTES / 1024 / 1024));
        }
        Ok(())
    };
    let read = |path: &std::path::Path, missing: &str| -> Result<Vec<u8>, String> {
        read_size(path, missing)?;
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    let bytes = match place {
        PetPlace::Repository { repo: Some(repo), .. } if bellwether_brain::pets::BUNDLED.contains(&name) => {
            let path = repo.join("pets").join(name).join("animations.xml");
            let bytes = read(&path, &format!("{} is missing: build it with `node scripts/build-pet.mjs build {name}`", path.display()))?;
            verify_pet(name, &bytes)?;
            bytes
        }
        PetPlace::Repository { data_dir, .. } => {
            let path = data_dir.join("pets").join(name).join("animations.xml");
            let bytes = read(&path, &format!("{} is missing: fetch it with `node scripts/fetch-pets.mjs` (a development build; see docs/DEVELOPMENT.md)", path.display()))?;
            // The same pins scripts/fetch-pets.mjs uses (pets.lock.json, compiled in): a pet
            // that isn't pinned, or whose file doesn't match, is refused.
            verify_pet(name, &bytes)?;
            bytes
        }
        PetPlace::App { resources, home } => {
            let bundled = resources.as_ref().map(|r| r.join("pets").join(name).join("animations.xml")).filter(|p| p.exists());
            match bundled {
                Some(path) => {
                    let bytes = read(&path, "")?;
                    verify_pet(name, &bytes).map_err(|e| format!("the bundled pet {name}: {e}"))?;
                    bytes
                }
                None => {
                    let path = bellwether_brain::pets::pet_file(home, name);
                    read_size(&path, &format!("no pet named {name:?}: it isn't bundled with this app, and {} doesn't exist. Import one with `bellwether pets import <animations.xml> --name {name}`, or set pets.default in config.json to a pet you imported", path.display()))?;
                    bellwether_brain::pets::load(home, name)?
                }
            }
        }
    };
    let xml = String::from_utf8(bytes).map_err(|_| format!("the pet {name} is not UTF-8"))?;
    reject_doctype(&xml)?;
    Ok(xml)
}

/// The pet's animations.xml (see `PetPlace`), plus what the webview needs to draw it. The
/// name comes from config (`pets.default`) and must be a plain name: no path.
#[tauri::command]
fn get_pet(shared: tauri::State<'_, Shared>) -> Result<PetPayload, String> {
    let config = shared.config.read().unwrap().clone();
    // No pets.default: Bellwether's own sheep (plan D8).
    let folder = config.pointer("/pets/default").and_then(Value::as_str).unwrap_or(bellwether_brain::pets::DEFAULT_PET).to_string();
    if folder.is_empty() || !folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(format!("pets.default {folder:?} must be a plain folder name"));
    }
    let xml = pet_xml(&shared.pets, &folder).map_err(|e| {
        log(&format!("error: refusing to load pet {folder}: {e}"));
        e
    })?;
    // config.json's map wins; a bundled pet otherwise brings its own (all 14 behaviours).
    let animation_map = config.pointer(&format!("/pets/animationMap/{folder}")).cloned().or_else(|| bundled_map(&folder)).unwrap_or(Value::Null);
    let num = |p: &str, d: f64| config.pointer(p).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d);
    let palette = sheep_palette(config.pointer("/pets/palette"));
    let display = serde_json::json!({
        "palette": palette,
        "scale": num("/pets/scale", 1.5),
        "lambScale": num("/pets/lambScale", 0.6),
        // "horns" (default): only the horns take the session colour; "whole": the old tint.
        "tint": match config.pointer("/pets/tint").and_then(Value::as_str) { Some("whole") => "whole", _ => "horns" },
        "slotWidthPx": num("/strip/slotWidthPx", 110.0),
        "homeWanderPx": num("/strip/homeWanderPx", 120.0),
        "fadeOpacity": config.pointer("/fade/opacity").and_then(Value::as_f64).filter(|v| (0.0..=1.0).contains(v)).unwrap_or(0.25),
        "selftest": shared.selftest,
        "trace": crate::settings::env_os("SHELL_TRACE").is_some(),
        "snapshotAfterMs": shared.snapshot.as_ref().map(|(_, secs)| secs * 1000.0),
        "snapshotEveryMs": shared.snapshot.as_ref().and(shared.snapshot_every).map(|secs| secs * 1000.0),
        "demoDrag": shared.demo_drag,
    });
    let overlays = bundled_overlays(&folder);
    Ok(PetPayload { folder, xml, animation_map, overlays, display })
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
    // Its page is up: it can draw a carried sheep from now on (D10).
    strips.ready.lock().unwrap().insert(window.label().to_string());
    let snap = snap.map(|s| {
        let p = places.lock().unwrap();
        let mine = strips.monitor_of(window.label()).unwrap_or_else(|| p.primary.clone());
        let open: std::collections::HashSet<String> = strips.by_monitor.lock().unwrap().keys().cloned().collect();
        let mut v = placement::view_for(&s, &mine, &p.primary, &p.placed, &open, &p.book, &p.dropped);
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
    let id = clean_id(&id);
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

/// Keeps a sprite id to the characters ids are made of (as `sprite_clicked` does).
fn clean_id(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ':' || *c == '.' || *c == '_' || *c == '@').take(128).collect()
}

/// A sheep in the brain's snapshot by its sprite id (sheep only, not lambs).
fn sheep_by_id<'a>(snap: &'a Value, id: &str) -> Option<&'a Value> {
    snap.get("flock").and_then(Value::as_array)?.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(id))
}

/// Dragging a sheep along the strip (D10, milestone 12). The pressed strip's webview owns
/// the gesture and says where it is in it; the shell holds the window's mouse, knows
/// where the cursor is, and keeps the drop records.
///
/// - `press`: the window holds the mouse until the gesture ends (clickthrough.rs).
/// - `lift`: past the threshold, a sheep is carried: `grabDx` (cursor x − its left edge)
///   and `width`. Strips open on every monitor.
/// - `release`: the end of a click (only the hold ends), or the drop of a lifted sheep. A
///   drop is recorded for the monitor under the cursor; on the pressed strip's own monitor
///   at `x`, the webview's own centre for it. Returns `{monitor, centreX, seq, ghost, here}`.
/// - `cancel`: nothing is recorded (`reason` for the log).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn sprite_drag(
    phase: String,
    id: String,
    grab_dx: Option<f64>,
    width: Option<f64>,
    x: Option<f64>,
    reason: Option<String>,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    brain: tauri::State<'_, Arc<Brain>>,
    places: tauri::State<'_, Arc<std::sync::Mutex<Places>>>,
    drag: tauri::State<'_, clickthrough::Drag>,
    strips: tauri::State<'_, strips::Strips>,
    shared: tauri::State<'_, Shared>,
) -> Value {
    let id = clean_id(&id);
    let label = window.label().to_string();
    let num = |v: Option<f64>| v.filter(|v| v.is_finite() && v.abs() < 100_000.0);
    match phase.as_str() {
        "press" => {
            let mut h = drag.hold.lock().unwrap();
            // One hold at a time: a new press replaces any other.
            *h = Some(clickthrough::Hold::new(&label, &id, std::time::Instant::now()));
            Value::Null
        }
        "lift" => {
            let snap = brain.current();
            let sheep = snap.as_ref().and_then(|s| sheep_by_id(s, &id));
            {
                let mut h = drag.hold.lock().unwrap();
                let Some(hold) = h.as_mut().filter(|h| h.label == label && h.id == id) else { return Value::Null };
                hold.lifted = true;
                hold.grab_dx = num(grab_dx).unwrap_or(0.0);
                hold.width = num(width).filter(|w| *w > 0.0).unwrap_or(0.0);
                hold.colour = sheep.and_then(|s| s.get("colourIndex")).and_then(Value::as_i64).unwrap_or(0);
                hold.alert = sheep.and_then(|s| s.pointer("/alert/kind")).and_then(Value::as_str).map(str::to_string);
            }
            log(&format!("sheep {id} picked up"));
            // Strips on every monitor, now rather than on the next pass.
            drag.kick();
            Value::Null
        }
        "release" => {
            let hold = {
                let mut h = drag.hold.lock().unwrap();
                if h.as_ref().is_some_and(|h| h.label == label && h.id == id) {
                    h.take()
                } else {
                    None
                }
            };
            let Some(hold) = hold.filter(|h| h.lifted) else { return Value::Null };
            let identity = brain.current().as_ref().and_then(|s| sheep_by_id(s, &id)).and_then(|s| s.get("identityKey")).and_then(Value::as_str).map(str::to_string);
            let Some(identity) = identity else {
                log("drag cancelled: its session ended");
                brain.update_view(|v| v.carry = None);
                return Value::Null;
            };
            let mut p = places.lock().unwrap();
            let pressed = strips.monitor_of(&label).unwrap_or_else(|| p.primary.clone());
            // The monitor under the cursor, unless that is the pressed strip's own (or has
            // no strip ready, or the gesture is --demo-drag's): then the pressed strip's own
            // centre for the sheep, which it has drawn there.
            let at = if shared.demo_drag { None } else { clickthrough::cursor_now(&app) };
            let (monitor, centre) = match at.filter(|a| a.monitor != pressed && strips.is_ready(&a.label)) {
                Some(a) => {
                    let half = hold.width / 2.0;
                    (a.monitor, (a.x - hold.grab_dx + half).clamp(half, (a.w - half).max(half)))
                }
                None => (pressed.clone(), num(x).unwrap_or(0.0)),
            };
            p.seq += 1;
            let seq = p.seq;
            let ghost = p.placed.get(&identity).map(|pl| pl.monitor.as_str()).unwrap_or(p.primary.as_str()) == monitor;
            p.dropped.insert(identity, placement::Dropped { monitor: monitor.clone(), centre_x: centre, seq, sheep: id.clone() });
            drop(p);
            log(&format!("sheep {id} dropped on {monitor:?} at x {} ({})", centre.round(), if ghost { "ghost" } else { "guest" }));
            brain.update_view(|v| {
                v.carry = None;
                v.placement += 1;
            });
            drag.kick();
            serde_json::json!({"monitor": monitor, "centreX": centre, "seq": seq, "ghost": ghost, "here": monitor == pressed})
        }
        "cancel" => {
            let hold = {
                let mut h = drag.hold.lock().unwrap();
                if h.as_ref().is_some_and(|h| h.label == label && h.id == id) {
                    h.take()
                } else {
                    None
                }
            };
            if hold.is_some_and(|h| h.lifted) {
                let reason: String = reason.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || " -'".contains(*c)).take(64).collect();
                log(&format!("drag cancelled: {}", if reason.is_empty() { "the strip cancelled it" } else { &reason }));
                brain.update_view(|v| v.carry = None);
                drag.kick();
            }
            Value::Null
        }
        _ => Value::Null,
    }
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
    /// With --demo: a scripted drag through the strip's own state machine (D10).
    demo_drag: bool,
    /// Open the onboarding window now (`open -b <id> --args --onboarding`).
    onboarding: bool,
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
        demo_drag: args.iter().any(|a| a == "--demo") && args.iter().any(|a| a == "--demo-drag"),
        onboarding: args.iter().any(|a| a == "--onboarding"),
        locate: value("--locate"),
    }
}

pub fn run() {
    // `Bellwether --action <name>`: the app's side of the CLI hand-off (actions.rs). One
    // action, its outcome written down, then quit; no window, no brain.
    let argv: Vec<String> = std::env::args().collect();
    if let Some(i) = argv.iter().position(|a| a == "--action") {
        std::process::exit(actions::run(argv.get(i + 1).map(String::as_str).unwrap_or("")));
    }
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
    log::init(settings.log_file.clone());
    match &settings.agent_dir {
        Some(dir) => log(&format!("starting: repository layout, agent dir {}, data {}, brain on 127.0.0.1:{}", dir.display(), settings.data_dir.display(), settings.port)),
        None => log(&format!("starting: Bellwether {}, app data {}, brain on 127.0.0.1:{}", bellwether_brain::VERSION, settings.data_dir.display(), settings.port)),
    }

    let brain = Brain::new();
    let strip_height = settings.f64_at("/strip/heightPx", 140.0);
    let app_layout = settings.layout == settings::Layout::App;
    let brain_cfg = brain::BrainConfig {
        port: settings.port,
        token: settings.token.clone(),
        spawn: flags.spawn_brain && !flags.selftest,
        child_env: settings.brain_env.clone(),
        child_gets_token: settings.brain_gets_token,
        check_version: app_layout && !flags.selftest,
        home: settings.data_dir.clone(),
        notify: None,
        agent_check: None,
    };
    let shared = Shared {
        pets: if app_layout { PetPlace::App { resources: settings.resources.clone(), home: settings.data_dir.clone() } } else { PetPlace::Repository { data_dir: settings.data_dir.clone(), repo: settings.agent_dir.clone() } },
        config: std::sync::RwLock::new(settings.config.clone()),
        selftest: flags.selftest,
        snapshot: flags.snapshot.clone().map(|p| (p, flags.snapshot_after)),
        snapshot_every: flags.snapshot_every,
        demo_drag: flags.demo_drag,
    };
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
    // The onboarding window (plan D6): the app's, or a repository debug build's when asked.
    let onboarding_ctx = onboarding::Ctx {
        layout: settings.layout,
        home: settings.data_dir.clone(),
        port: settings.port,
        // Where bundled pets are: the app's Resources, or a repository's checkout (its pets/).
        resources: if app_layout { settings.resources.clone() } else { settings.agent_dir.clone() },
        user_home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        env_config: std::env::var("BELLWETHER_CONFIG").ok().filter(|c| !c.is_empty()),
        env_token: std::env::var("BELLWETHER_HOOK_TOKEN").ok().filter(|t| !t.is_empty()),
    };
    let checks_only = flags.selftest || flags.demo || flags.locate.is_some();
    let open_onboarding = !checks_only && onboarding::allowed(settings.layout) && (flags.onboarding || (app_layout && !bellwether_core::shellstate::onboarded(&settings.data_dir)));
    if flags.onboarding && !onboarding::allowed(settings.layout) {
        log("--onboarding: the setup window is Bellwether.app's; a development build is set up by hand (docs/DEVELOPMENT.md)");
    }

    let brain_for_setup = brain.clone();
    // The single-instance lock (/tmp/<identifier>_si.sock) is keyed by the port: one strip
    // per brain. On the default port the identifier is the build's own, so a rebuilt
    // repository strip and an already-running one (any earlier build) still exclude each
    // other; a test bundle or a dev strip on another port never hands off to them.
    let mut context = tauri::generate_context!();
    // Updates (plan D7, milestone 9): only a build that may update itself (the Developer ID
    // app, with the updater configuration of tauri.dist.conf.json) registers the plugin and
    // gets the tray item. No capability grants an `updater:` permission (updater.rs).
    let bundle_id = identity::bundle_id();
    let overrides = login_items::bundle_agent_overrides();
    let updater_off = updater::refusal(&updater::Build {
        layout: settings.layout,
        plugin_config: context.config().plugins.0.get("updater"),
        bundle_id: bundle_id.as_deref(),
        test_build: overrides.0.is_some() || overrides.1.is_some(),
        ad_hoc: bundle_id.as_deref() == Some(identity::PERMANENT_ID) && identity::ad_hoc(),
        test_endpoint: updater::test_endpoint().is_some(),
    });
    let updates_on = updater_off.is_none() && !checks_only;
    let (update_policy, policy_warning) = updater::policy(&settings.config);
    if let Some(w) = policy_warning {
        log(&format!("warning: {w}"));
    }
    match (&updater_off, checks_only) {
        (Some(why), _) => log(&format!("updates: off ({why})")),
        (None, true) => log("updates: off (--selftest, --demo or --locate)"),
        (None, false) => log(&format!("updates: on, from {}", updater::test_endpoint().unwrap_or_else(|| "the release feed".into()))),
    }
    let lock_id = single_instance_id(&context.config().identifier, settings.port);
    context.config_mut().identifier = lock_id;
    let mut builder = tauri::Builder::default();
    // One strip at a time. Only `--locate` skips this: it opens no window and quits
    // after one lookup. `--selftest` keeps it, because it does open a window (the pet is
    // parsed by the webview's own XML parser), so it is refused while a strip runs.
    if flags.locate.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(move |app, argv, _cwd| {
            // `open -b <id> --args --onboarding` while the app runs: the window, here.
            if argv.iter().any(|a| a == "--onboarding") && !checks_only {
                log("another launch asked for the setup window");
                onboarding::open(app);
            } else {
                log("another bellwether shell tried to start; this one keeps running");
            }
        }));
    }
    if updates_on {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let app = builder
        .manage(brain.clone())
        .manage(Hitboxes::default())
        .manage(strips::Strips::default())
        .manage(locator.clone())
        .manage(Arc::new(std::sync::Mutex::new(Places { primary: String::new(), placed: Default::default(), book: Default::default(), dropped: Default::default(), seq: 0 })))
        .manage(clickthrough::Drag::default())
        .manage(clickthrough::Screens::default())
        .manage(shared)
        .manage(onboarding::Onboarding::new(onboarding_ctx))
        .invoke_handler(tauri::generate_handler![
            get_pet,
            get_snapshot,
            report_hitboxes,
            sprite_clicked,
            sprite_drag,
            shell_log,
            debug_snapshot,
            onboarding::setup_status,
            onboarding::hooks_preview,
            onboarding::hooks_apply,
            onboarding::login_items_set,
            onboarding::permission_request,
            onboarding::pet_select,
            onboarding::pet_import,
            onboarding::migration_preview,
            onboarding::migration_apply,
            onboarding::cli_tool_install,
            onboarding::setup_close
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            let monitor = app.primary_monitor()?.ok_or("no primary monitor")?;
            app.state::<Arc<std::sync::Mutex<Places>>>().lock().unwrap().primary = strips::monitor_name(&monitor);
            // The monitors for the cursor poll (D10); the placement loop keeps them current.
            *app.state::<clickthrough::Screens>().now.lock().unwrap() = (monitors_of(&handle), monitor.scale_factor());
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
            // The brain first (milestone 9): after an update the relaunched app starts its
            // brain child before the tray and the windows, which shortens the gap in which
            // no brain answers (`--demo` has no brain).
            if !flags.demo {
                let b = brain_for_setup.clone();
                let mut cfg = brain_cfg;
                let h = handle.clone();
                cfg.notify = Some(Box::new(move |text| tray::late_notice(&h, text)));
                std::thread::spawn(move || brain::run(b, cfg));
            }
            // Milestone 7: the tray, and the thread that hides the strip while sharing
            // or when the tray says so. The tray switches start as they were left, so a
            // strip hidden from the tray opens hidden (never flashing up first).
            let mut kept = privacy::load_kept(&data_dir);
            app.manage(tray::Switches { kept: std::sync::Mutex::new(kept), data_dir: data_dir.clone(), port, token: token.clone() });
            if !flags.selftest {
                // Legacy agents and login items waiting for approval (plan D5): the app's
                // own concern, so only in the app layout. The repository's strip is itself
                // run by a legacy agent.
                let notices = if app_layout && !flags.demo { tray::startup_notices() } else { Vec::new() };
                if let Err(e) = tray::build(&handle, kept, false, &notices, app_layout && !flags.demo, updates_on) {
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
            if open_onboarding {
                onboarding::open(&handle);
            }
            if !flags.demo && !flags.selftest {
                let (h, b, l) = (handle.clone(), brain_for_setup.clone(), locator.clone());
                std::thread::spawn(move || placement_loop(h, b, l, relocate, strip_height, capture_exclusion, port, token_for_places));
            }
            let (h, b) = (handle.clone(), brain_for_setup.clone());
            std::thread::spawn(move || privacy_loop(h, b, sharing_names, strip_height));

            if flags.demo {
                // Debug: one sheep per behaviour, without a brain, to check the
                // animation map by eye. No hooks, no brain, nothing persisted.
                log("--demo: showing a synthetic flock; the brain is not contacted");
                if flags.demo_drag {
                    log("--demo-drag: the strip will press demo sheep 2, carry it onto sheep 4 and drop it, then click sheep 1");
                }
                brain_for_setup.set_demo(demo_snapshot());
            }
            let h = handle.clone();
            let demo_drag = flags.demo_drag;
            std::thread::spawn(move || clickthrough::run(h, fade_radius, demo_drag));
            if updates_on {
                updater::schedule(handle.clone(), update_policy);
            }

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
        .build(context)
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
 * Every 250 ms: whether the strip should be hidden (the tray's Hide everything; a sharing process,
 * read every 500 ms, debounced 2 s on the way back), applied to every strip window,
 * and the tray's Quiet check kept in step with the brain.
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
            for (label, w) in tauri::Manager::webview_windows(&app) {
                // The strips only: the onboarding window isn't hidden while sharing.
                if strips::is_strip(&label) {
                    let _ = if hidden { w.hide() } else { w.show() };
                }
            }
            log(&format!(
                "strip {}{}",
                if hidden { "hidden" } else { "shown" },
                if kept.hidden {
                    " (Hide everything is on; to show it again: the tray's Hide everything (for screen sharing), or `bellwether show`)"
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

/// The identifier the single-instance lock is named after: the build's own on the default
/// port (47417), `<identifier>.p<port>` on any other. It changes only the lock (and Tauri's
/// own path-resolver folders, which the shell doesn't use), never the bundle id macOS sees.
pub fn single_instance_id(identifier: &str, port: u16) -> String {
    if port == bellwether_brain::DEFAULT_PORT {
        identifier.to_string()
    } else {
        format!("{identifier}.p{port}")
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
        let mons = monitors_of(&h);
        let scale = h.primary_monitor().ok().flatten().map(|m| m.scale_factor());
        if let Some(scale) = scale {
            *h.state::<clickthrough::Screens>().now.lock().unwrap() = (mons.clone(), scale);
        }
        let _ = tx.send(mons);
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
        // Every 500 ms, or at once when a sheep is lifted or dropped (strips to open).
        app.state::<clickthrough::Drag>().wait(Duration::from_millis(500));
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
            app.state::<clickthrough::Drag>().cancel(&brain, "the displays changed", None);
        }
        last_monitors = monitors.clone();
        let Some(snap) = brain.current() else { continue };
        let sheep = placement::sheep_of(&snap);
        let now = Instant::now();
        let primary = places.lock().unwrap().primary.clone();
        let mut changed = false;
        let present: HashSet<String> = sheep.iter().map(|s| s.identity.clone()).collect();
        // The sheep being carried is leaving or gone: the drag ends at once (D10).
        {
            let drag = app.state::<clickthrough::Drag>();
            let held = drag.hold.lock().unwrap().as_ref().map(|h| h.id.clone());
            if let Some(id) = held {
                let live = snap.get("flock").and_then(Value::as_array).is_some_and(|f| {
                    f.iter().any(|s| {
                        let mine = s.get("id").and_then(Value::as_str) == Some(id.as_str()) || s.get("lambs").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.get("id").and_then(Value::as_str) == Some(id.as_str())));
                        mine && s.get("state").and_then(Value::as_str) != Some("leaving")
                    })
                });
                if !live {
                    drag.cancel(&brain, "its session ended", None);
                }
            }
        }
        // Drops that end now: the session ended, or the monitor went (D10).
        {
            let mut p = places.lock().unwrap();
            for (identity, reason) in placement::ended_drops(&p.dropped, &sheep, &monitors) {
                if let Some(d) = p.dropped.remove(&identity) {
                    log(&format!("drop for {} ended: {reason}", d.sheep));
                    // Back to normal placement: located again on this pass.
                    p.placed.remove(&identity);
                    changed = true;
                }
            }
        }
        // Who needs a lookup, done at most LOCATE_WORKERS at a time, so one slow terminal
        // (a herdr socket that hangs until the 4 s kill) doesn't hold up the others.
        let todo: Vec<&placement::Sheep> = sheep
            .iter()
            .filter(|s| {
                let p = places.lock().unwrap();
                let prev = p.placed.get(&s.identity).cloned();
                placement::needs_locate(prev.as_ref(), s, now, relocate, displays_changed, p.dropped.contains_key(&s.identity))
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
            placement::strips_needed(&primary, &monitors, &p.placed, &present, &p.dropped, app.state::<clickthrough::Drag>().lifted())
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
            let (book, report) = placement::assign_slots(&sheep, &p.placed, &primary, &p.book, &p.dropped);
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
            .map(|i| placement::Sheep { identity: format!("s{i}"), label: String::new(), alert: String::new(), terminal: serde_json::json!({"herdrPane": format!("w1:p{i}"), "herdrSocket": format!("/s/{i}")}), slots: Default::default(), leaving: false })
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

    /// The webview surface (plan D1, D6; milestone 5's, 7's and 12's "done when"): two
    /// capabilities, each granting an exact list to its own windows: the strip's seven to
    /// `strip-*` (`sprite_drag` since milestone 12, D10), the onboarding window's eleven to
    /// `onboarding`, none shared. The handler list, build.rs and the autogenerated permission
    /// files name the same eighteen, and no other capability exists.
    #[test]
    fn the_capabilities_list_exactly_their_permissions() {
        const SEVEN: [&str; 7] = ["get_pet", "get_snapshot", "report_hitboxes", "sprite_clicked", "sprite_drag", "shell_log", "debug_snapshot"];
        let allow = |list: &[&str]| -> Vec<String> { list.iter().map(|c| format!("allow-{}", c.replace('_', "-"))).collect() };
        let granted = |cap: &Value| -> Vec<String> { cap["permissions"].as_array().unwrap().iter().map(|p| p.as_str().expect("a plain permission name, no scope object").to_string()).collect() };
        let strip: Value = serde_json::from_str(include_str!("../capabilities/strip.json")).unwrap();
        assert_eq!(granted(&strip), allow(&SEVEN), "the strip: exactly the seven, in this order");
        assert_eq!(strip["windows"], serde_json::json!(["strip-*"]));
        let onboarding: Value = serde_json::from_str(include_str!("../capabilities/onboarding.json")).unwrap();
        assert_eq!(granted(&onboarding), allow(&onboarding::COMMANDS), "the onboarding window: exactly the eleven of D6, in its order");
        assert_eq!(onboarding["windows"], serde_json::json!([onboarding::LABEL]));
        for cap in [&strip, &onboarding] {
            assert!(cap.get("remote").is_none() && cap.get("platforms").is_none());
        }
        assert!(!SEVEN.iter().any(|c| onboarding::COMMANDS.contains(c)), "no command is in both");
        assert!(!strips::is_strip(onboarding::LABEL), "the onboarding window never matches strip-*");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut caps: Vec<String> = std::fs::read_dir(dir.join("capabilities")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        caps.sort();
        assert_eq!(caps, ["onboarding.json", "strip.json"]);
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["app"]["security"]["capabilities"], serde_json::json!(["strip", "onboarding"]));
        let mut all: Vec<&str> = SEVEN.iter().chain(onboarding::COMMANDS.iter()).copied().collect();
        let src = include_str!("lib.rs");
        let handler: Vec<&str> = src.split(concat!("generate_", "handler![")).nth(1).unwrap().split(']').next().unwrap().split(',').map(str::trim).filter(|c| !c.is_empty()).map(|c| c.trim_start_matches("onboarding::")).collect();
        assert_eq!(handler, all);
        let build = include_str!("../build.rs");
        let list = build.split(".commands(&[").nth(1).unwrap().split(']').next().unwrap();
        let mut declared: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        declared.sort();
        all.sort();
        assert_eq!(declared, all);
        let mut generated: Vec<String> = std::fs::read_dir(dir.join("permissions").join("autogenerated")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        generated.sort();
        assert_eq!(generated, all.iter().map(|c| format!("{c}.toml")).collect::<Vec<_>>());
    }

    #[test]
    fn the_single_instance_lock_is_per_port_and_unchanged_on_the_live_port() {
        // The plugin's socket: /tmp/<identifier with . and - as _>_si.sock.
        let sock = |id: String| format!("/tmp/{}_si.sock", id.replace(['.', '-'], "_"));
        // The dev identifier, from the one file that names it (rename.test.mjs).
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let dev = conf["identifier"].as_str().unwrap();
        assert_eq!(sock(single_instance_id(dev, 47417)), format!("/tmp/{}_si.sock", dev.replace('.', "_")), "the running repository strip's own lock, as every earlier build named it");
        assert_eq!(sock(single_instance_id("com.fanzootechnology.bellwether", 47417)), "/tmp/com_fanzootechnology_bellwether_si.sock");
        assert_eq!(sock(single_instance_id(dev, 47432)), format!("/tmp/{}_p47432_si.sock", dev.replace('.', "_")));
        assert_eq!(sock(single_instance_id("com.fanzootechnology.bellwether", 47431)), "/tmp/com_fanzootechnology_bellwether_p47431_si.sock");
        assert_ne!(single_instance_id("x", 47431), single_instance_id("x", 47432));
        let src = include_str!("lib.rs");
        assert!(src.contains(concat!("context.config_mut().identifier = ", "lock_id;")) && src.contains(concat!(".build(", "context)")));
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bellwether-pets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A small pet file `pets import` accepts (a 1×1 PNG, one animation).
    const TINY_PET: &str = "<animations><header><title>tiny</title></header><image><tilesx>1</tilesx><tilesy>1</tilesy><png>iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==</png><transparency>Magenta</transparency></image><spawns><spawn id=\"1\"><x>0</x><y>0</y><next probability=\"100\">1</next></spawn></spawns><animations><animation id=\"1\"><name>walk</name><start><x>0</x><y>0</y><interval>200</interval></start><end><x>0</x><y>0</y><interval>200</interval></end><sequence repeat=\"0\" repeatfrom=\"0\"><frame>0</frame></sequence></animation></animations></animations>";

    #[test]
    fn the_app_loads_imported_pets_by_their_pin_and_never_a_path() {
        let home = temp("app");
        let src = home.join("src.xml");
        std::fs::write(&src, TINY_PET).unwrap();
        bellwether_brain::pets::import(&home, &src, Some("tiny"), false).unwrap();
        let app = PetPlace::App { resources: None, home: home.clone() };
        assert_eq!(pet_xml(&app, "tiny").unwrap(), TINY_PET);
        let e = pet_xml(&app, "bellwether").unwrap_err();
        assert!(e.contains("isn't bundled") && e.contains("pets import"), "{e}");
        // Changed after import: refused.
        std::fs::write(bellwether_brain::pets::pet_file(&home, "tiny"), TINY_PET.replace("tiny", "tinz")).unwrap();
        assert!(pet_xml(&app, "tiny").unwrap_err().contains("changed after it was imported"));
        // Not pinned at all: refused.
        let other = bellwether_brain::pets::pet_file(&home, "loose");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, TINY_PET).unwrap();
        assert!(pet_xml(&app, "loose").is_err());
        // A bundled pet must match the pin compiled into the binary.
        let res = temp("res");
        let bundled = res.join("pets").join("esheep64").join("animations.xml");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, TINY_PET).unwrap();
        let app = PetPlace::App { resources: Some(res.clone()), home: home.clone() };
        assert!(pet_xml(&app, "esheep64").unwrap_err().contains("doesn't match its pinned sha256"));
        // The repository keeps data/pets and pets.lock.json.
        let repo = PetPlace::Repository { data_dir: home.clone(), repo: None };
        assert!(pet_xml(&repo, "tiny").unwrap_err().contains("not a pinned pet"));
        assert!(pet_xml(&repo, "nothere").unwrap_err().contains("node scripts/fetch-pets.mjs"));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&res);
    }

    /// Plan D8, milestone M10a: a bundled pet loads from the checkout (a repository build)
    /// or the app's Resources only while it matches the "bundled" pin compiled in, and
    /// brings its own map of all 14 behaviours; a fetched pet has no such map.
    #[test]
    fn bundled_pets_load_by_their_pin_and_bring_their_own_map() {
        let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let data = temp("bundled-data");
        let repo = PetPlace::Repository { data_dir: data.clone(), repo: Some(checkout.clone()) };
        let xml = pet_xml(&repo, "bellwether").unwrap();
        assert!(xml.contains("<title>Bellwether sheep</title>"));
        let map = bundled_map("bellwether").expect("bellwether's pin carries its map");
        for (b, _) in bellwether_brain::pets::BEHAVIOURS {
            assert!(map.get(b).is_some(), "the bundled map has {b}");
        }
        assert_eq!(bundled_map("esheep64"), None, "a fetched pet brings no map");
        let overlays = bundled_overlays("bellwether");
        assert!(overlays.as_object().is_some_and(|o| !o.is_empty()), "the sheep's sleeping z: {overlays}");
        assert_eq!(bundled_overlays("esheep64"), Value::Null, "a fetched pet has no overlays");
        let res = temp("bundled-res");
        let file = res.join("pets").join("bellwether").join("animations.xml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, xml.replace("Bellwether sheep", "Bellwether sheeq")).unwrap();
        let app = PetPlace::App { resources: Some(res.clone()), home: data.clone() };
        let e = pet_xml(&app, "bellwether").unwrap_err();
        assert!(e.contains("doesn't match its pinned sha256") && e.contains("Bellwether's own pets"), "{e}");
        std::fs::write(&file, &xml).unwrap();
        assert_eq!(pet_xml(&app, "bellwether").unwrap(), xml);
        // Without a checkout, a repository build can't find it in data/ (it isn't fetched).
        let bare = PetPlace::Repository { data_dir: data.clone(), repo: None };
        assert!(pet_xml(&bare, "bellwether").is_err());
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&res);
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
