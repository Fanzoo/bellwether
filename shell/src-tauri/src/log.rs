//! The shell's log: one line per event with a millisecond timestamp, to stderr and to
//! data/shell.log. Milestone 5's 300 ms alert checks read these timestamps. Lines
//! never contain bubble text: snapshots are logged by rev and count only.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    *FILE.lock().unwrap() = Some(path);
}

pub fn log(line: &str) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let text = format!("{ms} pasture-shell: {line}\n");
    let _ = std::io::stderr().write_all(text.as_bytes());
    if let Some(path) = FILE.lock().unwrap().as_ref() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(text.as_bytes());
        }
    }
}
