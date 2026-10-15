//! Milestone 7's "Done when": the hook diff the onboarding window shows equals `bellwether
//! hooks install`'s output for the same file, token masked. For every installer vector
//! (test/fixtures/installer-vectors), this runs the real binary's dry run under a temp HOME
//! and BELLWETHER_HOME, and compares its stdout with the window's preview of the same file,
//! byte for byte; then the same for `hooks uninstall` after an install by the window.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bellwether_shell_lib::onboarding::{Ctx, Layout, Onboarding};
use bellwether_core::hooks::Action;

const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";

fn free_port() -> u16 {
    loop {
        let p = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        if p != 47417 && p >= 1024 {
            return p;
        }
    }
}

/// The CLI's stdout (the diff) for `hooks <sub>` (a dry run) with HOME = `home`.
fn cli(home: &Path, app_home: &Path, sub: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bellwether-shell"))
        .args(["hooks", sub])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("BELLWETHER_HOME", app_home)
        .env("BELLWETHER_HOOK_TOKEN", TOKEN)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            // Only the child this test started, by its own handle.
            let _ = child.kill();
            panic!("hooks {sub} didn't finish within 60 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_window_shows_exactly_the_diff_hooks_install_prints() {
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/installer-vectors");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&vectors).unwrap().map(|e| e.unwrap().path()).filter(|p| p.join("settings.json").exists()).collect();
    dirs.sort();
    assert!(dirs.len() >= 10, "the installer vectors are there");
    let root = std::env::temp_dir().join(format!("bw-onboarding-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let base = root.join(&name);
        let (home, app_home) = (base.join("home"), base.join("app"));
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&app_home).unwrap();
        let port = free_port();
        std::fs::write(app_home.join("config.json"), format!("{{\"port\": {port}}}\n")).unwrap();
        let file = home.join(".claude").join("settings.json");
        std::fs::copy(dir.join("settings.json"), &file).unwrap();
        let ob = Onboarding::new(Ctx { layout: Layout::App, home: app_home.clone(), port, resources: None, user_home: home.clone(), env_config: None, env_token: Some(TOKEN.into()) });

        let shown = ob.hooks_preview(Action::Install).unwrap();
        let printed = cli(&home, &app_home, "install");
        assert!(!printed.is_empty(), "{name}: the CLI printed a diff");
        assert_eq!(shown["diffText"].as_str().unwrap(), printed, "{name}: the window's install diff is the CLI's");
        assert!(!printed.contains(TOKEN) && printed.contains("test…"), "{name}: masked in both");

        // Install from the window, then compare the uninstall previews too.
        let r = ob.hooks_apply(shown["planId"].as_str().unwrap()).unwrap();
        assert_eq!(r["outcome"], "applied", "{name}: {r}");
        let shown = ob.hooks_preview(Action::Uninstall).unwrap();
        assert_eq!(shown["diffText"].as_str().unwrap(), cli(&home, &app_home, "uninstall"), "{name}: the window's uninstall diff is the CLI's");
    }
    let _ = std::fs::remove_dir_all(&root);
}
