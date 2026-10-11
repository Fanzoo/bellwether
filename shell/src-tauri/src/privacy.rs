//! Unobtrusiveness: when the strip hides, and what the tray
//! switches keep between runs.
//!
//! The strip hides, whole, while:
//! - the tray's "Hide the strip" is on (kept in data/shell-state.json), or
//! - a process named in `privacy.sharingProcesses` is running (Zoom's `CptHost` while
//!   it shares, say), when `privacy.hideWhenSharing` is on.
//! It hides at once and shows again only after the reason has been gone for 2 s, so a
//! share that stops and starts again doesn't flash the strip.
//!
//! Full-screen apps need no detector on macOS: the strip isn't `fullScreenAuxiliary`,
//! so macOS keeps it off full-screen Spaces (M3-10, measured in Milestone 7).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a reason to hide must be gone before the strip shows again.
pub const SHOW_DEBOUNCE: Duration = Duration::from_secs(2);
/// How often the process list is read.
pub const SHARING_POLL: Duration = Duration::from_millis(500);

/// The first running process whose name is in `wanted` (case-insensitive, whole name).
pub fn sharing_match(running: &[String], wanted: &[String]) -> Option<String> {
    running.iter().find(|n| wanted.iter().any(|w| !w.is_empty() && w.eq_ignore_ascii_case(n))).cloned()
}

/// The names from `privacy.sharingProcesses` for this OS.
pub fn wanted_names(config: &Value) -> Vec<String> {
    let os = if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" };
    config
        .pointer(&format!("/privacy/sharingProcesses/{os}"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(64).collect())
        .unwrap_or_default()
}

/// Why each `privacy.sharingProcesses` entry for this OS can never match (empty if all
/// can). The same rules as brain/privacy.mjs, which `bellwether doctor` uses.
pub fn sharing_warnings(config: &Value) -> Vec<String> {
    let os = if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" };
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

/// What the tray switches keep between runs (data/shell-state.json). Mute is the
/// brain's (data/state.json), so `bellwether mute` and the tray agree.
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

pub fn save_kept(data_dir: &Path, kept: Kept) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join("shell-state.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&json!({"version": 1, "hidden": kept.hidden, "showLabels": kept.show_labels}))?)?;
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_process_list_includes_this_test() {
        let names = running_names();
        assert!(names.len() > 10);
        let mut cached = ProcessNames::default();
        let t = Instant::now();
        assert_eq!(cached.read(t).len(), cached.read(t + Duration::from_millis(500)).len(), "the cache gives the same list");
        let me = std::env::current_exe().unwrap();
        let mine: String = me.file_name().unwrap().to_string_lossy().chars().take(31).collect();
        assert!(names.iter().any(|n| *n == mine), "{mine} not in the list: {:?}", names.iter().filter(|n| n.starts_with("bellwether")).collect::<Vec<_>>());
    }
}
