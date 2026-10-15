//! `migrate`'s step 5 with the brain check on (`check_brain`, what the CLI sets when HOME
//! is the real home): the pasture entries move only once a brain on the port takes the
//! token under X-Bellwether-Token. Run against a real Rust brain on a free port, one that
//! holds another token, and none (BR-3K's M4 finding D6b: the path that runs on the
//! maintainer's Mac had no test).

use std::path::{Path, PathBuf};

const OLD: &str = "pastureTokenNotReal_0123456789ab";

fn pasture_settings(port: u16) -> String {
    format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"keep"}}]}},{{"hooks":[{{"type":"http","url":"http://127.0.0.1:{port}/hook","timeout":1,"headers":{{"X-Pasture-Token":"{OLD}"}}}}]}}]}}}}"#)
}

/// An old repository install whose config names `port`, with the token in its `.env`.
fn world(root: &Path, name: &str, port: u16) -> (PathBuf, PathBuf, PathBuf) {
    let base = root.join(name);
    let old = base.join("pasture");
    std::fs::create_dir_all(old.join("data")).unwrap();
    std::fs::create_dir_all(old.join("brain")).unwrap();
    std::fs::write(old.join(".env"), format!("PASTURE_HOOK_TOKEN={OLD}\n")).unwrap();
    std::fs::write(old.join("config.json"), format!(r#"{{"port": {port}}}"#)).unwrap();
    let settings = base.join("settings.json");
    std::fs::write(&settings, pasture_settings(port)).unwrap();
    std::fs::create_dir_all(base.join("user")).unwrap();
    (old, settings, base)
}

fn run(old: &Path, settings: &Path, base: &Path) -> String {
    let o = bellwether_brain::migrate::Options {
        from: Some(old.to_path_buf()),
        env_from: None,
        settings: settings.to_path_buf(),
        write: true,
        force: false,
        home: base.join("app"),
        user_home: base.join("user"),
        check_brain: true,
    };
    bellwether_brain::migrate::migrate(&o).unwrap().lines.join("\n")
}

/// A Rust brain on a free port with `token`; returns its port and a stop handle.
fn brain(token: &str) -> (u16, tokio::sync::oneshot::Sender<()>, std::thread::JoinHandle<()>) {
    let bound = bellwether_brain::bind(0).unwrap();
    let port = bound.port;
    let opts = bellwether_brain::Options::new(token.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let h = std::thread::spawn(move || {
        bellwether_brain::serve(bound, opts, Some(rx)).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    (port, tx, h)
}

#[test]
fn the_entries_move_only_once_a_brain_takes_the_token() {
    let root = std::env::temp_dir().join(format!("bw-migrate-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // A brain that takes the token: the entries become bellwether's.
    let (port, stop, h) = brain(OLD);
    let (old, settings, base) = world(&root, "takes", port);
    let text = run(&old, &settings, &base);
    let _ = stop.send(());
    let _ = h.join();
    let after = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains("replaced 1 pasture entry with bellwether's"), "{text}");
    assert!(after.contains("X-Bellwether-Token") && !after.contains("X-Pasture-Token"), "{after}");
    assert!(after.contains("\"command\": \"keep\""));

    // A brain that holds another token answers 403: the entries stay as they were.
    let (port, stop, h) = brain("anotherTokenNotReal_0123456789");
    let (old, settings, base) = world(&root, "refuses", port);
    let text = run(&old, &settings, &base);
    let _ = stop.send(());
    let _ = h.join();
    assert!(text.contains("left as they are") && text.contains("403"), "{text}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), pasture_settings(port));

    // No brain at all: the entries stay too.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (old, settings, base) = world(&root, "none", free);
    let text = run(&old, &settings, &base);
    assert!(text.contains("left as they are: no brain answers"), "{text}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), pasture_settings(free));
    let _ = std::fs::remove_dir_all(&root);
}
