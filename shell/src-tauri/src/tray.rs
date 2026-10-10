//! The menu-bar icon: Mute, Hide the strip, Show labels, Quit.
//! Everything here runs in Rust; the webview gains no command or permission.
//!
//! - Mute is the brain's do-not-disturb (POST /control), the same as `pasture mute`,
//!   so it is kept in data/state.json and the two always agree.
//! - Hide and Show labels are the shell's own, kept in data/shell-state.json.
//! - Autostart is not here: `pasture autostart` (Milestone 3) stays the one way to set
//!   it up, and the tray doesn't write LaunchAgents (deviation M7-4).

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

pub fn build(app: &AppHandle<Wry>, kept: Kept, muted: bool) -> tauri::Result<()> {
    let mute = CheckMenuItem::with_id(app, "mute", "Mute (do not disturb)", true, muted, None::<&str>)?;
    let hide = CheckMenuItem::with_id(app, "hide", "Hide the strip", true, kept.hidden, None::<&str>)?;
    let labels = CheckMenuItem::with_id(app, "labels", "Show labels", true, kept.show_labels, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit pasture", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&mute, &hide, &labels, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id("pasture")
        .icon(icon())
        .icon_as_template(true)
        .tooltip("pasture")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    app.manage(TrayItems { mute, hide, labels });
    log("tray: Mute, Hide the strip, Show labels, Quit");
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
