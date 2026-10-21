//! The shell's connection to the brain: one thread that long-polls GET /state and keeps
//! the latest snapshot, and starts the brain if nothing answers /health: this very
//! executable as `serve` (distribution plan D1, milestone 5), never Node.
//!
//! The webview never talks to the brain; it asks for snapshots through the
//! `get_snapshot` command, which waits on this module's condition variable.

use serde_json::Value;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::log::log;

/// The shell's own part of what the webview shows (Milestone 7): whether the strip is
/// hidden, the tray's "Show labels", and which sprites the cursor is near (the fade).
/// It travels with each snapshot as `shell`, with its own `rev`, so the webview's one
/// long poll wakes for either.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellView {
    pub rev: i64,
    pub hidden: bool,
    pub show_labels: bool,
    pub near: Vec<String>,
    /// Bumped when sheep move between monitors (Milestone 8), to wake every strip.
    pub placement: u64,
    /// Per strip window label: the Dock's extent in that strip's x (Milestone 10,
    /// Accessibility). Empty without the permission: the full-width strip.
    pub dock: Vec<(String, f64, f64)>,
    /// Per strip window label: the ids of its sprite boxes the cursor is on, by the
    /// click-through poll (deviation 136). The webview shows a hover label only for one
    /// of these: once the cursor is off every box the window stops hearing the mouse, so
    /// this is its only word that the cursor has gone. Windows with none are left out.
    pub over: Vec<(String, Vec<String>)>,
    /// A sheep being carried, when it is drawn by a strip other than the pressed one
    /// (D10, milestone 12): that strip draws it, the pressed strip doesn't.
    pub carry: Option<crate::clickthrough::Carry>,
    /// The last drag the shell cancelled itself: (pressed strip, sprite id, number).
    pub cancel: Option<(String, String, u64)>,
}

impl ShellView {
    fn json(&self) -> Value {
        // `placement`: a strip applies the flock again when it changes (a drop, a sheep moving
        // monitor), even with no new brain snapshot.
        serde_json::json!({"rev": self.rev, "hidden": self.hidden, "showLabels": self.show_labels, "near": self.near, "placement": self.placement})
    }

    /// The view as one strip window sees it (its own Dock extent, or none).
    pub fn json_for(&self, label: &str) -> Value {
        let mut v = self.json();
        v["dock"] = self.dock.iter().find(|(l, _, _)| l == label).map_or(Value::Null, |(_, lo, hi)| serde_json::json!({"lo": lo, "hi": hi}));
        v["over"] = serde_json::json!(self.over.iter().find(|(l, _)| l == label).map_or(&[][..], |(_, ids)| &ids[..]));
        // The carried sheep: where to draw it (the strip under the cursor), or that it is
        // drawn elsewhere (the pressed strip); other strips hear nothing.
        v["carry"] = match &self.carry {
            Some(c) if c.label == label => serde_json::json!({"id": c.id, "x": c.x, "y": c.y, "grabDx": c.grab_dx, "colourIndex": c.colour, "alert": c.alert}),
            Some(c) if c.pressed == label => serde_json::json!({"id": c.id, "away": true}),
            _ => Value::Null,
        };
        v["dragCancel"] = match &self.cancel {
            Some((l, id, n)) if l == label => serde_json::json!({"id": id, "n": n}),
            _ => Value::Null,
        };
        v
    }
}

pub struct Brain {
    state: Mutex<Option<Value>>,
    changed: Condvar,
    view: Mutex<ShellView>,
    /// The brain process, if the shell started it. Stopped on quit; a brain that was
    /// already running (or started by launchd) is left alone.
    pub child: Mutex<Option<Child>>,
    /// When the child was last started, so one that keeps dying isn't restarted in a loop.
    started_at: Mutex<Option<Instant>>,
}

impl Brain {
    pub fn new() -> Arc<Brain> {
        Arc::new(Brain { state: Mutex::new(None), changed: Condvar::new(), view: Mutex::new(ShellView::default()), child: Mutex::new(None), started_at: Mutex::new(None) })
    }

    /// The current snapshot (with the shell's view as `shell`) as soon as its rev is
    /// greater than `after` or the view's rev is greater than `view_after`, or whatever
    /// there is after `timeout`.
    pub fn wait_for(&self, after: i64, view_after: i64, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.state.lock().unwrap();
        loop {
            let view = self.view.lock().unwrap().clone();
            if let Some(snap) = guard.as_ref() {
                if snap.get("rev").and_then(Value::as_i64).unwrap_or(-1) > after || view.rev > view_after {
                    return Some(with_view(snap, &view));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return guard.as_ref().map(|s| with_view(s, &view));
            }
            guard = self.changed.wait_timeout(guard, deadline - now).unwrap().0;
        }
    }

    pub fn view(&self) -> ShellView {
        self.view.lock().unwrap().clone()
    }

    /// Changes the view; if anything changed, bumps its rev and wakes the webview.
    pub fn update_view(&self, f: impl FnOnce(&mut ShellView)) {
        let changed = {
            let mut v = self.view.lock().unwrap();
            let before = v.clone();
            f(&mut v);
            v.rev = before.rev;
            if *v != before {
                v.rev += 1;
                true
            } else {
                false
            }
        };
        if changed {
            // Taken so a waiter can't miss the wake between its check and its wait.
            let _g = self.state.lock().unwrap();
            self.changed.notify_all();
        }
    }

    /// The latest snapshot, if any.
    pub fn current(&self) -> Option<Value> {
        self.state.lock().unwrap().clone()
    }

    /// Whether the brain says do-not-disturb is on.
    pub fn muted(&self) -> bool {
        self.state.lock().unwrap().as_ref().and_then(|s| s.get("muted")).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn set_demo(&self, snap: Value) {
        self.set(snap);
    }

    fn set(&self, snap: Value) {
        *self.state.lock().unwrap() = Some(snap);
        self.changed.notify_all();
    }

    /// Stops the brain this shell started (only that one, by its own handle): SIGTERM, so
    /// it saves state.json, then SIGKILL after 2 s.
    pub fn stop_child(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            log(&format!("stopping the brain the shell started (pid {})", child.id()));
            #[cfg(unix)]
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn with_view(snap: &Value, view: &ShellView) -> Value {
    let mut s = snap.clone();
    if let Some(o) = s.as_object_mut() {
        o.insert("shell".into(), view.json());
    }
    s
}

/// POST /control {action} (the tray's Quiet (alerts only)), on the caller's thread.
pub fn control(port: u16, token: Option<&str>, action: &str) -> Result<(), String> {
    control_json(port, token, &serde_json::json!({ "action": action }))
}

/// POST /control with any message (`placed`, Milestone 8).
pub fn control_json(port: u16, token: Option<&str>, msg: &Value) -> Result<(), String> {
    let token = token.map(str::to_string).or_else(crate::settings::token_now).ok_or("BELLWETHER_HOOK_TOKEN is not set")?;
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(5))).http_status_as_error(false).build().new_agent();
    let resp = agent
        .post(&format!("http://127.0.0.1:{port}/control"))
        .header("X-Bellwether-Token", token)
        .header("Content-Type", "application/json")
        .send(msg.to_string())
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

pub struct BrainConfig {
    pub port: u16,
    pub token: Option<String>,
    /// Start a brain when none answers (not with `--no-spawn-brain`).
    pub spawn: bool,
    /// The started brain's environment additions (settings.rs `brain_env`).
    pub child_env: Vec<(String, std::path::PathBuf)>,
    /// Repository layout: hand the started brain the token too.
    pub child_gets_token: bool,
    /// App layout: compare the answering brain's version with this app's, and restart
    /// this app's own login agent when they differ (plan D5).
    pub check_version: bool,
    /// The strip's home: while a migration swaps the brains there (`legacy/swapping`), no
    /// brain is started here, so the new login agent gets the port (plan D9).
    pub home: std::path::PathBuf,
    /// A line for the tray menu (the version check's warning), once the tray exists.
    pub notify: Option<Box<dyn Fn(String) + Send>>,
    /// The version check's launchd and bounds; None: the real ones (`agent::Check::here`).
    /// Tests give a stand-in.
    pub agent_check: Option<crate::agent::Check>,
}

/// Runs forever on its own thread.
pub fn run(brain: Arc<Brain>, mut cfg: BrainConfig) {
    let base = format!("http://127.0.0.1:{}", cfg.port);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(40)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    if cfg.check_version {
        let check = cfg.agent_check.take().unwrap_or_else(crate::agent::Check::here);
        check_version(&agent, cfg.port, check, cfg.notify.as_deref());
    }
    let mut token = match cfg.token.clone() {
        Some(t) => t,
        None => wait_for_token(),
    };
    let mut rev: i64 = -1;
    let mut spawned_once = false;
    let mut failures = 0u32;
    loop {
        let url = format!("{base}/state?after={rev}&wait=25000");
        match agent.get(&url).header("X-Bellwether-Token", &token).call() {
            Ok(mut resp) if resp.status() == 200 => {
                failures = 0;
                match resp.body_mut().read_json::<Value>() {
                    Ok(snap) => {
                        let new_rev = snap.get("rev").and_then(Value::as_i64).unwrap_or(rev);
                        if new_rev != rev {
                            let n = snap.get("flock").and_then(Value::as_array).map_or(0, |a| a.len());
                            log(&format!("snapshot rev {new_rev} applied: {n} sheep"));
                            rev = new_rev;
                            brain.set(snap);
                        }
                    }
                    Err(e) => log(&format!("warning: /state answered something that isn't a snapshot: {e}")),
                }
            }
            Ok(resp) => {
                failures += 1;
                if resp.status() == 403 {
                    // The token may have changed under us (`bellwether token rotate`).
                    if let Some(fresh) = crate::settings::token_now().filter(|t| *t != token) {
                        log("the brain refused the shell's token (403); the token changed since the strip started, so the new one is used now");
                        token = fresh;
                        continue;
                    }
                    log("error: the brain refused the shell's token (403); the brain and the strip must use the same hook token (`bellwether doctor` compares them)");
                    std::thread::sleep(Duration::from_secs(10));
                } else {
                    log(&format!("warning: /state answered HTTP {}", resp.status()));
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            Err(e) => {
                failures += 1;
                if failures == 1 {
                    log(&format!("brain not answering on {base} ({e})"));
                }
                let swapping = cfg.spawn && bellwether_brain::toapp::swapping(&cfg.home);
                if swapping {
                    if failures == 1 {
                        log("a migration is moving the brain to the app's login agent; not starting one here");
                    }
                } else if cfg.spawn && !spawned_once && !healthy(&agent, &base) {
                    spawned_once = true;
                    spawn(&brain, &cfg, &token);
                } else if cfg.spawn && spawned_once {
                    respawn_if_dead(&brain, &cfg, &token);
                }
                std::thread::sleep(Duration::from_millis(if failures < 5 { 250 } else { 2000 }));
            }
        }
    }
}

/// No token yet (the strip started before `bellwether token init`): look again every
/// few seconds, and connect as soon as one appears.
fn wait_for_token() -> String {
    let problem = crate::settings::token_problem();
    // The token module's messages for a missing token already begin "no hook token".
    let problem = if problem.starts_with("no hook token") { problem } else { format!("no usable hook token: {problem}") };
    log(&format!("error: {problem}; the strip stays empty until there is one. Checking again every 3 s"));
    loop {
        std::thread::sleep(Duration::from_secs(3));
        if let Some(t) = crate::settings::token_now() {
            log("found the hook token; connecting to the brain");
            return t;
        }
    }
}

fn healthy(agent: &ureq::Agent, base: &str) -> bool {
    health(agent, base).is_some()
}

/// `/health`, if a brain answers within a second.
fn health(agent: &ureq::Agent, base: &str) -> Option<Value> {
    let mut r = agent.get(&format!("{base}/health")).config().timeout_global(Some(Duration::from_secs(1))).build().call().ok()?;
    (r.status() == 200).then(|| r.body_mut().read_json::<Value>().ok()).flatten()
}

/// At start, the app compares its version with the brain's (plan D5, deviation 135). A brain
/// of another version that is this app's own login agent is restarted (agent.rs, bounded);
/// anything else is left alone, with a line in the log. A restart that doesn't take is said
/// in the tray menu through `notify`.
fn check_version(agent: &ureq::Agent, port: u16, mut check: crate::agent::Check, notify: Option<&(dyn Fn(String) + Send)>) {
    let base = format!("http://127.0.0.1:{port}");
    let ours = bellwether_brain::VERSION;
    if let Some(why) = check.host.off() {
        log(&format!("brain version check: {why}"));
    }
    let outcome = crate::agent::reconcile(&mut check, &mut || health(agent, &base), ours);
    log(&outcome.log_line(port, ours));
    if let (Some(text), Some(notify)) = (outcome.notice(ours), notify) {
        notify(text);
    }
}

/// The command that starts a brain: this executable as `serve` on the strip's port, with
/// the strip's home, marked with BELLWETHER_PARENT_PID so it exits within a second of the
/// strip dying, even by SIGKILL. A brain started by launchd or by hand never has it.
pub fn child_command(exe: &std::path::Path, cfg: &BrainConfig, token: &str, parent: u32) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(["serve", "--port", &cfg.port.to_string()]);
    for (k, v) in &cfg.child_env {
        cmd.env(k, v);
    }
    if cfg.child_gets_token {
        cmd.env("BELLWETHER_HOOK_TOKEN", token);
    }
    // The brain writes its own log (brain.log); stdout and stderr would only repeat it.
    cmd.env("BELLWETHER_PARENT_PID", parent.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd
}

fn spawn(brain: &Brain, cfg: &BrainConfig, token: &str) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log(&format!("error: could not find this executable to start the brain ({e})"));
            return;
        }
    };
    *brain.started_at.lock().unwrap() = Some(Instant::now());
    match child_command(&exe, cfg, token, std::process::id()).spawn() {
        Ok(child) => {
            log(&format!("started the brain: {} serve --port {} (pid {}); it stops when the strip does", exe.display(), cfg.port, child.id()));
            *brain.child.lock().unwrap() = Some(child);
        }
        Err(e) => log(&format!("error: could not start the brain with {} serve ({e})", exe.display())),
    }
}

/// A started brain that has exited is started again, at most once every 10 s (it may be
/// failing: a held port, a bad config; its brain.log says why).
fn respawn_if_dead(brain: &Brain, cfg: &BrainConfig, token: &str) {
    let status = match brain.child.lock().unwrap().as_mut() {
        Some(child) => child.try_wait().ok().flatten(),
        None => return,
    };
    let Some(status) = status else { return };
    if brain.started_at.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
        return;
    }
    log(&format!("the brain the shell started has exited ({status}); starting it again (its brain.log says why it stopped)"));
    spawn(brain, cfg, token);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A brain on a free port whose /health is the agent test world's files (version, pid),
    /// so the stand-in launchctl's "restart" changes what it answers. Anything else: 503.
    #[cfg(unix)]
    fn world_brain(dir: std::path::PathBuf) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 47417);
        std::thread::spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let resp = if String::from_utf8_lossy(&buf[..n]).starts_with("GET /health") {
                    let v = std::fs::read_to_string(dir.join("version")).unwrap_or_default();
                    let pid = std::fs::read_to_string(dir.join("pid")).unwrap_or_default();
                    let body = format!("{{\"ok\":true,\"name\":\"bellwether\",\"version\":\"{}\",\"pid\":{}}}", v.trim(), pid.trim());
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                } else {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                };
                let _ = s.write_all(resp.as_bytes());
            }
        });
        port
    }

    // The launchctl stand-in is a shell script: Unix only (Windows plan W1).
    #[cfg(unix)]
    fn wired(w: &crate::agent::tests::World) -> (BrainConfig, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let cfg = BrainConfig {
            port: world_brain(w.dir.clone()),
            token: Some("testTokenNotReal_abcdefghijklmnop".into()),
            spawn: false,
            child_env: vec![],
            child_gets_token: false,
            check_version: true,
            home: w.dir.clone(),
            notify: Some(Box::new(move |t| tx.lock().unwrap().send(t).unwrap())),
            agent_check: Some(w.check()),
        };
        (cfg, rx)
    }

    /// Deviation 135, the path that failed for the maintainer: the strip's brain thread
    /// (`run`) checks the version before anything else, restarts this app's agent, and
    /// hands a restart that doesn't take to the tray (`notify`). A no-op check fails this.
    // The launchctl stand-in is a shell script: Unix only (Windows plan W1).
    #[cfg(unix)]
    #[test]
    fn the_brain_thread_restarts_an_old_agent_and_tells_the_tray_when_it_cant() {
        use crate::agent::tests::World;
        let ours = bellwether_brain::VERSION;
        // The update case: the agent's executable was deleted; one restart brings `ours`.
        let w = World::new("run-ok", "0.0.1-old", 4001, Some(4001));
        std::fs::write(w.dir.join("new-version"), ours).unwrap();
        std::fs::write(w.dir.join("exe-4001"), "deleted").unwrap();
        let (cfg, rx) = wired(&w);
        std::thread::spawn(move || run(Brain::new(), cfg));
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(w.dir.join("version")).unwrap_or_default().trim() != ours {
            assert!(Instant::now() < deadline, "the brain thread never restarted the agent: {:?}", w.calls());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(w.calls().iter().any(|c| c.starts_with("kickstart -k")), "{:?}", w.calls());
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "a restart that took is no notice");
        // A restart that doesn't take: two tries, then the menu line, through `notify`.
        let w = World::new("run-stuck", "0.0.1-old", 4001, Some(4001));
        std::fs::write(w.dir.join("stuck"), "").unwrap();
        let (cfg, rx) = wired(&w);
        std::thread::spawn(move || run(Brain::new(), cfg));
        let notice = rx.recv_timeout(Duration::from_secs(15)).expect("no tray notice from the brain thread");
        assert_eq!(notice, format!("Brain is still 0.0.1-old, not {ours} (see bellwether doctor)"));
        assert_eq!(w.calls().iter().filter(|c| c.starts_with("kickstart -k")).count(), 2);
        // The app wires that notice to the tray, and the real check is the default.
        let lib = include_str!("lib.rs");
        let at = lib.find("cfg.notify = Some(Box::new(move |text| tray::late_notice(&h, text)));").expect("notify wired to the tray");
        assert!(at < lib.find("std::thread::spawn(move || brain::run(b, cfg));").unwrap());
        let code = include_str!("brain.rs").split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("let check = cfg.agent_check.take().unwrap_or_else(crate::agent::Check::here);"));
    }

    #[test]
    fn the_child_is_this_executable_as_serve_marked_with_the_parent() {
        let cfg = BrainConfig { port: 47999, token: None, spawn: true, child_env: vec![("BELLWETHER_HOME".into(), "/tmp/bw-home".into())], child_gets_token: false, check_version: true, home: "/tmp/bw-home".into(), notify: None, agent_check: None };
        let cmd = child_command(std::path::Path::new("/x/Bellwether.app/Contents/MacOS/Bellwether"), &cfg, "tokenNotPassed_0123456789", 4242);
        assert_eq!(cmd.get_program(), "/x/Bellwether.app/Contents/MacOS/Bellwether");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["serve", "--port", "47999"]);
        let env: std::collections::HashMap<_, _> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert_eq!(env["BELLWETHER_PARENT_PID"].as_deref(), Some("4242"));
        assert_eq!(env["BELLWETHER_HOME"].as_deref(), Some("/tmp/bw-home"));
        assert!(!env.contains_key("BELLWETHER_HOOK_TOKEN"), "the app's brain reads hook-token itself");
        let repo = BrainConfig { child_gets_token: true, ..cfg };
        let cmd = child_command(std::path::Path::new("/r/target/release/bellwether-shell"), &repo, "repoToken_0123456789abc", 7);
        assert!(cmd.get_envs().any(|(k, v)| k == "BELLWETHER_HOOK_TOKEN" && v == Some(std::ffi::OsStr::new("repoToken_0123456789abc"))));
        // No Node anywhere: the brain is this binary.
        let src = include_str!("brain.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("cli.mjs") && !code.contains("node_path") && !code.contains("nodePath"));
    }
}
