//! Distribution plan D5 and milestone 6: no file is ever written to ~/Library/LaunchAgents
//! by the app. This runs the real binary (a development build, outside any bundle, so
//! SMAppService is never reached) with a temp HOME through every command and action that
//! touches login items, LaunchAgents or the files around them, then checks that HOME's
//! LaunchAgents folder is exactly as it was: not created when it was missing, and with
//! the same files, bytes and modification times when it held legacy plists.
//!
//! Not run here: `autostart settings` and the `login-items-settings` action (they open
//! System Settings on screen), the Automation actions (Apple Events), and the strip (a
//! window). The strip's startup check is `bellwether_brain::legacy::scan_here`, which
//! only reads (legacy.rs, `scanning_only_reads`; tray.rs, its test).

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";
const LIVE_PORT: u16 = 47417;

/// A port nothing listens on (bound, then released); never the live one.
fn free_port() -> u16 {
    loop {
        let p = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        if p != LIVE_PORT && p >= 1024 {
            return p;
        }
    }
}

/// Every entry under `dir` (recursively): its kind, size, bytes and mtime.
fn snapshot(dir: &Path) -> Option<BTreeMap<PathBuf, (bool, Vec<u8>, SystemTime)>> {
    if std::fs::symlink_metadata(dir).is_err() {
        return None;
    }
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p.clone());
            }
            let bytes = if m.is_file() { std::fs::read(&p).unwrap() } else { Vec::new() };
            out.insert(p, (m.is_dir(), bytes, m.modified().unwrap()));
        }
    }
    let m = std::fs::metadata(dir).unwrap();
    out.insert(dir.to_path_buf(), (true, Vec::new(), m.modified().unwrap()));
    Some(out)
}

struct World {
    home: PathBuf,
    app_home: PathBuf,
    repo: PathBuf,
    settings: PathBuf,
    port: u16,
}

fn world(root: &Path, name: &str, legacy_plists: bool) -> World {
    let base = root.join(name);
    let home = base.join("home");
    let app_home = base.join("app");
    let repo = base.join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(repo.join("data")).unwrap();
    std::fs::create_dir_all(repo.join("brain")).unwrap();
    let port = free_port();
    std::fs::write(repo.join(".env"), format!("BELLWETHER_HOOK_TOKEN={TOKEN}\n")).unwrap();
    std::fs::write(repo.join("config.json"), format!("{{\"port\": {port}}}\n")).unwrap();
    std::fs::write(repo.join("data").join("state.json"), "{}\n").unwrap();
    std::fs::create_dir_all(&app_home).unwrap();
    std::fs::write(app_home.join("config.json"), format!("{{\"port\": {port}}}\n")).unwrap();
    let settings = base.join("settings.json");
    std::fs::write(&settings, "{}\n").unwrap();
    if legacy_plists {
        let agents = home.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        for label in ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.brain"] {
            let what = if label.ends_with("brain") { "brain/cli.mjs</string><string>serve" } else { "shell/src-tauri/target/release/bellwether-shell" };
            std::fs::write(
                agents.join(format!("{label}.plist")),
                format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>/usr/local/bin/node</string><string>{}/{what}</string></array><key>WorkingDirectory</key><string>{}</string></dict></plist>\n", repo.display(), repo.display()),
            )
            .unwrap();
        }
        // Give the files an old mtime, so a rewrite with the same bytes still shows.
        let old = SystemTime::now() - Duration::from_secs(3600);
        for e in std::fs::read_dir(&agents).unwrap() {
            let f = std::fs::File::options().append(true).open(e.unwrap().path()).unwrap();
            f.set_modified(old).unwrap();
        }
    }
    World { home, app_home, repo, settings, port }
}

/// Runs the binary with `argv0` and `args` in the world's environment; at most 60 s.
fn run(w: &World, argv0: &str, args: &[&str]) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bellwether-shell"));
    cmd.arg0(argv0)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &w.home)
        .env("BELLWETHER_HOME", &w.app_home)
        .current_dir(&w.home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            // Only the child this test started, by its own handle.
            let _ = child.kill();
            panic!("{argv0} {args:?} didn't finish within 60 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn nothing_is_ever_written_to_launch_agents() {
    let root = std::env::temp_dir().join(format!("bw-no-launchagents-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, legacy) in [("empty", false), ("legacy", true)] {
        let w = world(&root, name, legacy);
        let agents = w.home.join("Library").join("LaunchAgents");
        let before = snapshot(&agents);
        assert_eq!(before.is_some(), legacy);
        let settings = w.settings.to_string_lossy().into_owned();
        let port = w.port.to_string();
        let repo = w.repo.to_string_lossy().into_owned();
        let cli: Vec<Vec<&str>> = vec![
            vec!["autostart"],
            vec!["autostart", "status"],
            vec!["autostart", "install"],
            vec!["autostart", "install", "--write"],
            vec!["autostart", "install", "--write", "--brain-only"],
            vec!["autostart", "uninstall", "--write"],
            vec!["autostart", "uninstall", "--write", "--brain-only"],
            vec!["token", "init"],
            vec!["hooks", "install", "--write", "--settings", &settings],
            vec!["doctor", "--settings", &settings, "--port", &port],
            vec!["doctor", "--json", "--settings", &settings, "--port", &port],
            vec!["migrate", "--from", &repo, "--settings", &settings],
            vec!["migrate", "--from", &repo, "--settings", &settings, "--write"],
            vec!["hooks", "uninstall", "--write", "--settings", &settings],
            vec!["shell", "stop"],
        ];
        let mut said = Vec::new();
        for args in &cli {
            let (code, text) = run(&w, "bellwether", args);
            said.push(format!("bellwether {} -> {code:?}\n{text}", args.join(" ")));
        }
        // The app's side of the hand-off (`--action`), as LaunchServices would start it.
        for action in ["login-items-status", "brain-agent-register", "main-app-register", "brain-agent-unregister", "main-app-unregister"] {
            let (code, text) = run(&w, "Bellwether", &["--action", action]);
            said.push(format!("--action {action} -> {code:?}\n{text}"));
        }
        let all = said.join("\n");
        assert_eq!(snapshot(&agents), before, "{name}: ~/Library/LaunchAgents changed. What the commands said:\n{all}");
        // Outside the app, install and uninstall say so and change nothing.
        assert!(all.contains("bellwether autostart install --write -> Some(2)\nbellwether: not running from Bellwether.app"), "{all}");
        // The commands did run: the hooks went in and out, and the token file exists.
        assert!(w.app_home.join("hook-token").exists(), "{all}");
        if legacy {
            // Doctor names the plists (launchd isn't asked under a temp HOME), and migrate
            // lists them without touching them.
            assert!(all.contains("local.bellwether.brain (plist present), local.bellwether.shell (plist present), local.pasture.brain (plist present)"), "{all}");
            assert!(all.contains("the app doesn't write or remove LaunchAgents"), "{all}");
        } else {
            assert!(!all.contains("legacy agents"), "{all}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}
