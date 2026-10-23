//! state.json: the only state kept between runs (muted, and per identity its colour,
//! slots and label). A port of brain/persist.mjs: a missing or unreadable file starts
//! fresh; the flock itself is rebuilt from incoming events. Written as 2-space pretty JSON
//! with a newline, mode 0600 (on Windows a DACL for the user only, acl.rs), through a
//! temporary file flushed to disk and a rename, so a crash leaves the old file or the new
//! one, never a torn one.

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

/// Creates a directory (and its parents) for files only this user may read. Windows (W2):
/// a directory this creates gets a protected DACL for the user only, inherited by what is
/// made in it (acl.rs), as 0700 is on Unix; one that was there already keeps its own, as on
/// Unix (DirBuilder's mode applies only to what it creates).
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(windows)]
    {
        let fresh = !dir.exists();
        std::fs::create_dir_all(dir)?;
        if fresh {
            // Best effort: a volume without ACLs (FAT32) still gets the directory; doctor
            // says what the token's DACL is.
            let _ = crate::acl::make_private(dir, true);
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    std::fs::create_dir_all(dir)
}

/// How long a rename over a file is retried while it is in use (Windows plan WD8).
pub const RENAME_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether a failed rename may work a moment later: on Windows, replacing a file another
/// process has open without `FILE_SHARE_DELETE` (an editor, an antivirus scan, Claude
/// Code's watcher) fails with access denied (5) or a sharing violation (32).
pub fn rename_is_transient(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5) | Some(32))
}

/// `rename(tmp, file)`, retried for up to `limit` while the error is transient
/// (`transient`); the last error otherwise. `rename` is the call itself (tests give one).
pub fn rename_with_retry(tmp: &Path, file: &Path, limit: std::time::Duration, transient: impl Fn(&std::io::Error) -> bool, rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match rename(tmp, file) {
            Err(e) if transient(&e) && std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            r => return r,
        }
    }
}

/// `rename(tmp, file)`, retried for a second on Windows while `file` is in use.
pub fn replace_file(tmp: &Path, file: &Path) -> std::io::Result<()> {
    rename_with_retry(tmp, file, RENAME_RETRY, rename_is_transient, |a, b| std::fs::rename(a, b))
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
        // Windows (W2): 0600 is a protected DACL for the user only, set before anything is
        // written, so the text is never in a file others could read. Best effort, as above.
        #[cfg(windows)]
        if mode & 0o077 == 0 {
            let _ = crate::acl::make_private(tmp, false);
        }
        f.write_all(text.as_bytes())?;
        if sync {
            f.sync_all()?;
        }
        replace_file(tmp, file)
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

    /// WD8: a rename over a file in use is retried while the error is transient, up to the
    /// limit, then the error stands; any other error is at once.
    #[test]
    fn a_rename_over_a_busy_file_is_retried_then_gives_up() {
        use std::cell::Cell;
        let (a, b) = (Path::new("t"), Path::new("f"));
        let busy = || std::io::Error::from_raw_os_error(32);
        let tries = Cell::new(0);
        let r = rename_with_retry(a, b, std::time::Duration::from_secs(5), |e| e.raw_os_error() == Some(32), |_, _| {
            tries.set(tries.get() + 1);
            if tries.get() < 3 { Err(busy()) } else { Ok(()) }
        });
        assert!(r.is_ok() && tries.get() == 3, "busy twice, then done");
        tries.set(0);
        let r = rename_with_retry(a, b, std::time::Duration::from_millis(120), |e| e.raw_os_error() == Some(32), |_, _| {
            tries.set(tries.get() + 1);
            Err(busy())
        });
        assert_eq!(r.unwrap_err().raw_os_error(), Some(32));
        assert!(tries.get() >= 2, "retried until the limit");
        tries.set(0);
        let r = rename_with_retry(a, b, std::time::Duration::from_secs(5), |e| e.raw_os_error() == Some(32), |_, _| {
            tries.set(tries.get() + 1);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(r.is_err() && tries.get() == 1, "not transient: no retry");
        assert_eq!(rename_is_transient(&busy()), cfg!(windows), "only Windows retries");
    }

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
