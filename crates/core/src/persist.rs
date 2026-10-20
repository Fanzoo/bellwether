//! state.json: the only state kept between runs (muted, and per identity its colour,
//! slots and label). A port of brain/persist.mjs: a missing or unreadable file starts
//! fresh; the flock itself is rebuilt from incoming events. Written as 2-space pretty JSON
//! with a newline, mode 0600, through a temporary file flushed to disk and a rename, so a
//! crash leaves the old file or the new one, never a torn one.

use std::io::Write;
use std::path::Path;

use crate::config::io_code;
use crate::json::{self, Js};

/// The file's contents when they look like a state file (an object whose `identities`
/// is an object), else Null; and the warnings to log.
pub fn load_state(file: &Path) -> (Js, Vec<String>) {
    let raw = match std::fs::read(file) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Js::Null, vec![]),
        Err(e) => return (Js::Null, vec![format!("could not read {} ({}); starting with no saved colours or slots", file.display(), io_code(&e))]),
    };
    if let Ok(v) = json::parse(&raw) {
        if v.is_plain_object() && v.get("identities").is_plain_object() {
            return (v, vec![]);
        }
    }
    (Js::Null, vec![format!("{} is not a valid state file; starting with no saved colours or slots", file.display())])
}

/// Creates a directory (and its parents) for files only this user may read.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// Writes `text` to `file` through `tmp` and a rename, the new file created with `mode`.
pub fn write_atomic(file: &Path, tmp: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    write_atomic_sync(file, tmp, text, mode, false)
}

/// The same, flushed to disk before the rename when `sync` (the token file, state.json).
pub fn write_atomic_sync(file: &Path, tmp: &Path, text: &str, mode: u32, sync: bool) -> std::io::Result<()> {
    let _ = std::fs::remove_file(tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let result = (|| {
        let mut f = opts.open(tmp)?;
        f.write_all(text.as_bytes())?;
        if sync {
            f.sync_all()?;
        }
        std::fs::rename(tmp, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

/// `saveState(file, value)`: pretty JSON and a newline, 0600, `<file>.<pid>.tmp` flushed to
/// disk, then rename. The brain calls it on its writer thread (brain/src/saver.rs), so the
/// flush never holds up a hook (M13; until 1.1 it ran on the serving thread unflushed).
pub fn save_state(file: &Path, value: &Js) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        create_private_dir(dir)?;
    }
    let tmp = file.with_file_name(format!("{}.{}.tmp", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), std::process::id()));
    write_atomic_sync(file, &tmp, &(json::stringify_pretty(value, "  ") + "\n"), 0o600, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("bw-persist-{}", std::process::id()));
        let file = dir.join("state.json");
        let v = json::parse(r#"{"version":1,"muted":false,"identities":{"a":{"slots":{},"lastSeen":1}}}"#).unwrap();
        save_state(&file, &v).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().ends_with("}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(load_state(&file).0, v);
        for bad in ["null", "[]", "{\"identities\":[]}", "{", ""] {
            std::fs::write(&file, bad).unwrap();
            let (v, w) = load_state(&file);
            assert_eq!(v, Js::Null);
            assert!(w[0].contains("is not a valid state file"), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
