//! Unobtrusiveness: when the strip hides, and what the tray
//! switches keep between runs.
//!
//! The strip hides, whole, while:
//! - the tray's "Hide everything (for screen sharing)" is on (kept in data/shell-state.json), or
//! - a process named in `privacy.sharingProcesses` is running (Zoom's `CptHost` while
//!   it shares, say), when `privacy.hideWhenSharing` is on.
//! It hides at once and shows again only after the reason has been gone for 2 s, so a
//! share that stops and starts again doesn't flash the strip.
//!
//! A full-screen app on a macOS full-screen Space needs no detector: the strip isn't
//! `fullScreenAuxiliary`, so macOS keeps it off those Spaces (M3-10, measured in
//! Milestone 7). Some apps do their own full screen instead, on an ordinary Space with the
//! Dock and menu bar hidden (Parallels' "Optimize full screen for games"); the strip
//! stayed over them, on the screen's bottom edge. `covered_monitors` finds those, and
//! with `privacy.hideWhenFullscreen` on (the default) that monitor's strip hides, with
//! the same 2 s debounce on the way back.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a reason to hide must be gone before the strip shows again.
pub const SHOW_DEBOUNCE: Duration = Duration::from_secs(2);
/// How often the process list is read.
pub const SHARING_POLL: Duration = Duration::from_millis(500);

/// How often the window list is read for the full-screen check.
pub const FULLSCREEN_POLL: Duration = Duration::from_millis(500);

/// The monitors (by name) an app covers with its own full screen. Both must hold:
/// - the Dock has no window on screen although its settings say it shows (not auto-hidden):
///   an app has hidden it. This is what tells a full screen from a maximised window;
/// - the frontmost ordinary window on the monitor spans its full width, from at most the
///   menu bar's height down to its bottom edge.
///
/// Measured with Parallels (2026-10-28): its window is 0,33 1728x1084 on a 1728x1117
/// screen, and the Dock's window (layer 20, the whole screen) is gone. During the swipe
/// onto its Space the Dock is still there, so nothing is covered until it settles.
pub fn covered_monitors(windows: &[(i32, crate::locator::Win)], monitors: &[(String, crate::locator::Rect)], dock_autohide: bool) -> Vec<String> {
    if dock_autohide || windows.iter().any(|(_, w)| w.owner == "Dock") {
        return Vec::new();
    }
    const MENU_BAR: f64 = 40.0;
    monitors
        .iter()
        .filter(|(_, m)| {
            let front = windows.iter().filter(|(layer, _)| *layer == 0).map(|(_, w)| w.bounds).find(|b| {
                let w = (b.x + b.w).min(m.x + m.w) - b.x.max(m.x);
                let h = (b.y + b.h).min(m.y + m.h) - b.y.max(m.y);
                w >= 50.0 && h >= 50.0
            });
            front.is_some_and(|b| b.x <= m.x + 1.0 && b.x + b.w >= m.x + m.w - 1.0 && b.y <= m.y + MENU_BAR && b.y + b.h >= m.y + m.h - 1.0)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// The first running process whose name is in `wanted` (case-insensitive, whole name).
pub fn sharing_match(running: &[String], wanted: &[String]) -> Option<String> {
    running.iter().find(|n| wanted.iter().any(|w| !w.is_empty() && w.eq_ignore_ascii_case(n))).cloned()
}

/// This build's platform name, as `privacy.sharingProcesses` spells it.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "linux"
    }
}

/// `privacy.captureExclusion` when config.json doesn't set it: on everywhere but Windows.
/// On Windows it hides the sheep from the user's own screenshots too, so it is an opt-in
/// there (Windows plan WD5, the maintainer's decision Q9, 2026-10-20). macOS keeps what it
/// has always shipped.
pub fn capture_exclusion_default(os: &str) -> bool {
    os != "win32"
}

/// Whether the strips are left out of screen captures, and why: config.json's
/// `privacy.captureExclusion` when it is a boolean (on any OS), else this OS's default.
pub fn capture_exclusion(config: &Value, os: &str) -> (bool, String) {
    match config.pointer("/privacy/captureExclusion").and_then(Value::as_bool) {
        Some(v) => (v, "privacy.captureExclusion in config.json".into()),
        None => {
            let why = match os {
                "darwin" => "the macOS default",
                "win32" => "the Windows default; privacy.captureExclusion: true turns it on",
                _ => "the default",
            };
            (capture_exclusion_default(os), why.into())
        }
    }
}

/// The names from `privacy.sharingProcesses` for this OS.
pub fn wanted_names(config: &Value) -> Vec<String> {
    let os = os_name();
    config
        .pointer(&format!("/privacy/sharingProcesses/{os}"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(64).collect())
        .unwrap_or_default()
}

/// Why each `privacy.sharingProcesses` entry for this OS can never match (empty if all
/// can). The same rules as brain/privacy.mjs, which `bellwether doctor` uses.
pub fn sharing_warnings(config: &Value) -> Vec<String> {
    sharing_warnings_for(config, os_name())
}

/// The same rules for one platform's list (`darwin`, `win32`, `linux`), as
/// sharingProcessWarnings(config, platform) in brain/privacy.mjs has them.
pub fn sharing_warnings_for(config: &Value, os: &str) -> Vec<String> {
    let Some(list) = config.pointer(&format!("/privacy/sharingProcesses/{os}")) else { return Vec::new() };
    let Some(list) = list.as_array() else { return vec![format!("privacy.sharingProcesses.{os} is not a list of process names")] };
    list.iter()
        .filter_map(|v| {
            let why = match v.as_str() {
                None => "is not a string",
                Some(n) if n.trim().is_empty() => "is empty",
                Some(n) if n != n.trim() => "has spaces at either end; process names are compared whole",
                Some(n) if n.contains('/') => "is a path; list process names (such as CptHost), not paths",
                Some(n) if os == "darwin" && n.len() > 31 => "is longer than 31 bytes, and macOS cuts process names there; use its first 31 bytes",
                Some(n) if os == "darwin" && n.to_ascii_lowercase().ends_with(".exe") => "is a Windows program name; it never runs on macOS",
                Some(_) => return None,
            };
            Some(format!("privacy.sharingProcesses.{os} entry {v} {why}"))
        })
        .collect()
}

/// The file's modification time, to notice `bellwether show` / `bellwether hide`.
pub fn kept_stamp(data_dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(kept_path(data_dir)).and_then(|m| m.modified()).ok()
}

/// Hide at once; show only after `SHOW_DEBOUNCE` with no reason to hide.
pub struct Debounce {
    hidden: bool,
    clear_since: Option<Instant>,
}

impl Debounce {
    pub fn new() -> Debounce {
        Debounce { hidden: false, clear_since: None }
    }

    /// Feeds whether there is a reason to hide now; returns whether the strip is hidden.
    pub fn update(&mut self, reason: bool, now: Instant) -> bool {
        if reason {
            self.hidden = true;
            self.clear_since = None;
        } else if self.hidden {
            let since = *self.clear_since.get_or_insert(now);
            if now.duration_since(since) >= SHOW_DEBOUNCE {
                self.hidden = false;
                self.clear_since = None;
            }
        }
        self.hidden
    }
}

/// What the tray switches keep between runs (data/shell-state.json): "Hide everything (for
/// screen sharing)" and "Show labels". "Quiet (alerts only)" is the brain's mute
/// (data/state.json), so `bellwether mute` and the tray agree.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Kept {
    pub hidden: bool,
    pub show_labels: bool,
}

pub fn kept_path(data_dir: &Path) -> PathBuf {
    data_dir.join("shell-state.json")
}

/// Missing or unreadable means the defaults (shown, no labels).
pub fn load_kept(data_dir: &Path) -> Kept {
    let Ok(text) = std::fs::read_to_string(kept_path(data_dir)) else { return Kept::default() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Kept::default() };
    Kept { hidden: v.get("hidden") == Some(&Value::Bool(true)), show_labels: v.get("showLabels") == Some(&Value::Bool(true)) }
}

/// Keeps an `onboarded` flag the file already has (the app's onboarding, milestone 7).
pub fn save_kept(data_dir: &Path, kept: Kept) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join("shell-state.json.tmp");
    let mut v = json!({"version": 1, "hidden": kept.hidden, "showLabels": kept.show_labels});
    if bellwether_core::shellstate::onboarded(data_dir) {
        v["onboarded"] = Value::Bool(true);
    }
    std::fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
    std::fs::rename(tmp, kept_path(data_dir))
}

/// Names of the running processes (macOS: libproc, no subprocess). Names longer than
/// 31 characters are cut there by the kernel (measured), so list names of at most 31.
#[cfg(all(target_os = "macos", test))]
pub fn running_names() -> Vec<String> {
    running_pids().into_iter().filter_map(proc_name).collect()
}

#[cfg(target_os = "macos")]
fn running_pids() -> Vec<i32> {
    use std::os::raw::c_void;
    let mut pids = vec![0i32; 4096];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * std::mem::size_of::<i32>()) as i32) };
    if n <= 0 {
        return Vec::new();
    }
    pids.truncate((n as usize).min(4096));
    pids.retain(|&p| p > 0);
    pids
}

#[cfg(target_os = "macos")]
fn proc_name(pid: i32) -> Option<String> {
    use std::os::raw::c_void;
    let mut buf = [0u8; 256];
    let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
}

/// The process list, cheaply: one call for the pids, and a name lookup only for pids
/// not seen before (about 700 lookups a poll cost ~0.3% CPU, measured). Everything is
/// looked up afresh every 10 s, in case a pid was reused.
// Read on macOS only until W5 (a Toolhelp snapshot on Windows).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Default)]
pub struct ProcessNames {
    names: std::collections::HashMap<i32, Option<String>>,
    refreshed: Option<Instant>,
}

impl ProcessNames {
    #[cfg(target_os = "macos")]
    pub fn read(&mut self, now: Instant) -> Vec<String> {
        if self.refreshed.map_or(true, |t| now.duration_since(t) >= Duration::from_secs(10)) {
            self.names.clear();
            self.refreshed = Some(now);
        }
        let pids = running_pids();
        let live: std::collections::HashSet<i32> = pids.iter().copied().collect();
        self.names.retain(|p, _| live.contains(p));
        pids.iter().filter_map(|&p| self.names.entry(p).or_insert_with(|| proc_name(p)).clone()).collect()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read(&mut self, _now: Instant) -> Vec<String> {
        Vec::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator::{Rect, Win};

    fn w(layer: i32, owner: &str, x: f64, y: f64, wd: f64, h: f64) -> (i32, Win) {
        (layer, Win { pid: 1, owner: owner.into(), bounds: Rect { x, y, w: wd, h } })
    }
    const MAIN: (f64, f64, f64, f64) = (0.0, 0.0, 1728.0, 1117.0);
    fn monitors() -> Vec<(String, Rect)> {
        vec![("main".into(), Rect { x: MAIN.0, y: MAIN.1, w: MAIN.2, h: MAIN.3 }), ("side".into(), Rect { x: 1728.0, y: 0.0, w: 1920.0, h: 1080.0 })]
    }

    #[test]
    fn parallels_games_full_screen_covers_its_monitor() {
        // Recorded with Parallels' "Optimize full screen for games", once settled.
        let wins = [
            w(26, "Parallels Desktop", 0.0, 0.0, 1728.0, 33.0),
            w(24, "Window Server", 0.0, 0.0, 1728.0, 33.0),
            w(5, "Bellwether", 0.0, 911.0, 1728.0, 140.0),
            w(0, "Parallels Desktop", 0.0, -5.0, 1728.0, 54.0),
            w(0, "Parallels Desktop", 0.0, 33.0, 1728.0, 1084.0),
        ];
        assert_eq!(covered_monitors(&wins, &monitors(), false), ["main"]);
        assert!(covered_monitors(&wins, &monitors(), true).is_empty(), "an auto-hidden Dock proves nothing");
    }

    #[test]
    fn ordinary_desktops_and_the_swipe_onto_parallels_cover_nothing() {
        // Recorded: the normal desktop (the Dock's window is there).
        let desktop = [w(20, "Dock", 0.0, 0.0, 1728.0, 1117.0), w(5, "Bellwether", 0.0, 911.0, 1728.0, 140.0), w(0, "iTerm2", 10.0, 43.0, 1708.0, 997.0)];
        assert!(covered_monitors(&desktop, &monitors(), false).is_empty());
        // Recorded: mid-swipe, Parallels already covers everything but the Dock is still up.
        let swipe = [w(0, "Parallels Desktop", 0.0, 0.0, 1728.0, 1117.0), w(27, "Dock", 0.0, 0.0, 1728.0, 1117.0), w(0, "iTerm2", 154.0, 1019.0, 1708.0, 997.0)];
        assert!(covered_monitors(&swipe, &monitors(), false).is_empty());
        // A maximised window with the Dock shown: the Dock proves it isn't full screen.
        let zoomed = [w(20, "Dock", 0.0, 0.0, 1728.0, 1117.0), w(0, "Safari", 0.0, 33.0, 1728.0, 1084.0)];
        assert!(covered_monitors(&zoomed, &monitors(), false).is_empty());
    }

    #[test]
    fn only_the_monitor_whose_front_window_covers_it_is_covered() {
        let wins = [
            w(0, "Parallels Desktop", 0.0, 33.0, 1728.0, 1084.0),
            w(0, "Notes", 1800.0, 100.0, 800.0, 600.0),
            w(0, "Safari", 1728.0, 25.0, 1920.0, 1055.0),
        ];
        assert_eq!(covered_monitors(&wins, &monitors(), false), ["main"], "Notes is in front on the side monitor");
        // A window short of the bottom, or narrower than the monitor, covers nothing.
        let short = [w(0, "Parallels Desktop", 0.0, 33.0, 1728.0, 1000.0)];
        assert!(covered_monitors(&short, &monitors(), false).is_empty());
        let narrow = [w(0, "Parallels Desktop", 100.0, 33.0, 1600.0, 1084.0)];
        assert!(covered_monitors(&narrow, &monitors(), false).is_empty());
    }

    #[test]
    fn capture_exclusion_defaults_off_on_windows_only_and_config_wins_everywhere() {
        let none = serde_json::json!({"privacy": {"hideWhenSharing": true}});
        assert_eq!(capture_exclusion(&none, "darwin"), (true, "the macOS default".into()), "macOS unchanged");
        assert_eq!(capture_exclusion(&none, "win32"), (false, "the Windows default; privacy.captureExclusion: true turns it on".into()), "Windows: the sheep stay in his screenshots (Q9)");
        assert_eq!(capture_exclusion(&serde_json::json!({}), "linux"), (true, "the default".into()));
        for os in ["darwin", "win32", "linux"] {
            for v in [true, false] {
                let c = serde_json::json!({"privacy": {"captureExclusion": v}});
                assert_eq!(capture_exclusion(&c, os), (v, "privacy.captureExclusion in config.json".into()), "{os} {v}");
            }
            // Not a boolean: the OS's default, as an absent key.
            let odd = serde_json::json!({"privacy": {"captureExclusion": "yes"}});
            assert_eq!(capture_exclusion(&odd, os).0, capture_exclusion_default(os));
        }
        // The example config doesn't set it, so a copy of it gets each OS's default.
        let example: Value = serde_json::from_str(include_str!("../../../config.example.json")).unwrap();
        assert_eq!(example.pointer("/privacy/captureExclusion"), None);
        assert!(!capture_exclusion(&example, "win32").0 && capture_exclusion(&example, "darwin").0);
        assert_eq!(os_name(), if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" });
    }

    #[test]
    fn sharing_names_match_whole_and_case_insensitively() {
        let running: Vec<String> = ["launchd", "zoom.us", "CptHost", "Terminal"].iter().map(|s| s.to_string()).collect();
        let wanted = vec!["cpthost".to_string(), "zoom.us Screen Share".to_string()];
        assert_eq!(sharing_match(&running, &wanted), Some("CptHost".into()));
        assert_eq!(sharing_match(&running, &["Cpt".to_string()]), None, "a part of a name is not a match");
        assert_eq!(sharing_match(&running, &[String::new()]), None);
        assert_eq!(sharing_match(&[], &wanted), None);
    }

    #[test]
    fn wanted_names_come_from_this_os() {
        let cfg = json!({"privacy": {"sharingProcesses": {"darwin": ["CptHost"], "win32": ["CptHost.exe"], "linux": []}}});
        let names = wanted_names(&cfg);
        if cfg!(target_os = "macos") {
            assert_eq!(names, vec!["CptHost".to_string()]);
        }
        assert!(wanted_names(&json!({})).is_empty());
    }

    #[test]
    fn entries_that_can_never_match_are_reported() {
        let cfg = json!({"privacy": {"sharingProcesses": {"darwin": ["CptHost", "", "x".repeat(40), "/Applications/zoom.us", "Teams.exe", " zoom ", 3], "win32": [], "linux": []}}});
        let w = sharing_warnings(&cfg);
        if cfg!(target_os = "macos") {
            assert_eq!(w.len(), 6, "{w:?}");
            assert!(w[1].contains("longer than 31 bytes"));
            assert!(w.iter().all(|m| !m.contains("entry \"CptHost\"")));
        }
        assert!(sharing_warnings(&json!({})).is_empty());
    }

    #[test]
    fn hides_at_once_and_shows_after_two_quiet_seconds() {
        let t0 = Instant::now();
        let mut d = Debounce::new();
        assert!(!d.update(false, t0));
        assert!(d.update(true, t0));
        assert!(d.update(false, t0 + Duration::from_millis(500)), "still hidden 0.5 s after");
        assert!(d.update(true, t0 + Duration::from_millis(1500)), "the reason came back: the wait starts over");
        assert!(d.update(false, t0 + Duration::from_millis(1600)));
        assert!(d.update(false, t0 + Duration::from_millis(3500)));
        assert!(!d.update(false, t0 + Duration::from_millis(3700)), "2 s after the reason went");
    }

    #[test]
    fn kept_switches_round_trip_and_default_when_missing_or_bad() {
        let dir = std::env::temp_dir().join(format!("bellwether-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_kept(&dir), Kept::default());
        save_kept(&dir, Kept { hidden: true, show_labels: true }).unwrap();
        assert_eq!(load_kept(&dir), Kept { hidden: true, show_labels: true });
        std::fs::write(kept_path(&dir), "not json").unwrap();
        assert_eq!(load_kept(&dir), Kept::default());
        // The onboarding window's Done is kept by the tray's switches, and never added by them.
        save_kept(&dir, Kept::default()).unwrap();
        assert!(!std::fs::read_to_string(kept_path(&dir)).unwrap().contains("onboarded"));
        bellwether_core::shellstate::set_onboarded(&dir).unwrap();
        save_kept(&dir, Kept { hidden: true, show_labels: false }).unwrap();
        assert!(bellwether_core::shellstate::onboarded(&dir));
        assert_eq!(load_kept(&dir), Kept { hidden: true, show_labels: false });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_process_list_includes_this_test() {
        let names = running_names();
        assert!(names.len() > 10);
        // The same list from the cache as from a fresh read. Other tests running at the same
        // time start processes (launchctl, codesign), so a pair that differs is read again.
        let same = (0..20).any(|_| {
            let mut cached = ProcessNames::default();
            let t = Instant::now();
            cached.read(t).len() == cached.read(t + Duration::from_millis(500)).len()
        });
        assert!(same, "the cache gives the same list");
        let me = std::env::current_exe().unwrap();
        let mine: String = me.file_name().unwrap().to_string_lossy().chars().take(31).collect();
        assert!(names.iter().any(|n| *n == mine), "{mine} not in the list: {:?}", names.iter().filter(|n| n.starts_with("bellwether")).collect::<Vec<_>>());
    }
}
