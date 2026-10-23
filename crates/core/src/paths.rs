//! The user's home and Claude Code's settings file, on every OS (Windows plan WD8, W2).
//!
//! - **The home** is `HOME` on macOS and Linux, and `USERPROFILE` on Windows, where `HOME`
//!   is often unset and Git Bash sets its own. Every read of the user's home goes through
//!   `user_home`.
//! - **Claude Code's user settings** are `~/.claude/settings.json` (`%USERPROFILE%\.claude`
//!   on Windows), or `$CLAUDE_CONFIG_DIR/settings.json` when that is set: Claude Code keeps
//!   its settings there then, so the installer, doctor and the guards use it too.
//!
//! The `_in` forms take the environment as a function, so the rules are tested without
//! changing the test process's own environment.

use std::ffi::OsString;
use std::path::PathBuf;

/// The variable that holds the user's home on this OS.
pub const HOME_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

fn non_empty(v: Option<OsString>) -> Option<OsString> {
    v.filter(|v| !v.is_empty())
}

/// The user's home, from `var` (an environment lookup): `USERPROFILE` on Windows (then
/// `HOME`, if that is all there is), `HOME` elsewhere.
pub fn user_home_in(var: impl Fn(&str) -> Option<OsString>, windows: bool) -> Option<PathBuf> {
    let first = if windows { "USERPROFILE" } else { "HOME" };
    let home = non_empty(var(first)).or_else(|| if windows { non_empty(var("HOME")) } else { None });
    home.map(PathBuf::from)
}

/// The user's home, or None when the environment doesn't say.
pub fn user_home() -> Option<PathBuf> {
    user_home_in(|k| std::env::var_os(k), cfg!(windows))
}

/// Claude Code's configuration directory: `CLAUDE_CONFIG_DIR` when set, else
/// `<home>/.claude`.
pub fn claude_dir_in(var: impl Fn(&str) -> Option<OsString>, windows: bool) -> PathBuf {
    if let Some(d) = non_empty(var("CLAUDE_CONFIG_DIR")) {
        return PathBuf::from(d);
    }
    user_home_in(var, windows).unwrap_or_default().join(".claude")
}

pub fn claude_dir() -> PathBuf {
    claude_dir_in(|k| std::env::var_os(k), cfg!(windows))
}

/// Claude Code's user settings file: `<claude_dir>/settings.json`.
pub fn user_settings() -> PathBuf {
    claude_dir().join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(*v))
    }

    #[test]
    fn the_home_is_userprofile_on_windows_and_home_elsewhere() {
        let both = env(&[("HOME", "/c/Users/jeff/gitbash"), ("USERPROFILE", "C:\\Users\\jeff")]);
        assert_eq!(user_home_in(&both, true), Some(PathBuf::from("C:\\Users\\jeff")), "Git Bash's HOME is not the profile");
        assert_eq!(user_home_in(&both, false), Some(PathBuf::from("/c/Users/jeff/gitbash")));
        assert_eq!(user_home_in(env(&[("HOME", "/h")]), true), Some(PathBuf::from("/h")), "Windows without USERPROFILE: HOME, if that is all");
        assert_eq!(user_home_in(env(&[("USERPROFILE", "C:\\u")]), false), None, "macOS never reads USERPROFILE");
        assert_eq!(user_home_in(env(&[("HOME", ""), ("USERPROFILE", "")]), true), None, "empty is unset");
        assert_eq!(HOME_VAR, if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    }

    #[test]
    fn claude_config_dir_moves_the_settings_file_on_every_os() {
        for windows in [false, true] {
            let set = env(&[("HOME", "/h"), ("USERPROFILE", "/h"), ("CLAUDE_CONFIG_DIR", "/cfg/claude")]);
            assert_eq!(claude_dir_in(&set, windows), PathBuf::from("/cfg/claude"));
            let unset = env(&[("HOME", "/h"), ("USERPROFILE", "/h"), ("CLAUDE_CONFIG_DIR", "")]);
            assert_eq!(claude_dir_in(&unset, windows), PathBuf::from("/h").join(".claude"));
        }
    }
}
