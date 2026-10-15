//! `bellwether doctor`, the brain's half (distribution plan, milestone 4): every check that
//! doesn't need the app bundle, ported from brain/doctor.mjs. The app's own checks
//! (identity, signature, login items, Accessibility) live with the shell
//! (shell/src-tauri/src/cli.rs), which runs [`checks`], adds its lines, and ends with
//! [`finish`] (the `next` line, `--e2e`, the exit code).
//!
//! What changed from the Node doctor (test/DIVERGENCES.md): no `node`, `cargo`, `old
//! names` or LaunchAgent `autostart` lines (the app has no Node, builds nothing, reads no
//! `PASTURE_*` names, and starts at login through SMAppService); new lines for the token
//! file's mode, legacy LaunchAgents, `onFailure: "block"`, obsolete config keys and the
//! strip's WebKit switch; entries with `X-Pasture-*` headers FAIL, because this brain
//! refuses them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bellwether_core::config::{Config, ConfigError};
use bellwether_core::hooks::hook_block;
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::lossless::{self, Lj};
use bellwether_core::settings_checks::{self, Check, BAD_CONFIG, NOT_RUNNING, OK, UNEXPECTED};
use bellwether_core::{jsstr, token as tok};

use crate::client::{self, Failure};
use crate::{BIND_HOST, DOCTOR_PING};

/// A refused request older than this is history, not a problem (INFO, not WARN).
pub const REJECTION_RECENT_MS: f64 = 15.0 * 60.0 * 1000.0;

/// The four LaunchAgent labels of the repository versions (plan D5).
pub const LEGACY_LABELS: [&str; 4] = ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.brain", "local.pasture.shell"];

/// Where Claude Code reads managed (policy) settings on this OS.
pub fn managed_settings() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"))
    } else if cfg!(windows) {
        Some(PathBuf::from("C:\\ProgramData\\ClaudeCode\\managed-settings.json"))
    } else {
        Some(PathBuf::from("/etc/claude-code/managed-settings.json"))
    }
}

/// What doctor looks at.
pub struct Input {
    pub settings_file: PathBuf,
    /// The app's files (BELLWETHER_HOME).
    pub home: PathBuf,
    /// The user's home directory (for ~/Library/LaunchAgents).
    pub user_home: PathBuf,
    /// BELLWETHER_HOOK_TOKEN, if set.
    pub env_token: Option<String>,
    pub config: Result<Config, ConfigError>,
    /// `--port`: check a brain on another port than the config's.
    pub port: Option<u16>,
    pub probe_timeout: Duration,
    /// Project and local settings are `<project>/.claude/settings*.json`.
    pub project_dir: Option<PathBuf>,
    pub managed_file: Option<PathBuf>,
    pub e2e: bool,
    /// The strip's sharing-name warnings for this config (computed by the shell, which
    /// owns those rules), and how many names there are.
    pub sharing: Option<(Vec<String>, usize)>,
}

impl Input {
    /// The port the brain is checked on.
    pub fn port(&self) -> Option<u16> {
        self.port.or_else(|| self.config.as_ref().ok().map(Config::port))
    }
}

pub struct Report {
    pub checks: Vec<Check>,
    pub exit_code: i32,
    pub settings_file: PathBuf,
}

/// The token for the checks: Ok(token) when usable.
pub fn token_of(input: &Input) -> Result<(String, tok::Source), ConfigError> {
    tok::read(&input.home, input.env_token.clone())
}

/// Every brain-side check, in the Node doctor's order, before `next`.
pub fn checks(input: &Input) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    let add = |out: &mut Vec<Check>, name: &str, status: &'static str, detail: String, code: i32| out.push(Check::new(name, status, detail, code));

    match &input.config {
        Ok(c) => {
            let detail = match &c.file {
                Some(f) => f.display().to_string(),
                None => format!("no config.json in {}: the defaults (every key is optional)", input.home.display()),
            };
            add(&mut out, "config", "PASS", detail, BAD_CONFIG);
            let obsolete = obsolete_keys(c);
            if !obsolete.is_empty() {
                add(&mut out, "config keys", "WARN", format!("{} {} obsolete in the app and ignored: {}", obsolete.join(", "), if obsolete.len() == 1 { "is" } else { "are" }, "the app has no Node runtime (brain.nodePath) and starts at login through System Settings (autostart)"), OK);
            }
        }
        Err(e) => add(&mut out, "config", "FAIL", e.message.clone(), BAD_CONFIG),
    }

    let token = match token_of(input) {
        Ok((t, tok::Source::Env)) => {
            add(&mut out, "token", "PASS", format!("from BELLWETHER_HOOK_TOKEN in the environment ({} chars)", t.len()), BAD_CONFIG);
            Some(t)
        }
        Ok((t, tok::Source::File(f))) => {
            out.push(token_file_mode(&f));
            Some(t)
        }
        Err(e) => {
            // Say where the bad token came from: the environment, or the token file.
            let from_env = input.env_token.as_deref().is_some_and(|t| !t.is_empty());
            add(&mut out, "token", "FAIL", if from_env { format!("BELLWETHER_HOOK_TOKEN in the environment: {}", e.message) } else { e.message }, BAD_CONFIG);
            None
        }
    };

    add(&mut out, "claude", "INFO", claude_version(), OK);

    let Some(port) = input.port() else { return out };
    let config = input.config.as_ref().ok();
    brain_checks(port, token.as_deref(), input.probe_timeout, &mut out);

    let settings = settings_checks::check_settings_file(&input.settings_file, port, token.as_deref());
    let installed = settings.iter().any(|c| c.name == "hooks installed" && c.found.unwrap_or(0) > 0);
    out.extend(settings);
    let down = out.iter().any(|c| c.name == "brain" && c.status == "FAIL" && c.detail.starts_with("brain not running"));
    if installed && down {
        add(&mut out, "hook errors", "WARN", "hooks are installed but the brain is not answering: every Claude Code session prints a visible \"<Event> hook error … ECONNREFUSED\" line on each prompt and tool call until a brain runs (`bellwether autostart install --write` starts one at login; `bellwether serve` runs one now)".into(), OK);
    }
    out.extend(settings_checks::check_other_levels(input.project_dir.as_deref(), input.managed_file.as_deref(), port, None));

    if let Some(c) = config {
        if c.palette_upgraded {
            add(&mut out, "palette", "INFO", "config.json has the old default sheep colours (white and pale blue first, hard to tell apart), so the new default palette is used. Your config.json is unchanged; to keep the old colours, change any one of them".into(), OK);
        }
    }
    if let Some((warnings, count)) = &input.sharing {
        if warnings.is_empty() {
            add(&mut out, "sharing names", "PASS", format!("{count} process name(s) that hide the strip while sharing"), OK);
        } else {
            add(&mut out, "sharing names", "WARN", format!("{}. The strip won't hide for these while you share", warnings.join("; ")), OK);
        }
    }
    if bellwether_core::shellstate::read(&input.home).0 {
        add(&mut out, "strip", "INFO", "the strip is hidden (the tray's \"Hide the strip\", or `bellwether hide`); `bellwether show` brings it back".into(), OK);
    }
    let log = shell_log(&input.home);
    out.extend(check_locator_log(log.as_deref()));
    if let Some(c) = webkit_check(log.as_deref()) {
        out.push(c);
    }
    if let Some(c) = config {
        out.push(pets_check(&input.home, c));
    }
    let legacy = legacy_agents(&input.user_home);
    if !legacy.is_empty() {
        add(
            &mut out,
            "legacy agents",
            "WARN",
            format!(
                "{} in {}: the repository version's login items. Two brains on one port fight over it; `bellwether migrate` (the app's migration, milestone 8) moves them aside",
                legacy.join(", "),
                input.user_home.join("Library").join("LaunchAgents").display()
            ),
            OK,
        );
    }
    out
}

/// The `next` line, `--e2e`, and the exit code: 3 if config, token or a settings file is
/// wrong; 2 if the brain isn't running or the hooks aren't installed; 1 for any other
/// failure; 0 when nothing failed.
pub fn finish(input: &Input, mut checks: Vec<Check>) -> Report {
    if input.config.is_ok() {
        let installed = checks.iter().any(|c| c.name == "hooks installed" && c.found.unwrap_or(0) > 0);
        let next = next_step(&checks, installed);
        checks.push(Check::new("next", "INFO", next, OK));
    }
    if input.e2e {
        let healthy = checks.iter().any(|c| c.name == "brain token" && c.status == "PASS");
        let token = token_of(input).ok().map(|(t, _)| t);
        match (healthy, input.port(), token) {
            (true, Some(port), Some(token)) => {
                let (ok, detail) = run_e2e(port, &token, input.probe_timeout);
                checks.push(Check::new("e2e", if ok { "PASS" } else { "FAIL" }, detail, UNEXPECTED));
            }
            _ => checks.push(Check::new("e2e", "FAIL", "skipped: needs a running brain that accepts the token (see the checks above)", NOT_RUNNING)),
        }
    }
    let failed: Vec<&Check> = checks.iter().filter(|c| c.status == "FAIL").collect();
    let exit_code = if failed.iter().any(|c| c.exit_code == BAD_CONFIG) {
        BAD_CONFIG
    } else if failed.iter().any(|c| c.exit_code == NOT_RUNNING) {
        NOT_RUNNING
    } else if failed.is_empty() {
        OK
    } else {
        UNEXPECTED
    };
    Report { checks, exit_code, settings_file: input.settings_file.clone() }
}

/// The report as `doctor` prints it.
pub fn render(r: &Report) -> String {
    let mut s = format!("bellwether doctor (settings: {})\n", r.settings_file.display());
    for c in &r.checks {
        s.push_str(&format!("  {:<4}  {:<22} {}\n", c.status, c.name, c.detail));
    }
    let fails = r.checks.iter().filter(|c| c.status == "FAIL").count();
    if fails > 0 {
        s.push_str(&format!("\n{fails} check(s) failed (exit {}).\n", r.exit_code));
    } else {
        s.push_str("\nAll checks passed.\n");
    }
    s
}

/// The report as `doctor --json` prints it: `JSON.stringify(result, null, 2)`.
pub fn to_json(r: &Report) -> String {
    let checks: Vec<Js> = r
        .checks
        .iter()
        .map(|c| {
            let mut o = Obj::new().with("name", jsstr::from_str(&c.name)).with("status", c.status).with("detail", jsstr::from_str(&c.detail)).with("exitCode", c.exit_code as f64);
            if let Some(n) = c.found {
                o.set("found", n as f64);
            }
            Js::Obj(o)
        })
        .collect();
    let v = Js::Obj(Obj::new().with("checks", Js::Arr(checks)).with("exitCode", r.exit_code as f64).with("settingsFile", jsstr::from_str(&r.settings_file.display().to_string())));
    json::stringify_pretty(&v, "  ") + "\n"
}

/// The one thing to do next, in the order that keeps a brain running whenever hooks are
/// installed: config, token, a brain, anything else failing, then hooks.
pub fn next_step(checks: &[Check], installed: bool) -> String {
    let status = |n: &str| checks.iter().find(|c| c.name == n).map(|c| c.status);
    if status("config") == Some("FAIL") {
        return "fix config.json (the config line above says what is wrong)".into();
    }
    if status("token") == Some("FAIL") {
        return "make the hook token: `bellwether token init`".into();
    }
    if status("brain") != Some("PASS") {
        return "start the brain: at login with `bellwether autostart install --write`, or now with `bellwether serve`".into();
    }
    if let Some(f) = checks.iter().find(|c| c.status == "FAIL" && !["config", "token", "brain", "e2e"].contains(&c.name.as_str())) {
        return format!("fix \"{}\": {}", f.name, f.detail);
    }
    if !installed {
        return "install the hooks: `bellwether hooks install` (preview), then the same with --write; then open a new Claude Code session".into();
    }
    if checks.iter().any(|c| c.status == "FAIL") {
        return "fix the FAIL lines above first".into();
    }
    "all set: open a Claude Code session and its sheep appears above the Dock".into()
}

/// Config keys the app no longer reads (plan, Configuration).
fn obsolete_keys(c: &Config) -> Vec<String> {
    let Some(file) = &c.file else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let Ok(v) = json::parse(&text) else { return Vec::new() };
    let mut out = Vec::new();
    if !v.get("brain").get("nodePath").is_undefined() {
        out.push("brain.nodePath".to_string());
    }
    if !v.get("autostart").is_undefined() {
        out.push("autostart".to_string());
    }
    out
}

/// The token file's mode: only its owner may read it.
pub fn token_file_mode(file: &Path) -> Check {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(file) {
            Ok(m) if m.permissions().mode() & 0o077 == 0 => Check::new("token", "PASS", format!("{} (0600)", file.display()), BAD_CONFIG),
            Ok(m) => Check::new("token", "FAIL", format!("{} is mode {:o}: other users can read it. Run `chmod 600 '{}'`", file.display(), m.permissions().mode() & 0o777, file.display()), BAD_CONFIG),
            Err(e) => Check::new("token", "FAIL", format!("{}: {e}", file.display()), BAD_CONFIG),
        }
    }
    #[cfg(not(unix))]
    Check::new("token", "PASS", file.display().to_string(), BAD_CONFIG)
}

fn claude_version() -> String {
    let child = std::process::Command::new("claude").arg("--version").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn();
    let Ok(mut child) = child else { return "claude not found on PATH (fine for the brain; needed for --e2e)".into() };
    match wait_output(&mut child, Duration::from_secs(5)) {
        Some((0, out)) => out.trim().to_string(),
        _ => "claude not found on PATH (fine for the brain; needed for --e2e)".into(),
    }
}

/// Waits for a child with a deadline; kills it (by its own pid) when it runs over.
fn wait_output(child: &mut std::process::Child, limit: Duration) -> Option<(i32, String)> {
    use std::io::Read;
    let deadline = Instant::now() + limit;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some((status.code().unwrap_or(-1), reader.join().unwrap_or_default())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// What answers /health on the port.
pub enum Health {
    Bellwether(serde_json::Value),
    Refused,
    Timeout,
    Other(String),
}

pub fn probe_health(port: u16, timeout: Duration) -> Health {
    match client::request(port, "GET", "/health", &[], None, timeout) {
        Ok(r) => {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or(serde_json::Value::Null);
            if body["name"] == "bellwether" && body["ok"].as_bool().unwrap_or(false) {
                Health::Bellwether(body)
            } else {
                Health::Other(format!("HTTP {} that isn't bellwether's /health", r.status))
            }
        }
        Err(Failure::Refused) => Health::Refused,
        Err(Failure::Code(c)) if c == "ETIMEDOUT" => Health::Timeout,
        Err(f) => Health::Other(f.code()),
    }
}

/// The pid and command name listening on the port, from `lsof` (read only).
fn port_owner(port: u16) -> String {
    if cfg!(windows) {
        return String::new();
    }
    let out = std::process::Command::new("lsof").args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
    let Ok(out) = out else { return String::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text.lines().find_map(|l| l.strip_prefix('p'));
    let cmd = text.lines().find_map(|l| l.strip_prefix('c'));
    match pid {
        Some(p) => format!(" (pid {p}, {})", cmd.unwrap_or("?")),
        None => String::new(),
    }
}

/// The executable of a process, from `ps` (read only), to say which brain answers.
fn process_path(pid: u64) -> Option<String> {
    if pid == 0 || cfg!(windows) {
        return None;
    }
    let out = std::process::Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn local_time(ms: f64) -> String {
    // Seconds since midnight in the local zone, from libc where there is one.
    #[cfg(unix)]
    {
        let secs = (ms / 1000.0) as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
        }
    }
    let s = (ms / 1000.0) as u64 % 86_400;
    format!("{:02}:{:02}:{:02} UTC", s / 3600, s % 3600 / 60, s % 60)
}

/// /health, which brain answers it, recent refusals, and whether /hook takes the token
/// (doctor's ping, which the brain counts but never shows).
fn brain_checks(port: u16, token: Option<&str>, timeout: Duration, out: &mut Vec<Check>) {
    let ms = timeout.as_millis();
    match probe_health(port, timeout) {
        Health::Refused => out.push(Check::new(
            "brain",
            "FAIL",
            format!("brain not running — run `bellwether serve` (nothing listening on {BIND_HOST}:{port}). With start at login on, launchd starts it: `bellwether autostart status`"),
            NOT_RUNNING,
        )),
        Health::Timeout => out.push(Check::new("brain", "FAIL", format!("something on {BIND_HOST}:{port} accepted the connection but didn't answer /health within {ms} ms (a brain started with --hang, or a hung process{})", port_owner(port)), NOT_RUNNING)),
        Health::Other(detail) => out.push(Check::new("brain", "FAIL", format!("port {port} is held by another process{}; it answered {detail}. Stop it, or set a different \"port\" in config.json and re-run `bellwether hooks install`", port_owner(port)), NOT_RUNNING)),
        Health::Bellwether(h) => {
            let pid = h["pid"].as_u64().unwrap_or(0);
            if h["mode"] == "hang" {
                out.push(Check::new("brain", "FAIL", format!("pid {pid} is running with the debug flag --hang: /hook never answers, so every hook stalls for its full timeout. Restart it as plain `bellwether serve`"), NOT_RUNNING));
                return;
            }
            let which = process_path(pid).map(|p| if p.ends_with("node") { format!("the Node brain ({p})") } else { format!("the Rust brain ({p})") }).unwrap_or_else(|| "a brain".into());
            let accepted = h["counters"]["accepted"].as_u64().unwrap_or(0);
            let last = h["lastEventAt"].as_f64().map(|t| format!(", last {}", local_time(t))).unwrap_or_default();
            out.push(Check::new("brain", "PASS", format!("pid {pid}, v{}, mode {}, {accepted} events accepted{last}: {which}", h["version"].as_str().unwrap_or("?"), h["mode"].as_str().unwrap_or("?")), OK));
            let rejected: Vec<(String, u64)> = h["counters"]["rejected"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_u64().filter(|n| *n > 0).map(|n| (k.clone(), n))).collect()).unwrap_or_default();
            if !rejected.is_empty() {
                let last = h["counters"]["lastRejectedAt"].as_f64();
                let recent = last.is_none_or(|t| (crate::now_ms() as f64) - t < REJECTION_RECENT_MS);
                let counts = rejected.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join(", ");
                if recent {
                    out.push(Check::new("rejections", "WARN", format!("the brain has refused requests: {counts}{}. token=N usually means the settings file holds an old token: re-run `bellwether hooks install --write`", last.map(|t| format!(", the last at {}", local_time(t))).unwrap_or_default()), OK));
                } else {
                    out.push(Check::new("rejections", "INFO", format!("{counts} refused since the brain started, the last at {}; none in the last 15 minutes (the counts reset when the brain restarts)", local_time(last.unwrap_or(0.0))), OK));
                }
            }
            if let Some(token) = token {
                let body = format!("{{\"hook_event_name\":\"{DOCTOR_PING}\"}}");
                match client::request(port, "POST", "/hook", &[("Content-Type", "application/json"), ("X-Bellwether-Token", token)], Some(body.as_bytes()), timeout) {
                    Ok(r) if r.status == 204 => out.push(Check::new("brain token", "PASS", "the brain accepts the hook token in use (204, empty)", OK)),
                    Ok(r) if r.status == 403 => out.push(Check::new("brain token", "FAIL", "the brain refused the hook token in use (403): it was started with a different BELLWETHER_HOOK_TOKEN or token file. Restart it", BAD_CONFIG)),
                    Err(Failure::Code(c)) if c == "ETIMEDOUT" => out.push(Check::new("brain token", "FAIL", format!("/health answers but /hook didn't answer within {ms} ms: the brain is hung or unresponsive. Restart it"), NOT_RUNNING)),
                    Ok(r) => out.push(Check::new("brain token", "FAIL", format!("unexpected answer from /hook: {}", r.status), UNEXPECTED)),
                    Err(f) => out.push(Check::new("brain token", "FAIL", format!("unexpected answer from /hook: {}", f.code()), UNEXPECTED)),
                }
            }
        }
    }
}

/// The strip's log: `<logs>/shell.log`, else `<home>/shell.log`.
fn shell_log(home: &Path) -> Option<String> {
    let mut candidates = vec![crate::home::log_dir().join("shell.log"), home.join("shell.log")];
    candidates.dedup();
    for f in candidates {
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        let Ok(mut file) = std::fs::File::open(&f) else { continue };
        use std::io::{Read, Seek, SeekFrom};
        let len = meta.len().min(512 * 1024);
        let _ = file.seek(SeekFrom::Start(meta.len() - len));
        let mut buf = Vec::new();
        let _ = file.take(len).read_to_end(&mut buf);
        return Some(String::from_utf8_lossy(&buf).into_owned());
    }
    None
}

/// The strip's own lines ("<ms> bellwether-shell: <message>", never one the webview sent
/// through shell_log), from its last start; and all of them.
fn own_lines(text: &str) -> (Vec<&str>, usize) {
    let own: Vec<&str> = text
        .split('\n')
        .filter_map(|l| {
            let (ms, rest) = l.split_once(' ')?;
            if ms.is_empty() || !ms.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let m = rest.strip_prefix("bellwether-shell: ")?;
            (!m.starts_with("webview:")).then_some(m)
        })
        .collect();
    let start = own.iter().rposition(|m| m.starts_with("starting:")).unwrap_or(0);
    (own, start)
}

/// What the strip's locator ran into since it last started (brain/doctor.mjs checkLocatorLog).
pub fn check_locator_log(text: Option<&str>) -> Vec<Check> {
    let Some(text) = text else { return Vec::new() };
    let (own, start) = own_lines(text);
    let recent: Vec<&str> = own[start..].iter().copied().filter(|m| m.starts_with("locator: ")).collect();
    let mut out = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for m in &recent {
        if let Some(rest) = m.strip_prefix("locator: Automation for ") {
            if let Some((app, tail)) = rest.split_once(' ') {
                if tail.starts_with("was refused") && !app.is_empty() && !refused.iter().any(|r| r == app) {
                    refused.push(app.to_string());
                }
            }
        }
    }
    if !refused.is_empty() {
        out.push(Check::new("automation", "WARN", format!("the overlay was refused Automation for {0}: sheep go by the terminal app's frontmost window, and a click only brings {1} forward instead of selecting the tab. Allow it in System Settings > Privacy & Security > Automation (Bellwether > {2}), then restart the strip", refused.join(" and "), refused.join("/"), refused.join(", ")), OK));
    }
    let herdr = recent.iter().find_map(|m| m.strip_prefix("locator: herdr CLI not found at \"").and_then(|r| r.find('"').map(|i| format!("\"{}\"", &r[..i]))));
    let dock = own[start..].iter().rev().find(|m| m.starts_with("dock: Accessibility"));
    let granted_before = own[..start].iter().any(|m| m.starts_with("dock: Accessibility granted"));
    match dock {
        Some(d) if d.contains("not granted") && granted_before => out.push(Check::new("dock", "WARN", "Accessibility was granted to an earlier build of the strip and is not granted to this one, so the sheep use the full-width strip. System Settings > Privacy & Security > Accessibility still lists the old entry, which belongs to the previous build: remove it with −, then grant Bellwether again", OK)),
        Some(d) if d.contains("not granted") || d.contains("withdrawn") => out.push(Check::new("dock", "INFO", "the strip uses its full width; to have the sheep stand on the Dock itself, turn Bellwether on in System Settings > Privacy & Security > Accessibility", OK)),
        Some(_) => out.push(Check::new("dock", "PASS", "Accessibility granted: sheep stand on the Dock's top edge", OK)),
        None => {}
    }
    if let Some(path) = herdr {
        out.push(Check::new("herdr CLI", "WARN", format!("the overlay couldn't run herdr at {path} (locator.herdrPath), so sheep in herdr panes are placed and focused through iTerm2/Terminal only"), OK));
    }
    out
}

/// Whether the strip could switch WebKit's occlusion detection off (plan D4: a guarded
/// private call that a future macOS could remove).
pub fn webkit_check(text: Option<&str>) -> Option<Check> {
    let (own, start) = own_lines(text?);
    let line = own[start..].iter().rev().find(|m| m.starts_with("webview occlusion detection"))?;
    Some(if line.starts_with("webview occlusion detection disabled") {
        Check::new("webkit", "PASS", "the strip switched WebKit's occlusion detection off: sheep keep moving while the strip is on screen", OK)
    } else {
        Check::new("webkit", "WARN", "the strip could not switch WebKit's occlusion detection off (this macOS lacks the switch): the strip may be throttled and sheep may pause while it is on screen", OK)
    })
}

/// The pet the strip draws: an imported pet is checked against pets.user.lock.json.
fn pets_check(home: &Path, c: &Config) -> Check {
    let name = c.merged.get("pets").get("default").as_str().map(jsstr::to_string_lossy).unwrap_or_else(|| "esheep64".into());
    let file = home.join("pets").join(&name).join("animations.xml");
    if !file.exists() {
        return Check::new("pets", "INFO", format!("{name} is not an imported pet (no {}); the strip draws it from its own pets. `bellwether pets import <animations.xml>` imports one", file.display()), OK);
    }
    match crate::pets::verify(home, &name) {
        Ok(()) => Check::new("pets", "PASS", format!("{name}, imported, sha256 matches pets.user.lock.json"), OK),
        Err(why) => Check::new("pets", "WARN", why, OK),
    }
}

/// LaunchAgents of the repository versions still in ~/Library/LaunchAgents.
pub fn legacy_agents(user_home: &Path) -> Vec<String> {
    let dir = user_home.join("Library").join("LaunchAgents");
    LEGACY_LABELS.iter().filter(|l| dir.join(format!("{l}.plist")).exists()).map(|l| l.to_string()).collect()
}

/// `doctor --e2e`: one real, cheap `claude -p` run (haiku) with bellwether's hooks from a
/// throwaway settings file, in a temp directory, with user settings and MCP servers
/// excluded, so nothing of yours runs. Passes when the brain's accepted counter rises by
/// at least the three events such a run produces.
pub fn run_e2e(port: u16, token: &str, timeout: Duration) -> (bool, String) {
    let Health::Bellwether(before) = probe_health(port, timeout) else { return (false, "the brain stopped answering /health before the run".into()) };
    let dir = std::env::temp_dir().join(format!("bellwether-e2e-{}-{}", std::process::id(), crate::now_ms()));
    if let Err(e) = crate::home::create_private_dir(&dir) {
        return (false, format!("could not make a temp directory: {e}"));
    }
    let hooks_file = dir.join("hooks-only.json");
    let text = lossless::stringify(&hook_block(port, token), "  ") + "\n";
    if let Err(e) = bellwether_core::hooks::write_replace(&hooks_file, text.as_bytes(), 0o600) {
        let _ = std::fs::remove_dir_all(&dir);
        return (false, format!("could not write {}: {e}", hooks_file.display()));
    }
    let started = Instant::now();
    let result = run_claude(&dir, &hooks_file);
    let verdict = match result {
        Err(e) => (false, format!("could not run claude: {e}")),
        Ok((code, tail)) if code != 0 => {
            let last: Vec<&str> = tail.trim().split('\n').collect();
            let joined = last[last.len().saturating_sub(3)..].join(" | ");
            let cleaned = bellwether_core::redact::Cleaner::new(300, &[]).map(|c| jsstr::to_string_lossy(&c.clean(&jsstr::from_str(&joined)))).unwrap_or_default();
            (false, format!("claude -p exited {code}{}", if cleaned.is_empty() { " with nothing on stderr".to_string() } else { format!(": {cleaned}") }))
        }
        Ok(_) => {
            std::thread::sleep(Duration::from_millis(300));
            let after = match probe_health(port, timeout) {
                Health::Bellwether(b) => b["counters"]["accepted"].as_i64().unwrap_or(0),
                _ => 0,
            };
            let delta = after - before["counters"]["accepted"].as_i64().unwrap_or(0);
            let secs = started.elapsed().as_secs_f64();
            if delta >= 3 {
                (true, format!("a live `claude -p` (haiku) delivered {delta} hook events to the brain in {secs:.1} s"))
            } else {
                (false, format!("claude -p ran ({secs:.1} s) but the brain saw only {delta} new events (expected ≥ 3). Is Claude Code skipping HTTP hooks? Run with --debug-file to see"))
            }
        }
    };
    let _ = std::fs::remove_dir_all(&dir);
    verdict
}

/// Runs `claude -p` like a fresh terminal would (without the session markers of a parent
/// Claude Code), up to 120 s. Returns its exit code and the end of its output.
fn run_claude(cwd: &Path, hooks_file: &Path) -> Result<(i32, String), String> {
    let mut cmd = std::process::Command::new("claude");
    cmd.args(["-p", "Reply with the single word OK.", "--model", "haiku", "--settings"]).arg(hooks_file).args(["--setting-sources", "project,local", "--strict-mcp-config", "--no-session-persistence"]);
    cmd.current_dir(cwd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k == "CLAUDECODE" || k == "AI_AGENT" || k == "CLAUDE_PID" || k == "CLAUDE_EFFORT" || k.starts_with("CLAUDE_CODE_") {
            cmd.env_remove(k);
        }
    }
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "claude is not on PATH".to_string() } else { e.to_string() })?;
    // claude -p reports some failures (authentication, for one) on stdout, so both feed
    // the cause; only the last few lines are ever shown.
    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let readers: Vec<_> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
        .map(|mut r| {
            let tail = tail.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut t = tail.lock().unwrap();
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if t.len() > 4000 {
                        let cut = t.len() - 4000;
                        let at = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                        t.replace_range(..at, "");
                    }
                }
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(120);
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(1),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break 1;
            }
        }
    };
    for r in readers {
        let _ = r.join();
    }
    let t = tail.lock().unwrap().clone();
    Ok((code, t))
}

/// The settings a doctor run reads by default: the user's ~/.claude/settings.json.
pub fn user_settings() -> PathBuf {
    bellwether_core::outfile::user_settings()
}

/// A JS-flavoured view of a settings file for callers that have one parsed already.
pub fn parse_settings(text: &str) -> Result<Lj, String> {
    lossless::parse(text)
}
