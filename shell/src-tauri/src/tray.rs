//! The menu-bar icon: Quiet (alerts only), Hide everything (for screen sharing), Show
//! labels, Set up Bellwether… (the app only), Quit.
//! Everything here runs in Rust; the webview gains no command or permission.
//!
//! - "Quiet (alerts only)" (id `mute`) is the brain's do-not-disturb (POST /control), the
//!   same as `bellwether mute`, so it is kept in data/state.json and the two always agree.
//!   The strip stops animating, but a sheep that needs you still shows its "!".
//! - "Hide everything (for screen sharing)" (id `hide`) hides the whole strip, "!"
//!   included; it and Show labels are the shell's own, kept in data/shell-state.json.
//! - "Set up Bellwether…" (id `setup`, Bellwether.app only) opens the onboarding window
//!   (plan D6, milestone 7).
//! - Autostart is not here: `bellwether autostart` stays the one way to set it up, and
//!   nothing here writes LaunchAgents (plan D5).
//! - Notices (plan D5, milestone 6), above the switches, found once at start: legacy
//!   LaunchAgents still installed (one line naming them; information only), and a login
//!   item waiting for approval (a line that opens System Settings' Login Items pane).

use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::brain::{self, Brain};
use crate::log::log;
use crate::privacy::{self, Kept};

/// The tray switches the shell keeps, shared with the privacy thread.
pub struct Switches {
    pub kept: Mutex<Kept>,
    pub data_dir: std::path::PathBuf,
    pub port: u16,
    pub token: Option<String>,
}

pub struct TrayItems {
    pub mute: CheckMenuItem<Wry>,
    pub hide: CheckMenuItem<Wry>,
    pub labels: CheckMenuItem<Wry>,
}

/// One line at the top of the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Clicking it opens System Settings' Login Items pane; otherwise it is greyed out
    /// (information only).
    pub opens_login_items: bool,
}

/// The tray's notices: one line naming the legacy agents, if any; one line per login item
/// waiting for approval.
pub fn notices(legacy: &[bellwether_brain::legacy::Agent], waiting: &[crate::login_items::Service]) -> Vec<Notice> {
    let mut out = Vec::new();
    if !legacy.is_empty() {
        out.push(Notice {
            text: format!("Legacy agents: {} (see bellwether doctor)", legacy.iter().map(|a| a.short()).collect::<Vec<_>>().join(", ")),
            opens_login_items: false,
        });
    }
    for s in waiting {
        out.push(Notice { text: format!("Approve the {} in Login Items…", s.name()), opens_login_items: true });
    }
    out
}

/// What the app finds at start (app layout only): legacy agents (launchd asked only when
/// HOME is the real home) and login items waiting for approval. Read only.
pub fn startup_notices() -> Vec<Notice> {
    use crate::login_items::{self, Service, Status};
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let legacy = bellwether_brain::legacy::scan_here(&home);
    if legacy.is_empty() {
        log("legacy agents: none");
    } else {
        log(&format!("warning: legacy agents: {}; two brains on one port fight over it (`bellwether doctor` says more)", bellwether_brain::legacy::summary(&legacy)));
    }
    let waiting: Vec<Service> = if crate::identity::session() == crate::identity::Session::Current {
        [Service::Brain, Service::MainApp].into_iter().filter(|s| login_items::status(*s) == Status::RequiresApproval).collect()
    } else {
        Vec::new()
    };
    notices(&legacy, &waiting)
}

/// The menu's switch labels, in order (ids `mute`, `hide`, `labels`). "Quiet" keeps a
/// waiting sheep's "!"; "Hide everything" hides the whole strip, "!" included.
pub const QUIET: &str = "Quiet (alerts only)";
pub const HIDE: &str = "Hide everything (for screen sharing)";
pub const SETUP: &str = "Set up Bellwether…";

/// The menu, top to bottom, as words (the log line and its test).
pub fn menu_words(notices: usize, setup: bool) -> String {
    format!("{}{QUIET}, {HIDE}, Show labels, {}Quit", if notices == 0 { String::new() } else { format!("{notices} notice(s), ") }, if setup { format!("{SETUP}, ") } else { String::new() })
}

/// `setup`: the "Set up Bellwether…" item, which opens the onboarding window (the app
/// layout only; plan D6).
pub fn build(app: &AppHandle<Wry>, kept: Kept, muted: bool, notices: &[Notice], setup: bool) -> tauri::Result<()> {
    let mute = CheckMenuItem::with_id(app, "mute", QUIET, true, muted, None::<&str>)?;
    let hide = CheckMenuItem::with_id(app, "hide", HIDE, true, kept.hidden, None::<&str>)?;
    let labels = CheckMenuItem::with_id(app, "labels", "Show labels", true, kept.show_labels, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Bellwether", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&mute, &hide, &labels, &PredefinedMenuItem::separator(app)?])?;
    if setup {
        menu.append(&MenuItem::with_id(app, "setup", SETUP, true, None::<&str>)?)?;
    }
    menu.append(&quit)?;
    if !notices.is_empty() {
        menu.prepend(&PredefinedMenuItem::separator(app)?)?;
        for (i, n) in notices.iter().enumerate().rev() {
            let id = if n.opens_login_items { "login-items".to_string() } else { format!("notice-{i}") };
            menu.prepend(&MenuItem::with_id(app, id, &n.text, n.opens_login_items, None::<&str>)?)?;
            log(&format!("tray: notice: {}", n.text));
        }
    }
    TrayIconBuilder::with_id("bellwether")
        .icon(icon())
        .icon_as_template(true)
        .tooltip("Bellwether")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    app.manage(TrayItems { mute, hide, labels });
    log(&format!("tray: {}", menu_words(notices.len(), setup)));
    Ok(())
}

fn on_menu(app: &AppHandle<Wry>, id: &str) {
    let switches = app.state::<Switches>();
    let brain = app.state::<Arc<Brain>>().inner().clone();
    match id {
        "mute" => {
            let action = if brain.muted() { "unmute" } else { "mute" };
            let (port, token) = (switches.port, switches.token.clone());
            std::thread::spawn(move || match brain::control(port, token.as_deref(), action) {
                Ok(()) => log(&format!("tray: {action}")),
                Err(e) => log(&format!("warning: tray {action} failed: {e} (is the brain running?)")),
            });
        }
        "hide" | "labels" => {
            let kept = {
                let mut k = switches.kept.lock().unwrap();
                if id == "hide" {
                    k.hidden = !k.hidden;
                } else {
                    k.show_labels = !k.show_labels;
                }
                *k
            };
            if let Err(e) = privacy::save_kept(&switches.data_dir, kept) {
                log(&format!("warning: could not save {}: {e}", privacy::kept_path(&switches.data_dir).display()));
            }
            log(&format!("tray: hide {} labels {}", kept.hidden, kept.show_labels));
            brain.update_view(|v| v.show_labels = kept.show_labels);
            sync(app, brain.muted(), kept);
        }
        "setup" => {
            log("tray: Set up Bellwether…");
            crate::onboarding::open(app);
        }
        "login-items" => {
            log("tray: opening System Settings > General > Login Items");
            crate::login_items::open_settings();
        }
        "quit" => {
            log("tray: quit");
            app.exit(0);
        }
        _ => {}
    }
}

/// Keeps the check marks in step with the state (the CLI can mute too).
pub fn sync(app: &AppHandle<Wry>, muted: bool, kept: Kept) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.mute.set_checked(muted);
        let _ = items.hide.set_checked(kept.hidden);
        let _ = items.labels.set_checked(kept.show_labels);
    }
}

/// A small sheep for the menu bar, drawn here (template image: black on transparent,
/// so macOS tints it for light and dark menu bars). No icon file to ship.
fn icon() -> Image<'static> {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    let mut dot = |x: f64, y: f64, r: f64| {
        for py in 0..N {
            for px in 0..N {
                let (dx, dy) = (px as f64 + 0.5 - x, py as f64 + 0.5 - y);
                if dx * dx + dy * dy <= r * r {
                    rgba[(py * N + px) * 4 + 3] = 255;
                }
            }
        }
    };
    // Fleece: overlapping puffs; then the head and four legs.
    for (x, y, r) in [(10.0, 14.0, 6.0), (16.0, 11.0, 6.5), (22.0, 14.0, 6.0), (13.0, 18.0, 6.0), (19.0, 18.0, 6.0)] {
        dot(x, y, r);
    }
    dot(27.0, 12.0, 3.5);
    for x in [10.0, 13.0, 19.0, 22.0] {
        for y in 22..28 {
            dot(x, y as f64, 1.2);
        }
    }
    Image::new_owned(rgba, N as u32, N as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bellwether_brain::legacy::{Agent, Loaded};
    use crate::login_items::Service;

    #[test]
    fn the_tray_names_legacy_agents_and_offers_the_login_items_pane() {
        assert!(notices(&[], &[]).is_empty(), "nothing to say: no extra lines");
        let legacy = [
            Agent { label: "local.bellwether.brain", plist: Some("/u/agents/local.bellwether.brain.plist".into()), loaded: Loaded::Running(25449) },
            Agent { label: "local.pasture.shell", plist: Some("/u/agents/local.pasture.shell.plist".into()), loaded: Loaded::No },
        ];
        let n = notices(&legacy, &[Service::Brain]);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].text, "Legacy agents: local.bellwether.brain (running), local.pasture.shell (starts at login) (see bellwether doctor)");
        assert!(!n[0].opens_login_items, "information only");
        assert_eq!(n[1], Notice { text: "Approve the brain agent in Login Items…".into(), opens_login_items: true });
        // The click opens the pane, through SMAppService; the menu never registers or
        // unregisters an item. (tester-m3 checks that tray.rs runs no command and writes
        // no file.)
        let src = include_str!("tray.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("\"login-items\" => {") && code.contains("crate::login_items::open_settings()"));
        assert!(!code.contains("register("), "the tray changes no login item");
    }

    /// Jeff's relabel (deviation 61): the ids and what they do are unchanged; only the words
    /// say which one keeps the "!" (Quiet) and which hides everything (Hide everything).
    #[test]
    fn the_switches_say_what_they_hide() {
        assert_eq!(menu_words(0, false), "Quiet (alerts only), Hide everything (for screen sharing), Show labels, Quit");
        assert_eq!(menu_words(1, true), "1 notice(s), Quiet (alerts only), Hide everything (for screen sharing), Show labels, Set up Bellwether…, Quit");
        let src = include_str!("tray.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        // (Split, so tester-m3's scan of tray.rs for menu ids doesn't count these.)
        assert!(code.contains(concat!("CheckMenuItem::with_", r#"id(app, "mute", QUIET,"#)) && code.contains(concat!("CheckMenuItem::with_", r#"id(app, "hide", HIDE,"#)));
        assert!(code.contains(r#""mute" => {"#) && code.contains(r#"let action = if brain.muted() { "unmute" } else { "mute" };"#), "Quiet is still the brain's mute");
        assert!(code.contains(r#"if id == "hide" {"#) && code.contains("k.hidden = !k.hidden;"), "Hide everything is still shell-state's hidden");
        assert!(!code.contains("Mute (do not disturb)") && !code.contains("\"Hide the strip\""));
    }
}
