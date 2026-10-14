//! The command line of the Rust binary (distribution plan, milestone 1). It runs when the
//! executable is called as `bellwether` (the lower-case symlink on PATH) or with a
//! subcommand. Milestone 1 has what the skeleton needs; the rest of today's Node CLI is
//! ported in milestones 3 and 4, with the same names, flags, output and exit codes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::login_items::{self, Service, Status};
use bellwether_brain::exit;

const USAGE: &str = "usage (the app's built-in CLI; milestones 1 and 3 of the distribution plan):
  bellwether serve [--port <n>] [--hang | --slow=<ms>]   run the brain in the foreground
  bellwether record [--out <file>]                       serve, and record event shapes (no content) to recordings/
  bellwether status [--watch] [--json]                   the flock as text (long-polls /state with --watch)
  bellwether mute | unmute                               do-not-disturb (the tray's Mute; kept across restarts)
  bellwether show | hide                                 show or hide the strip (the tray's Hide; kept across restarts)
  bellwether replay <file.jsonl> [--speed N] [--port P] [--tag <pane>]
  bellwether replay --burst <events/s> [--seconds S] [--port P]
  bellwether token init [--from <.env>] [--force]        make the hook token file, only if there is none (never printed);
                                                         --from takes the token of a repository install's .env;
                                                         --force replaces one that is there but unusable
  bellwether token rotate [--write] [--settings <path>]  a new token, and the running brain told to switch (dry run unless --write)
  bellwether doctor [--port <n>]                         check the app, its brain, token and login items
  bellwether autostart status                            the two login items (brain agent, app at login)
  bellwether autostart install [--write] [--brain-only] [--via-app]
  bellwether autostart uninstall [--write] [--brain-only] [--via-app]
                                                         dry run unless --write; --via-app hands the call
                                                         to Bellwether.app instead of making it here
  bellwether version
  bellwether help

The app's files: ~/Library/Application Support/Bellwether/ (BELLWETHER_HOME overrides it).
";

/// `args` is the whole argv. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let Some(command) = rest.first().map(String::as_str) else {
        print!("{USAGE}");
        return exit::OK;
    };
    match command_result(command, &rest[1..]) {
        Ok(code) => code,
        Err((message, code)) => {
            eprintln!("bellwether: {message}");
            code
        }
    }
}

/// One command's outcome: its exit code, or a message for stderr and the exit code.
fn command_result(command: &str, tail: &[String]) -> CliResult {
    match command {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(exit::OK)
        }
        "version" | "--version" => {
            println!("bellwether {} (rust)", bellwether_brain::VERSION);
            Ok(exit::OK)
        }
        "serve" | "record" | "status" | "mute" | "unmute" | "show" | "hide" | "replay" | "token" => Ok(bellwether_brain::cli::run(command, tail)),
        "doctor" => doctor(tail),
        "autostart" => autostart(tail),
        // The conformance harness's hidden commands: only in a build with the
        // `conformance` feature (never a release). Without it they are unknown commands.
        #[cfg(feature = "conformance")]
        hidden if hidden.starts_with("__") => Ok(crate::conformance::run(hidden, tail)),
        _ => Err((format!("unknown command {command:?}.\n\n{USAGE}"), exit::UNEXPECTED)),
    }
}

type CliResult = Result<i32, (String, i32)>;

fn bad(message: impl Into<String>) -> (String, i32) {
    (message.into(), exit::BAD_CONFIG)
}

/// Splits flags from words; refuses anything not in `booleans` or `values`.
fn flags(args: &[String], booleans: &[&str], values: &[&str]) -> Result<(Vec<String>, Vec<(String, String)>), (String, i32)> {
    let (mut words, mut set) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(name) = a.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (name, None),
            };
            if booleans.contains(&name) && inline.is_none() {
                set.push((name.to_string(), String::new()));
            } else if values.contains(&name) {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).filter(|v| !v.starts_with("--")).cloned().ok_or_else(|| bad(format!("--{name} needs a value")))?
                    }
                };
                set.push((name.to_string(), v));
            } else {
                return Err(bad(format!("unknown option {a:?}")));
            }
        } else {
            words.push(a.clone());
        }
        i += 1;
    }
    Ok((words, set))
}

fn has(set: &[(String, String)], name: &str) -> bool {
    set.iter().any(|(k, _)| k == name)
}

fn value<'a>(set: &'a [(String, String)], name: &str) -> Option<&'a str> {
    set.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

// --- autostart -----------------------------------------------------------------------

fn autostart(args: &[String]) -> CliResult {
    let (words, set) = flags(args, &["write", "brain-only", "via-app"], &[])?;
    // Through the symlink on PATH, SMAppService can't see the app (it goes by
    // NSBundle.mainBundle): run the same command again from the bundle's executable.
    #[cfg(target_os = "macos")]
    if !has(&set, "via-app") && !crate::identity::started_from_bundle() {
        if let Some(bundle) = crate::identity::bundle_path() {
            let exe = bundle.join("Contents").join("MacOS").join("Bellwether");
            let status = std::process::Command::new(&exe).arg("autostart").args(args).status().map_err(|e| (format!("couldn't run {}: {e}", exe.display()), exit::UNEXPECTED))?;
            return Ok(status.code().unwrap_or(exit::UNEXPECTED));
        }
    }
    let sub = words.first().map(String::as_str).unwrap_or("status");
    if words.len() > 1 {
        return Err(bad(format!("unexpected argument {:?}", words[1])));
    }
    if !matches!(sub, "status" | "install" | "uninstall") {
        return Err(bad(format!("unknown autostart command {sub:?} (status, install, uninstall)")));
    }
    // From a terminal that predates the user's last login, SMAppService answers notFound
    // for everything and registrations can't be trusted (M1 finding F9).
    if crate::identity::session() == crate::identity::Session::Stale {
        return Err((login_items::STALE_SESSION.to_string(), exit::NOT_RUNNING));
    }
    let services: Vec<Service> = if has(&set, "brain-only") { vec![Service::Brain] } else { vec![Service::Brain, Service::MainApp] };
    if sub == "install" {
        // All of them before any is registered; register() checks each again.
        login_items::preflight_here(&services, true).map_err(|m| (m, exit::BAD_CONFIG))?;
    }
    match sub {
        "status" => {
            if has(&set, "write") {
                return Err(bad("--write is for install and uninstall"));
            }
            if has(&set, "via-app") {
                return via_app("login-items-status");
            }
            for s in [Service::Brain, Service::MainApp] {
                println!("{}", login_items::advice(s, login_items::status(s)));
            }
            Ok(exit::OK)
        }
        "install" | "uninstall" => {
            let on = sub == "install";
            if !has(&set, "write") {
                for s in &services {
                    println!("would {} the {} (now {}); add --write to do it", if on { "register" } else { "unregister" }, s.name(), login_items::status(*s).word());
                }
                return Ok(exit::OK);
            }
            let mut code = exit::OK;
            for s in &services {
                if has(&set, "via-app") {
                    let action = match (s, on) {
                        (Service::Brain, true) => "brain-agent-register",
                        (Service::Brain, false) => "brain-agent-unregister",
                        (Service::MainApp, true) => "main-app-register",
                        (Service::MainApp, false) => "main-app-unregister",
                    };
                    if via_app(action)? != exit::OK {
                        code = exit::UNEXPECTED;
                    }
                    continue;
                }
                let r = if on { login_items::register(*s) } else { login_items::unregister(*s) };
                match r {
                    Ok(st) => {
                        println!("{}", login_items::advice(*s, st));
                        if st == Status::RequiresApproval {
                            login_items::open_settings();
                        }
                    }
                    Err(e) => {
                        eprintln!("bellwether: {}: {e}", s.name());
                        code = exit::UNEXPECTED;
                    }
                }
            }
            // Unregistering doesn't quit what is already running: stop this bundle's own
            // strip (never another copy, never the repository's bellwether-shell), and say
            // what is still running.
            if !on && !has(&set, "via-app") {
                stop_own_instances(services.contains(&Service::MainApp));
            }
            Ok(code)
        }
        _ => Err(bad(format!("unknown autostart command {sub:?} (status, install, uninstall)"))),
    }
}

/// Whether a command line is the app (the strip), not `serve` or a CLI command.
pub fn is_app_mode(args: &str) -> bool {
    let words: Vec<String> = args.split_whitespace().map(String::from).collect();
    crate::dispatch::mode_of(&words) == crate::dispatch::Mode::App && !words.iter().any(|w| w == "--action")
}

/// After `uninstall --write`: SIGTERM to this bundle's running strip (when the app item
/// was removed), then report anything of this bundle still running. Only processes whose
/// executable is this exact bundle's, never this process or the ones that started it.
fn stop_own_instances(app_removed: bool) {
    let me = std::process::id() as i32;
    let parent = unsafe { libc::getppid() };
    let procs: Vec<(i32, i32, String)> = crate::identity::bundle_processes().into_iter().filter(|(pid, _, _)| *pid != me && *pid != parent).collect();
    let strips: Vec<i32> = procs.iter().filter(|(_, _, a)| is_app_mode(a)).map(|(p, _, _)| *p).collect();
    if app_removed {
        for pid in &strips {
            unsafe { libc::kill(*pid, libc::SIGTERM) };
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && strips.iter().any(|p| unsafe { libc::kill(*p, 0) } == 0) {
            std::thread::sleep(Duration::from_millis(100));
        }
        for pid in &strips {
            if unsafe { libc::kill(*pid, 0) } == 0 {
                println!("the strip (pid {pid}) didn't quit within 5 s; quit it from the menu-bar sheep, or `kill {pid}`");
            } else {
                println!("stopped the running strip (pid {pid})");
            }
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    for (pid, ppid, args) in crate::identity::bundle_processes().into_iter().filter(|(pid, _, _)| *pid != me && *pid != parent) {
        if !app_removed && is_app_mode(&args) {
            println!("still running: the strip (pid {pid}); `autostart uninstall --write` without --brain-only stops it");
        } else if !is_app_mode(&args) && !args.contains("--action") {
            println!("still running: pid {pid} ({args}, parent {ppid}); it wasn't started by the login item, so it was left alone");
        }
    }
}

/// Hands one action to the app through LaunchServices and waits up to 30 s for its
/// outcome file (distribution plan D4).
fn via_app(action: &str) -> CliResult {
    let bundle = crate::identity::bundle_path().ok_or_else(|| ("not part of Bellwether.app: --via-app needs the app bundle".to_string(), exit::UNEXPECTED))?;
    let home = bellwether_brain::home::dir();
    let outcome = home.join("actions").join(format!("{action}.json"));
    let _ = std::fs::remove_file(&outcome);
    let mut cmd = std::process::Command::new("/usr/bin/open");
    cmd.args(["-n", "-g", "-a"]).arg(&bundle);
    if let Some(h) = std::env::var_os("BELLWETHER_HOME") {
        let mut kv = std::ffi::OsString::from("BELLWETHER_HOME=");
        kv.push(h);
        cmd.arg("--env").arg(kv);
    }
    cmd.args(["--args", "--action", action]);
    let status = cmd.status().map_err(|e| (format!("couldn't run open: {e}"), exit::UNEXPECTED))?;
    if !status.success() {
        return Err((format!("`open` couldn't launch {}", bundle.display()), exit::UNEXPECTED));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&outcome) {
            let _ = std::fs::remove_file(&outcome);
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            println!("{} (by the app)", v["message"].as_str().unwrap_or("?"));
            return Ok(if v["ok"].as_bool() == Some(true) { exit::OK } else { exit::UNEXPECTED });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err((format!("the app didn't report back within 30 s (expected {})", outcome.display()), exit::UNEXPECTED))
}

// --- doctor --------------------------------------------------------------------------

struct Check {
    status: &'static str,
    name: &'static str,
    detail: String,
    code: i32,
}

fn doctor(args: &[String]) -> CliResult {
    let (words, set) = flags(args, &[], &["port"])?;
    if let Some(w) = words.first() {
        return Err(bad(format!("unexpected argument {w:?}")));
    }
    let session = crate::identity::session();
    let mut checks: Vec<Check> = Vec::new();
    let mut add = |status: &'static str, name: &'static str, detail: String, code: i32| checks.push(Check { status, name, detail, code: if status == "FAIL" { code } else { exit::OK } });

    // Which app this is.
    match crate::identity::bundle_path() {
        Some(bundle) => {
            let id = crate::identity::bundle_id().unwrap_or_else(|| "(no CFBundleIdentifier)".into());
            add("PASS", "identity", format!("{id} {} from {}", bellwether_brain::VERSION, bundle.display()), exit::OK);
            let (status, detail) = signature(&bundle, session);
            add(status, "signature", detail, exit::UNEXPECTED);
        }
        None => add("INFO", "identity", format!("a development binary ({}), not Bellwether.app: no login items, and macOS permissions belong to this exact file", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()), exit::OK),
    }

    // The app's files and the token.
    let home = bellwether_brain::home::dir();
    add("INFO", "app data", home.display().to_string(), exit::OK);
    let token = match bellwether_brain::home::token(&home).and_then(|t| bellwether_brain::check_token(&t).map(|_| t)) {
        Ok(t) => {
            let from_env = std::env::var("BELLWETHER_HOOK_TOKEN").ok().filter(|t| !t.is_empty()).is_some();
            if from_env {
                add("PASS", "token", "from BELLWETHER_HOOK_TOKEN in the environment".into(), exit::OK);
            } else {
                token_file_mode(&bellwether_brain::home::token_file(&home), &mut add);
            }
            Some(t)
        }
        Err(e) => {
            add("FAIL", "token", e.message, exit::BAD_CONFIG);
            None
        }
    };

    // The brain.
    let port = match value(&set, "port") {
        Some(v) => v.parse::<u16>().ok().filter(|p| *p >= 1024).ok_or_else(|| bad("--port takes an integer from 1024 to 65535"))?,
        None => match bellwether_brain::home::configured_port(&home) {
            Ok(p) => p.unwrap_or(bellwether_brain::DEFAULT_PORT),
            Err(e) => {
                add("FAIL", "config", e.message, exit::BAD_CONFIG);
                bellwether_brain::DEFAULT_PORT
            }
        },
    };
    let brain_pid = brain_checks(port, token.as_deref(), &mut add);

    // Login items. The brain answering on the port is this app's agent when launchd (pid
    // 1) runs this very executable for it.
    if crate::identity::in_app_bundle() {
        let ours = brain_pid.and_then(|pid| crate::identity::bundle_processes().into_iter().find(|(p, _, _)| *p as u64 == pid)).filter(|(_, ppid, _)| *ppid == 1).map(|(p, _, _)| p);
        for s in [Service::Brain, Service::MainApp] {
            let (status, detail) = login_verdict(s, login_items::status(s), session, if s == Service::Brain { ours } else { None });
            add(status, if s == Service::Brain { "login: brain" } else { "login: app" }, detail, exit::OK);
        }
    }

    // LaunchAgents from the repository versions: two brains would fight over the port.
    let agents = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library").join("LaunchAgents");
    let legacy: Vec<String> = ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.brain", "local.pasture.shell"]
        .iter()
        .filter(|l| agents.join(format!("{l}.plist")).exists())
        .map(|l| l.to_string())
        .collect();
    if !legacy.is_empty() {
        add("WARN", "legacy agents", format!("{} in {}: the repository version's login items. Two brains on one port fight over it; the app's migration (milestone 8) moves them aside", legacy.join(", "), agents.display()), exit::OK);
    }

    // Accessibility, for this exact identity (unknown from an old session).
    if session == crate::identity::Session::Stale {
        add("INFO", "accessibility", "can't check from this terminal session (it predates your last login); open a new terminal window".into(), exit::OK);
    } else if crate::ax::trusted() {
        add("PASS", "accessibility", "granted to this app: sheep can stand on the Dock".into(), exit::OK);
    } else {
        add("INFO", "accessibility", "not granted to this app: the strip uses its full width. If you granted it before and it now says not granted, System Settings > Privacy & Security > Accessibility still lists the old entry: remove it with −, then grant again".into(), exit::OK);
    }

    // An enabled brain agent with no brain answering: launchd is refusing or failing it.
    let agent_on = checks.iter().any(|c| c.name == "login: brain" && c.status == "PASS");
    let brain_down = checks.iter().any(|c| c.name == "brain" && c.status == "FAIL");
    if agent_on && brain_down {
        let (status, detail) = agent_down_hint(agent_port(), port);
        checks.push(Check { status, name: "login: brain", detail, code: exit::OK });
    }
    for c in &checks {
        println!("  {:<4}  {:<22} {}", c.status, c.name, c.detail);
    }
    let fails: Vec<&Check> = checks.iter().filter(|c| c.status == "FAIL").collect();
    println!("{}", if fails.is_empty() { "all checks passed".to_string() } else { format!("{} check(s) failed", fails.len()) });
    Ok(if fails.iter().any(|c| c.code == exit::BAD_CONFIG) {
        exit::BAD_CONFIG
    } else if fails.iter().any(|c| c.code == exit::NOT_RUNNING) {
        exit::NOT_RUNNING
    } else if fails.is_empty() {
        exit::OK
    } else {
        exit::UNEXPECTED
    })
}

fn token_file_mode(file: &Path, add: &mut impl FnMut(&'static str, &'static str, String, i32)) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(file) {
            Ok(m) if m.permissions().mode() & 0o077 == 0 => add("PASS", "token", format!("{} (0600)", file.display()), exit::OK),
            Ok(m) => add("FAIL", "token", format!("{} is mode {:o}: other users can read it. Run `chmod 600 '{}'`", file.display(), m.permissions().mode() & 0o777, file.display()), exit::BAD_CONFIG),
            Err(e) => add("FAIL", "token", format!("{}: {e}", file.display()), exit::BAD_CONFIG),
        }
    }
    #[cfg(not(unix))]
    add("PASS", "token", file.display().to_string(), exit::OK);
}

/// /health, which brain answers it (Rust or Node, by its process), and whether /hook
/// accepts the token (doctor's ping, which the brain counts but never shows).
/// Returns the answering brain's pid, if one answered.
fn brain_checks(port: u16, token: Option<&str>, add: &mut impl FnMut(&'static str, &'static str, String, i32)) -> Option<u64> {
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_millis(1500))).http_status_as_error(false).build().new_agent();
    let base = format!("http://127.0.0.1:{port}");
    let health = agent.get(&format!("{base}/health")).call();
    let mut resp = match health {
        Ok(r) => r,
        Err(e) => {
            let why = e.to_string();
            let detail = if why.contains("onnection refused") || why.contains("Connection refused") {
                format!("nothing listening on 127.0.0.1:{port}. With start at login on, launchd starts it (`bellwether autostart status`); or run `bellwether serve`")
            } else {
                format!("127.0.0.1:{port} didn't answer /health ({why}): a hung process, or a brain started with --hang")
            };
            add("FAIL", "brain", detail, exit::NOT_RUNNING);
            return None;
        }
    };
    let body: serde_json::Value = resp.body_mut().read_to_string().ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null);
    if resp.status() != 200 || body["name"] != "bellwether" {
        add("FAIL", "brain", format!("port {port} is held by something that isn't a bellwether brain (it answered {}). Stop it, or use another port", resp.status()), exit::NOT_RUNNING);
        return None;
    }
    let pid = body["pid"].as_u64().unwrap_or(0);
    let which = process_path(pid).map(|p| if p.ends_with("/node") || p.ends_with("node") { format!("the Node brain ({p})") } else { format!("the Rust brain ({p})") }).unwrap_or_else(|| "an unknown process".into());
    if body["mode"] == "hang" {
        add("FAIL", "brain", format!("pid {pid} runs with --hang: /hook never answers, so every hook stalls for its full timeout"), exit::NOT_RUNNING);
        return Some(pid);
    }
    add("PASS", "brain", format!("{which}, version {}, pid {pid}, on 127.0.0.1:{port}", body["version"].as_str().unwrap_or("?")), exit::OK);
    let Some(token) = token else { return Some(pid) };
    let ping = agent
        .post(&format!("{base}/hook"))
        .header("Content-Type", "application/json")
        .header("X-Bellwether-Token", token)
        .send(r#"{"hook_event_name":"BellwetherDoctorPing"}"#);
    match ping.map(|r| r.status().as_u16()) {
        Ok(204) => add("PASS", "brain token", "/hook accepts the token (204, empty)".into(), exit::OK),
        Ok(403) => add("FAIL", "brain token", "the brain refused the token (403): it was started with a different one; restart it".into(), exit::BAD_CONFIG),
        Ok(s) => add("FAIL", "brain token", format!("/hook answered {s}; it must only ever answer 204 or 403"), exit::UNEXPECTED),
        Err(e) => add("FAIL", "brain token", format!("/hook didn't answer: {e}"), exit::NOT_RUNNING),
    }
    Some(pid)
}

/// The port the bundled brain agent listens on: its plist's `--port`, else `port` in
/// the config.json of its home (the plist's BELLWETHER_HOME, else the default), else
/// 47417.
fn agent_port() -> u16 {
    let (port, home) = login_items::bundle_agent_overrides();
    port.or_else(|| {
        let home = home.map(PathBuf::from).unwrap_or_else(|| {
            // The default home, whatever BELLWETHER_HOME this shell has.
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library").join("Application Support").join("Bellwether")
        });
        bellwether_brain::home::configured_port(&home).ok().flatten()
    })
    .unwrap_or(bellwether_brain::DEFAULT_PORT)
}

/// The doctor line when the brain agent is enabled but nothing answered on the port
/// doctor checked. Only when that is the agent's own port can launchd be at fault.
pub fn agent_down_hint(agent_port: u16, checked: u16) -> (&'static str, String) {
    if agent_port != checked {
        return ("INFO", format!("the brain agent listens on port {agent_port}, not {checked}: run `bellwether doctor --port {agent_port}` to check it"));
    }
    ("WARN", "the brain agent is enabled but no brain answers. If Console or ~/Library/Logs/DiagnosticReports shows \"Launch Constraint Violation\", the login-item record was made by a differently signed build: run `bellwether autostart uninstall --write`, then `install --write` again (after one refused launch macOS replaces the record)".into())
}

/// One login item's doctor line. From an old session, or when macOS says the brain
/// agent isn't registered while this app's agent is plainly running under launchd, the
/// status can't be trusted, and doctor says so instead of "never registered" (F9).
pub fn login_verdict(s: Service, st: Status, session: crate::identity::Session, our_agent_pid: Option<i32>) -> (&'static str, String) {
    if session == crate::identity::Session::Stale {
        return ("INFO", format!("{}: {}", s.name(), login_items::STALE_SESSION));
    }
    if let Some(pid) = our_agent_pid {
        if matches!(st, Status::NotFound | Status::NotRegistered | Status::Unavailable) {
            return ("WARN", format!("{}: macOS tells this terminal it's {}, yet this app's brain runs under launchd (pid {pid}). This terminal is probably from before your last login: open a new terminal window and run doctor again", s.name(), st.word()));
        }
    }
    let status = match st {
        Status::Enabled => "PASS",
        Status::NotRegistered | Status::NotFound => "INFO",
        Status::RequiresApproval | Status::Unavailable => "WARN",
    };
    (status, login_items::advice(s, st))
}

/// The executable of a process, from `ps` (read only).
fn process_path(pid: u64) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let out = std::process::Command::new("/bin/ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The code signature, from `codesign -dv` (which prints to stderr). The Team ID is
/// read but never printed.
fn signature(bundle: &Path, session: crate::identity::Session) -> (&'static str, String) {
    let out = match std::process::Command::new("/usr/bin/codesign").arg("-dv").arg("--verbose=2").arg(bundle).output() {
        Ok(o) => o,
        Err(e) => return ("WARN", format!("couldn't run codesign: {e}")),
    };
    signature_verdict(&String::from_utf8_lossy(&out.stderr), out.status.success(), session)
}

/// The doctor line for `codesign -dv --verbose=2` output (stderr).
pub fn signature_verdict(text: &str, ok: bool, session: crate::identity::Session) -> (&'static str, String) {
    if !ok {
        return ("WARN", format!("not signed ({})", text.lines().last().unwrap_or("").trim()));
    }
    let runtime = text.lines().any(|l| l.starts_with("CodeDirectory") && l.contains("(runtime)"));
    let rt = if runtime { ", hardened runtime" } else { ", WITHOUT the hardened runtime (notarization needs it)" };
    if text.contains("Signature=adhoc") {
        return ("WARN", "ad-hoc signed: macOS permissions are lost on every rebuild".into());
    }
    let raw = text.lines().find_map(|l| l.strip_prefix("Authority=")).unwrap_or("");
    let team_set = text.lines().any(|l| l.starts_with("TeamIdentifier=") && l != "TeamIdentifier=not set");
    // An old login session can't read certificate names: codesign prints
    // "Authority=(unavailable)". That isn't a signing problem.
    if raw == "(unavailable)" || raw.is_empty() {
        let why = if session == crate::identity::Session::Stale { "this terminal session predates your last login; open a new terminal window to see it" } else { "codesign couldn't read the certificate chain" };
        return ("INFO", format!("{}{rt}; the certificate name is unavailable ({why})", if team_set { "signed with a Team ID" } else { "signed" }));
    }
    let authority = mask_team(raw);
    let status = if authority.starts_with("Developer ID Application") && runtime { "PASS" } else { "WARN" };
    (status, format!("{authority}{rt}"))
}

/// "Developer ID Application: Name (ABCDE12345)" without the Team ID in brackets:
/// doctor's output gets pasted into issues and docs.
fn mask_team(authority: &str) -> String {
    match authority.rfind(" (") {
        Some(i) if authority.ends_with(')') && authority.len() - i == 13 && authority[i + 2..authority.len() - 1].bytes().all(|b| b.is_ascii_alphanumeric()) => authority[..i].to_string(),
        _ => authority.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::identity::Session;

    #[test]
    fn an_old_session_never_reads_as_never_registered() {
        for st in [Status::NotFound, Status::NotRegistered, Status::Enabled] {
            let (status, detail) = login_verdict(Service::Brain, st, Session::Stale, None);
            assert_eq!(status, "INFO");
            assert!(detail.contains("predates your last login") && !detail.contains("never had it registered"), "{detail}");
        }
        let (status, detail) = login_verdict(Service::Brain, Status::NotFound, Session::Current, Some(4242));
        assert_eq!(status, "WARN");
        assert!(detail.contains("pid 4242") && detail.contains("new terminal window") && !detail.contains("never"), "{detail}");
        assert_eq!(login_verdict(Service::Brain, Status::Enabled, Session::Current, Some(4242)).0, "PASS");
        let (status, detail) = login_verdict(Service::Brain, Status::NotFound, Session::Current, None);
        assert_eq!(status, "INFO");
        assert!(detail.contains("never had it registered"));
    }

    #[test]
    fn an_unreadable_certificate_is_not_a_signing_failure() {
        let stale = "Identifier=com.fanzootechnology.bellwether\nCodeDirectory v=20500 size=10635 flags=0x10000(runtime) hashes=321+7 location=embedded\nAuthority=(unavailable)\nTeamIdentifier=AB12CD34EF\n";
        let (status, detail) = signature_verdict(stale, true, Session::Stale);
        assert_eq!(status, "INFO");
        assert!(detail.starts_with("signed with a Team ID, hardened runtime") && detail.contains("predates your last login"), "{detail}");
        assert!(!detail.contains("AB12CD34EF"));
        let good = "CodeDirectory v=20500 flags=0x10000(runtime)\nAuthority=Developer ID Application: Some Co (AB12CD34EF)\nAuthority=Developer ID Certification Authority\nTeamIdentifier=AB12CD34EF\n";
        assert_eq!(signature_verdict(good, true, Session::Current), ("PASS", "Developer ID Application: Some Co, hardened runtime".to_string()));
        assert_eq!(signature_verdict("CodeDirectory flags=0x10002(adhoc,runtime)\nSignature=adhoc\n", true, Session::Current).0, "WARN");
    }

    #[test]
    fn a_different_port_is_not_a_launch_constraint_problem() {
        let (status, detail) = agent_down_hint(47418, 47992);
        assert_eq!(status, "INFO");
        assert!(detail.contains("listens on port 47418, not 47992") && detail.contains("--port 47418"), "{detail}");
        assert!(!detail.contains("Launch Constraint"));
        let (status, detail) = agent_down_hint(47418, 47418);
        assert_eq!(status, "WARN");
        assert!(detail.contains("Launch Constraint Violation"));
    }

    #[test]
    fn only_the_strip_counts_as_the_app() {
        assert!(is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether"));
        assert!(is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether --demo"));
        assert!(!is_app_mode("Bellwether serve --port 47418"));
        assert!(!is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether doctor"));
        assert!(!is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether --action login-items-status"));
    }

    #[test]
    fn the_team_id_is_never_printed() {
        assert_eq!(mask_team("Developer ID Application: Some Co, Inc. (AB12CD34EF)"), "Developer ID Application: Some Co, Inc.");
        assert_eq!(mask_team("Developer ID Certification Authority"), "Developer ID Certification Authority");
        assert_eq!(mask_team("Name (short)"), "Name (short)");
    }

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn flags_are_strict() {
        let (w, set) = flags(&s(&["install", "--write", "--brain-only"]), &["write", "brain-only"], &[]).unwrap();
        assert_eq!(w, ["install"]);
        assert!(has(&set, "write") && has(&set, "brain-only"));
        assert!(flags(&s(&["--nope"]), &["write"], &[]).is_err());
        assert!(flags(&s(&["--write=1"]), &["write"], &[]).is_err());
        assert!(flags(&s(&["--port"]), &[], &["port"]).is_err());
        assert_eq!(value(&flags(&s(&["--port=47418"]), &[], &["port"]).unwrap().1, "port"), Some("47418"));
    }

    /// The hidden commands of the conformance harness (distribution plan D2).
    const HIDDEN: [&str; 7] = ["__fixture", "__redact", "__normalise", "__json", "__diff", "__env", "__list"];

    /// A build without the `conformance` feature (every release build) doesn't know
    /// them: "unknown command", exit 1.
    #[test]
    #[cfg(not(feature = "conformance"))]
    fn without_the_conformance_feature_hidden_commands_are_unknown() {
        for c in HIDDEN {
            match command_result(c, &[]) {
                Err((message, code)) => {
                    assert_eq!(code, exit::UNEXPECTED, "{c}");
                    assert!(message.starts_with(&format!("unknown command {c:?}")), "{c}: {message}");
                }
                Ok(code) => panic!("{c} ran (exit {code}) in a build without the conformance feature"),
            }
        }
    }

    #[test]
    #[cfg(feature = "conformance")]
    fn with_the_conformance_feature_hidden_commands_are_known() {
        // `__list` reads nothing and prints the areas; an unknown `__` word is still refused.
        assert_eq!(command_result("__list", &[]), Ok(exit::OK));
        for c in HIDDEN.iter().filter(|c| **c != "__list") {
            assert!(crate::conformance::AREAS.iter().any(|(a, _)| c.strip_prefix("__") == Some(*a)), "{c} has no area");
        }
        assert_eq!(command_result("__nope", &[]), Ok(exit::UNEXPECTED));
        assert!(command_result("nope", &[]).is_err());
    }

    /// Nothing that makes a release build turns the feature on: not a default feature,
    /// not the Tauri configs' `build.features`, not the build script.
    #[test]
    fn release_builds_never_enable_the_conformance_feature() {
        let cargo = include_str!("../Cargo.toml");
        let features = cargo.split("[features]").nth(1).expect("a [features] table").split("\n[").next().unwrap();
        for line in features.lines().filter(|l| l.trim_start().starts_with("default")) {
            assert!(!line.contains("conformance"), "a default feature: {line}");
        }
        for conf in [include_str!("../tauri.conf.json"), include_str!("../tauri.dist.conf.json")] {
            let v: serde_json::Value = serde_json::from_str(conf).unwrap();
            assert!(v["build"].get("features").is_none(), "build.features in a Tauri config");
            assert!(!conf.contains("conformance"));
        }
        // The brain crate's own feature (test knobs in `serve`) is on only through the shell's.
        let brain = include_str!("../../../crates/brain/Cargo.toml");
        let brain_features = brain.split("[features]").nth(1).expect("the brain's [features]").split("\n[").next().unwrap();
        assert!(!brain_features.lines().any(|l| l.trim_start().starts_with("default")), "a default feature in crates/brain");
        let script = include_str!("../../../scripts/build-app.mjs");
        assert!(!script.contains("--features") && !script.contains("conformance"), "scripts/build-app.mjs passes a feature");
    }

    /// test/DIVERGENCES.md 13: the app's help is its own text (the Node CLI's lists `shell`,
    /// `hooks`, `pets`, `migrate`); it names every brain command milestone 3 ported.
    #[test]
    fn help_lists_the_brain_commands() {
        for c in ["serve [--port <n>]", "record [--out <file>]", "status [--watch] [--json]", "mute | unmute", "show | hide", "replay <file.jsonl>", "replay --burst", "token init", "token rotate", "version"] {
            assert!(USAGE.contains(&format!("bellwether {c}")), "{c}");
        }
    }

    #[test]
    fn unknown_commands_exit_1_and_bad_flags_exit_3() {
        // A hidden command that isn't ported yet (milestone 4): "unknown command" without the
        // conformance feature, "not ported yet" with it; exit 1 either way.
        assert_eq!(run(&s(&["bellwether", "__json"])), exit::UNEXPECTED);
        assert_eq!(run(&s(&["bellwether", "autostart", "install", "--nope"])), exit::BAD_CONFIG);
        assert_eq!(run(&s(&["bellwether", "autostart", "sideways"])), exit::BAD_CONFIG);
        assert_eq!(run(&s(&["bellwether", "version"])), exit::OK);
    }
}
