//! The shell's connection to the brain: one thread that long-polls GET /state and keeps
//! the latest snapshot, and starts the brain if nothing answers /health.
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
    /// already running (or autostarted at login) is left alone.
    pub child: Mutex<Option<Child>>,
}

impl Brain {
    pub fn new() -> Arc<Brain> {
        Arc::new(Brain { state: Mutex::new(None), changed: Condvar::new(), view: Mutex::new(ShellView::default()), child: Mutex::new(None) })
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

    pub fn stop_child(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            log(&format!("stopping the brain the shell started (pid {})", child.id()));
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

/// POST /control {action} (the tray's Mute), on the caller's thread.
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
    pub node_path: String,
    pub agent_dir: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub spawn: bool,
}

/// Runs forever on its own thread.
pub fn run(brain: Arc<Brain>, cfg: BrainConfig) {
    let base = format!("http://127.0.0.1:{}", cfg.port);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(40)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let token = match cfg.token.clone() {
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
                    log("error: the brain refused the shell's token (403); the brain and the shell must use the same BELLWETHER_HOOK_TOKEN");
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
                if cfg.spawn && !spawned_once && !healthy(&agent, &base) {
                    spawned_once = true;
                    spawn(&brain, &cfg);
                } else if cfg.spawn && spawned_once {
                    respawn_if_dead(&brain, &cfg);
                }
                std::thread::sleep(Duration::from_millis(if failures < 5 { 500 } else { 2000 }));
            }
        }
    }
}

/// No token yet (the strip started before `bellwether token init`): look again every
/// few seconds, and connect as soon as one appears.
fn wait_for_token() -> String {
    log("error: BELLWETHER_HOOK_TOKEN is not set (environment or the .env); the strip stays empty until it is. Checking again every 3 s (`bellwether token init` adds one)");
    loop {
        std::thread::sleep(Duration::from_secs(3));
        if let Some(t) = crate::settings::token_now() {
            log("found BELLWETHER_HOOK_TOKEN in the .env; connecting to the brain");
            return t;
        }
    }
}

fn healthy(agent: &ureq::Agent, base: &str) -> bool {
    agent
        .get(&format!("{base}/health"))
        .config()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

fn spawn(brain: &Brain, cfg: &BrainConfig) {
    let _ = std::fs::create_dir_all(&cfg.data_dir);
    let log_path = cfg.data_dir.join("brain-from-shell.log");
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log_path);
    let mut cmd = Command::new(&cfg.node_path);
    // The CLI loads its own .env (brain/envpath.mjs), by the same rule as settings.rs.
    cmd.args(["brain/cli.mjs", "serve"])
        .current_dir(&cfg.agent_dir)
        // Marks this brain as the shell's: it exits when the shell is gone, even after
        // a SIGKILL that gives the shell no chance to stop it. A brain started by the
        // LaunchAgent or by hand never has this variable.
        .env("BELLWETHER_PARENT_PID", std::process::id().to_string())
        .stdin(Stdio::null());
    match out {
        Ok(f) => {
            let f2 = f.try_clone().ok();
            cmd.stdout(Stdio::from(f));
            if let Some(f2) = f2 {
                cmd.stderr(Stdio::from(f2));
            }
        }
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            log(&format!("started the brain: {} brain/cli.mjs serve (pid {}, log {})", cfg.node_path, child.id(), log_path.display()));
            *brain.child.lock().unwrap() = Some(child);
        }
        Err(e) => log(&format!("error: could not start the brain with {:?} ({e}); set brain.nodePath in config.json", cfg.node_path)),
    }
}

fn respawn_if_dead(brain: &Brain, cfg: &BrainConfig) {
    let dead = match brain.child.lock().unwrap().as_mut() {
        Some(child) => matches!(child.try_wait(), Ok(Some(_))),
        None => false,
    };
    if dead {
        log("the brain the shell started has exited; starting it again");
        spawn(brain, cfg);
    }
}
