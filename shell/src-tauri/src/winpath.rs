//! The command-line tool on Windows (Windows plan WD7, W7): the setup window adds the
//! install's `bin\` (where the console `bellwether.exe` is) to the user's own PATH,
//! `HKCU\Environment\Path`, and can take it out again. Nothing else in the value changes,
//! its type is kept (`REG_EXPAND_SZ` when there was none), and `WM_SETTINGCHANGE` tells
//! Explorer, so terminals opened from then on find `bellwether`. Terminals already open
//! keep the PATH they started with. No administrator rights: the machine's PATH is never
//! touched.
//!
//! The pure parts build on every OS (the Mac tests them); the registry calls are Windows
//! only.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

use serde_json::{json, Value};

/// Where the value lives, as the setup window names it.
pub const WHERE: &str = r"HKCU\Environment\Path";

/// One PATH entry as Windows compares it: no surrounding spaces or quotes, `\` for `/`, no
/// trailing `\`, any case.
fn norm(entry: &str) -> String {
    entry.trim().trim_matches('"').replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether `path` (a PATH value, `;`-separated) has `dir` in it.
pub fn has(path: &str, dir: &str) -> bool {
    let want = norm(dir);
    path.split(';').any(|e| !e.trim().is_empty() && norm(e) == want)
}

/// `path` with `dir` added at the end, or None when it is there already. The other
/// entries stay exactly as they were, empty ones too.
pub fn with_dir(path: Option<&str>, dir: &str) -> Option<String> {
    match path {
        Some(p) if has(p, dir) => None,
        Some(p) if !p.trim().is_empty() => Some(format!("{};{dir}", p.trim_end_matches(';'))),
        _ => Some(dir.to_string()),
    }
}

/// `path` without `dir` (every entry for it), or None when it isn't there. The other
/// entries stay exactly as they were.
pub fn without_dir(path: &str, dir: &str) -> Option<String> {
    if !has(path, dir) {
        return None;
    }
    let want = norm(dir);
    Some(path.split(';').filter(|e| e.trim().is_empty() || norm(e) != want).collect::<Vec<_>>().join(";"))
}

/// Other folders on the PATH with a `bellwether.exe` in them (another install, or the test
/// folder), which a terminal may find first.
pub fn others(path: &str, dir: &str, exists: &dyn Fn(&Path) -> bool) -> Vec<String> {
    let want = norm(dir);
    path.split(';').map(str::trim).filter(|e| !e.is_empty() && norm(e) != want && !e.contains('%') && exists(&Path::new(e).join("bellwether.exe"))).map(str::to_string).collect()
}

/// What the setup window shows: `dir` is the install's `bin\` (None: not an install),
/// `user_path` the user's PATH value as it is now (None: there is none).
pub fn state(dir: Option<&Path>, user_path: Result<Option<String>, String>, exists: &dyn Fn(&Path) -> bool) -> Value {
    let Some(dir) = dir else {
        return json!({ "state": "unavailable", "detail": crate::winstart::NOT_AN_INSTALL.replace("start at login", "the command-line tool") });
    };
    let d = crate::winstart::plain(dir).display().to_string();
    let path = match user_path {
        Ok(p) => p,
        Err(e) => return json!({ "state": "unavailable", "path": d, "detail": format!("couldn't read your PATH ({WHERE}): {e}") }),
    };
    let current = path.as_deref().unwrap_or("");
    let others = others(current, &d, exists);
    let installed = has(current, &d);
    json!({
        "state": if installed { "installed" } else { "absent" },
        "path": d,
        "where": WHERE,
        "change": if installed { format!("Remove takes {d} out of your PATH ({WHERE}); nothing else in it changes.") } else { format!("Adds {d} to the end of your PATH ({WHERE}); nothing else in it changes, and no administrator rights are needed.") },
        "others": others,
    })
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::path::PathBuf;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};

    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_MORE_DATA: u32 = 234;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The install's `bin\`, where the console exe is.
    pub fn bin_dir() -> Option<PathBuf> {
        crate::winstart::install_exe().and_then(|e| e.parent().map(|d| d.join("bin")))
    }

    /// The user's PATH, unexpanded, and its type; None when there is no value.
    pub fn read() -> Result<Option<(String, u32)>, String> {
        let (key, name) = (wide("Environment"), wide("Path"));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut size: u32 = 0;
        let mut kind: u32 = 0;
        loop {
            let mut buf = vec![0u16; (size as usize / 2).max(1)];
            let mut got = (buf.len() * 2) as u32;
            let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), flags, &mut kind, buf.as_mut_ptr().cast(), &mut got) };
            match r {
                0 => {
                    let n = (got as usize / 2).min(buf.len());
                    let text = String::from_utf16_lossy(&buf[..n]);
                    return Ok(Some((text.trim_end_matches('\0').to_string(), kind)));
                }
                ERROR_FILE_NOT_FOUND => return Ok(None),
                ERROR_MORE_DATA if got > size => size = got,
                e => return Err(format!("error {e}")),
            }
        }
    }

    fn write(value: &str, kind: u32) -> Result<(), String> {
        let k = wide("Environment");
        let mut key: HKEY = std::ptr::null_mut();
        let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut()) };
        if r != 0 {
            return Err(format!("couldn't open HKCU\\Environment (error {r})"));
        }
        let (name, data) = (wide("Path"), wide(value));
        let r = unsafe { RegSetValueExW(key, name.as_ptr(), 0, kind, data.as_ptr().cast(), (data.len() * 2) as u32) };
        unsafe { RegCloseKey(key) };
        if r != 0 {
            return Err(format!("couldn't write {WHERE} (error {r})"));
        }
        Ok(())
    }

    /// Tells Explorer (and so every terminal started from now on) that the environment
    /// changed. 5 s at most per window that doesn't answer.
    fn broadcast() {
        let what = wide("Environment");
        let mut result: usize = 0;
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, what.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
    }

    /// The step's state, read now.
    pub fn state_here() -> Value {
        state(bin_dir().as_deref(), read().map(|r| r.map(|(p, _)| p)), &|p| p.is_file())
    }

    /// Adds (or with `remove`, takes out) the install's `bin\`, then says so to Explorer.
    pub fn apply(remove: bool) -> Result<Value, String> {
        let dir = bin_dir().ok_or_else(|| crate::winstart::NOT_AN_INSTALL.replace("start at login", "the command-line tool"))?;
        let d = crate::winstart::plain(&dir).display().to_string();
        let now = read()?;
        let kind = match now.as_ref().map(|(_, k)| *k) {
            Some(k) if k == REG_SZ => REG_SZ,
            _ => REG_EXPAND_SZ,
        };
        let current = now.as_ref().map(|(p, _)| p.as_str());
        let new = if remove { current.and_then(|p| without_dir(p, &d)) } else { with_dir(current, &d) };
        let mut out;
        match new {
            Some(value) => {
                write(&value, kind)?;
                broadcast();
                out = state_here();
                out["detail"] = Value::from(if remove {
                    format!("Took {d} out of your PATH. Terminals opened from now on don't find `bellwether`; ones already open keep it until you close them.")
                } else {
                    format!("Added {d} to your PATH. Terminals opened from now on find `bellwether`; ones already open don't: close and reopen them.")
                });
            }
            None => {
                out = state_here();
                out["detail"] = Value::from(if remove { format!("{d} isn't on your PATH; nothing changed.") } else { format!("{d} is already on your PATH; nothing changed.") });
            }
        }
        crate::log::log(&format!("onboarding: command-line tool: {}", out["detail"].as_str().unwrap_or("")));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN: &str = r"C:\Users\me\AppData\Local\Bellwether\bin";

    #[test]
    fn adding_appends_once_and_leaves_every_other_entry_as_it_was() {
        assert_eq!(with_dir(None, BIN).unwrap(), BIN, "no PATH value yet");
        assert_eq!(with_dir(Some(""), BIN).unwrap(), BIN);
        let path = r"%USERPROFILE%\AppData\Local\Microsoft\WindowsApps;;C:\Tools;";
        assert_eq!(with_dir(Some(path), BIN).unwrap(), format!(r"%USERPROFILE%\AppData\Local\Microsoft\WindowsApps;;C:\Tools;{BIN}"));
        // Already there, however it is spelled: nothing to do.
        for spelled in [BIN.to_string(), BIN.to_uppercase(), format!(r"{BIN}\"), BIN.replace('\\', "/"), format!(" \"{BIN}\" ")] {
            assert_eq!(with_dir(Some(&format!(r"C:\Tools;{spelled}")), BIN), None, "{spelled}");
        }
    }

    #[test]
    fn removing_takes_out_only_bellwethers_entry() {
        let path = format!(r"C:\Tools;{};%USERPROFILE%\bin;;{BIN}\", BIN.to_uppercase());
        assert_eq!(without_dir(&path, BIN).unwrap(), r"C:\Tools;%USERPROFILE%\bin;");
        assert_eq!(without_dir(r"C:\Tools;C:\Users\me\AppData\Local\Bellwether\bin2", BIN), None, "a longer name isn't ours");
        // Add, then remove: back to the bytes it had.
        let before = r"C:\A;C:\B";
        assert_eq!(without_dir(&with_dir(Some(before), BIN).unwrap(), BIN).unwrap(), before);
    }

    #[test]
    fn the_setup_window_sees_the_state_the_change_and_other_copies() {
        let exists = |p: &Path| p.to_string_lossy().replace('/', "\\").eq_ignore_ascii_case(r"C:\Test\bin\bellwether.exe");
        let s = state(Some(Path::new(BIN)), Ok(Some(r"C:\Test\bin;C:\Tools".into())), &exists);
        assert_eq!(s["state"], "absent");
        assert!(s["change"].as_str().unwrap().starts_with(&format!("Adds {BIN} to the end of your PATH ({WHERE})")));
        assert_eq!(s["others"], json!([r"C:\Test\bin"]));
        let s = state(Some(Path::new(BIN)), Ok(Some(format!("C:\\Tools;{BIN}"))), &exists);
        assert_eq!(s["state"], "installed");
        assert!(s["change"].as_str().unwrap().starts_with("Remove takes"));
        assert_eq!(state(Some(Path::new(BIN)), Ok(None), &exists)["state"], "absent");
        let s = state(None, Ok(None), &exists);
        assert_eq!(s["state"], "unavailable");
        assert!(s["detail"].as_str().unwrap().contains("the command-line tool needs Bellwether.exe"), "{s}");
        assert!(state(Some(Path::new(BIN)), Err("error 5".into()), &exists)["detail"].as_str().unwrap().contains("couldn't read your PATH"));
    }
}
