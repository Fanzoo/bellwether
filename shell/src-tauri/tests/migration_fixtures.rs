//! Distribution plan D9 and milestone 8: `bellwether migrate --to-app` and `--undo` on the
//! three prepared fixture HOMEs (a renamed repository install, a pre-rename pasture install,
//! hook entries alone), run through the real binary.
//!
//! Nothing here can reach the real launchd, the real login items or the live install:
//! - every run gets a temp HOME (and BELLWETHER_TEST_ROOT, so a run that somehow got the
//!   real HOME is refused before it reads anything; `migration.rs`, `choose`);
//! - launchctl is `fake-launchctl` and the login items are `fake-login-items`, two small
//!   shell scripts written here (BELLWETHER_LAUNCHCTL, BELLWETHER_LOGIN_ITEMS). The binary
//!   refuses a stand-in that is launchctl itself;
//! - "launchd's" agents are processes this test starts: a Rust brain playing the old Node
//!   brain on a free port (never 47417), and a `sleep` playing the old strip. The fake
//!   launchctl's `bootout` stops exactly the pid this test wrote for that label; the fake
//!   login items' `register` starts this binary's `serve` on the same port as the app's
//!   brain agent would, and `unregister` stops that pid. Every pid is tracked and stopped
//!   at the end, each after checking what it runs.

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bellwether_core::hooks::{self as installer, Action};
use bellwether_core::lossless;

const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";
const LIVE_PORT: u16 = 47417;
const BIN: &str = env!("CARGO_BIN_EXE_bellwether-shell");

const PET: &str = "<animations><header><title>tiny</title></header><image><tilesx>2</tilesx><tilesy>1</tilesy><png>iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==</png><transparency>Magenta</transparency></image><spawns><spawn id=\"1\"><x>0</x><y>0</y><next probability=\"100\">1</next></spawn></spawns><animations><animation id=\"1\"><name>walk</name><start><x>0</x><y>0</y><interval>200</interval></start><end><x>0</x><y>0</y><interval>200</interval></end><sequence repeat=\"0\" repeatfrom=\"0\"><frame>1</frame><frame>0</frame></sequence></animation></animations></animations>";

const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
# A stand-in for launchctl, for tests only. It never runs the real one. "Loaded" labels are
# files in $FAKE_STATE/loaded holding the pid the test started for them.
S="$FAKE_STATE"
echo "$*" >> "$S/launchctl.log"
case "$1" in
  print)
    label="${2##*/}"
    if [ -f "$S/loaded/$label" ]; then
      printf '%s = {\n\tstate = running\n' "$2"
      pid=$(cat "$S/loaded/$label")
      [ -n "$pid" ] && printf '\tpid = %s\n' "$pid"
      printf '}\n'
      exit 0
    fi
    echo "Bad request." >&2
    echo "Could not find service \"$label\" in domain for user gui: 501" >&2
    exit 113 ;;
  bootout)
    label="${2##*/}"
    if [ -f "$S/loaded/$label" ]; then
      pid=$(cat "$S/loaded/$label")
      rm -f "$S/loaded/$label"
      if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null
        i=0
        while kill -0 "$pid" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
      fi
      exit 0
    fi
    echo "Boot-out failed: 3: No such process" >&2
    exit 3 ;;
  bootstrap)
    label=$(basename "$3" .plist)
    : > "$S/loaded/$label"
    exit 0 ;;
  *)
    echo "fake launchctl: $1 isn't supported" >&2
    exit 64 ;;
esac
"#;

const FAKE_LOGIN_ITEMS: &str = r#"#!/bin/sh
# A stand-in for the app's two login items (SMAppService), for tests only. `register` plays
# the brain agent: this binary's `serve` on the test port, with the app's own files.
S="$FAKE_STATE"
echo "$1" >> "$S/login.log"
case "$1" in
  register)
    if [ -f "$S/kill-parent" ]; then
      # BR-3K's M8 D1: the run dies (a closed terminal, Ctrl-C) while it waits for the brain.
      rm -f "$S/kill-parent"
      kill -9 "$PPID"
      sleep 1
      exit 1
    fi
    if [ -f "$S/register-fails" ]; then
      echo "brain agent: waiting for your approval"
      exit 1
    fi
    : > "$S/registered"
    if [ ! -f "$S/no-brain" ]; then
      "$FAKE_BIN" serve --port "$FAKE_PORT" </dev/null >/dev/null 2>&1 &
      echo $! > "$S/app-brain.pid"
    fi
    echo "brain agent: enabled"
    echo "app at login: enabled" ;;
  status)
    if [ -f "$S/registered" ]; then echo registered; else echo off; fi ;;
  unregister)
    rm -f "$S/registered"
    if [ -f "$S/app-brain.pid" ]; then
      pid=$(cat "$S/app-brain.pid")
      kill "$pid" 2>/dev/null
      i=0
      while kill -0 "$pid" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
      rm -f "$S/app-brain.pid"
    fi
    echo "app at login: off"
    echo "brain agent: off" ;;
esac
"#;

/// A port nothing listens on (bound, then released); never the live one.
fn free_port() -> u16 {
    loop {
        let p = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        if p != LIVE_PORT && p > 47417 {
            return p;
        }
    }
}

/// The command line of a pid (ps; read only).
fn command_of(pid: i32) -> String {
    let o = Command::new("/bin/ps").args(["-o", "command=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// Processes this test started: reaped as they exit; the ones still running at the end are
/// stopped, each by its pid and only while it still runs what was started.
#[derive(Default)]
struct Procs(Vec<(i32, String, Arc<AtomicBool>)>);

impl Procs {
    fn start(&mut self, mut cmd: Command, what: &str) -> i32 {
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let pid = child.id() as i32;
        let gone = Arc::new(AtomicBool::new(false));
        let g = gone.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            g.store(true, Ordering::SeqCst);
        });
        self.0.push((pid, what.to_string(), gone));
        pid
    }

    fn alive(&self, pid: i32) -> bool {
        self.0.iter().find(|(p, _, _)| *p == pid).is_some_and(|(_, _, g)| !g.load(Ordering::SeqCst))
    }
}

impl Drop for Procs {
    fn drop(&mut self) {
        for (pid, what, gone) in &self.0 {
            if !gone.load(Ordering::SeqCst) && command_of(*pid).contains(what.as_str()) {
                unsafe { libc::kill(*pid, libc::SIGTERM) };
            }
        }
    }
}

/// A brain the fake login items started (not this test's child): stopped by its pid, only
/// while it runs this binary's `serve`.
fn stop_stub_brain(state: &Path) {
    if let Ok(t) = std::fs::read_to_string(state.join("app-brain.pid")) {
        if let Ok(pid) = t.trim().parse::<i32>() {
            if pid > 1 && command_of(pid).contains(&format!("{BIN} serve")) {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Renamed,
    Pasture,
    HooksOnly,
}

struct World {
    kind: Kind,
    base: PathBuf,
    home: PathBuf,
    app: PathBuf,
    repo: PathBuf,
    agents: PathBuf,
    settings: PathBuf,
    state: PathBuf,
    port: u16,
    labels: [&'static str; 2],
    procs: Procs,
    stub_pids: Vec<i32>,
}

impl Drop for World {
    fn drop(&mut self) {
        stop_stub_brain(&self.state);
        // The fake old agents (this test's children) are stopped by `procs`.
    }
}

fn plist(label: &str, args: &[String], wd: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n{}  </array>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n  <key>RunAtLoad</key>\n  <true/>\n</dict>\n</plist>\n",
        args.iter().map(|a| format!("    <string>{a}</string>\n")).collect::<String>(),
        wd.display()
    )
}

/// The three fixture HOMEs of the test plan (plists, `.env`, `data/`, settings).
fn world(root: &Path, kind: Kind) -> World {
    let base = root.join(format!("{kind:?}").to_lowercase());
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    let app = home.join("Library").join("Application Support").join("Bellwether");
    let agents = home.join("Library").join("LaunchAgents");
    let repo = base.join(if kind == Kind::Pasture { "pasture" } else { "bellwether" });
    let state = base.join("stubs");
    std::fs::create_dir_all(state.join("loaded")).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let port = free_port();
    let settings = home.join(".claude").join("settings.json");
    let labels = if kind == Kind::Pasture { ["local.pasture.brain", "local.pasture.shell"] } else { ["local.bellwether.brain", "local.bellwether.shell"] };

    // The settings file: another tool's hook, and Bellwether's (pasture's headers before the rename).
    let mut s = lossless::parse("{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"afplay ready.wav\"\n          }\n        ]\n      }\n    ]\n  },\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"echo hi\"\n  }\n}\n").unwrap();
    s = installer::install(&s, port, TOKEN).unwrap();
    let mut text = lossless::stringify(&s, "  ") + "\n";
    if kind == Kind::Pasture {
        text = text.replace("X-Bellwether-", "X-Pasture-");
    }
    std::fs::write(&settings, &text).unwrap();

    if kind == Kind::HooksOnly {
        // No repository and no agents: only the hooks (and the app's config naming the
        // test port, so the live port is never involved).
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("config.json"), format!("{{\n  \"port\": {port}\n}}\n")).unwrap();
    } else {
        std::fs::create_dir_all(repo.join("brain")).unwrap();
        std::fs::write(repo.join("brain").join("cli.mjs"), "// the old CLI\n").unwrap();
        let data = repo.join("data");
        std::fs::create_dir_all(data.join("backups")).unwrap();
        std::fs::create_dir_all(data.join("pets").join("esheep64")).unwrap();
        let key = if kind == Kind::Pasture { "PASTURE_HOOK_TOKEN" } else { "BELLWETHER_HOOK_TOKEN" };
        std::fs::write(repo.join(".env"), format!("# the old install's .env\n{key}={TOKEN}\n")).unwrap();
        std::fs::write(repo.join("config.json"), format!("{{\n  \"port\": {port},\n  \"brain\": {{\n    \"nodePath\": \"/usr/local/bin/node\"\n  }},\n  \"autostart\": true,\n  \"pets\": {{\n    \"default\": \"esheep64\"\n  }}\n}}\n")).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        std::fs::write(data.join("state.json"), format!("{{\"version\":1,\"muted\":true,\"identities\":{{\"herdr:w1:p1\":{{\"colourIndex\":2,\"slots\":{{\"main\":3}},\"label\":\"meadow\",\"lastSeen\":{now}}},\"herdr:w1:p2\":{{\"colourIndex\":5,\"slots\":{{\"main\":0}},\"label\":\"barn\",\"lastSeen\":{now}}}}}}}")).unwrap();
        // As the repository strip writes it (brain/privacy.mjs); pasture's said `labels`.
        let label_key = if kind == Kind::Pasture { "labels" } else { "showLabels" };
        std::fs::write(data.join("shell-state.json"), format!("{{\n  \"version\": 1,\n  \"hidden\": false,\n  \"{label_key}\": true\n}}")).unwrap();
        std::fs::write(data.join("installs.json"), format!("{{\n  \"{}\": {{\n    \"events\": [\"Stop\"],\n    \"hooks\": false\n  }}\n}}\n", settings.display())).unwrap();
        std::fs::write(data.join("backups").join("settings-20261001-120000.json"), "{\"old\": true}\n").unwrap();
        std::fs::write(data.join("pets").join("esheep64").join("animations.xml"), PET).unwrap();
        std::fs::create_dir_all(&agents).unwrap();
        let node = "/usr/local/bin/node".to_string();
        std::fs::write(agents.join(format!("{}.plist", labels[0])), plist(labels[0], &[node, format!("{}/brain/cli.mjs", repo.display()), "serve".into()], &repo)).unwrap();
        let strip = if kind == Kind::Pasture { "pasture-shell" } else { "bellwether-shell" };
        std::fs::write(agents.join(format!("{}.plist", labels[1])), plist(labels[1], &[format!("{}/shell/src-tauri/target/release/{strip}", repo.display()), "--no-spawn-brain".into()], &repo)).unwrap();
        // Another of the user's LaunchAgents, never touched.
        std::fs::write(agents.join("com.example.other.plist"), plist("com.example.other", &["/usr/bin/true".into()], &home)).unwrap();
    }
    for (name, text) in [("fake-launchctl", FAKE_LAUNCHCTL), ("fake-login-items", FAKE_LOGIN_ITEMS)] {
        std::fs::write(state.join(name), text).unwrap();
        std::fs::set_permissions(state.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    World { kind, base, home, app, repo, agents, settings, state, port, labels, procs: Procs::default(), stub_pids: Vec::new() }
}

impl World {
    /// "launchd" runs the old brain (a Rust brain on the test port, with the old token) and
    /// the old strip (a sleep).
    fn start_old_agents(&mut self) {
        let old_home = self.base.join("old-brain-home");
        std::fs::create_dir_all(&old_home).unwrap();
        let mut brain = Command::new(BIN);
        brain.args(["serve", "--port", &self.port.to_string()]).env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &self.base).env("BELLWETHER_HOME", &old_home).env("BELLWETHER_HOOK_TOKEN", TOKEN);
        let pid = self.procs.start(brain, BIN);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !health(self.port).is_some_and(|(_, p)| p == pid) {
            assert!(Instant::now() < deadline, "the old brain didn't come up");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::write(self.state.join("loaded").join(self.labels[0]), pid.to_string()).unwrap();
        let mut strip = Command::new("/bin/sleep");
        strip.arg("600");
        let spid = self.procs.start(strip, "sleep 600");
        std::fs::write(self.state.join("loaded").join(self.labels[1]), spid.to_string()).unwrap();
        self.stub_pids = vec![pid, spid];
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("BELLWETHER_TEST_ROOT", &self.base)
            .env("BELLWETHER_LAUNCHCTL", self.state.join("fake-launchctl"))
            .env("BELLWETHER_LOGIN_ITEMS", self.state.join("fake-login-items"))
            .env("BELLWETHER_TEST_VERIFY_MS", "4000")
            .env("FAKE_STATE", &self.state)
            .env("FAKE_BIN", BIN)
            .env("FAKE_PORT", self.port.to_string())
            .current_dir(&self.base)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("{args:?} didn't finish within 90 s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    fn log(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.state.join(name)).unwrap_or_default().lines().filter(|l| !l.starts_with("print ") && *l != "status").map(str::to_string).collect()
    }

    fn uid(&self) -> u32 {
        unsafe { libc::getuid() }
    }
}

/// `/health`'s name and pid on the port.
fn health(port: u16) -> Option<(String, i32)> {
    assert_ne!(port, LIVE_PORT);
    let r = bellwether_brain::client::request(port, "GET", "/health", &[], None, Duration::from_millis(500)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
    Some((v["name"].as_str()?.to_string(), v["pid"].as_i64()? as i32))
}

/// The actions a preview lists (its "   - " lines).
fn actions(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.starts_with("   - ")).map(|l| l[5..].to_string()).collect()
}

/// Every file under `dir`, relative, with directories marked by a trailing slash.
fn tree(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(dir).unwrap().display().to_string();
            if p.is_dir() {
                out.insert(format!("{rel}/"));
                stack.push(p);
            } else {
                out.insert(rel);
            }
        }
    }
    out
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn expected_actions(w: &World) -> Vec<String> {
    let uid = w.uid();
    let h = &w.app;
    let mut a = Vec::new();
    match w.kind {
        Kind::HooksOnly => {
            a.push(format!("copy the token of the hook entries in {} into {}/hook-token (same value, not shown)", w.settings.display(), h.display()));
        }
        _ => {
            a.push(format!("copy the token from {}/.env into {}/hook-token (same value, not shown, so running sessions keep working)", w.repo.display(), h.display()));
            a.push(format!("copy config.json from {}/config.json, without autostart and brain.nodePath (the app doesn't read them)", w.repo.display()));
            a.push("copy state.json: 2 session colour(s) and slot(s), and mute".into());
            a.push("copy shell-state.json: Hide everything off, Show labels on".into());
            a.push("copy installs.json (which events the hook install created)".into());
            a.push("copy backups/settings-20261001-120000.json (0600)".into());
            a.push("copy the pet esheep64, as an imported pet (checked and pinned as `pets import` does)".into());
            for l in w.labels {
                a.push(format!("stop {l} (launchctl bootout gui/{uid}/{l})"));
            }
            for l in w.labels {
                a.push(format!("move {}/{l}.plist to {}/legacy/", w.agents.display(), h.display()));
            }
        }
    }
    a.push("register the brain agent and the app at login".into());
    a.push(format!("wait up to 4 s for the new brain to answer on port {} and take the token", w.port));
    if w.kind == Kind::Pasture {
        a.push("rewrite 16 pasture entries (X-Pasture-*) with the current header names, a backup first (diff below, token masked)".into());
    }
    a
}

fn check_fixture(kind: Kind) {
    let root = std::env::temp_dir().join(format!("bw-m8-{}", std::process::id()));
    let mut w = world(&root, kind);
    assert!(!bellwether_brain::migrate::is_real_home(&w.home));
    if kind != Kind::HooksOnly {
        w.start_old_agents();
    }
    let original_settings = std::fs::read(&w.settings).unwrap();
    let original_plists: Vec<(PathBuf, Vec<u8>)> = w.labels.iter().map(|l| w.agents.join(format!("{l}.plist"))).filter(|p| p.exists()).map(|p| (p.clone(), std::fs::read(&p).unwrap())).collect();
    let app_before = tree(&w.app);
    // What the settings file must hold afterwards: the installer's own rewrite of it.
    let expected_settings = installer::plan_edit(&w.settings, Action::Install, w.port, TOKEN, None).unwrap().new_text.into_bytes();

    // The preview: exactly the expected actions, and nothing changed.
    let show = std::env::var_os("BW_M8_SHOW").is_some();
    let (code, out, err) = w.run(&["migrate", "--to-app"]);
    if show {
        eprintln!("==== {kind:?}: migrate --to-app\n{out}{err}");
    }
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(actions(&out), expected_actions(&w), "{kind:?} preview:\n{out}");
    assert!(out.contains("Preview only; nothing was changed"), "{out}");
    assert!(!out.contains(TOKEN) && !err.contains(TOKEN), "the token is never printed");
    assert_eq!(std::fs::read(&w.settings).unwrap(), original_settings);
    assert_eq!(tree(&w.app), app_before, "a preview writes nothing");
    assert!(w.log("launchctl.log").is_empty() && w.log("login.log").is_empty(), "a preview only asks launchd (print)");

    // --write.
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    if show {
        eprintln!("==== {kind:?}: migrate --to-app --write\n{out}{err}");
    }
    assert_eq!(code, 0, "{kind:?} --write:\n{out}{err}");
    assert!(!out.contains(TOKEN) && !err.contains(TOKEN), "the token is never printed");
    let uid = w.uid();
    let booted: Vec<String> = if kind == Kind::HooksOnly { vec![] } else { w.labels.iter().map(|l| format!("bootout gui/{uid}/{l}")).collect() };
    assert_eq!(w.log("launchctl.log"), booted);
    assert_eq!(w.log("login.log"), ["register"]);
    for pid in &w.stub_pids {
        assert!(!w.procs.alive(*pid), "launchd's bootout stopped the old agent (pid {pid})");
    }
    // The new brain answers with the same token: a session started before keeps working.
    let (name, pid) = health(w.port).expect("the new brain answers");
    assert_eq!(name, "bellwether");
    assert!(!w.stub_pids.contains(&pid));
    let hook = bellwether_brain::client::request(w.port, "POST", "/hook", &[("Content-Type", "application/json"), ("X-Bellwether-Token", TOKEN)], Some(b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s-before\"}"), Duration::from_secs(2)).unwrap();
    assert_eq!(hook.status, 204, "the old hooks' token is the new brain's");
    assert!(out.contains("the new brain answers on port"), "{out}");
    assert!(out.contains("7. Doctor"), "{out}");

    // Application Support.
    let mut want: BTreeSet<String> = ["hook-token", "legacy/", "legacy/migration.json", "legacy/migrate.lock"].into_iter().map(String::from).collect();
    want.extend(app_before.iter().cloned());
    if kind != Kind::HooksOnly {
        for f in ["config.json", "state.json", "shell-state.json", "installs.json", "backups/", "backups/settings-20261001-120000.json", "pets/", "pets/esheep64/", "pets/esheep64/animations.xml", "pets.user.lock.json"] {
            want.insert(f.into());
        }
        for l in w.labels {
            want.insert(format!("legacy/{l}.plist"));
        }
    }
    let got = tree(&w.app);
    // The new brain's own files (its log, and the state it saves) come and go with it.
    let brain_own = |f: &String| f.starts_with("logs") || (kind == Kind::HooksOnly && f == "state.json");
    let backups_new: Vec<&String> = got.iter().filter(|f| f.starts_with("backups/settings-") && !want.contains(*f)).collect();
    if kind == Kind::Pasture {
        assert_eq!(backups_new.len(), 1, "one backup of the settings file before its rewrite: {got:?}");
    } else {
        assert!(backups_new.is_empty(), "{got:?}");
    }
    let rest: BTreeSet<String> = got.iter().filter(|f| !brain_own(f) && !backups_new.contains(f) && !(kind == Kind::Pasture && *f == "backups/")).cloned().collect();
    let want: BTreeSet<String> = want.into_iter().filter(|f| !(kind == Kind::Pasture && f == "backups/")).collect();
    assert_eq!(rest, want, "{kind:?}: Application Support");
    assert_eq!(std::fs::read_to_string(w.app.join("hook-token")).unwrap(), format!("{TOKEN}\n"));
    assert_eq!(mode(&w.app.join("hook-token")), 0o600);
    if kind != Kind::HooksOnly {
        assert_eq!(std::fs::read_to_string(w.app.join("config.json")).unwrap(), format!("{{\n  \"port\": {},\n  \"brain\": {{}},\n  \"pets\": {{\n    \"default\": \"esheep64\"\n  }}\n}}\n", w.port));
        let st: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.app.join("state.json")).unwrap()).unwrap();
        assert_eq!(st["muted"], true);
        assert_eq!(st["identities"]["herdr:w1:p1"]["colourIndex"], 2);
        assert_eq!(st["identities"]["herdr:w1:p1"]["slots"]["main"], 3);
        assert_eq!(st["identities"]["herdr:w1:p2"]["colourIndex"], 5);
        assert_eq!(std::fs::read_to_string(w.app.join("shell-state.json")).unwrap(), "{\n  \"version\": 1,\n  \"hidden\": false,\n  \"showLabels\": true\n}", "the strip's own layout; Show labels kept");
        let installs = |p: &Path| -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
        let (old, new) = (installs(&w.repo.join("data/installs.json")), installs(&w.app.join("installs.json")));
        let key = w.settings.display().to_string();
        assert_eq!(new[&key], old[&key], "the old record's realpath key and value are copied unchanged");
        if kind == Kind::Renamed {
            assert_eq!(std::fs::read(w.app.join("installs.json")).unwrap(), std::fs::read(w.repo.join("data/installs.json")).unwrap());
        } else {
            // Step 5's install adds its own record, under the settings file's realpath.
            let real = std::fs::canonicalize(&w.settings).unwrap().display().to_string();
            assert_eq!(new[&real]["events"], serde_json::json!([]), "{new}");
        }
        assert_eq!(std::fs::read(w.app.join("backups/settings-20261001-120000.json")).unwrap(), b"{\"old\": true}\n");
        assert_eq!(mode(&w.app.join("backups/settings-20261001-120000.json")), 0o600);
        assert_eq!(std::fs::read_to_string(w.app.join("pets/esheep64/animations.xml")).unwrap(), PET);
        for (p, bytes) in &original_plists {
            assert_eq!(&std::fs::read(w.app.join("legacy").join(p.file_name().unwrap())).unwrap(), bytes);
        }
        // The repository is never changed.
        assert!(w.repo.join(".env").exists() && w.repo.join("data/state.json").exists());
    }
    // No legacy plists in ~/Library/LaunchAgents; the user's other agent untouched.
    for l in ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.brain", "local.pasture.shell"] {
        assert!(!w.agents.join(format!("{l}.plist")).exists(), "{l}");
    }
    if kind != Kind::HooksOnly {
        assert!(w.agents.join("com.example.other.plist").exists());
    }
    // The settings bytes.
    let after = std::fs::read(&w.settings).unwrap();
    if kind == Kind::Pasture {
        assert_eq!(after, expected_settings);
        assert!(!String::from_utf8_lossy(&after).contains("X-Pasture-"));
    } else {
        assert_eq!(after, original_settings, "the renamed install's hooks are already current");
    }

    // A second run: nothing to migrate.
    for args in [&["migrate", "--to-app"][..], &["migrate", "--to-app", "--write"][..]] {
        let (code, out, err) = w.run(args);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.starts_with("Nothing to migrate"), "{args:?}: {out}");
    }
    assert_eq!(w.log("login.log"), ["register"], "the second run registers nothing");

    // --undo: a preview, then the original settings bytes and plists back.
    let (code, out, err) = w.run(&["migrate", "--undo"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("- unregister the brain agent and the app at login"), "{out}");
    assert_eq!(after, std::fs::read(&w.settings).unwrap(), "a preview changes nothing");
    let (code, out, err) = w.run(&["migrate", "--undo", "--write"]);
    if show {
        eprintln!("==== {kind:?}: migrate --undo --write\n{out}{err}");
    }
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(w.log("login.log"), ["register", "unregister"]);
    let mut lc = booted.clone();
    lc.extend(original_plists.iter().map(|(p, _)| format!("bootstrap gui/{uid} {}", p.display())));
    assert_eq!(w.log("launchctl.log"), lc, "the agents that were loaded are started again");
    assert_eq!(std::fs::read(&w.settings).unwrap(), original_settings, "{kind:?}: the original settings bytes");
    for (p, bytes) in &original_plists {
        assert_eq!(&std::fs::read(p).unwrap(), bytes, "{}", p.display());
    }
    assert!(health(w.port).is_none(), "the app's brain stopped with its login item");
    assert!(!w.app.join("legacy/migration.json").exists());
    let (_, out, _) = w.run(&["migrate", "--undo"]);
    assert!(out.starts_with("Nothing to undo"), "{out}");
    // And the old install can be migrated again (its plists are back).
    if kind != Kind::HooksOnly {
        let (code, out, _) = w.run(&["migrate", "--to-app"]);
        assert_eq!(code, 0);
        assert!(out.contains("move "), "{out}");
    }
    drop(w);
    let _ = std::fs::remove_dir_all(root.join(format!("{kind:?}").to_lowercase()));
    // The shared root goes with the last fixture (the others may still be running).
    let _ = std::fs::remove_dir(&root);
}

#[test]
fn a_renamed_repository_install_moves_into_the_app_and_back() {
    check_fixture(Kind::Renamed);
}

#[test]
fn a_pasture_install_moves_into_the_app_and_back() {
    check_fixture(Kind::Pasture);
}

#[test]
fn hook_entries_alone_move_into_the_app_and_back() {
    check_fixture(Kind::HooksOnly);
}

/// The forced failure of the Done-when list: a dummy listener holds the port. The run stops
/// before anything changes, says what holds the port, and the old setup keeps running.
#[test]
fn a_port_held_by_something_else_leaves_the_old_setup_running() {
    let root = std::env::temp_dir().join(format!("bw-m8-held-{}", std::process::id()));
    let mut w = world(&root, Kind::Renamed);
    // "launchd" has the old agents loaded but the old brain isn't running; a dummy holds the port.
    let listener = std::net::TcpListener::bind(("127.0.0.1", w.port)).unwrap();
    let mut strip = Command::new("/bin/sleep");
    strip.arg("600");
    let spid = w.procs.start(strip, "sleep 600");
    std::fs::write(w.state.join("loaded").join(w.labels[0]), "").unwrap();
    std::fs::write(w.state.join("loaded").join(w.labels[1]), spid.to_string()).unwrap();
    let settings = std::fs::read(&w.settings).unwrap();
    let plists: Vec<Vec<u8>> = w.labels.iter().map(|l| std::fs::read(w.agents.join(format!("{l}.plist"))).unwrap()).collect();
    // The dummy accepts and never answers: /health times out, lsof names this process.
    let accept = std::thread::spawn(move || {
        let mut held = Vec::new();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok((c, _)) = listener.accept() {
                held.push(c);
            }
            std::thread::sleep(Duration::from_millis(20));
            if std::fs::metadata(std::env::temp_dir().join(format!("bw-m8-held-{}-done", std::process::id()))).is_ok() {
                break;
            }
        }
    });
    let (preview_code, preview, _) = w.run(&["migrate", "--to-app"]);
    assert_eq!(preview_code, 0);
    let me = std::process::id();
    assert!(preview.contains(&format!("STOP: port {} is held by pid {me}", w.port)), "{preview}");
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    if std::env::var_os("BW_M8_SHOW").is_some() {
        eprintln!("==== held port: migrate --to-app --write (exit {code})\n{out}{err}");
    }
    let _ = std::fs::write(std::env::temp_dir().join(format!("bw-m8-held-{}-done", std::process::id())), "");
    accept.join().unwrap();
    let _ = std::fs::remove_file(std::env::temp_dir().join(format!("bw-m8-held-{}-done", std::process::id())));
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains(&format!("port {} is held by pid {me} (", w.port)) && err.contains("which isn't a bellwether or pasture brain"), "{err}");
    assert!(err.contains("Nothing was changed: the old setup keeps running"), "{err}");
    assert!(w.log("launchctl.log").is_empty(), "nothing was booted out: {:?}", w.log("launchctl.log"));
    assert!(w.log("login.log").is_empty());
    assert!(w.procs.alive(spid), "the old strip still runs");
    assert!(w.state.join("loaded").join(w.labels[0]).exists() && w.state.join("loaded").join(w.labels[1]).exists());
    for (l, bytes) in w.labels.iter().zip(&plists) {
        assert_eq!(&std::fs::read(w.agents.join(format!("{l}.plist"))).unwrap(), bytes);
    }
    assert_eq!(std::fs::read(&w.settings).unwrap(), settings);
    assert!(!w.app.exists(), "nothing was written into the app's folder");
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}

/// The swap undoes itself when the new brain never answers (here: the login items register
/// but start nothing), and the old agents come back.
#[test]
fn a_new_brain_that_never_comes_up_puts_the_old_agents_back() {
    let root = std::env::temp_dir().join(format!("bw-m8-nobrain-{}", std::process::id()));
    let mut w = world(&root, Kind::Pasture);
    w.start_old_agents();
    std::fs::write(w.state.join("no-brain"), "").unwrap();
    let settings = std::fs::read(&w.settings).unwrap();
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("didn't work") && err.contains("after 4 s") && err.contains("back as they were"), "{err}");
    let uid = w.uid();
    let mut want: Vec<String> = w.labels.iter().map(|l| format!("bootout gui/{uid}/{l}")).collect();
    want.extend(w.labels.iter().map(|l| format!("bootstrap gui/{uid} {}", w.agents.join(format!("{l}.plist")).display())));
    assert_eq!(w.log("launchctl.log"), want);
    assert_eq!(w.log("login.log"), ["register", "unregister"]);
    for l in w.labels {
        assert!(w.agents.join(format!("{l}.plist")).exists());
        assert!(w.state.join("loaded").join(l).exists(), "{l} is loaded again");
    }
    assert_eq!(std::fs::read(&w.settings).unwrap(), settings, "the hooks were not touched");
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}

/// POSTs a hook with `token`, as a session started before the migration does; the status.
fn hook_with(port: u16, token: &str) -> u16 {
    assert_ne!(port, LIVE_PORT);
    bellwether_brain::client::request(port, "POST", "/hook", &[("Content-Type", "application/json"), ("X-Bellwether-Token", token)], Some(b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s-before\"}"), Duration::from_secs(2)).map(|r| r.status).unwrap_or(0)
}

/// BR-3K's M8 D3: the app already has a hook token of its own (it was run before), different
/// from the one the old install's sessions carry. The migration replaces it with theirs, says
/// so in the preview, keeps the app's in legacy/, and a running session's hook still gets 204.
#[test]
fn a_different_app_token_is_replaced_so_running_sessions_keep_working() {
    let root = std::env::temp_dir().join(format!("bw-m8-tokdiff-{}", std::process::id()));
    let mut w = world(&root, Kind::Renamed);
    w.start_old_agents();
    const APP_TOKEN: &str = "appTokenDifferent_0123456789abcdefghij";
    std::fs::create_dir_all(&w.app).unwrap();
    std::fs::set_permissions(&w.app, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(w.app.join("hook-token"), format!("{APP_TOKEN}\n")).unwrap();
    std::fs::set_permissions(w.app.join("hook-token"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let (code, out, err) = w.run(&["migrate", "--to-app"]);
    assert_eq!(code, 0, "{out}{err}");
    let first = actions(&out)[0].clone();
    assert!(first.starts_with(&format!("replace the app's own hook token ({}/hook-token) with the token from {}/.env: running sessions carry that one", w.app.display(), w.repo.display())), "{first}");
    assert!(!out.contains(TOKEN) && !out.contains(APP_TOKEN), "neither token is printed");
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("replaced the app's own token with the token from"), "{out}");
    assert_eq!(std::fs::read_to_string(w.app.join("hook-token")).unwrap(), format!("{TOKEN}\n"));
    assert_eq!(std::fs::read_to_string(w.app.join("legacy/hook-token.before-migration")).unwrap(), format!("{APP_TOKEN}\n"));
    assert_eq!(mode(&w.app.join("legacy/hook-token.before-migration")), 0o600);
    assert_eq!(hook_with(w.port, TOKEN), 204, "a session started before the migration keeps working");
    assert_eq!(hook_with(w.port, APP_TOKEN), 403);
    let settings = std::fs::read_to_string(&w.settings).unwrap();
    assert!(settings.contains(TOKEN) && !settings.contains(APP_TOKEN), "the hook entries keep the sessions' token");
    let (_, out, _) = w.run(&["migrate", "--to-app"]);
    assert!(out.starts_with("Nothing to migrate"), "{out}");
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}

/// BR-3K's M8 D1: the run is killed (SIGKILL) during step 4, after the old agents were booted
/// out and their plists moved, while it registers the app's login items. Nothing runs then;
/// the leftover swap marker doesn't block the app's own brain; a rerun says the migration is
/// unfinished (never "Nothing to migrate"), finishes it, and the old token works again.
#[test]
fn a_run_killed_during_step_4_is_finished_by_the_next_run() {
    let root = std::env::temp_dir().join(format!("bw-m8-killed-{}", std::process::id()));
    let mut w = world(&root, Kind::Renamed);
    w.start_old_agents();
    std::fs::write(w.state.join("kill-parent"), "").unwrap();
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    assert_eq!(code, -1, "killed by a signal: {out}{err}");
    assert!(health(w.port).is_none(), "nothing answers: the old brain was booted out");
    for l in w.labels {
        assert!(w.app.join("legacy").join(format!("{l}.plist")).exists(), "{l} moved before the kill");
    }
    assert!(w.app.join("legacy/swapping").exists(), "the marker was left behind");
    assert!(!bellwether_brain::toapp::swapping(&w.app), "but its run is dead, so the app may start a brain");
    assert!(w.app.join("legacy/migrate.lock").exists(), "and the lock file (its flock went with the run)");
    // The rerun: unfinished, not "Nothing to migrate".
    let (code, out, err) = w.run(&["migrate", "--to-app"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!out.starts_with("Nothing to migrate"), "{out}");
    assert!(out.contains("didn't finish: it stopped during step 4") && out.contains("migrate --undo --write"), "{out}");
    assert!(actions(&out).contains(&"register the brain agent and the app at login".to_string()), "{out}");
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    assert_eq!(code, 0, "the rerun finishes it (the dead run's lock is taken over): {out}{err}");
    assert!(out.contains("the new brain answers on port"), "{out}");
    assert_eq!(w.log("login.log"), ["register", "register"]);
    assert_eq!(hook_with(w.port, TOKEN), 204);
    assert!(!w.app.join("legacy/swapping").exists());
    let (_, out, _) = w.run(&["migrate", "--to-app"]);
    assert!(out.starts_with("Nothing to migrate"), "finished now: {out}");
    // A finished migration whose login items are gone since is unfinished again.
    std::fs::remove_file(w.state.join("registered")).unwrap();
    let (_, out, _) = w.run(&["migrate", "--to-app"]);
    assert!(out.contains("didn't finish: the app's login items aren't registered"), "{out}");
    std::fs::write(w.state.join("registered"), "").unwrap();
    // And the undo still puts everything back.
    let (code, out, err) = w.run(&["migrate", "--undo", "--write"]);
    assert_eq!(code, 0, "{out}{err}");
    for l in w.labels {
        assert!(w.agents.join(format!("{l}.plist")).exists());
    }
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}

/// BR-3K's M8 D2: an undo that can't put a plist back (LaunchAgents not writable) changes
/// nothing: the login items stay registered, the record stays, exit non-zero, and a rerun
/// after the fix succeeds.
#[test]
fn an_undo_that_cannot_put_the_plists_back_changes_nothing_and_can_run_again() {
    let root = std::env::temp_dir().join(format!("bw-m8-undo-{}", std::process::id()));
    let mut w = world(&root, Kind::Renamed);
    w.start_old_agents();
    let original: Vec<Vec<u8>> = w.labels.iter().map(|l| std::fs::read(w.agents.join(format!("{l}.plist"))).unwrap()).collect();
    let (code, out, err) = w.run(&["migrate", "--to-app", "--write"]);
    assert_eq!(code, 0, "{out}{err}");
    std::fs::set_permissions(&w.agents, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (code, out, err) = w.run(&["migrate", "--undo", "--write"]);
    std::fs::set_permissions(&w.agents, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(code, 0, "{out}{err}");
    assert!(err.contains("can't be written") && err.contains("Nothing was changed") && err.contains("record is kept"), "{err}");
    assert!(!out.contains("Done."), "{out}");
    assert_eq!(w.log("login.log"), ["register"], "the login items were not unregistered");
    assert!(w.app.join("legacy/migration.json").exists(), "the record is kept");
    assert_eq!(hook_with(w.port, TOKEN), 204, "the app's brain still runs");
    for l in w.labels {
        assert!(w.app.join("legacy").join(format!("{l}.plist")).exists() && !w.agents.join(format!("{l}.plist")).exists());
    }
    // Fixed: the same command now puts everything back.
    let (code, out, err) = w.run(&["migrate", "--undo", "--write"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Done."), "{out}");
    for (l, bytes) in w.labels.iter().zip(&original) {
        assert_eq!(&std::fs::read(w.agents.join(format!("{l}.plist"))).unwrap(), bytes);
    }
    assert_eq!(w.log("login.log"), ["register", "unregister"]);
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}
