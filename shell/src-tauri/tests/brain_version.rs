//! Deviation 135, through the real binary: doctor flags a brain whose version isn't the
//! app's (WARN, with the fix for this app's own login agent, and "left alone" for any other
//! brain), reports Accessibility in one line, the strip's, and finds a process of a bundle
//! that was replaced under it.
//!
//! Nothing here reaches the real launchd or the live install: a temp HOME and
//! BELLWETHER_HOME, launchctl is `fake-launchctl` (BELLWETHER_LAUNCHCTL, which only answers
//! `print` and logs every call), and the "brains" are a small /health server in this test
//! or this binary's own `serve`, on free ports (never 47417). The binary isn't inside an app
//! bundle, so with the stand-in its agent label is the permanent id's.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_bellwether-shell");
const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";
const LABEL: &str = "com.fanzootechnology.bellwether.brain";

const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/launchctl.log"
case "$1" in
  print)
    if [ -f "$D/agent-pid" ]; then printf '%s = {\n\tstate = running\n\tpid = %s\n}\n' "$2" "$(cat "$D/agent-pid")"; exit 0; fi
    echo "Could not find service" >&2; exit 113 ;;
esac
exit 64
"#;

/// Starts `serve --port 0` (the brain binds a port the OS picks) and returns it with the
/// port it announced. A port picked here and handed over later could be taken in between
/// (the other tests here bind ports too); `serve` then exits at once ("already in use"),
/// which made the bundle-swap test fail now and then (milestone 12 fixes, deviation 139).
fn serve_on_any_port(mut cmd: Command) -> (std::process::Child, u16) {
    use std::io::BufRead;
    let mut child = cmd.args(["serve", "--port", "0"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = Vec::new();
    let port = loop {
        let Some(Ok(line)) = lines.next() else {
            let _ = child.kill();
            panic!("serve stopped before it was listening: {seen:?} (exit {:?})", child.wait());
        };
        if let Some(rest) = line.split("listening on http://127.0.0.1:").nth(1) {
            break rest.split(|c: char| !c.is_ascii_digit()).next().unwrap().parse::<u16>().unwrap();
        }
        seen.push(line);
    };
    assert_ne!(port, 47417);
    // Keep reading, so the brain never blocks on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    (child, port)
}

/// A brain stand-in: /health says `version` and `pid`; /hook answers 204.
fn fake_brain(version: &str, pid: i64) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert_ne!(port, 47417);
    let body = serde_json::json!({"ok": true, "name": "bellwether", "version": version, "pid": pid, "port": port, "mode": "normal", "counters": {"accepted": 0}}).to_string();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let resp = if req.starts_with("GET /health") {
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            } else {
                "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string()
            };
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

struct World {
    base: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let base = std::env::temp_dir().join(format!("bw-brain-version-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["home", "app/logs", "state"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let fake = base.join("state/fake-launchctl");
        std::fs::write(&fake, FAKE_LAUNCHCTL).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        World { base }
    }

    fn doctor(&self, port: u16) -> (i32, String) {
        let mut child = Command::new(BIN)
            .args(["doctor", "--port", &port.to_string()])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", self.base.join("home"))
            .env("BELLWETHER_HOME", self.base.join("app"))
            .env("BELLWETHER_HOOK_TOKEN", TOKEN)
            .env("BELLWETHER_TEST_ROOT", &self.base)
            .env("BELLWETHER_LAUNCHCTL", self.base.join("state/fake-launchctl"))
            .current_dir(&self.base)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("doctor didn't finish within 60 s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn line<'a>(out: &'a str, name: &str) -> Vec<&'a str> {
        // "  STAT  <name padded to 22> detail"
        out.lines().filter(|l| l.len() > 30 && l.starts_with("  ") && l.get(8..31).is_some_and(|n| n.trim_end() == name)).collect()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("state/launchctl.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// launchd's agent pid matches, but this binary isn't inside an app bundle, so the agent
/// can't be its own: a WARN that leaves it alone (the wording for this app's own agent,
/// with the kickstart fix, is agent.rs's `doctor_names_the_fix_…`; a bundle can't be
/// faked here without SMAppService).
#[test]
fn an_older_brain_is_a_warn_and_a_development_binary_never_claims_the_agent() {
    let w = World::new("agent");
    std::fs::write(w.base.join("state/agent-pid"), "4242").unwrap();
    let port = fake_brain("0.0.1-old", 4242);
    let (_, out) = w.doctor(port);
    let brain = World::line(&out, "brain");
    assert_eq!(brain.len(), 1, "{out}");
    assert!(brain[0].trim_start().starts_with("WARN"), "{out}");
    assert!(brain[0].contains("v0.0.1-old") && brain[0].contains(&format!("this app is {}, and this brain isn't this app's login agent (this binary isn't inside an app bundle)", env!("CARGO_PKG_VERSION"))), "{out}");
    assert!(!brain[0].contains("kickstart"), "{out}");
    // A brain answers: `next` never says to start one (the hooks come first here: none installed).
    assert!(!World::line(&out, "next")[0].contains("start the brain"), "{out}");
    // Doctor only asks; it never restarts anything.
    let uid = unsafe { libc::getuid() };
    assert_eq!(w.calls(), [format!("print gui/{uid}/{LABEL}")]);
}

#[test]
fn an_older_brain_that_isnt_the_apps_agent_is_left_alone() {
    let w = World::new("other");
    std::fs::write(w.base.join("state/agent-pid"), "4777").unwrap();
    let port = fake_brain("0.0.1-old", 4242);
    let (_, out) = w.doctor(port);
    let brain = World::line(&out, "brain");
    assert!(brain[0].trim_start().starts_with("WARN"), "{out}");
    assert!(brain[0].contains(&format!("isn't this app's login agent (launchd runs {LABEL} as pid 4777)")) && brain[0].contains("Stop it (pid 4242)") && !brain[0].contains("kickstart"), "{out}");
    assert!(w.calls().iter().all(|c| c.starts_with("print ")), "{:?}", w.calls());
}

#[test]
fn the_same_version_passes_and_launchd_is_not_asked() {
    let w = World::new("same");
    std::fs::write(w.base.join("app/hook-token"), TOKEN).unwrap();
    std::fs::set_permissions(w.base.join("app/hook-token"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", w.base.join("home")).env("BELLWETHER_HOME", w.base.join("app")).env("BELLWETHER_HOOK_TOKEN", TOKEN);
    let (mut serve, port) = serve_on_any_port(cmd);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = String::new();
    while Instant::now() < deadline {
        out = w.doctor(port).1;
        if World::line(&out, "brain").first().is_some_and(|l| l.trim_start().starts_with("PASS")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Only the brain this test started, by its own handle.
    unsafe { libc::kill(serve.id() as i32, libc::SIGTERM) };
    let _ = serve.wait();
    let brain = World::line(&out, "brain");
    assert!(brain[0].trim_start().starts_with("PASS") && brain[0].contains(&format!("v{}", env!("CARGO_PKG_VERSION"))) && !brain[0].contains("this app is"), "{out}");
    assert!(w.calls().is_empty(), "{:?}", w.calls());
}

#[test]
fn accessibility_is_one_line_from_the_strips_log() {
    let w = World::new("ax");
    let port = fake_brain(env!("CARGO_PKG_VERSION"), 4242);
    let (_, out) = w.doctor(port);
    let ax = World::line(&out, "accessibility");
    assert_eq!(ax.len(), 1, "{out}");
    assert!(World::line(&out, "dock").is_empty(), "{out}");
    assert!(ax[0].contains("the strip's log has no Accessibility line"), "{out}");
    // The strip said it has it: the one line says so (a development binary can't see strips).
    std::fs::write(w.base.join("app/logs/shell.log"), "1 bellwether-shell: starting: Bellwether x\n2 bellwether-shell: dock: Accessibility granted to this binary: sheep stand on the Dock's top edge, within its width\n").unwrap();
    let (_, out) = w.doctor(port);
    let ax = World::line(&out, "accessibility");
    assert_eq!(ax.len(), 1, "{out}");
    assert!(World::line(&out, "dock").is_empty(), "{out}");
    assert!(ax[0].trim_start().starts_with("PASS") && ax[0].contains("from the strip's log") && ax[0].contains("Accessibility granted"), "{out}");
    assert!(!out.contains("not granted to this app"), "{out}");
}

/// Deviation 135, item 3: a process of this bundle is still found after an update swapped
/// the bundle and deleted its executable (doctor's "is the strip running", `shell stop`).
/// A copy of this binary in a scratch bundle plays it: started by its full path (as
/// LaunchServices starts the strip), then its bundle is deleted under it.
#[test]
fn a_process_whose_bundle_was_replaced_is_still_this_bundles() {
    use bellwether_shell_lib::identity::{pid_exe, processes_of, Exe};
    let w = World::new("swap");
    let macos = w.base.join("Bellwether.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::copy(BIN, macos.join("Bellwether")).unwrap();
    let exe = macos.join("Bellwether").canonicalize().unwrap();
    let mut cmd = Command::new(&exe);
    cmd.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", w.base.join("home")).env("BELLWETHER_HOME", w.base.join("app")).env("BELLWETHER_HOOK_TOKEN", TOKEN);
    // Listening (its port bound) before anything is looked at: it stays up until killed.
    let (mut child, _) = serve_on_any_port(cmd);
    let pid = child.id() as i32;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !processes_of(&exe).iter().any(|(p, _, _)| *p == pid) {
        assert!(Instant::now() < deadline, "the copy never showed up as running {}", exe.display());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(pid_exe(pid), Exe::At(exe.clone()));
    std::fs::remove_dir_all(w.base.join("Bellwether.app")).unwrap();
    let found = processes_of(&exe);
    let deleted = pid_exe(pid);
    let elsewhere = processes_of(&w.base.join("Other.app/Contents/MacOS/Bellwether"));
    let alive = child.try_wait().unwrap().is_none();
    // Only the process this test started, by its own handle.
    let _ = child.kill();
    let _ = child.wait();
    assert!(alive, "the brain was still running while it was looked up");
    assert_eq!(deleted, Exe::Deleted, "proc_pidpath after the bundle is gone");
    assert!(found.iter().any(|(p, _, a)| *p == pid && a.ends_with("serve --port 0")), "{found:?}");
    assert!(!elsewhere.iter().any(|(p, _, _)| *p == pid), "never another path's");
}
