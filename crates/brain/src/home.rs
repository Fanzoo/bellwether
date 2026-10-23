//! Where the app keeps its files (distribution plan D3), and the hook token.
//!
//!   ~/Library/Application Support/Bellwether/   (BELLWETHER_HOME overrides it)
//!     hook-token     one line, 0600, in a 0700 directory
//!     config.json    optional; every key has a default (BELLWETHER_CONFIG names another file)
//!     state.json     muted, and per identity its colour, slots and label
//!     shell-state.json, recordings/
//!   ~/Library/Logs/Bellwether/brain.log           (<BELLWETHER_HOME>/logs when it is set)
//!
//! BELLWETHER_HOOK_TOKEN in the environment overrides the token file (tests, CI).
//! `bellwether token init` writes the token file (crates/core's token module).

use std::path::{Path, PathBuf};

use crate::{exit, BrainError};

/// `%USERPROFILE%` on Windows, `$HOME` elsewhere (crates/core's paths).
fn user_home() -> PathBuf {
    bellwether_core::paths::user_home().unwrap_or_else(|| PathBuf::from("."))
}

/// The app-data directory.
pub fn dir() -> PathBuf {
    if let Some(h) = std::env::var_os("BELLWETHER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    platform_dir()
}

#[cfg(target_os = "macos")]
fn platform_dir() -> PathBuf {
    user_home().join("Library").join("Application Support").join("Bellwether")
}

#[cfg(windows)]
fn platform_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(user_home).join("Bellwether")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| user_home().join(".config")).join("bellwether")
}

/// Where brain.log goes. A test home keeps its logs inside it.
pub fn log_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("BELLWETHER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h).join("logs");
    }
    #[cfg(target_os = "macos")]
    return user_home().join("Library").join("Logs").join("Bellwether");
    #[cfg(not(target_os = "macos"))]
    return platform_dir().join("logs");
}

pub fn token_file(home: &Path) -> PathBuf {
    home.join("hook-token")
}

/// The token: BELLWETHER_HOOK_TOKEN if set and not empty, else the first line of
/// `hook-token` (crates/core's token module). Never logged or printed.
pub fn token(home: &Path) -> Result<String, BrainError> {
    if let Some(t) = std::env::var("BELLWETHER_HOOK_TOKEN").ok().filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    let file = token_file(home);
    match bellwether_core::token::read_file(home) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(BrainError::new(format!("no hook token: BELLWETHER_HOOK_TOKEN is not set, and {} doesn't exist or is empty. Make one with `bellwether token init`", file.display()), exit::BAD_CONFIG)),
        Err(e) => Err(BrainError::new(e.message, exit::BAD_CONFIG)),
    }
}

/// `port` from `<home>/config.json`, if that file exists. `host` is refused, as in the
/// Node brain: the bind address is never configuration.
pub fn configured_port(home: &Path) -> Result<Option<u16>, BrainError> {
    let file = home.join("config.json");
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(None) };
    let bad = |m: String| BrainError::new(m, exit::BAD_CONFIG);
    let config: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(format!("{} is not valid JSON: {e}", file.display())))?;
    if config.get("host").is_some() {
        return Err(bad(format!("{}: \"host\" is not configurable; bellwether only ever listens on 127.0.0.1", file.display())));
    }
    match config.get("port") {
        None => Ok(None),
        Some(p) => p.as_u64().filter(|p| (1024..=65535).contains(p)).map(|p| Some(p as u16)).ok_or_else(|| bad(format!("{}: port must be 1024–65535", file.display()))),
    }
}

/// Creates a directory (and its parents) for files only this user may read.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    bellwether_core::persist::create_private_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_from_config() {
        let d = std::env::temp_dir().join(format!("bellwether-home-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(configured_port(&d).unwrap(), None, "no config.json");
        std::fs::write(d.join("config.json"), r#"{"port": 47418}"#).unwrap();
        assert_eq!(configured_port(&d).unwrap(), Some(47418));
        std::fs::write(d.join("config.json"), r#"{"port": 80}"#).unwrap();
        assert!(configured_port(&d).is_err());
        std::fs::write(d.join("config.json"), r#"{"port": 47418, "host": "0.0.0.0"}"#).unwrap();
        assert!(configured_port(&d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
