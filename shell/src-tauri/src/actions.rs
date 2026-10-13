//! The app's side of the action hand-off (distribution plan D4): `Bellwether --action
//! <name>`, normally launched through LaunchServices (`open -n -a Bellwether.app --args
//! --action <name>`), so macOS attributes what it does to Bellwether and not to the
//! terminal the CLI runs in. It does one thing, writes the outcome to
//! `<app data>/actions/<name>.json` as {action, startedAt, finishedAt, ok, message},
//! prints it, and quits. It opens no window and starts no brain.
//!
//! Milestone 1 has the actions needed to prove the foundations: the two login items and
//! one harmless Apple Event per terminal (it asks how many windows the terminal has,
//! which raises the Automation prompt the first time). Not `version`: AppleScript reads
//! an app's version, name and id from its bundle without sending an event (measured).

use crate::login_items::{self, Service};

pub const NAMES: &[&str] = &[
    "login-items-status",
    "brain-agent-register",
    "brain-agent-unregister",
    "main-app-register",
    "main-app-unregister",
    "automation-iterm2",
    "automation-terminal",
];

/// Runs one action; returns the exit code (0 when it succeeded).
pub fn run(name: &str) -> i32 {
    let started = bellwether_brain::now_ms();
    let (ok, message) = if NAMES.contains(&name) { perform(name) } else { (false, format!("unknown action {name:?}; known: {}", NAMES.join(", "))) };
    let outcome = serde_json::json!({ "action": name, "startedAt": started, "finishedAt": bellwether_brain::now_ms(), "ok": ok, "message": message });
    println!("{outcome}");
    if NAMES.contains(&name) {
        let dir = bellwether_brain::home::dir().join("actions");
        let written = bellwether_brain::home::create_private_dir(&dir).and_then(|_| {
            let tmp = dir.join(format!(".{name}.json.tmp"));
            std::fs::write(&tmp, format!("{outcome}\n"))?;
            std::fs::rename(&tmp, dir.join(format!("{name}.json")))
        });
        if let Err(e) = written {
            eprintln!("bellwether: couldn't write the outcome to {}: {e}", dir.display());
        }
    }
    if ok { 0 } else { 1 }
}

fn perform(name: &str) -> (bool, String) {
    let service = |s: Service, register: bool| {
        let r = if register { login_items::register(s) } else { login_items::unregister(s) };
        match r {
            Ok(st) => (true, login_items::advice(s, st)),
            Err(e) => (false, format!("{}: {e}", s.name())),
        }
    };
    match name {
        "login-items-status" => {
            let lines: Vec<String> = [Service::Brain, Service::MainApp].iter().map(|s| login_items::advice(*s, login_items::status(*s))).collect();
            (true, lines.join("; "))
        }
        "brain-agent-register" => service(Service::Brain, true),
        "brain-agent-unregister" => service(Service::Brain, false),
        "main-app-register" => service(Service::MainApp, true),
        "main-app-unregister" => service(Service::MainApp, false),
        "automation-iterm2" => automation("com.googlecode.iterm2", "iTerm2"),
        "automation-terminal" => automation("com.apple.Terminal", "Terminal"),
        _ => (false, "unknown action".into()),
    }
}

/// One harmless Apple Event, sent the way the locator sends them: /usr/bin/osascript
/// as a child of this process. A terminal that isn't running is never launched.
fn automation(bundle_id: &str, app: &str) -> (bool, String) {
    let running = osascript(&format!("application id \"{bundle_id}\" is running"), 10);
    match running {
        Ok(out) if out.trim() == "true" => {}
        Ok(_) => return (false, format!("{app} isn't running; start it and try again (nothing was sent)")),
        Err(e) => return (false, format!("couldn't ask whether {app} is running: {e}")),
    }
    match osascript(&format!("tell application id \"{bundle_id}\" to count windows"), 120) {
        Ok(v) => (true, format!("Automation for {app} is allowed (it answered: {} windows)", v.trim())),
        Err(e) if e.contains("-1743") => (false, format!("Automation for {app} was refused (-1743): allow Bellwether in System Settings > Privacy & Security > Automation > {app}")),
        Err(e) => (false, format!("the Apple Event to {app} failed: {e}")),
    }
}

/// Runs osascript with a time limit; stdout on success, stderr on failure.
fn osascript(script: &str, timeout_secs: u64) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/osascript").args(["-e", script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            Ok(None) => {
                // Only the child this function started, by its own handle.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {timeout_secs} s (a permission prompt may still be open)"));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(text(&out.stderr))
    }
}
