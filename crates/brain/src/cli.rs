//! The brain's commands (distribution plan, milestones 3 and 4): `serve`, `record`,
//! `status`, `mute`/`unmute`, `show`/`hide`, `replay`, `token init|rotate`, `hooks
//! print|install|uninstall`, `pets import` and `migrate`. The same names,
//! flags, output and exit codes as the Node CLI (brain/cli.mjs), except where the app's
//! layout differs: the token is a file, not a `.env` entry (D3), and the app's files live
//! in its home (`BELLWETHER_HOME`). The token is masked in everything printed.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bellwether_core::cliargs::{self, js_number};
use bellwether_core::config::Config;
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::{jsstr, mask, outfile, replay as rp, shellstate, status as render, token as tok};

use crate::client::{self, Failure};
use crate::{exit, home, BrainError, Logger, Mode, Options, BIND_HOST, VERSION};

type CmdResult = Result<i32, BrainError>;

/// The token in use, for masking what this process prints.
static TOKEN: OnceLock<String> = OnceLock::new();

fn masked(text: &str) -> String {
    mask::mask(text, TOKEN.get().map(String::as_str))
}

/// Prints to stdout, the token masked.
pub fn out(text: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(masked(text).as_bytes());
    let _ = o.flush();
}

/// A line to stderr, the token masked.
pub fn err_line(text: &str) {
    let _ = std::io::stderr().write_all((masked(text) + "\n").as_bytes());
}

/// Runs one command. Returns the exit code.
pub fn run(command: &str, args: &[String]) -> i32 {
    let r = match command {
        "serve" => serve(args, false),
        "record" => serve(args, true),
        "status" => status(args),
        "mute" | "unmute" => mute(command, args),
        "show" | "hide" => show(command, args),
        "replay" => replay(args),
        "token" => token(args),
        "hooks" => hooks(args),
        "pets" => pets(args),
        "migrate" => crate::migrate::command(args),
        _ => Err(BrainError::new(format!("unknown command {command:?}"), exit::UNEXPECTED)),
    };
    match r {
        Ok(code) => code,
        Err(e) => {
            err_line(&format!("bellwether: {}", e.message));
            e.code
        }
    }
}

fn bad(m: impl Into<String>) -> BrainError {
    BrainError::new(m, exit::BAD_CONFIG)
}

pub fn config(home: &Path) -> Result<Config, BrainError> {
    Ok(bellwether_core::config::load(home, std::env::var("BELLWETHER_CONFIG").ok().as_deref())?)
}

fn env_token() -> Option<String> {
    std::env::var("BELLWETHER_HOOK_TOKEN").ok()
}

/// The token in use, checked, and remembered for masking.
fn read_token(home: &Path) -> Result<(String, tok::Source), BrainError> {
    let (t, source) = tok::read(home, env_token())?;
    let _ = TOKEN.set(t.clone());
    Ok((t, source))
}

/// A path the user typed: relative to the directory they ran the command from, with `.`
/// and `..` folded (`path.resolve`).
pub fn user_path(p: &str) -> PathBuf {
    bellwether_core::hooks::resolve(Path::new(p))
}

/// The token for masking only, when the command doesn't need one: never an error.
pub fn remember_token(home: &Path) {
    if let Ok((t, _)) = tok::read(home, env_token()) {
        let _ = TOKEN.set(t);
    }
}

/// `new Date().toISOString().replace(/[:.]/g, '-')`: 2026-10-08T18-41-07-123Z
pub fn file_stamp(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}-{:02}-{:02}-{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, ms % 1000)
}

// ------------------------------------------------------------------ serve and record

/// The `--port` a test gives serve: 1024 to 65535, or 0 for any free port.
fn port_arg(v: &str) -> Result<u16, BrainError> {
    match canonical_port(v) {
        Some(p) if p == 0 || p >= 1024 => Ok(p),
        _ => Err(bad("--port takes an integer from 1024 to 65535")),
    }
}

/// A port as a plain decimal: `47417`, never `047417`, `+47417`, ` 47417` or `4.7417e4`
/// (the same rule as `hooks --port` in both CLIs), so one spelling means one port.
pub fn canonical_port(v: &str) -> Option<u16> {
    let p = js_number(v);
    (p.fract() == 0.0 && (0.0..=65535.0).contains(&p) && json::number_to_string(p) == v).then_some(p as u16)
}

/// Test knobs for the conformance suite: shorter timings and a lower connection cap,
/// from the environment. Only in builds with the `conformance` feature (never a release).
#[cfg(feature = "conformance")]
fn test_knobs(opts: &mut Options) {
    let ms = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    if let Some(n) = ms("BELLWETHER_TEST_TICK_MS") {
        opts.timings.tick = Duration::from_millis(n.max(1));
    }
    if let Some(n) = ms("BELLWETHER_TEST_PERSIST_MS") {
        opts.timings.persist = Duration::from_millis(n.max(1));
    }
    if let Some(n) = ms("BELLWETHER_TEST_PUBLISH_MS") {
        opts.timings.publish_ms = n as f64;
    }
    if let Some(n) = ms("BELLWETHER_TEST_MAX_CONNECTIONS") {
        opts.max_connections = n as usize;
    }
    if std::env::var_os("BELLWETHER_TEST_NO_FD_RAISE").is_some() {
        opts.raise_fd_limit = false;
    }
}

#[cfg(not(feature = "conformance"))]
fn test_knobs(_: &mut Options) {}

fn serve(args: &[String], record: bool) -> CmdResult {
    let values: &[&str] = if record { &["slow", "out", "port"] } else { &["slow", "port"] };
    let a = cliargs::parse(args, &["hang"], values, 0)?;
    let home = home::dir();
    let config = config(&home)?;
    let (token, source) = read_token(&home)?;
    let hang = a.has("hang");
    let slow = a.value("slow").map(js_number).unwrap_or(0.0);
    if hang && slow != 0.0 && !slow.is_nan() {
        return Err(bad("--hang and --slow are exclusive"));
    }
    if !slow.is_finite() || slow < 0.0 {
        return Err(bad("--slow takes a delay in milliseconds, e.g. --slow=1500"));
    }
    let port = match a.value("port") {
        Some(v) => port_arg(v)?,
        None => config.port(),
    };
    let record_to = if record {
        let f = match a.value("out") {
            Some(o) => user_path(o),
            None => home.join("recordings").join(format!("{}.jsonl", file_stamp(crate::now_ms()))),
        };
        outfile::guard_record(&f)?;
        // Opened before the port is bound, as Node does: a directory that can't be
        // written is exit 3 naming the file, whatever holds the port.
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&f).map_err(|e| bad(format!("cannot open {}: {}", f.display(), bellwether_core::config::io_code(&e))))?;
        Some(f)
    } else {
        None
    };
    let mode = if hang {
        Mode::Hang
    } else if slow > 0.0 {
        Mode::Slow(slow as u64)
    } else {
        Mode::Normal
    };
    let log = Logger::to_file(home::log_dir().join("brain.log"));
    let bound = crate::bind(port)?;
    let note = match mode {
        Mode::Hang => " (debug: --hang, /hook never answers)".to_string(),
        Mode::Slow(ms) => format!(" (debug: --slow={ms})"),
        Mode::Normal => String::new(),
    };
    let mut opts = Options::new(token, mode, log.clone());
    opts.token_file = match source {
        tok::Source::File(f) => Some(f),
        tok::Source::Env => None,
    };
    opts.config = config;
    opts.home = Some(home);
    opts.record_to = record_to.clone();
    test_knobs(&mut opts);
    opts.announce.push(format!("listening on http://{BIND_HOST}:{}{note}, pid {}, version {VERSION} (rust)", bound.port, std::process::id()));
    if let Some(f) = &record_to {
        opts.announce.push(format!("recording to {}", f.display()));
    }
    let result = crate::serve(bound, opts, None);
    if let Ok(accepted) = &result {
        log.log(&format!("stopped ({accepted} events accepted)"));
    }
    // The last lines out before exit, but never held up for long by a stalled stderr.
    log.flush(Duration::from_millis(500));
    result?;
    Ok(exit::OK)
}

// ------------------------------------------------------------------ the brain's API

fn not_running(port: u16, f: &Failure) -> BrainError {
    match f {
        Failure::Refused => BrainError::new(format!("brain not running — run `bellwether serve` (nothing listening on {BIND_HOST}:{port})"), exit::NOT_RUNNING),
        Failure::Code(c) => BrainError::new(format!("could not reach the brain on port {port}: {c}"), exit::NOT_RUNNING),
    }
}

fn refused_token() -> BrainError {
    BrainError::new("the brain refused the token (403); it was started with a different hook token. Restart it (`bellwether serve`)", exit::BAD_CONFIG)
}

/// GET /state, the usual failures as CLI errors with a cause.
pub fn get_state(port: u16, token: &str, after: f64, wait_ms: u64) -> Result<Js, BrainError> {
    let path = format!("/state?after={}&wait={wait_ms}", json::number_to_string(after));
    let r = client::request(port, "GET", &path, &[("X-Bellwether-Token", token)], None, Duration::from_millis(wait_ms + 10_000)).map_err(|f| not_running(port, &f))?;
    if r.status == 403 {
        return Err(refused_token());
    }
    match json::parse(&r.text()) {
        Ok(v) if r.status == 200 => Ok(v),
        _ => Err(BrainError::new(format!("unexpected answer from /state: HTTP {}", r.status), exit::UNEXPECTED)),
    }
}

/// POST /control, with the same failure messages as get_state.
pub fn post_control(port: u16, token: &str, msg: &Js) -> Result<Js, BrainError> {
    let body = json::stringify(msg);
    let r = client::request(port, "POST", "/control", &[("X-Bellwether-Token", token), ("Content-Type", "application/json")], Some(body.as_bytes()), Duration::from_secs(5)).map_err(|f| not_running(port, &f))?;
    if r.status == 403 {
        return Err(refused_token());
    }
    let v = json::parse(&r.text()).unwrap_or(Js::Null);
    if r.status != 200 || !v.get("ok").truthy() {
        let why = v.get("error").as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {}", r.status));
        return Err(BrainError::new(format!("the brain refused {}: {why}", msg.get("action").as_str().unwrap_or("?")), exit::UNEXPECTED));
    }
    Ok(v)
}

fn status(args: &[String]) -> CmdResult {
    let a = cliargs::parse(args, &["watch", "json"], &[], 0)?;
    let home = home::dir();
    let config = config(&home)?;
    let (token, _) = read_token(&home)?;
    if a.has("watch") && a.has("json") {
        return Err(bad("--watch and --json are exclusive"));
    }
    let port = config.port();
    let palette: Vec<String> = config.palette().iter().map(|c| jsstr::from_str(c)).collect();
    let tty = std::io::stdout().is_terminal();
    let show = |snap: &Js| jsstr::to_string_lossy(&render::render(snap, &palette, tty));
    let mut snap = get_state(port, &token, -1.0, 0)?;
    if a.has("json") {
        out(&(json::stringify_pretty(&snap, "  ") + "\n"));
        return Ok(exit::OK);
    }
    if !a.has("watch") {
        out(&show(&snap));
        return Ok(exit::OK);
    }
    let clear = if tty { "\x1b[2J\x1b[H" } else { "\n" };
    loop {
        out(&format!("{clear}{}", show(&snap)));
        let rev = snap.get("rev").as_num().unwrap_or(-1.0);
        match get_state(port, &token, rev, 25_000) {
            Ok(s) => snap = s,
            Err(e) => {
                // The brain went away: say so, keep watching, pick up when it's back.
                out(&format!("{clear}{}\nwaiting for the brain…\n", e.message));
                std::thread::sleep(Duration::from_secs(2));
                if let Ok(s) = get_state(port, &token, -1.0, 0) {
                    snap = s;
                }
            }
        }
    }
}

/// Do-not-disturb, the same switch as the tray's Quiet (alerts only); kept in state.json.
fn mute(command: &str, args: &[String]) -> CmdResult {
    cliargs::parse(args, &[], &[], 0)?;
    let home = home::dir();
    let config = config(&home)?;
    let (token, _) = read_token(&home)?;
    post_control(config.port(), &token, &Js::Obj(Obj::new().with("action", command)))?;
    out(if command == "mute" { "muted: the strip stops animating; only a sheep that needs you shows a badge\n" } else { "unmuted\n" });
    Ok(exit::OK)
}

/// The tray's "Hide everything (for screen sharing)", from the terminal; a running strip follows within a second.
fn show(command: &str, args: &[String]) -> CmdResult {
    cliargs::parse(args, &[], &[], 0)?;
    let home = home::dir();
    let (_, labels) = shellstate::read(&home);
    shellstate::write(&home, command == "hide", labels).map_err(|e| BrainError::new(format!("can't write {} ({e})", shellstate::file(&home).display()), exit::UNEXPECTED))?;
    out(&format!("{}: a running shell follows within a second, and the shell starts this way next time ({})\n", if command == "hide" { "hidden" } else { "shown" }, shellstate::file(&home).display()));
    Ok(exit::OK)
}

// ------------------------------------------------------------------ replay

fn num(name: &str, v: &str, min: f64, max: f64) -> Result<f64, BrainError> {
    let n = js_number(v);
    if !n.is_finite() || n < min || n > max {
        return Err(bad(format!("--{name} must be a number from {} to {}", json::number_to_string(min), json::number_to_string(max))));
    }
    Ok(n)
}

fn replay(args: &[String]) -> CmdResult {
    let a = cliargs::parse(args, &[], &["speed", "port", "burst", "seconds", "tag"], 1)?;
    let home = home::dir();
    let config = config(&home)?;
    let (token, _) = read_token(&home)?;
    let port = match a.value("port") {
        Some(v) => match canonical_port(v) {
            Some(p) if p >= 1 => p,
            _ => return Err(bad("--port must be a number from 1 to 65535")),
        },
        None => config.port(),
    };
    let speed = match a.value("speed") {
        Some(v) => num("speed", v, 0.01, 1000.0)?,
        None => 1.0,
    };
    let (lines, what) = if let Some(b) = a.value("burst") {
        if !a.positional.is_empty() {
            return Err(bad("give either a file or --burst, not both"));
        }
        let rate = num("burst", b, 1.0, 5000.0)?;
        let seconds = match a.value("seconds") {
            Some(v) => num("seconds", v, 0.1, 60.0)?,
            None => 2.0,
        };
        let lines = rp::burst(rate, seconds);
        let what = format!("a synthetic burst of {} events at {}/s", lines.len(), json::number_to_string(rate));
        (lines, what)
    } else {
        let Some(file) = a.positional.first() else {
            return Err(bad("usage: bellwether replay <file.jsonl> [--speed N] [--port P]  |  bellwether replay --burst <events/s> [--seconds S]"));
        };
        if a.value("seconds").is_some() {
            return Err(bad("--seconds only goes with --burst"));
        }
        let lines = rp::read_recording(&user_path(file))?;
        let what = format!("{} events from {file}", lines.len());
        (lines, what)
    };
    let tag = a.value("tag").map(str::to_string);
    if let Some(t) = &tag {
        if !(1..=40).contains(&t.chars().count()) || !t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')) {
            return Err(bad("--tag must be 1–40 characters of [A-Za-z0-9_.:-]"));
        }
    }
    err_line(&format!("replaying {what} to {BIND_HOST}:{port} at {}×{} …", json::number_to_string(speed), tag.as_ref().map(|t| format!(" as {t}")).unwrap_or_default()));
    let r = run_replay(&lines, port, &token, speed, tag.as_deref())?;
    let f = |x: f64| if x.is_finite() { format!("{x:.2}") } else { "–".into() };
    let mut text = format!(
        "replayed {} events in {:.2} s\n/hook latency: p50 {} ms, p99 {} ms, max {} ms\nsnapshots: {} new, at most {} in any one second\n",
        lines.len(),
        r.elapsed,
        f(r.p50),
        f(r.p99),
        f(r.max),
        r.snapshots,
        r.max_per_second
    );
    if r.failures.is_empty() {
        text.push_str("every /hook answer was an empty 204\n");
    } else {
        text.push_str(&format!("FAILURES ({}):\n  {}\n", r.failures.len(), r.failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n  ")));
    }
    out(&text);
    Ok(if r.failures.is_empty() { exit::OK } else { exit::UNEXPECTED })
}

pub struct ReplayResult {
    pub elapsed: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
    pub failures: Vec<String>,
    pub snapshots: usize,
    pub max_per_second: usize,
}

/// Posts the lines to /hook on schedule, long-polling /state alongside to count
/// snapshots, and measures /hook latency (brain/replay.mjs `replay`).
pub fn run_replay(lines: &[Js], port: u16, token: &str, speed: f64, tag: Option<&str>) -> Result<ReplayResult, BrainError> {
    if lines.is_empty() {
        return Err(bad("nothing to replay: the file has no events"));
    }
    let t0 = lines[0].get("normalised").get("receivedAt").as_num().unwrap_or(0.0);
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let snapshots = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let polling = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let poller = {
        let (failures, snapshots, polling, token) = (failures.clone(), snapshots.clone(), polling.clone(), token.to_string());
        std::thread::spawn(move || {
            let mut rev = -1.0;
            while polling.load(std::sync::atomic::Ordering::Relaxed) {
                let path = format!("/state?after={}&wait=1000", json::number_to_string(rev));
                let r = match client::request(port, "GET", &path, &[("X-Bellwether-Token", &token)], None, Duration::from_secs(5)) {
                    Ok(r) => r,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                };
                let v = json::parse(&r.text()).ok();
                let Some(v) = v.filter(|_| r.status == 200) else {
                    failures.lock().unwrap().push(format!("/state answered {}", r.status));
                    return;
                };
                let now_rev = v.get("rev").as_num().unwrap_or(-1.0);
                if now_rev > rev {
                    if rev >= 0.0 {
                        snapshots.lock().unwrap().push(Instant::now());
                    }
                    rev = now_rev;
                }
            }
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    let few = lines.len() <= 200;
    let mut latencies = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let n = line.get("normalised");
        let due = Duration::from_secs_f64(((n.get("receivedAt").as_num().unwrap_or(t0) - t0) / speed / 1000.0).max(0.0));
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
        let (payload, headers) = rp::rebuild(line, tag);
        let body = json::stringify(&payload);
        let mut hs: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), jsstr::to_string_lossy(&render::js_string(v)))).collect();
        hs.push(("X-Bellwether-Token".into(), token.into()));
        hs.push(("Content-Type".into(), "application/json".into()));
        let refs: Vec<(&str, &str)> = hs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let sent_at = crate::now_ms();
        let t = Instant::now();
        match client::request(port, "POST", "/hook", &refs, Some(body.as_bytes()), Duration::from_secs(5)) {
            Ok(r) => {
                latencies.push(t.elapsed().as_secs_f64() * 1000.0);
                if r.status != 204 || !r.body.is_empty() {
                    failures.lock().unwrap().push(format!("line {}: HTTP {}{}", i + 1, r.status, if r.body.is_empty() { String::new() } else { format!(" with a {}-byte body", r.body.len()) }));
                }
            }
            Err(Failure::Refused) => {
                polling.store(false, std::sync::atomic::Ordering::Relaxed);
                return Err(BrainError::new(format!("brain not running — run `bellwether serve` (nothing listening on {BIND_HOST}:{port})"), exit::NOT_RUNNING));
            }
            Err(f) => failures.lock().unwrap().push(format!("line {}: {}", i + 1, f.code())),
        }
        if few {
            let s = |k: &str| n.get(k).as_str().map(|v| format!(" {}", jsstr::to_string_lossy(v))).unwrap_or_default();
            err_line(&format!("  {:>4}  {sent_at}  {}{}{}", i + 1, jsstr::to_string_lossy(&render::js_string(n.get("event"))), s("toolName"), s("notificationType")));
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    std::thread::sleep(Duration::from_millis(600));
    polling.store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = poller.join();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| latencies.get(((p / 100.0) * latencies.len() as f64).floor().min(latencies.len().saturating_sub(1) as f64) as usize).copied().unwrap_or(f64::NAN);
    let snaps = snapshots.lock().unwrap().clone();
    let mut max_per_second = 0;
    for i in 0..snaps.len() {
        let j = snaps[i..].iter().take_while(|t| t.duration_since(snaps[i]) < Duration::from_secs(1)).count();
        max_per_second = max_per_second.max(j);
    }
    let failures = failures.lock().unwrap().clone();
    Ok(ReplayResult { elapsed, p50: pct(50.0), p99: pct(99.0), max: latencies.last().copied().unwrap_or(f64::NAN), failures, snapshots: snaps.len(), max_per_second })
}

// ------------------------------------------------------------------ token

const TOKEN_USAGE: &str = "usage: bellwether token init [--from <.env file>] [--force] | bellwether token rotate [--write] [--settings <path>]";

fn token(args: &[String]) -> CmdResult {
    let Some(sub) = args.first().filter(|s| !s.starts_with("--")) else { return Err(bad(TOKEN_USAGE)) };
    let rest = &args[1..];
    let home = home::dir();
    match sub.as_str() {
        "init" => {
            let a = cliargs::parse(rest, &["force"], &["from"], 0)?;
            let r = tok::init(&home, a.value("from").map(user_path).as_deref(), a.has("force"))?;
            out(&format!("{}\n", r.message));
            Ok(if r.unusable { exit::BAD_CONFIG } else { exit::OK })
        }
        "rotate" => rotate(&home, rest),
        _ => Err(bad(TOKEN_USAGE)),
    }
}

/// `token rotate [--write]` (plan D3, rotation): a new token file, the running brain told
/// to switch (it accepts the old token for one more minute), then bellwether's hook entries
/// in the settings file rewritten with the new token (`hooks install`, the same exact-URL
/// ownership, backup and hash check). The brain is told first, so the entries carry a
/// token it accepts at every moment. A dry run shows the settings diff with the new token
/// as `<new token>`.
fn rotate(home: &Path, args: &[String]) -> CmdResult {
    use bellwether_core::hooks::{self as installer, Action};
    let a = cliargs::parse(args, &["write"], &["settings"], 0)?;
    let config = config(home)?;
    let port = config.port();
    if env_token().is_some_and(|t| !t.is_empty()) {
        return Err(bad("the token in use comes from BELLWETHER_HOOK_TOKEN in the environment; unset it to rotate the token file"));
    }
    let file = tok::file(home);
    let Some(old) = tok::read_file(home)? else {
        return Err(bad(format!("no token to rotate: {} doesn't exist. `bellwether token init` makes one", file.display())));
    };
    let _ = TOKEN.set(old.clone());
    let settings = a.value("settings").map(user_path).unwrap_or_else(outfile::user_settings);
    // The settings file is read and planned before anything is written: one that can't be
    // rewritten stops the rotation with nothing changed.
    let ours = |plan: &installer::Plan| installer::sum_groups(&installer::count_groups(&plan.before, port)) > 0;
    let preview = installer::plan_edit(&settings, Action::Install, port, "<new token>", Some(home))?;
    let rewrite = ours(&preview);
    if !a.has("write") {
        let mut text = format!("would write a new hook token to {} and ask the brain on {BIND_HOST}:{port} to switch to it (the old one keeps working for a minute).\n", file.display());
        if rewrite {
            text.push_str(&format!("would rewrite bellwether's hook entries in {} with it:\n", settings.display()));
            text.push_str(&bellwether_core::diff::unified_diff(&preview.old_text, &preview.new_text, &format!("{} (current)", settings.display()), &format!("{} (after token rotate)", settings.display()), 3));
        } else {
            text.push_str(&format!("{} has no bellwether hook entries on port {port}; nothing to rewrite there.\n", settings.display()));
        }
        text.push_str("Nothing was written; re-run with --write to do it.\n");
        out(&text);
        return Ok(exit::OK);
    }
    let new = tok::generate()?;
    let mut plan = if rewrite { Some(installer::plan_edit(&settings, Action::Install, port, &new, Some(home))?) } else { None };
    tok::write(home, &new)?;
    // The entries change only once the brain on the port has switched, or when no brain
    // runs at all (it reads the token file when it starts). A brain that answers but
    // doesn't switch (the repository's Node brain has no `reload-token`) would refuse
    // every rewritten entry: put the old token back and stop.
    let (switched, brain) = match tell_brain(port, &old) {
        Switch::Switched => (true, format!("the brain on {BIND_HOST}:{port} switched to it and accepts the old one for another minute")),
        Switch::NoBrain => (false, format!("no brain answered on {BIND_HOST}:{port}; it reads the new token when it starts")),
        Switch::Refused(why) => {
            let back = match tok::write(home, &old) {
                Ok(_) => format!("The token file is back to the old token and {} was not touched, so nothing changed", settings.display()),
                Err(e) => format!("Putting the old token back into {} failed too ({}): run `bellwether token rotate --write` again once the brain is stopped", file.display(), e.message),
            };
            return Err(BrainError::new(
                format!("the brain on {BIND_HOST}:{port} didn't switch to the new token ({why}). {back}. A brain without `reload-token` (the repository's Node brain) can't follow a rotation: stop it first, or rotate the token it reads"),
                exit::UNEXPECTED,
            ));
        }
    };
    let mut text = format!("wrote a new hook token to {} ({}; the value isn't shown); {brain}\n", file.display(), bellwether_core::acl::PRIVATE);
    if let Some(plan) = plan.as_mut() {
        match installer::apply_plan(plan, &home.join("backups"), Some(home), &file_stamp(crate::now_ms())) {
            Ok(backup) => {
                text.push_str(&format!("rewrote bellwether's hook entries in {} with the new token", settings.display()));
                if let Some(b) = backup {
                    text.push_str(&format!("; backup of the previous file: {}", b.display()));
                }
                text.push('\n');
                for w in &plan.warnings {
                    err_line(&format!("warning: {w}"));
                }
            }
            Err(e) => {
                // The entries still carry the old token: the token file and the brain go back
                // to it, so all three agree again.
                out(&text);
                let file_back = tok::write(home, &old);
                let brain_back = if !switched {
                    Switch::NoBrain
                } else {
                    tell_brain(port, &new)
                };
                let state = match (&file_back, &brain_back) {
                    (Ok(_), Switch::Switched) => "The token file and the brain are back on the old token; nothing else changed".to_string(),
                    (Ok(_), Switch::NoBrain) if !switched => "The token file is back on the old token; nothing else changed".to_string(),
                    (Ok(_), Switch::NoBrain) => format!("The token file is back on the old token, but the brain on {BIND_HOST}:{port} stopped answering: it reads the old token when it starts again"),
                    (Ok(_), Switch::Refused(why)) => format!("The token file is back on the old token, but the brain on {BIND_HOST}:{port} didn't switch back ({why}): restart it within a minute, or the hooks get 403"),
                    (Err(fe), _) => format!("Putting the old token back into {} failed ({}): the hook entries carry the old token and the brain the new one. Run `bellwether hooks install --write` now", file.display(), fe.message),
                };
                return Err(BrainError::new(format!("{} {state}.", e.message), e.code));
            }
        }
    } else {
        text.push_str(&format!("{} has no bellwether hook entries on port {port}; nothing to rewrite there.\n", settings.display()));
    }
    out(&text);
    Ok(exit::OK)
}

/// What the brain on the port did with `reload-token`.
pub enum Switch {
    Switched,
    /// Nothing listens on the port.
    NoBrain,
    /// Something answered, and didn't switch; why.
    Refused(String),
}

/// Posts `reload-token` with `token`, and says what came of it.
pub fn tell_brain(port: u16, token: &str) -> Switch {
    let body = json::stringify(&Js::Obj(Obj::new().with("action", "reload-token")));
    match client::request(port, "POST", "/control", &[("X-Bellwether-Token", token), ("Content-Type", "application/json")], Some(body.as_bytes()), Duration::from_secs(5)) {
        Err(Failure::Refused) => Switch::NoBrain,
        Err(f) => Switch::Refused(format!("it didn't answer: {}", f.code())),
        Ok(r) => {
            let v = json::parse(&r.text()).unwrap_or(Js::Null);
            if r.status == 200 && v.get("ok").truthy() {
                Switch::Switched
            } else {
                let why = v.get("error").as_str().map(jsstr::to_string_lossy).unwrap_or_default();
                Switch::Refused(format!("it answered {}{}", r.status, if why.is_empty() { String::new() } else { format!(": {why}") }))
            }
        }
    }
}

// ------------------------------------------------------------------ hooks

/// The CLI's own wording of a settings path: as given, else the default file.
fn settings_arg(a: &cliargs::Args) -> PathBuf {
    a.value("settings").map(user_path).unwrap_or_else(outfile::user_settings)
}

/// `bellwether hooks print [--out <file> [--force]] | install | uninstall [--write]
/// [--settings <path>] [--port <old port>]` (brain/cli.mjs `hooks`).
fn hooks(args: &[String]) -> CmdResult {
    use bellwether_core::hooks::{self as installer, Action};
    let a = cliargs::parse(args, &["write", "force"], &["settings", "out", "port"], 1)?;
    let sub = a.positional.first().map(String::as_str).unwrap_or("");
    let home = home::dir();
    let config = config(&home)?;
    // --port manages entries on another port (after a port change). Ownership is the exact
    // URL http://127.0.0.1:<port>/hook, so entries on the old port are untouched until
    // named this way.
    let mut port = config.port();
    if let Some(v) = a.value("port") {
        let n = js_number(v);
        if n.fract() != 0.0 || !(1024.0..=65535.0).contains(&n) || json::number_to_string(n) != v {
            return Err(bad("--port takes an integer from 1024 to 65535"));
        }
        port = n as u16;
        if sub != "uninstall" {
            return Err(bad("--port is only for `hooks uninstall` (to remove entries left on an old port); change \"port\" in config.json to move the install"));
        }
    }
    if sub == "print" {
        if a.has("write") || a.value("settings").is_some() {
            return Err(bad("`hooks print` takes only --out <file> [--force]"));
        }
        if a.has("force") && a.value("out").is_none() {
            return Err(bad("--force only applies to `hooks print --out <file>`"));
        }
        let (token, _) = read_token(&home)?;
        let text = bellwether_core::lossless::stringify(&installer::hook_block(port, &token), "  ") + "\n";
        let Some(o) = a.value("out") else {
            out(&text);
            return Ok(exit::OK);
        };
        // The block holds the token, so the file is private to its owner (0600).
        let target = user_path(o);
        outfile::guard_print(&target, a.has("force"))?;
        let fail = |e: std::io::Error| BrainError::new(format!("cannot write {}: {}", target.display(), bellwether_core::config::io_code(&e)), exit::UNEXPECTED);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        installer::write_replace(&target, text.as_bytes(), 0o600).map_err(fail)?;
        installer::set_mode(&target, 0o600).map_err(fail)?;
        // Windows (W2): the user's only, as 0600 is on Unix (best effort).
        #[cfg(windows)]
        let _ = bellwether_core::acl::make_private(&target, false);
        err_line(&format!("wrote {} ({}). Try it with: claude --settings {}", target.display(), bellwether_core::acl::PRIVATE, target.display()));
        return Ok(exit::OK);
    }
    let action = match sub {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        _ => return Err(BrainError::new("usage: bellwether hooks print [--out <file>] | install | uninstall [--write] [--settings <path>]", exit::UNEXPECTED)),
    };
    if a.value("out").is_some() || a.has("force") {
        return Err(bad(format!("`hooks {sub}` takes --settings and --write, not --out or --force")));
    }
    let file = settings_arg(&a);
    let token = match action {
        Action::Install => read_token(&home)?.0,
        Action::Uninstall => {
            remember_token(&home);
            String::new()
        }
    };
    // The install record belongs to the configured port's install; an uninstall of
    // old-port leftovers neither reads nor rewrites it.
    let record_dir = (port == config.port()).then_some(home.as_path());
    let mut plan = installer::plan_edit(&file, action, port, &token, record_dir)?;
    for w in &plan.warnings {
        err_line(&format!("warning: {w}"));
    }
    let shown = plan.warnings.len();
    let after = installer::sum_groups(&installer::count_groups(&plan.after, port));
    if !plan.changed {
        err_line(&match action {
            Action::Install => format!("already installed, no changes ({after} bellwether matcher groups in {})", file.display()),
            Action::Uninstall => format!("nothing to remove: no bellwether entries in {}", file.display()),
        });
        return Ok(exit::OK);
    }
    out(&hook_diff(&plan));
    let backups = home.join("backups");
    let summary = hook_summary(&plan, port, &backups);
    if !a.has("write") {
        err_line(&format!("\ndry run: {summary}\nNothing was written. Re-run with --write to apply."));
        return Ok(exit::OK);
    }
    let backup = installer::apply_plan(&mut plan, &backups, record_dir, &file_stamp(crate::now_ms()))?;
    err_line(&format!("\nwritten: {summary}{}", backup.map(|b| format!("\nbackup of the previous file: {}", b.display())).unwrap_or_default()));
    for w in &plan.warnings[shown..] {
        err_line(&format!("warning: {w}"));
    }
    Ok(exit::OK)
}

/// The diff `hooks install|uninstall` prints for a plan (unmasked: the CLI's `out` masks it,
/// and the onboarding window masks it the same way), so the window shows exactly what the
/// command shows for the same file.
pub fn hook_diff(plan: &bellwether_core::hooks::Plan) -> String {
    let file = plan.file.display();
    bellwether_core::diff::unified_diff(&plan.old_text, &plan.new_text, &format!("{file} (current)"), &format!("{file} (after {})", plan.action.word()), 3)
}

/// The summary after the diff ("bellwether matcher groups 0 -> 16 …"), without the dry run's
/// or the write's first words.
pub fn hook_summary(plan: &bellwether_core::hooks::Plan, port: u16, backups: &Path) -> String {
    use bellwether_core::hooks::{self as installer, EVENTS};
    let before = installer::sum_groups(&installer::count_groups(&plan.before, port));
    let after = installer::sum_groups(&installer::count_groups(&plan.after, port));
    format!(
        "bellwether matcher groups {before} -> {after} ({} events subscribed); {}",
        EVENTS.len(),
        if plan.layout_kept {
            "every other line of the file is unchanged.".to_string()
        } else {
            format!("every other entry keeps its value, BUT this file is not laid out the way bellwether writes JSON (spacing, inline arrays or mixed line endings), so the write also reformats existing lines, as the diff shows. An uninstall afterwards will not restore the original bytes; the backup in {}/ keeps them.", backups.display())
        }
    )
}

// ------------------------------------------------------------------ pets

const PETS_USAGE: &str = "usage: bellwether pets import <animations.xml> [--name <name>] [--force]";

/// `bellwether pets import <file> [--name <name>] [--force]` (plan D8).
fn pets(args: &[String]) -> CmdResult {
    let Some(sub) = args.first().filter(|s| !s.starts_with("--")) else { return Err(bad(PETS_USAGE)) };
    if sub == "fetch" {
        return Err(bad("`pets fetch` isn't in the app: it downloads the eSheep art, which the app doesn't redistribute. Import a pet file you have with `bellwether pets import <animations.xml>`"));
    }
    if sub != "import" {
        return Err(bad(PETS_USAGE));
    }
    let a = cliargs::parse(&args[1..], &["force"], &["name"], 1)?;
    let Some(file) = a.positional.first() else { return Err(bad(PETS_USAGE)) };
    let home = home::dir();
    let r = crate::pets::import(&home, &user_path(file), a.value("name"), a.has("force"))?;
    // Names come from the file: printed with control characters replaced, line by line, so
    // the layout's own line breaks stay (JSON already escapes C0 controls inside strings).
    let map = json::stringify_pretty(&Js::Obj(r.map.clone()), "  ").lines().map(crate::pets::printable).collect::<Vec<_>>().join("\n");
    out(&format!(
        "{} {} ({}, {}×{} tiles, {} animations) to {}, pinned in {} (sha256 {}…).\nThe strip draws it with \"pets\": {{ \"default\": \"{}\" }} in config.json; a proposed \"animationMap\" entry for it, by name matching:\n\"{}\": {map}\n",
        if r.replaced { "replaced" } else { "imported" },
        r.name,
        if r.pet.title.is_empty() { "untitled".to_string() } else { crate::pets::printable(&r.pet.title) },
        r.pet.tilesx,
        r.pet.tilesy,
        r.pet.animations.len(),
        r.file.display(),
        crate::pets::lock_file(&home).display(),
        &r.sha256[..12],
        r.name,
        r.name
    ));
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stamps_are_iso_with_dashes() {
        assert_eq!(file_stamp(0), "1970-01-01T00-00-00-000Z");
        assert_eq!(file_stamp(1_791_960_067_123), "2026-10-14T06-41-07-123Z");
        assert_eq!(file_stamp(951_782_400_000), "2000-02-29T00-00-00-000Z");
    }

    #[test]
    fn ports() {
        assert_eq!(port_arg("47418").unwrap(), 47418);
        assert_eq!(port_arg("0").unwrap(), 0);
        for bad in ["80", "x", "70000", "1.5", "-1", "047418", "+47418", " 47418", "4.7418e4"] {
            assert!(port_arg(bad).is_err(), "{bad}");
        }
    }
}
