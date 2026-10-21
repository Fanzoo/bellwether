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
  bellwether mute | unmute                               do-not-disturb (the tray's Quiet (alerts only); kept across restarts)
  bellwether show | hide                                 show or hide the strip (the tray's Hide everything; kept across restarts)
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
  bellwether migrate --to-app [--from <dir>] [--env <file>] [--settings <path>] [--force] [--write]
                                                         the same, and its LaunchAgents replaced by the app's login
                                                         items, the new brain checked, the hooks updated (D9)
  bellwether migrate --undo [--write]                    put the LaunchAgents and the settings file back as they were
  bellwether shell start | stop | restart                Bellwether.app's strip (stop leaves a launchd brain running)
  bellwether autostart status                            the two login items (brain agent, app at login)
  bellwether autostart install [--write] [--brain-only] [--via-app]
  bellwether autostart uninstall [--write] [--brain-only] [--via-app]
                                                         dry run unless --write; --via-app hands the call
                                                         to Bellwether.app instead of making it here.
                                                         Never writes ~/Library/LaunchAgents (SMAppService)
  bellwether autostart settings                          open System Settings > General > Login Items
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
    if let Some(code) = refused_without_home(command) {
        return code;
    }
    match command_result(command, &rest[1..]) {
        Ok(code) => code,
        Err((message, code)) => {
            eprintln!("bellwether: {message}");
            code
        }
    }
}

/// `home_refusal` for this process (outside a bundle, without BELLWETHER_HOME): says why
/// on stderr and gives the exit code (3). `run` asks it first, and main.rs asks it before
/// `serve`, which starts the brain without going through `run`.
pub fn refused_without_home(command: &str) -> Option<i32> {
    let message = home_refusal(command, crate::identity::in_app_bundle(), std::env::var_os("BELLWETHER_HOME"))?;
    eprintln!("bellwether: {message}");
    Some(exit::BAD_CONFIG)
}

/// Milestone 11 (M11-a): a binary outside an app bundle (a development build in
/// `target-dev/`) runs no command but `help` and `version` without BELLWETHER_HOME, which
/// would otherwise make it act on the installed app's own folder in Application Support.
/// Inside a bundle (the Homebrew symlink resolves into it) nothing changes. An unknown
/// command is left to say so. The strip's modes (`--demo`, no arguments, …) never get here.
fn home_refusal(command: &str, in_bundle: bool, home: Option<std::ffi::OsString>) -> Option<String> {
    let exempt = matches!(command, "help" | "--help" | "-h" | "version" | "--version");
    let known = command.starts_with("__") || KNOWN_COMMANDS.contains(&command);
    if in_bundle || exempt || !known || home.is_some_and(|h| !h.is_empty()) {
        return None;
    }
    Some(format!("this is a development binary outside Bellwether.app, and BELLWETHER_HOME isn't set, so `{command}` would use the installed app's files. In a checkout, `export BELLWETHER_HOME=\"$PWD/data\"` first (see docs/DEVELOPMENT.md); the app's own command is Bellwether.app's `Contents/MacOS/Bellwether`"))
}

/// Every command `command_result` knows (the hidden `__` ones aside).
const KNOWN_COMMANDS: [&str; 15] = ["migrate", "serve", "record", "status", "mute", "unmute", "show", "hide", "replay", "token", "hooks", "pets", "doctor", "shell", "autostart"];

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
        // The move into the app (D9) needs the login items, so it is the app's own command.
        "migrate" if tail.iter().any(|a| a == "--to-app" || a == "--undo") => crate::migration::command(tail),
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
    if !matches!(sub, "status" | "install" | "uninstall" | "settings") {
        return Err(bad(format!("unknown autostart command {sub:?} (status, install, uninstall, settings)")));
    }
    if sub != "install" && sub != "uninstall" && has(&set, "write") {
        return Err(bad("--write is for install and uninstall"));
    }
    if sub != "install" && sub != "uninstall" && has(&set, "brain-only") {
        return Err(bad("--brain-only is for install and uninstall"));
    }
    // Outside Bellwether.app there are no login items to change: say so before any
    // preview, rather than preview something that can't happen (a development binary
    // never reaches SMAppService, so nothing is registered and nothing is written).
    if !crate::identity::in_app_bundle() && (sub == "install" || sub == "uninstall") {
        return Err((NOT_THE_APP.to_string(), exit::NOT_RUNNING));
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
        "settings" => {
            if has(&set, "via-app") {
                return via_app("login-items-settings");
            }
            login_items::open_settings();
            println!("opened System Settings > General > Login Items & Extensions (Login Items before macOS 15)");
            Ok(exit::OK)
        }
        "status" => {
            if has(&set, "via-app") {
                return via_app("login-items-status");
            }
            let statuses: Vec<Status> = [Service::Brain, Service::MainApp].iter().map(|s| login_items::status(*s)).collect();
            for (s, st) in [Service::Brain, Service::MainApp].iter().zip(&statuses) {
                println!("{}", login_items::advice(*s, *st));
            }
            if statuses.contains(&Status::RequiresApproval) {
                offer_settings();
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
            let mut ended: Vec<Status> = Vec::new();
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
                    // The app reports in words; read the status back here.
                    ended.push(login_items::status(*s));
                    continue;
                }
                let r = if on { login_items::register(*s) } else { login_items::unregister(*s) };
                match r {
                    Ok(st) => {
                        println!("{}", login_items::advice(*s, st));
                        ended.push(st);
                    }
                    Err(e) => {
                        eprintln!("bellwether: {}: {e}", s.name());
                        code = exit::UNEXPECTED;
                    }
                }
            }
            if on && ended.contains(&Status::RequiresApproval) {
                offer_settings();
                if code == exit::OK {
                    code = login_items::install_exit(&ended);
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
        _ => Err(bad(format!("unknown autostart command {sub:?} (status, install, uninstall, settings)"))),
    }
}

/// What `autostart install|uninstall` says outside Bellwether.app.
pub const NOT_THE_APP: &str = "not running from Bellwether.app: login items belong to the app bundle (SMAppService), so a development binary has none to change, and nothing was registered or written. Build the app with `node scripts/build-app.mjs` and run its `Contents/MacOS/Bellwether autostart …` (docs/DEVELOPMENT.md)";

/// After a `requiresApproval`: offer to open the Login Items pane. Asked only when a
/// person is at the terminal (stdin and stdout both a terminal); otherwise the command
/// that opens it is printed. Never opens anything unasked.
fn offer_settings() {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !interactive {
        println!("`bellwether autostart settings` opens System Settings > General > Login Items & Extensions");
        return;
    }
    print!("Open System Settings > General > Login Items & Extensions now? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if wants_settings(&answer) {
        login_items::open_settings();
        println!("opened it: turn Bellwether on under \"Allow in the Background\", then run `bellwether autostart status`");
    } else {
        println!("left closed; `bellwether autostart settings` opens it later");
    }
}

/// The answer to [`offer_settings`]'s question: only an explicit "y" or "yes" opens the
/// pane; an empty line, Ctrl-D (end of input) or anything else leaves it closed.
pub fn wants_settings(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
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
    let parent = crate::sys::parent_pid();
    crate::identity::bundle_processes().into_iter().filter(|(pid, _, a)| *pid != me && *pid != parent && is_app_mode(a)).map(|(p, _, _)| p).collect()
}

/// SIGTERM to these pids (this bundle's strips), then up to 5 s for each to go. The
/// strip quits as from the menu: it stops a brain it started. Returns (pid, stopped).
fn stop_strips(strips: &[i32]) -> Vec<(i32, bool)> {
    for pid in strips {
        crate::sys::terminate(*pid);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && strips.iter().any(|p| crate::sys::alive(*p)) {
        std::thread::sleep(Duration::from_millis(100));
    }
    strips.iter().map(|p| (*p, !crate::sys::alive(*p))).collect()
}

/// After `uninstall --write`: SIGTERM to this bundle's running strip (when the app item
/// was removed), then report anything of this bundle still running. Only processes whose
/// executable is this exact bundle's, never this process or the ones that started it.
pub(crate) fn stop_own_instances(app_removed: bool) {
    if app_removed {
        for (pid, stopped) in stop_strips(&own_strips()) {
            if stopped {
                println!("stopped the running strip (pid {pid})");
            } else {
                println!("{}", stuck(pid));
            }
        }
    }
    stragglers(app_removed).iter().for_each(|x| println!("{x}"));
}

/// What is still running of this bundle after its login items went (`autostart uninstall`,
/// `migrate --undo`), 300 ms on: a strip only when the app item stayed, and any other
/// process but `--action` runs. Never this process or the ones that started it.
pub(crate) fn stragglers(app_removed: bool) -> Vec<String> {
    let me = std::process::id() as i32;
    let parent = crate::sys::parent_pid();
    std::thread::sleep(Duration::from_millis(300));
    let mut out = Vec::new();
    for (pid, ppid, args) in crate::identity::bundle_processes().into_iter().filter(|(pid, _, _)| *pid != me && *pid != parent) {
        if !app_removed && is_app_mode(&args) {
            out.push(format!("still running: the strip (pid {pid}); `autostart uninstall --write` without --brain-only stops it"));
        } else if !is_app_mode(&args) && !args.contains("--action") {
            out.push(format!("still running: pid {pid} ({args}, parent {ppid}); it wasn't started by the login item, so it was left alone"));
        }
    }
    out
}

fn stuck(pid: i32) -> String {
    format!("the strip (pid {pid}) didn't quit within 5 s; quit it from the menu-bar sheep, or `kill {pid}`")
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
        return Err(("`bellwether shell start|stop|restart` controls Bellwether.app's strip, and this is a development binary outside the app. A development strip is started by hand: `cargo run --manifest-path shell/src-tauri/Cargo.toml` (docs/DEVELOPMENT.md)".into(), exit::NOT_RUNNING));
    };
    if sub != "start" {
        let (said, running) = quit_strips().map_err(|e| (e, exit::UNEXPECTED))?;
        said.iter().for_each(|x| println!("{x}"));
        if sub == "stop" {
            if !running {
                println!("the strip was not running");
            }
            return Ok(exit::OK);
        }
    }
    println!("{}", start_strip(&bundle, sub == "restart")?);
    Ok(exit::OK)
}

/// SIGTERM to this bundle's running strips (`shell stop`, and `migrate --undo --write`): what
/// was stopped, and whether any ran; Err when one didn't quit within 5 s.
pub(crate) fn quit_strips() -> Result<(Vec<String>, bool), String> {
    let running = own_strips();
    quit_lines(&stop_strips(&running)).map(|said| (said, !running.is_empty()))
}

/// Each strip's outcome, in order: Ok with the "stopped" lines, or Err with every line, one
/// per line, the stopped ones included (deviation 134: they were dropped when a later one
/// failed).
fn quit_lines(outcomes: &[(i32, bool)]) -> Result<Vec<String>, String> {
    let said: Vec<String> = outcomes.iter().map(|&(pid, stopped)| if stopped { format!("stopped the strip (pid {pid})") } else { stuck(pid) }).collect();
    if outcomes.iter().all(|&(_, stopped)| stopped) {
        Ok(said)
    } else {
        Err(said.join("\n"))
    }
}

/// Starts `bundle`'s strip through LaunchServices unless one runs already (`shell start` and
/// `restart`, and the end of `migrate --to-app --write`): what happened.
pub(crate) fn start_strip(bundle: &Path, restart: bool) -> Result<String, (String, i32)> {
    if !restart {
        if let Some(pid) = own_strips().first() {
            return Ok(format!("the strip is already running (pid {pid})"));
        }
    }
    let mut cmd = std::process::Command::new("/usr/bin/open");
    cmd.args(["-g", "-a"]).arg(bundle);
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
            if crate::sys::alive(pid) {
                return Ok(format!("{} the strip (pid {pid}); it starts a brain of its own only if none answers", if restart { "restarted" } else { "started" }));
            }
            quit.push(pid);
            continue;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err((start_failure(bundle, &quit), exit::NOT_RUNNING))
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
    use bellwether_brain::doctor;
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
    let settings_file = a.value("settings").map(|s| bellwether_core::hooks::resolve(Path::new(s)));
    if a.has("e2e") {
        bellwether_brain::cli::err_line("doctor --e2e: running one `claude -p` with haiku (one small API call)…");
    }
    let report = doctor_report(&home, settings_file, port, a.has("e2e"));
    if a.has("json") {
        bellwether_brain::cli::out(&doctor::to_json(&report));
    } else {
        bellwether_brain::cli::out(&doctor::render(&report));
    }
    report.exit_code
}

/// Every doctor check, for `home` (the app's files), the settings file (default
/// `~/.claude/settings.json`) and the port (default: the config's): what `bellwether
/// doctor` prints, and what the onboarding window's last step shows in plain words.
pub fn doctor_report(home: &Path, settings_file: Option<PathBuf>, port: Option<u16>, e2e: bool) -> bellwether_brain::doctor::Report {
    use bellwether_brain::doctor::{self, Input};
    use bellwether_core::settings_checks::Check;
    let config = bellwether_core::config::load(home, std::env::var("BELLWETHER_CONFIG").ok().as_deref());
    let sharing = config.as_ref().ok().map(|c| {
        let v: serde_json::Value = serde_json::from_str(&bellwether_core::json::stringify(&c.merged)).unwrap_or(serde_json::Value::Null);
        let os = if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" };
        let count = v.pointer(&format!("/privacy/sharingProcesses/{os}")).and_then(|l| l.as_array()).map(|l| l.len()).unwrap_or(0);
        (crate::privacy::sharing_warnings_for(&v, os), count)
    });
    let input = Input {
        settings_file: settings_file.unwrap_or_else(doctor::user_settings),
        home: home.to_path_buf(),
        user_home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        env_token: std::env::var("BELLWETHER_HOOK_TOKEN").ok(),
        config,
        port,
        probe_timeout: Duration::from_millis(1000),
        project_dir: std::env::current_dir().ok(),
        managed_file: doctor::managed_settings(),
        e2e,
        sharing,
    };
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

    // Accessibility: the strip's, which is what puts sheep on the Dock. Never this process's:
    // macOS answers AXIsProcessTrusted for the app responsible for the caller, and a CLI run
    // from a terminal is the terminal's (deviation 135). The strip logs its own answer at
    // start and on every change; that line (`dock`, from the brain's log checks) becomes
    // this one, with whether a strip runs now.
    let dock = checks.iter().position(|c| c.name == "dock").map(|i| checks.remove(i));
    let strips = crate::identity::in_app_bundle().then(own_strips);
    let (status, detail) = accessibility_verdict(strips.as_deref(), dock.as_ref().map(|c| (c.status, c.detail.as_str())));
    add(&mut checks, status, "accessibility", detail);

    // A brain of another version than this app's (deviation 135): WARN, with the fix.
    if let doctor::Health::Bellwether(h) = doctor::probe_health(checked_port, input.probe_timeout) {
        let theirs = h["version"].as_str().unwrap_or("?").to_string();
        if theirs != bellwether_brain::VERSION {
            let pid = h["pid"].as_i64().unwrap_or(0);
            let check = crate::agent::Check::here();
            let label = check.host.label();
            let owner = crate::agent::owner(&check, label.as_deref(), pid);
            let suffix = crate::agent::doctor_suffix(&theirs, bellwether_brain::VERSION, pid, label.as_deref(), &owner);
            if let Some(c) = checks.iter_mut().find(|c| c.name == "brain" && c.status == "PASS") {
                c.status = "WARN";
                c.detail.push_str(&suffix);
            }
        }
    }

    // An enabled brain agent with no brain answering: launchd is refusing or failing it,
    // unless the agent listens on another port than the one checked (M1 review D3).
    let agent_on = checks.iter().any(|c| c.name == "login: brain" && c.status == "PASS");
    let brain_down = checks.iter().any(|c| c.name == "brain" && c.status == "FAIL");
    if agent_on && brain_down {
        let (status, detail) = agent_down_hint(agent_port(), checked_port);
        add(&mut checks, status, "login: brain", detail);
    }
    let mut report = doctor::finish(&input, checks);
    if !crate::identity::in_app_bundle() {
        for c in &mut report.checks {
            c.detail = dev_binary_wording(&c.detail);
        }
    }
    report
}

/// A development binary has no login items (it refuses `autostart install`), so where
/// doctor's shared lines suggest starting the brain at login, it says `bellwether serve`
/// only. Bellwether.app's lines are untouched.
fn dev_binary_wording(detail: &str) -> String {
    detail
        .replace("start the brain: at login with `bellwether autostart install --write`, or now with `bellwether serve`", "start the brain: `bellwether serve` (a development binary has no start at login; docs/DEVELOPMENT.md)")
        .replace("(`bellwether autostart install --write` starts one at login; `bellwether serve` runs one now)", "(`bellwether serve` runs one; a development binary has no start at login)")
        .replace(". With start at login on, launchd starts it: `bellwether autostart status`", "")
}

/// Doctor's one Accessibility line, from the strips running now (None: a development
/// binary, which can't find them) and the strip's own last word in its log (`dock`).
pub fn accessibility_verdict(strips: Option<&[i32]>, dock: Option<(&'static str, &str)>) -> (&'static str, String) {
    let said = |d: &str, s: &str| match s {
        "PASS" => "it had Accessibility".to_string(),
        "WARN" => format!("it didn't have Accessibility ({d})"),
        _ => "it didn't have Accessibility".to_string(),
    };
    match (strips, dock) {
        (Some([]), last) => (
            "INFO",
            format!(
                "the strip isn't running, so doctor can't tell whether it has Accessibility (macOS answers that for each app, and doctor runs as your terminal). Start it with `bellwether shell start`{}",
                last.map(|(s, d)| format!("; the last time it ran, {}", said(d, s))).unwrap_or_default()
            ),
        ),
        (Some(pids), Some((status, detail))) => {
            let who = format!("the strip (pid {})", pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
            let detail = match status {
                "PASS" => format!("granted to {who}: sheep stand on the Dock's top edge"),
                "WARN" => format!("not granted to {who}: {detail}"),
                _ => format!("not granted to {who}: {detail}"),
            };
            (status, detail)
        }
        (Some(pids), None) => ("INFO", format!("the strip (pid {}) is running, but its log (shell.log) doesn't say whether it has Accessibility; `bellwether shell restart` writes it again", pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "))),
        (None, Some((status, detail))) => (status, format!("from the strip's log (a development binary can't see whether a strip runs now): {detail}")),
        (None, None) => ("INFO", "a development binary: the strip's log has no Accessibility line, and doctor itself can't tell (macOS answers for each app, and doctor runs as your terminal)".into()),
    }
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
    fn autostart_outside_the_app_changes_nothing_and_says_why() {
        // This test binary is outside any bundle: install and uninstall refuse before a
        // preview, with exit 2; status reports "unavailable"; bad usage is still exit 3.
        for sub in ["install", "uninstall"] {
            for extra in [&[][..], &["--write"][..], &["--write", "--brain-only"][..]] {
                let mut a = s(&[sub]);
                a.extend(s(extra));
                let (message, code) = autostart(&a).unwrap_err();
                assert_eq!(code, exit::NOT_RUNNING, "{sub} {extra:?}");
                assert!(message.starts_with("not running from Bellwether.app") && message.contains("nothing was registered or written"), "{message}");
            }
        }
        assert_eq!(autostart(&s(&["status", "--write"])).unwrap_err().1, exit::BAD_CONFIG);
        assert_eq!(autostart(&s(&["settings", "--brain-only"])).unwrap_err().1, exit::BAD_CONFIG);
        assert_eq!(autostart(&s(&["sideways"])).unwrap_err().1, exit::BAD_CONFIG);
        assert!(USAGE.contains("bellwether autostart settings"));
    }

    #[test]
    fn waiting_for_approval_says_what_to_do_and_offers_the_pane() {
        for s in [Service::Brain, Service::MainApp] {
            let line = login_items::advice(s, Status::RequiresApproval);
            assert!(line.contains("System Settings > General > Login Items") && line.contains("Allow in the Background") && line.contains("bellwether autostart settings"), "{line}");
            let (status, detail) = login_verdict(s, Status::RequiresApproval, Session::Current, None);
            assert_eq!((status, detail.as_str()), ("WARN", line.as_str()), "doctor reports it as a WARN with the same advice");
        }
        assert_eq!(login_items::install_exit(&[Status::Enabled, Status::Enabled]), exit::OK);
        assert_eq!(login_items::install_exit(&[Status::Enabled, Status::RequiresApproval]), exit::NOT_RUNNING);
        // Only an explicit yes opens it: Enter, Ctrl-D (read_line gives ""), anything else: no.
        for yes in ["y\n", "Y", "yes\n", " Yes \n"] {
            assert!(wants_settings(yes), "{yes:?}");
        }
        for no in ["", "\n", "n\n", "No", "yeah", "y y", "ok", "\u{4}"] {
            assert!(!wants_settings(no), "{no:?}");
        }
        // The offer comes after install and status; it asks first and only at a terminal.
        let src = include_str!("cli.rs");
        let body = src.split("fn autostart(").nth(1).unwrap().split("\nfn ").next().unwrap();
        assert_eq!(body.matches("offer_settings();").count(), 2);
        let offer = src.split("fn offer_settings(").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert!(offer.find("is_terminal()").unwrap() < offer.find("read_line").unwrap() && offer.find("read_line").unwrap() < offer.find("open_settings()").unwrap());
    }

    #[test]
    fn only_the_strip_counts_as_the_app() {
        assert!(is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether"));
        assert!(is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether --demo"));
        assert!(!is_app_mode("Bellwether serve --port 47418"));
        assert!(!is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether doctor"));
        assert!(!is_app_mode("/x/Bellwether.app/Contents/MacOS/Bellwether --action login-items-status"));
    }

    /// Deviation 135: one Accessibility line, the strip's, never this process's (a CLI run
    /// from a terminal is the terminal's to macOS), and plain about a strip that isn't running.
    #[test]
    fn accessibility_is_the_strips_in_one_line() {
        let granted = Some(("PASS", "Accessibility granted: sheep stand on the Dock's top edge"));
        let not = Some(("INFO", "the strip uses its full width; to have the sheep stand on the Dock itself, turn Bellwether on in System Settings > Privacy & Security > Accessibility"));
        let earlier = Some(("WARN", "Accessibility was granted to an earlier build of the strip and is not granted to this one"));
        assert_eq!(accessibility_verdict(Some(&[46880]), granted), ("PASS", "granted to the strip (pid 46880): sheep stand on the Dock's top edge".to_string()));
        let (st, d) = accessibility_verdict(Some(&[46880]), not);
        assert!(st == "INFO" && d.starts_with("not granted to the strip (pid 46880): the strip uses its full width"), "{d}");
        assert_eq!(accessibility_verdict(Some(&[46880]), earlier).0, "WARN");
        let (st, d) = accessibility_verdict(Some(&[]), granted);
        assert_eq!(st, "INFO");
        assert!(d.starts_with("the strip isn't running, so doctor can't tell whether it has Accessibility") && d.contains("`bellwether shell start`") && d.ends_with("the last time it ran, it had Accessibility"), "{d}");
        assert!(!accessibility_verdict(Some(&[]), None).1.contains("last time"));
        assert_eq!(accessibility_verdict(Some(&[7]), None).0, "INFO");
        assert!(accessibility_verdict(None, granted).1.starts_with("from the strip's log"));
        // doctor never asks macOS about itself, and the strip's `dock` line is folded in.
        let src = include_str!("cli.rs");
        let body = src.split("pub fn doctor_report(").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert!(!body.contains("ax::trusted"), "the CLI's own trust is the terminal's, not the strip's");
        assert!(body.contains(r#"checks.iter().position(|c| c.name == "dock").map(|i| checks.remove(i))"#));
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

    /// A development binary's doctor never suggests `autostart install` (which it refuses):
    /// the shared lines are reworded; every phrase replaced is still in crates/brain doctor.rs.
    #[test]
    fn a_development_binarys_doctor_suggests_serve_not_autostart() {
        let next = "start the brain: at login with `bellwether autostart install --write`, or now with `bellwether serve`";
        assert_eq!(dev_binary_wording(next), "start the brain: `bellwether serve` (a development binary has no start at login; docs/DEVELOPMENT.md)");
        let errors = "… until a brain runs (`bellwether autostart install --write` starts one at login; `bellwether serve` runs one now)";
        assert!(!dev_binary_wording(errors).contains("autostart") && dev_binary_wording(errors).contains("`bellwether serve` runs one"));
        let brain = "brain not running — run `bellwether serve` (nothing listening on 127.0.0.1:47420). With start at login on, launchd starts it: `bellwether autostart status`";
        assert_eq!(dev_binary_wording(brain), "brain not running — run `bellwether serve` (nothing listening on 127.0.0.1:47420)");
        let shared = include_str!("../../../crates/brain/src/doctor.rs");
        for phrase in ["at login with `bellwether autostart install --write`, or now with `bellwether serve`", "(`bellwether autostart install --write` starts one at login; `bellwether serve` runs one now)", ". With start at login on, launchd starts it: `bellwether autostart status`"] {
            assert!(shared.contains(phrase), "doctor.rs no longer says {phrase:?}: update dev_binary_wording");
        }
    }

    /// M11-a: outside a bundle, every command but help and version needs BELLWETHER_HOME;
    /// inside one, or with it set, nothing is refused; an unknown command says so.
    #[test]
    fn a_development_binary_needs_bellwether_home() {
        let home = || Some(std::ffi::OsString::from("/x/data"));
        for c in ["doctor", "serve", "token", "hooks", "migrate", "shell", "autostart", "pets", "status", "__session"] {
            let m = home_refusal(c, false, None).unwrap_or_else(|| panic!("{c} wasn't refused"));
            assert!(m.contains("BELLWETHER_HOME") && m.contains("export BELLWETHER_HOME=\"$PWD/data\"") && m.contains("docs/DEVELOPMENT.md"), "{m}");
            assert_eq!(home_refusal(c, false, Some(std::ffi::OsString::new())).is_some(), true, "{c}: an empty BELLWETHER_HOME counts as unset");
            assert_eq!(home_refusal(c, false, home()), None, "{c} with BELLWETHER_HOME");
            assert_eq!(home_refusal(c, true, None), None, "{c} inside a bundle");
        }
        for c in ["help", "--help", "-h", "version", "--version", "bogus"] {
            assert_eq!(home_refusal(c, false, None), None, "{c}");
        }
        // Every command command_result knows is in the list (so none slips past the rule).
        let src = include_str!("cli.rs");
        let arms = src.split("fn command_result(").nth(1).unwrap().split("\n}\n").next().unwrap();
        for c in KNOWN_COMMANDS {
            assert!(arms.contains(&format!("\"{c}\"")), "{c} isn't a command");
        }
        // And `run` asks it before anything else.
        let run = src.split("pub fn run(").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert!(run.find("refused_without_home(").unwrap() < run.find("command_result(").unwrap());
        // So does main.rs, before `serve`.
        let main = include_str!("main.rs");
        assert!(main.contains("Mode::Serve => std::process::exit(bellwether_shell_lib::cli::refused_without_home(\"serve\").unwrap_or_else(|| bellwether_brain::cli::run(\"serve\", &args[2..]))),"), "main.rs serves without the check");
    }

    #[test]
    fn unknown_commands_exit_1_and_bad_flags_exit_3() {
        // A hidden word that isn't a command: "unknown command" without the conformance
        // feature, "unknown hidden command" with it; exit 1 either way.
        // (Through command_result: this test binary is outside a bundle and has no
        // BELLWETHER_HOME, so `run` would refuse them first; that refusal is pinned in
        // `a_development_binary_needs_bellwether_home`.)
        assert!(matches!(command_result("__nope", &[]), Ok(exit::UNEXPECTED) | Err((_, exit::UNEXPECTED))));
        assert_eq!(command_result("autostart", &s(&["install", "--nope"])).unwrap_err().1, exit::BAD_CONFIG);
        assert_eq!(command_result("autostart", &s(&["sideways"])).unwrap_err().1, exit::BAD_CONFIG);
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
        let body = src.split("fn start_strip(").nth(1).unwrap().split("\nfn ").next().unwrap();
        assert!(body.find("crate::sys::alive(pid)").unwrap() < body.find("Ok(format!(\"{} the strip (pid {pid})").unwrap(), "alive is checked before success is said");
        assert_eq!(command_result("shell", &s(&["start", "--dev"])).unwrap_err().1, exit::BAD_CONFIG);
    }

    /// Deviation 134: a strip that won't quit no longer hides the ones that did.
    #[test]
    fn every_strip_is_reported_when_one_does_not_quit() {
        assert_eq!(quit_lines(&[]), Ok(vec![]));
        assert_eq!(quit_lines(&[(11, true), (12, true)]), Ok(vec!["stopped the strip (pid 11)".to_string(), "stopped the strip (pid 12)".to_string()]));
        let e = quit_lines(&[(11, true), (12, false), (13, true)]).unwrap_err();
        assert_eq!(e.lines().collect::<Vec<_>>(), ["stopped the strip (pid 11)", &stuck(12), "stopped the strip (pid 13)"]);
        assert!(stuck(12).contains("didn't quit within 5 s") && stuck(12).contains("`kill 12`"));
    }
}
