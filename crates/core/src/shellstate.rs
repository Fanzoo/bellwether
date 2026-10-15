//! shell-state.json: the tray's "Hide everything (for screen sharing)" and "Show labels" (a port of the
//! shell-state half of brain/privacy.mjs). `bellwether show` / `hide` write it; the strip
//! watches it, so they work with or without a running strip.
//!
//! The app also keeps `"onboarded": true` there once the onboarding window's Done was
//! clicked (plan D6, milestone 7). Every writer keeps that key when it is present, and
//! never adds it otherwise, so a file without it stays byte-for-byte what Node writes.

use std::path::{Path, PathBuf};

use crate::json::{self, Js, Obj};

pub fn file(home: &Path) -> PathBuf {
    home.join("shell-state.json")
}

/// The switches; a missing or unreadable file means the defaults (shown, no labels).
pub fn read(home: &Path) -> (bool, bool) {
    let v = std::fs::read_to_string(file(home)).ok().and_then(|t| json::parse(&t).ok()).unwrap_or(Js::Null);
    (v.get("hidden") == &Js::Bool(true), v.get("showLabels") == &Js::Bool(true))
}

/// Whether the onboarding window was finished (its Done) for this home.
pub fn onboarded(home: &Path) -> bool {
    let v = std::fs::read_to_string(file(home)).ok().and_then(|t| json::parse(&t).ok()).unwrap_or(Js::Null);
    v.get("onboarded") == &Js::Bool(true)
}

/// Writes the switches the way the strip does (a temporary file, then a rename), keeping
/// an `onboarded` flag the file already has.
pub fn write(home: &Path, hidden: bool, show_labels: bool) -> std::io::Result<()> {
    write_all(home, hidden, show_labels, onboarded(home))
}

/// Marks onboarding done, keeping the switches as they are.
pub fn set_onboarded(home: &Path) -> std::io::Result<()> {
    let (hidden, show_labels) = read(home);
    write_all(home, hidden, show_labels, true)
}

fn write_all(home: &Path, hidden: bool, show_labels: bool, onboarded: bool) -> std::io::Result<()> {
    crate::persist::create_private_dir(home)?;
    let mut o = Obj::new().with("version", 1.0).with("hidden", hidden).with("showLabels", show_labels);
    if onboarded {
        o.set("onboarded", Js::Bool(true));
    }
    let text = json::stringify_pretty(&Js::Obj(o), "  ");
    crate::persist::write_atomic(&file(home), &home.join("shell-state.json.tmp"), &text, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_onboarded_flag_survives_show_and_hide_and_is_never_added_by_them() {
        let home = std::env::temp_dir().join(format!("bw-shellstate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        write(&home, true, false).unwrap();
        let plain = std::fs::read_to_string(file(&home)).unwrap();
        assert_eq!(plain, "{\n  \"version\": 1,\n  \"hidden\": true,\n  \"showLabels\": false\n}", "Node's shape, no flag");
        assert!(!onboarded(&home));
        set_onboarded(&home).unwrap();
        assert!(onboarded(&home));
        assert_eq!(read(&home), (true, false), "the switches are kept");
        write(&home, false, true).unwrap();
        assert!(onboarded(&home), "`bellwether show` keeps it");
        assert_eq!(read(&home), (false, true));
        let _ = std::fs::remove_dir_all(&home);
    }
}
