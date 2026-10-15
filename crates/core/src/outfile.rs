//! The guards for `record --out` and `hooks print --out` (a port of brain/outfile.mjs). A
//! typo must not destroy data: a Claude Code settings file is always refused, `--force` or
//! not; a directory or anything that isn't a regular file is refused with a message; an
//! existing file is appended to only if it is already a bellwether recording (or empty),
//! and replaced only if it is already a bellwether hooks file (or with `--force`).

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

/// Exactly what `hooks print` writes: only a `hooks` key, and only bellwether http
/// handlers in it.
pub fn is_bellwether_hooks_file(text: &str) -> bool {
    let Ok(Js::Obj(v)) = json::parse(text) else { return false };
    if v.keys() != ["hooks"] {
        return false;
    }
    let groups: Vec<&Js> = match v.get("hooks") {
        Some(Js::Obj(h)) => h.iter().map(|(_, g)| g).collect(),
        _ => Vec::new(),
    };
    let mut handlers: Vec<Option<&Js>> = Vec::new();
    for g in groups {
        let list: Vec<Option<&Js>> = match g {
            Js::Arr(a) => a.iter().map(Some).collect(),
            _ => vec![None],
        };
        for g in list {
            match g.map(|g| g.get("hooks")) {
                Some(Js::Arr(hs)) => handlers.extend(hs.iter().map(Some)),
                _ => handlers.push(None),
            }
        }
    }
    let url_ok = |u: &str| u.strip_prefix("http://127.0.0.1:").and_then(|r| {
        let digits = r.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0).then(|| r[digits..].starts_with("/hook"))
    }) == Some(true);
    !handlers.is_empty()
        && handlers.iter().all(|h| {
            let Some(h) = h else { return false };
            let headers = h.get("headers");
            let token = match headers.get("X-Bellwether-Token") {
                Js::Undefined | Js::Null => headers.get("X-Pasture-Token"),
                t => t,
            };
            h.get("type").as_str() == Some("http") && h.get("url").as_str().is_some_and(url_ok) && token.as_str().is_some()
        })
}

/// `guardOutFile(target, 'print', { force })`.
pub fn guard_print(target: &Path, force: bool) -> Result<(), ConfigError> {
    let Some(text) = guard_common(target, "hooks-only.json")? else { return Ok(()) };
    if force || is_bellwether_hooks_file(&text) {
        return Ok(());
    }
    Err(ConfigError::new(format!("{} already exists and is not a bellwether hooks file, so it was left alone. Choose another path, or pass --force to replace it.", target.display())))
}

/// What both guards check first. Returns the existing file's text, or None for no file.
fn guard_common(target: &Path, example: &str) -> Result<Option<String>, ConfigError> {
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
    let Some(stat) = stat else { return Ok(None) };
    if stat.is_dir() {
        return Err(ConfigError::new(format!("{} is a directory; give a file path, e.g. {}", target.display(), target.join(example).display())));
    }
    if !stat.is_file() {
        return Err(ConfigError::new(format!("{} is not a regular file; refusing to write it", target.display())));
    }
    Ok(Some(std::fs::read(target).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()))
}

/// `guardOutFile(target, 'record')`.
pub fn guard_record(target: &Path) -> Result<(), ConfigError> {
    let Some(text) = guard_common(target, "recording.jsonl")? else { return Ok(()) };
    if text.is_empty() || is_bellwether_recording(&text) {
        return Ok(());
    }
    Err(ConfigError::new(format!("{} already exists and is not a bellwether recording, so nothing was appended. Choose another path.", target.display())))
}
