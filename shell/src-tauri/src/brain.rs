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
}

impl ShellView {
    fn json(&self) -> Value {
        serde_json::json!({"rev": self.rev, "hidden": self.hidden, "showLabels": self.show_labels, "near": self.near})
    }

    /// The view as one strip window sees it (its own Dock extent, or none).
    pub fn json_for(&self, label: &str) -> Value {
        let mut v = self.json();
        v["dock"] = self.dock.iter().find(|(l, _, _)| l == label).map_or(Value::Null, |(_, lo, hi)| serde_json::json!({"lo": lo, "hi": hi}));
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
}

/// Runs forever on its own thread.
pub fn run(brain: Arc<Brain>, cfg: BrainConfig) {
    let base = format!("http://127.0.0.1:{}", cfg.port);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(40)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    if cfg.check_version {
        check_version(&agent, cfg.port);
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

/// What to do about the brain answering on the port, by its version (plan D5).
#[derive(Debug, PartialEq, Eq)]
pub enum VersionAction {
    /// The same version as this app, or not a bellwether brain at all.
    Keep,
    /// This app's own login agent, still running an older executable: restart it.
    RestartAgent,
    /// Another version that isn't this app's agent: left alone, and said so.
    Foreign,
}

pub fn version_action(health: &Value, ours: &str, is_our_agent: bool) -> VersionAction {
    if health.get("name").and_then(Value::as_str) != Some("bellwether") {
        return VersionAction::Keep;
    }
    match health.get("version").and_then(Value::as_str) {
        Some(v) if v == ours => VersionAction::Keep,
        _ if is_our_agent => VersionAction::RestartAgent,
        _ => VersionAction::Foreign,
    }
}

/// At start, the app compares its version with the brain's. A brain of another version
/// that is this bundle's login agent (launchd's child, running this bundle's executable)
/// is restarted with `launchctl kickstart -k`, and the app waits for `/health` to report
/// the new version. Anything else is left alone (it may be a repository install's brain,
/// or another copy of the app), with a line in the log.
fn check_version(agent: &ureq::Agent, port: u16) {
    let base = format!("http://127.0.0.1:{port}");
    let Some(h) = health(agent, &base) else { return };
    let ours = bellwether_brain::VERSION;
    let theirs = h.get("version").and_then(Value::as_str).unwrap_or("?").to_string();
    let pid = h.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
    let label = crate::login_items::brain_plist_name().and_then(|n| n.strip_suffix(".plist").map(str::to_string));
    // This app's login agent: a process running this bundle's executable under launchd
    // (ppid 1) AND the pid launchd itself gives for this bundle's agent label. A `serve`
    // started by hand and orphaned also has ppid 1, but launchd doesn't know it.
    let looks_ours = crate::identity::bundle_processes().iter().any(|(p, ppid, _)| *p == pid && *ppid == 1);
    let is_our_agent = looks_ours && label.as_deref().and_then(agent_pid) == Some(pid);
    match version_action(&h, ours, is_our_agent) {
        VersionAction::Keep => log(&format!("brain on port {port}: version {theirs}, pid {pid}{}", if is_our_agent { " (this app's login agent)" } else { "" })),
        VersionAction::Foreign => log(&format!(
            "warning: the brain on port {port} is version {theirs} (pid {pid}), not this app's {ours}, and it isn't this app's login agent{}, so it is left alone. `bellwether doctor` shows what it is",
            if looks_ours { " (it runs this app's executable under launchd, but launchd doesn't list it as the agent: started by hand?)" } else { "" }
        )),
        VersionAction::RestartAgent => {
            let Some(label) = label else { return };
            let target = format!("gui/{}/{label}", unsafe { libc::getuid() });
            log(&format!("the brain on port {port} is this app's login agent at version {theirs} (pid {pid}); this app is {ours}: restarting it (launchctl kickstart -k {target})"));
            let started = Instant::now();
            match run_limited("/bin/launchctl", &["kickstart", "-k", &target], Duration::from_secs(10)) {
                Ok(()) => {}
                Err(e) => {
                    log(&format!("warning: launchctl kickstart -k {target} failed: {e}"));
                    return;
                }
            }
            let deadline = started + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Some(h) = health(agent, &base) {
                    if h.get("version").and_then(Value::as_str) == Some(ours) {
                        let new_pid = h.get("pid").and_then(Value::as_i64).unwrap_or(0);
                        log(&format!("the login agent's brain is now version {ours} (pid {new_pid}), {} ms after the restart", started.elapsed().as_millis()));
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            log(&format!("warning: after the restart the brain on port {port} still doesn't report version {ours} within 10 s"));
        }
    }
}

/// The pid launchd gives for this user's agent `label` (`launchctl print gui/<uid>/<label>`,
/// read only), if it is running.
fn agent_pid(label: &str) -> Option<i32> {
    let out = Command::new("/bin/launchctl").args(["print", &format!("gui/{}/{label}", unsafe { libc::getuid() })]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| launchd_pid(&String::from_utf8_lossy(&out.stdout))).flatten()
}

/// The service's own `pid = N` line in `launchctl print` output: one tab deep (nested
/// dictionaries are deeper).
pub fn launchd_pid(text: &str) -> Option<i32> {
    text.lines().find_map(|l| l.strip_prefix("\tpid = ")).and_then(|v| v.trim().parse().ok())
}

/// Runs a command with a time limit; only the child it started is ever killed.
fn run_limited(program: &str, args: &[&str], limit: Duration) -> Result<(), String> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                return Err(format!("{st}: {}", err.trim()));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", limit.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
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

    #[test]
    fn the_version_check_restarts_only_this_apps_own_agent() {
        let h = |v: &str| serde_json::json!({"ok": true, "name": "bellwether", "version": v, "pid": 42});
        assert_eq!(version_action(&h("0.1.0"), "0.1.0", true), VersionAction::Keep);
        assert_eq!(version_action(&h("0.1.0"), "0.1.0", false), VersionAction::Keep);
        assert_eq!(version_action(&h("0.0.9"), "0.1.0", true), VersionAction::RestartAgent);
        assert_eq!(version_action(&h("0.0.9"), "0.1.0", false), VersionAction::Foreign, "a repository brain or another copy is never restarted");
        assert_eq!(version_action(&serde_json::json!({"name": "other", "version": "9"}), "0.1.0", true), VersionAction::Keep, "not a bellwether brain: not ours to judge");
        assert_eq!(version_action(&serde_json::json!({"name": "bellwether"}), "0.1.0", false), VersionAction::Foreign);
    }

    #[test]
    fn the_agents_pid_is_launchds_own() {
        let print = "gui/501/com.x.brain = {\n\tactive count = 1\n\tpath = /x\n\tstate = running\n\tprogram identifier = Contents/MacOS/Bellwether (mode: 2)\n\tpid = 6328\n\tendpoints = {\n\t\tpid = 1\n\t}\n}\n";
        assert_eq!(launchd_pid(print), Some(6328));
        assert_eq!(launchd_pid("gui/501/com.x.brain = {\n\tstate = not running\n\tlast exit code = 0\n}\n"), None, "not running: no pid");
        assert_eq!(launchd_pid("\t\tpid = 7\n"), None, "a nested pid is not the service's");
        // The restart needs both: launchd's pid for the label equals /health's pid.
        let src = include_str!("brain.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("looks_ours && label.as_deref().and_then(agent_pid) == Some(pid)"));
    }

    #[test]
    fn the_child_is_this_executable_as_serve_marked_with_the_parent() {
        let cfg = BrainConfig { port: 47999, token: None, spawn: true, child_env: vec![("BELLWETHER_HOME".into(), "/tmp/bw-home".into())], child_gets_token: false, check_version: true, home: "/tmp/bw-home".into() };
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
