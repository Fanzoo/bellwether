//! shell-state.json: the tray's "Hide the strip" and "Show labels" (a port of the
//! shell-state half of brain/privacy.mjs). `bellwether show` / `hide` write it; the strip
//! watches it, so they work with or without a running strip.

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

/// Writes the switches the way the strip does (a temporary file, then a rename).
pub fn write(home: &Path, hidden: bool, show_labels: bool) -> std::io::Result<()> {
    crate::persist::create_private_dir(home)?;
    let text = json::stringify_pretty(&Js::Obj(Obj::new().with("version", 1.0).with("hidden", hidden).with("showLabels", show_labels)), "  ");
    crate::persist::write_atomic(&file(home), &home.join("shell-state.json.tmp"), &text, 0o600)
}
