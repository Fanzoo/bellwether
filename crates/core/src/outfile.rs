//! The guard for `record --out` (a port of brain/outfile.mjs; `hooks print --out` joins
//! it with the installer in milestone 4). A typo must not destroy data: a Claude Code
//! settings file is always refused; a directory or anything that isn't a regular file is
//! refused with a message; an existing file is appended to only if it is already a
//! bellwether recording (or empty).

use std::path::{Path, PathBuf};

use crate::config::{io_code, ConfigError};
use crate::json::{self, Js};

/// `~/.claude/settings.json`, from HOME.
pub fn user_settings() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".claude").join("settings.json")
}

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// settings.json / settings.local.json in a `.claude` directory, managed-settings.json
/// anywhere, or the user file itself.
pub fn is_claude_settings_path(p: &Path) -> bool {
    let base = lower_name(p);
    let dir = p.parent().map(lower_name).unwrap_or_default();
    if base == "managed-settings.json" {
        return true;
    }
    if (base == "settings.json" || base == "settings.local.json") && dir == ".claude" {
        return true;
    }
    let norm = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| absolute(x));
    norm(p) == norm(&user_settings())
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Every line is a recording line: `{normalised, rawKeys, headersPresent}`.
pub fn is_bellwether_recording(text: &str) -> bool {
    let lines: Vec<&str> = text.split('\n').filter(|l| !crate::jsstr::trim(l).is_empty()).collect();
    !lines.is_empty()
        && lines.iter().all(|l| match json::parse(l) {
            Ok(Js::Obj(o)) => {
                let mut keys: Vec<&String> = o.keys();
                keys.sort();
                keys == ["headersPresent", "normalised", "rawKeys"]
            }
            _ => false,
        })
}

/// `guardOutFile(target, 'record')`.
pub fn guard_record(target: &Path) -> Result<(), ConfigError> {
    let (stat, real) = match std::fs::metadata(target) {
        Ok(m) => (Some(m), std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, target.to_path_buf()),
        Err(e) => return Err(ConfigError::new(format!("cannot use {}: {}", target.display(), io_code(&e)))),
    };
    if is_claude_settings_path(target) || is_claude_settings_path(&real) {
        return Err(ConfigError::new(format!(
            "refusing to write {}: it is a Claude Code settings file. Use `bellwether hooks install` to change settings (it shows a diff and keeps a backup).",
            target.display()
        )));
    }
    let Some(stat) = stat else { return Ok(()) };
    if stat.is_dir() {
        return Err(ConfigError::new(format!("{} is a directory; give a file path, e.g. {}", target.display(), target.join("recording.jsonl").display())));
    }
    if !stat.is_file() {
        return Err(ConfigError::new(format!("{} is not a regular file; refusing to write it", target.display())));
    }
    let text = std::fs::read(target).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    if text.is_empty() || is_bellwether_recording(&text) {
        return Ok(());
    }
    Err(ConfigError::new(format!("{} already exists and is not a bellwether recording, so nothing was appended. Choose another path.", target.display())))
}
