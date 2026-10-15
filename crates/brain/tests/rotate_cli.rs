//! `bellwether token rotate --write` against brains that switch, refuse, or aren't there,
//! and settings files that can't be written (BR-3K's M4 finding D1). The rule: the hook
//! entries change only once the brain on the port has switched, or when no brain runs at
//! all; whatever fails, the brain, the token file and the entries end up on one token, or
//! the command says plainly which one is off. One test, since the CLI reads
//! BELLWETHER_HOME and HOME from the process environment.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const OLD: &str = "rotateOldToken_0123456789abcd";

/// A fake brain: answers each POST /control with the next scripted (status, body), and
/// records the token each one carried. Stops after the script.
fn fake_brain(script: Vec<(u16, &'static str)>) -> (u16, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let h = std::thread::spawn(move || {
        for (status, body) in script {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(at) = text.find("\r\n\r\n") {
                    let len: usize = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                    if buf.len() >= at + 4 + len || n == 0 {
                        let token = text.lines().find_map(|l| l.strip_prefix("X-Bellwether-Token: ").map(str::to_string)).unwrap_or_default();
                        s2.lock().unwrap().push(token);
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let _ = write!(c, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (port, seen, h)
}

fn token_now(home: &Path) -> String {
    bellwether_core::token::read_file(home).unwrap().unwrap()
}

fn entries(port: u16, token: &str) -> String {
    format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"keep"}}]}},{{"hooks":[{{"type":"http","url":"http://127.0.0.1:{port}/hook","headers":{{"X-Bellwether-Token":"{token}"}}}}]}}]}}}}"#)
}

/// A fresh app home with the old token, a config on `port`, and installed entries.
fn setup(root: &Path, name: &str, port: u16) -> (PathBuf, PathBuf) {
    let home = root.join(name);
    std::fs::create_dir_all(&home).unwrap();
    bellwether_core::token::write(&home, OLD).unwrap();
    std::fs::write(home.join("config.json"), format!(r#"{{"port": {port}}}"#)).unwrap();
    let dir = root.join(format!("{name}-claude"));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = dir.join("settings.json");
    std::fs::write(&settings, entries(port, OLD)).unwrap();
    std::env::set_var("BELLWETHER_HOME", &home);
    (home, settings)
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn rotate_changes_the_entries_only_with_a_brain_that_switched_or_none() {
    let root = std::env::temp_dir().join(format!("bw-rotate-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("user")).unwrap();
    std::env::set_var("HOME", root.join("user"));
    std::env::remove_var("BELLWETHER_HOOK_TOKEN");
    std::env::remove_var("BELLWETHER_CONFIG");
    let run = |settings: &Path| bellwether_brain::cli::run("token", &["rotate".into(), "--write".into(), "--settings".into(), settings.display().to_string()]);

    // 1. A brain that answers but doesn't switch (the Node brain: 400 "unknown action"):
    //    exit 1, the token file back on the old token, the entries untouched.
    let (port, seen, h) = fake_brain(vec![(400, r#"{"ok":false,"error":"unknown action"}"#)]);
    let (home, settings) = setup(&root, "refuses", port);
    assert_eq!(run(&settings), 1);
    h.join().unwrap();
    assert_eq!(*seen.lock().unwrap(), [OLD]);
    assert_eq!(token_now(&home), OLD, "the token file is put back");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), entries(port, OLD), "the entries are untouched");

    // 2. No brain at all: the new token in the file and in the entries, exit 0.
    let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (home, settings) = setup(&root, "nobrain", free);
    assert_eq!(run(&settings), 0);
    let new = token_now(&home);
    assert_ne!(new, OLD);
    let after = std::fs::read_to_string(&settings).unwrap();
    assert!(after.contains(&new) && !after.contains(OLD), "the entries carry the new token");

    // 3. The settings write fails after the brain switched: the token file goes back and
    //    the brain is told again (with the new token, which it now holds); exit 1.
    #[cfg(unix)]
    {
        let (port, seen, h) = fake_brain(vec![(200, r#"{"ok":true}"#), (200, r#"{"ok":true}"#)]);
        let (home, settings) = setup(&root, "writefails", port);
        set_mode(settings.parent().unwrap(), 0o500);
        let code = run(&settings);
        set_mode(settings.parent().unwrap(), 0o700);
        assert_eq!(code, 1);
        h.join().unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "told, then told again");
        assert_eq!(seen[0], OLD);
        assert_ne!(seen[1], OLD, "the second call carries the new token the brain holds");
        assert_eq!(token_now(&home), OLD, "the token file is back on the old token");
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), entries(port, OLD));

        // 4. The same, but the brain refuses to switch back: still exit 1, the file and the
        //    entries agree on the old token, and the message says the brain needs a restart.
        let (port, seen, h) = fake_brain(vec![(200, r#"{"ok":true}"#), (400, r#"{"ok":false,"error":"nope"}"#)]);
        let (home, settings) = setup(&root, "restorefails", port);
        set_mode(settings.parent().unwrap(), 0o500);
        let code = run(&settings);
        set_mode(settings.parent().unwrap(), 0o700);
        assert_eq!(code, 1);
        h.join().unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(token_now(&home), OLD);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), entries(port, OLD));
    }
    let _ = std::fs::remove_dir_all(&root);
}
