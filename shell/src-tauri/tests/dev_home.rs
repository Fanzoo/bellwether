//! Milestone 11 (M11-a), through the binary: a development binary (outside an app bundle)
//! refuses every CLI command but `help` and `version` unless BELLWETHER_HOME is set, so it
//! never acts on the installed app's own folder; with it set, the CLI works as before; and
//! a copy inside a scratch `Bellwether.app`, reached through a symlink as Homebrew's is,
//! uses the app's home without it. Temp HOMEs only: nothing here reads or writes the real
//! Application Support, and no brain is left running.
// Unix process and symlink calls; the CLI on Windows is W2's, which runs these checks there.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_bellwether-shell");

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bw-dev-home-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `bellwether <args>` from `bin`, with a temp HOME and nothing else of the environment.
fn run(bin: &Path, home: &Path, bellwether_home: Option<&Path>, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(bin);
    cmd.arg0("bellwether").args(args).env_clear().env("PATH", "/usr/bin:/bin").env("HOME", home).stdin(Stdio::null());
    if let Some(h) = bellwether_home {
        cmd.env("BELLWETHER_HOME", h);
    }
    let out = cmd.output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

use std::os::unix::process::CommandExt;

#[test]
fn a_development_binary_needs_bellwether_home_for_every_command_but_help_and_version() {
    let home = temp("refused");
    for args in [&["doctor"][..], &["serve", "--port", "47998"], &["token", "init"], &["hooks", "print"], &["status"]] {
        let (code, out, err) = run(Path::new(BIN), &home, None, args);
        assert_eq!(code, 3, "{args:?}: {out}{err}");
        assert!(err.contains("BELLWETHER_HOME isn't set") && err.contains("export BELLWETHER_HOME=\"$PWD/data\"") && err.contains("docs/DEVELOPMENT.md"), "{args:?}: {err}");
    }
    assert!(!home.join("Library").exists(), "a refused command wrote into the HOME");
    let (code, out, _) = run(Path::new(BIN), &home, None, &["version"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("bellwether "), "{out}");
    let (code, out, _) = run(Path::new(BIN), &home, None, &["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("bellwether token init"), "{out}");
    // An empty BELLWETHER_HOME is no home.
    let (code, _, _) = run(Path::new(BIN), &home, Some(Path::new("")), &["token", "init"]);
    assert_eq!(code, 3);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn with_bellwether_home_the_cli_works_as_before() {
    let home = temp("with");
    let data = home.join("data");
    let (code, out, err) = run(Path::new(BIN), &home, Some(&data), &["token", "init"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(data.join("hook-token").exists());
    let (code, out, err) = run(Path::new(BIN), &home, Some(&data), &["hooks", "print"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("X-Bellwether-Token"), "{out}");
    assert!(!home.join("Library").exists(), "the app's folder under HOME was used");
    let _ = std::fs::remove_dir_all(&home);
}

/// The Homebrew case: `bellwether` on PATH is a symlink to the bundle's executable, which
/// resolves its own path (identity::bundle_path canonicalizes), so it is the app.
#[test]
fn a_copy_inside_a_bundle_reached_through_a_symlink_uses_the_app_home_without_it() {
    let base = temp("bundle");
    let home = base.join("home");
    let macos = base.join("Bellwether.app").join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let exe = macos.join("Bellwether");
    std::fs::copy(BIN, &exe).unwrap();
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let link = bin.join("bellwether");
    std::os::unix::fs::symlink(&exe, &link).unwrap();
    let (code, out, err) = run(&link, &home, None, &["token", "init"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(home.join("Library").join("Application Support").join("Bellwether").join("hook-token").exists(), "{out}{err}");
    let _ = std::fs::remove_dir_all(&base);
}
