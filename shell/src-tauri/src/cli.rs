//! The command line of the Rust binary (distribution plan, milestone 1). It runs when the
//! executable is called as `bellwether` (the lower-case symlink on PATH) or with a
//! subcommand. Milestone 1 has what the skeleton needs; the rest of today's Node CLI is
//! ported in milestones 3 and 4, with the same names, flags, output and exit codes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::login_items::{self, Service, Status};
use bellwether_brain::exit;

const USAGE: &str = "usage (the app's built-in CLI; milestones 1, 3, 4 and 5 of the distribution plan):
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
  bellwether token rotate [--write] [--settings <path>]  a new token: the brain told to switch, then the hook entries
                                                         rewritten with it (dry run unless --write)
  bellwether hooks print [--out <file> [--force]]        the hook block, as a settings file for claude --settings
  bellwether hooks install   [--write] [--settings <path>]   dry run unless --write
  bellwether hooks uninstall [--write] [--settings <path>] [--port <old port>]   dry run unless --write
  bellwether doctor [--json] [--e2e] [--settings <path>] [--port <n>]
                                                         check the app, its brain, token, hooks and login items
  bellwether pets import <animations.xml> [--name <name>] [--force]
                                                         import a desktopPet pet file of your own (checked, then pinned)
  bellwether migrate [--from <dir>] [--env <file>] [--settings <path>] [--force] [--write]
                                                         move a repository install's token, config, colours and hooks
                                                         into the app's files (preview unless --write)
  bellwether shell start | stop | restart                Bellwether.app's strip (stop leaves a launchd brain running)
  bellwether autostart status                            the two login items (brain agent, app at login)
  bellwether autostart install [--write] [--brain-only] [--via-app]
  bellwether autostart uninstall [--write] [--brain-only] [--via-app]
                                                         dry run unless --write; --via-app hands the call
                                                         to Bellwether.app instead of making it here
  bellwether version
  bellwether help

The app's files: ~/Library/Application Support/Bellwether/ (BELLWETHER_HOME overrides it).
Exit codes: 0 ok, 2 not installed or not running, 3 bad config or token, 1 unexpected.
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
        "serve" | "record" | "status" | "mute" | "unmute" | "show" | "hide" | "replay" | "token" | "hooks" | "pets" | "migrate" => Ok(bellwether_brain::cli::run(command, tail)),
        "doctor" => Ok(doctor(tail)),
        "shell" => shell(tail),
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

#[cfg(test)]
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

/// This bundle's running strips (app mode, not `serve`, the CLI or `--action`), never
/// this process or the ones that started it.
fn own_strips() -> Vec<i32> {
    let me = std::process::id() as i32;
    let parent = unsafe { libc::getppid() };
    crate::identity::bundle_processes().into_iter().filter(|(pid, _, a)| *pid != me && *pid != parent && is_app_mode(a)).map(|(p, _, _)| p).collect()
}

/// SIGTERM to these pids (this bundle's strips), then up to 5 s for each to go. The
/// strip quits as from the menu: it stops a brain it started. Returns (pid, stopped).
fn stop_strips(strips: &[i32]) -> Vec<(i32, bool)> {
    for pid in strips {
        unsafe { libc::kill(*pid, libc::SIGTERM) };
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && strips.iter().any(|p| unsafe { libc::kill(*p, 0) } == 0) {
        std::thread::sleep(Duration::from_millis(100));
    }
    strips.iter().map(|p| (*p, unsafe { libc::kill(*p, 0) } != 0)).collect()
}

/// After `uninstall --write`: SIGTERM to this bundle's running strip (when the app item
/// was removed), then report anything of this bundle still running. Only processes whose
/// executable is this exact bundle's, never this process or the ones that started it.
fn stop_own_instances(app_removed: bool) {
    let me = std::process::id() as i32;
    let parent = unsafe { libc::getppid() };
    if app_removed {
        for (pid, stopped) in stop_strips(&own_strips()) {
            if stopped {
                println!("stopped the running strip (pid {pid})");
            } else {
                println!("the strip (pid {pid}) didn't quit within 5 s; quit it from the menu-bar sheep, or `kill {pid}`");
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

// --- shell ---------------------------------------------------------------------------

const SHELL_USAGE: &str = "usage: bellwether shell start | stop | restart (the strip of Bellwether.app; `shell --dev` is a repository workflow, not the app's)";

/// `bellwether shell start | stop | restart`: Bellwether.app's strip (test/DIVERGENCES.md
/// 19). The arguments are checked as the Node CLI checks them.
fn shell(args: &[String]) -> CliResult {
    let a = bellwether_core::cliargs::parse(args, &[], &[], 1).map_err(|e| (e.message, e.code))?;
    match a.positional.first().map(String::as_str) {
        Some(sub @ ("start" | "stop" | "restart")) => shell_control(sub),
        Some("dock-permission") => Err(("`shell dock-permission` becomes `bellwether permissions accessibility` in the app (milestone 6): macOS must attribute the request to Bellwether, not to this terminal".into(), exit::UNEXPECTED)),
        _ => Err(bad(SHELL_USAGE)),
    }
}

/// - start: through LaunchServices (`open -g -a <this bundle>`), so macOS treats it as the
///   app (its permissions, its Dock-less menu-bar sheep), unless this bundle's strip already
///   runs. BELLWETHER_HOME, if set, is passed on.
/// - stop: SIGTERM to this bundle's running strip, by the executable's exact path (never
///   another copy of the app, never a repository's bellwether-shell); it quits as from the
///   menu, stopping a brain it started. A brain started by launchd keeps running.
/// - restart: stop, then start.
fn shell_control(sub: &str) -> CliResult {
    let Some(bundle) = crate::identity::bundle_path() else {
        return Err(("`bellwether shell start|stop|restart` controls Bellwether.app's strip, and this is a development binary outside the app. In a checkout, the repository's strip is `./bellwether shell start|stop|restart`".into(), exit::NOT_RUNNING));
    };
    if sub != "start" {
        let running = own_strips();
        for (pid, stopped) in stop_strips(&running) {
            if !stopped {
                return Err((format!("the strip (pid {pid}) didn't quit within 5 s; quit it from the menu-bar sheep, or `kill {pid}`"), exit::UNEXPECTED));
            }
            println!("stopped the strip (pid {pid})");
        }
        if sub == "stop" {
            if running.is_empty() {
                println!("the strip was not running");
            }
            return Ok(exit::OK);
        }
    } else if let Some(pid) = own_strips().first() {
        println!("the strip is already running (pid {pid})");
        return Ok(exit::OK);
    }
    let mut cmd = std::process::Command::new("/usr/bin/open");
    cmd.args(["-g", "-a"]).arg(&bundle);
    if let Some(h) = std::env::var_os("BELLWETHER_HOME").filter(|h| !h.is_empty()) {
        let mut kv = std::ffi::OsString::from("BELLWETHER_HOME=");
        kv.push(h);
        cmd.arg("--env").arg(kv);
    }
    let status = cmd.status().map_err(|e| (format!("couldn't run open: {e}"), exit::UNEXPECTED))?;
    if !status.success() {
        return Err((format!("`open` couldn't launch {}", bundle.display()), exit::UNEXPECTED));
    }
    // A strip that finds another one holding its single-instance lock (another copy of the
    // app on the same port) hands over and quits at once; so a pid counts as started only
    // once it is still alive 1.5 s after it appeared.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut quit: Vec<i32> = Vec::new();
    while std::time::Instant::now() < deadline {
        let fresh: Vec<i32> = own_strips().into_iter().filter(|p| !quit.contains(p)).collect();
        if let Some(&pid) = fresh.first() {
            std::thread::sleep(Duration::from_millis(1500));
            if unsafe { libc::kill(pid, 0) } == 0 {
                println!("{} the strip (pid {pid}); it starts a brain of its own only if none answers", if sub == "restart" { "restarted" } else { "started" });
                return Ok(exit::OK);
            }
            quit.push(pid);
            continue;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err((start_failure(&bundle, &quit), exit::NOT_RUNNING))
}

/// What `shell start` says when no strip stayed up.
pub fn start_failure(bundle: &Path, quit: &[i32]) -> String {
    match quit {
        [] => format!("asked macOS to open {}, but no strip of it is running after 10 s; its shell.log says why", bundle.display()),
        pids => format!(
            "the strip started (pid {}) but quit at once. Most likely another strip on the same port holds the single-instance lock (a second copy of Bellwether.app, which `shell stop` doesn't touch) and it handed over to that one; otherwise its shell.log says why",
            pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        ),
    }
}

// --- doctor --------------------------------------------------------------------------

/// `bellwether doctor [--json] [--e2e] [--settings <path>] [--port <n>]`: the brain's
/// checks (crates/brain doctor.rs, ported from brain/doctor.mjs) with the app's own lines
/// around them: which app this is and its signature first; then login items and
/// Accessibility. Returns the exit code.
fn doctor(args: &[String]) -> i32 {
    use bellwether_brain::doctor::{self, Input};
    use bellwether_core::settings_checks::Check;
    let a = match bellwether_core::cliargs::parse(args, &["json", "e2e"], &["settings", "port"], 0) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("bellwether: {}", e.message);
            return e.code;
        }
    };
    let port = match a.value("port") {
        Some(v) => match bellwether_brain::cli::canonical_port(v).filter(|p| *p >= 1024) {
            Some(p) => Some(p),
            None => {
                eprintln!("bellwether: --port takes an integer from 1024 to 65535");
                return exit::BAD_CONFIG;
            }
        },
        None => None,
    };
    let home = bellwether_brain::home::dir();
    bellwether_brain::cli::remember_token(&home);
    let config = bellwether_core::config::load(&home, std::env::var("BELLWETHER_CONFIG").ok().as_deref());
    let sharing = config.as_ref().ok().map(|c| {
        let v: serde_json::Value = serde_json::from_str(&bellwether_core::json::stringify(&c.merged)).unwrap_or(serde_json::Value::Null);
        let os = if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" };
        let count = v.pointer(&format!("/privacy/sharingProcesses/{os}")).and_then(|l| l.as_array()).map(|l| l.len()).unwrap_or(0);
        (crate::privacy::sharing_warnings_for(&v, os), count)
    });
    let input = Input {
        settings_file: a.value("settings").map(|s| bellwether_core::hooks::resolve(Path::new(s))).unwrap_or_else(doctor::user_settings),
        home: home.clone(),
        user_home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        env_token: std::env::var("BELLWETHER_HOOK_TOKEN").ok(),
        config,
        port,
        probe_timeout: Duration::from_millis(1000),
        project_dir: std::env::current_dir().ok(),
        managed_file: doctor::managed_settings(),
        e2e: a.has("e2e"),
        sharing,
    };
    if input.e2e {
        bellwether_brain::cli::err_line("doctor --e2e: running one `claude -p` with haiku (one small API call)…");
    }
    let session = crate::identity::session();
    let mut checks: Vec<Check> = Vec::new();
    let add = |checks: &mut Vec<Check>, status: &'static str, name: &str, detail: String| checks.push(Check::new(name, status, detail, exit::UNEXPECTED));

    // Which app this is.
    match crate::identity::bundle_path() {
        Some(bundle) => {
            let id = crate::identity::bundle_id().unwrap_or_else(|| "(no CFBundleIdentifier)".into());
            add(&mut checks, "PASS", "identity", format!("{id} {} from {}", bellwether_brain::VERSION, bundle.display()));
            let (status, detail) = signature(&bundle, session);
            add(&mut checks, status, "signature", detail);
        }
        None => add(&mut checks, "INFO", "identity", format!("a development binary ({}), not Bellwether.app: no login items, and macOS permissions belong to this exact file", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default())),
    }
    add(&mut checks, "INFO", "app data", home.display().to_string());

    // The brain's checks: config, token, the brain, the hook entries, the other settings
    // levels, the strip's log, pets, legacy LaunchAgents.
    checks.extend(doctor::checks(&input));
    let checked_port = input.port().unwrap_or(bellwether_brain::DEFAULT_PORT);

    // Login items. The brain answering on the port is this app's agent when launchd (pid
    // 1) runs this very executable for it.
    if crate::identity::in_app_bundle() {
        let brain_pid = match doctor::probe_health(checked_port, input.probe_timeout) {
            doctor::Health::Bellwether(h) => h["pid"].as_u64(),
            _ => None,
        };
        let ours = brain_pid.and_then(|pid| crate::identity::bundle_processes().into_iter().find(|(p, _, _)| *p as u64 == pid)).filter(|(_, ppid, _)| *ppid == 1).map(|(p, _, _)| p);
        for s in [Service::Brain, Service::MainApp] {
            let (status, detail) = login_verdict(s, login_items::status(s), session, if s == Service::Brain { ours } else { None });
            add(&mut checks, status, if s == Service::Brain { "login: brain" } else { "login: app" }, detail);
        }
    }

    // Accessibility, for this exact identity (unknown from an old session).
    if session == crate::identity::Session::Stale {
        add(&mut checks, "INFO", "accessibility", "can't check from this terminal session (it predates your last login); open a new terminal window".into());
    } else if crate::ax::trusted() {
        add(&mut checks, "PASS", "accessibility", "granted to this app: sheep can stand on the Dock".into());
    } else {
        add(&mut checks, "INFO", "accessibility", "not granted to this app: the strip uses its full width. If you granted it before and it now says not granted, System Settings > Privacy & Security > Accessibility still lists the old entry: remove it with −, then grant again".into());
    }

    // An enabled brain agent with no brain answering: launchd is refusing or failing it,
    // unless the agent listens on another port than the one checked (M1 review D3).
    let agent_on = checks.iter().any(|c| c.name == "login: brain" && c.status == "PASS");
    let brain_down = checks.iter().any(|c| c.name == "brain" && c.status == "FAIL");
    if agent_on && brain_down {
        let (status, detail) = agent_down_hint(agent_port(), checked_port);
        add(&mut checks, status, "login: brain", detail);
    }
    let report = doctor::finish(&input, checks);
    if a.has("json") {
        bellwether_brain::cli::out(&doctor::to_json(&report));
    } else {
        bellwether_brain::cli::out(&doctor::render(&report));
    }
    report.exit_code
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

    /// test/DIVERGENCES.md 13: the app's help is its own text; it names every brain command
    /// milestones 3 and 4 ported, with the hook flags of the Node CLI's help.
    #[test]
    fn help_lists_the_brain_commands() {
        for c in [
            "serve [--port <n>]",
            "record [--out <file>]",
            "status [--watch] [--json]",
            "mute | unmute",
            "show | hide",
            "replay <file.jsonl>",
            "replay --burst",
            "token init",
            "token rotate",
            "hooks print [--out <file> [--force]]",
            "hooks install   [--write] [--settings <path>]",
            "hooks uninstall [--write] [--settings <path>] [--port <old port>]",
            "doctor [--json] [--e2e] [--settings <path>] [--port <n>]",
            "pets import <animations.xml>",
            "migrate [--from <dir>]",
            "shell start | stop | restart",
            "version",
        ] {
            assert!(USAGE.contains(&format!("bellwether {c}")), "{c}");
        }
    }

    #[test]
    fn unknown_commands_exit_1_and_bad_flags_exit_3() {
        // A hidden word that isn't a command: "unknown command" without the conformance
        // feature, "unknown hidden command" with it; exit 1 either way.
        assert_eq!(run(&s(&["bellwether", "__nope"])), exit::UNEXPECTED);
        assert_eq!(run(&s(&["bellwether", "autostart", "install", "--nope"])), exit::BAD_CONFIG);
        assert_eq!(run(&s(&["bellwether", "autostart", "sideways"])), exit::BAD_CONFIG);
        assert_eq!(run(&s(&["bellwether", "version"])), exit::OK);
        // Outside Bellwether.app (this test binary), the strip commands refuse: exit 2,
        // nothing started or stopped.
        for sub in ["start", "stop", "restart"] {
            assert_eq!(command_result("shell", &s(&[sub])).unwrap_err().1, exit::NOT_RUNNING, "{sub}");
        }
        assert_eq!(command_result("shell", &s(&["bogus"])).unwrap_err().1, exit::BAD_CONFIG);
        // A strip that appeared and quit (a single-instance hand-off) is not "started".
        let b = std::path::Path::new("/x/Bellwether.app");
        assert!(start_failure(b, &[30100]).starts_with("the strip started (pid 30100) but quit at once"));
        assert!(start_failure(b, &[]).contains("no strip of it is running after 10 s"));
        let src = include_str!("cli.rs");
        let body = src.split("fn shell_control(").nth(1).unwrap().split("\nfn ").next().unwrap();
        assert!(body.find("libc::kill(pid, 0)").unwrap() < body.find("println!(\"{} the strip (pid {pid})").unwrap(), "alive is checked before success is printed");
        assert_eq!(command_result("shell", &s(&["start", "--dev"])).unwrap_err().1, exit::BAD_CONFIG);
    }
}
