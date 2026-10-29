//! The TerminalLocator: which window, and so
//! which monitor, holds a session's terminal, and how to bring it to the front.
//!
//! An ordered chain of strategies (`locator.strategies`). Each one needs some of the
//! terminal headers; the chain skips a strategy whose headers are missing, stops at the
//! first `exact` result, and keeps the first `app` result in case nothing does better.
//!
//! Everything outside this process goes through seams, so tests never run
//! AppleScript, herdr or `ps`:
//! - `Runner`: every command (`herdr`, `ps`, `osascript`, `open`);
//! - `Windows`: the on-screen window list (CoreGraphics; on Windows `EnumWindows`);
//! - `Native` (Windows only, plan WD4): the process list, the marker variables of a
//!   process's environment, and raising a window. Without it the macOS ways are used.
//!
//! Permissions. Placement works without any: `ps` finds the window-owning process, and
//! the window list gives its windows' bounds. AppleScript (`locator.appleScript`) only
//! sharpens that to the exact iTerm2 or Terminal window, and selects the tab on a
//! click; it needs Automation permission. A refusal is logged once per app and that
//! app's AppleScript is not tried again until the shell restarts.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ types

/// The snapshot's `terminal` object: the headers each hook carried.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terminal {
    pub program: Option<String>,
    pub iterm_session: Option<String>,
    pub term_session: Option<String>,
    pub herdr_pane: Option<String>,
    pub herdr_socket: Option<String>,
    pub tmux_pane: Option<String>,
    pub wt_session: Option<String>,
    /// CLAUDE_CODE_ENTRYPOINT: "claude-desktop" when the Claude desktop app runs the session.
    pub entrypoint: Option<String>,
}

impl Terminal {
    pub fn from_json(v: &Value) -> Terminal {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
        Terminal {
            program: s("program"),
            iterm_session: s("itermSession"),
            term_session: s("termSession"),
            herdr_pane: s("herdrPane"),
            herdr_socket: s("herdrSocket"),
            tmux_pane: s("tmuxPane"),
            wt_session: s("wtSession"),
            entrypoint: s("entrypoint"),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Terminal::default()
    }

    /// Environment markers a session's `claude` process carries (for process-tree).
    fn markers(&self) -> Vec<(&'static str, &str)> {
        let mut m = Vec::new();
        if let Some(v) = &self.herdr_pane {
            m.push(("HERDR_PANE_ID", v.as_str()));
        }
        if let Some(v) = &self.iterm_session {
            m.push(("ITERM_SESSION_ID", v.as_str()));
        }
        if let Some(v) = &self.term_session {
            m.push(("TERM_SESSION_ID", v.as_str()));
        }
        if let Some(v) = &self.tmux_pane {
            m.push(("TMUX_PANE", v.as_str()));
        }
        m
    }
}

/// A rectangle in global screen points, origin at the top left of the primary display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Exact,
    App,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    pub strategy: String,
    pub bounds: Option<Rect>,
    pub confidence: Confidence,
}

impl Located {
    fn none() -> Located {
        Located { strategy: "none".into(), bounds: None, confidence: Confidence::None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Focus {
    Focused(String),
    AppActivated(String),
    Unavailable(String),
}

#[derive(Debug, PartialEq)]
pub enum RunError {
    NotFound,
    Failed(String),
}

/// Runs a command and returns its standard output.
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, RunError>;
}

/// An on-screen window, front to back.
#[derive(Clone, Debug, PartialEq)]
pub struct Win {
    pub pid: i32,
    pub owner: String,
    pub bounds: Rect,
    /// The window's handle on Windows (what `Native::raise` takes); 0 on macOS.
    pub hwnd: u64,
    /// Minimised (Windows): it can be raised, but its bounds say nothing about a monitor.
    pub minimized: bool,
}

impl Win {
    #[cfg_attr(windows, allow(dead_code))]
    pub fn new(pid: i32, owner: impl Into<String>, bounds: Rect) -> Win {
        Win { pid, owner: owner.into(), bounds, hwnd: 0, minimized: false }
    }
}

/// What Windows needs that macOS gets from `ps` and `open` (plan WD4). Real: `win.rs`;
/// tests: a fake built from a `desktop.json`-shaped table.
pub trait Native: Send + Sync {
    /// Every process: pid, parent pid and exe name (`comm`). `uid` is 0 for this user's
    /// processes and `u32::MAX` for others' (only checked for the processes the locator
    /// filters by user: `claude.exe`, `node.exe` and `herdr.exe`). `tty` is empty.
    fn processes(&self) -> HashMap<i32, Proc>;
    /// The values of `keys` in this process's environment (this user's processes only).
    /// Nothing else from the environment is kept.
    fn environment(&self, pid: i32, keys: &[&str]) -> Option<Vec<(String, String)>>;
    /// Whether the process's command line contains `needle` (to recognise Claude Code run
    /// by `node.exe`). The command line itself is never kept or logged.
    fn command_line_has(&self, pid: i32, needle: &str) -> bool;
    /// Restores the window if minimised and brings it forward. `Err` says why not (the
    /// foreground was refused: its taskbar button flashes instead).
    fn raise(&self, hwnd: u64) -> Result<(), String>;
}

pub trait Windows: Send + Sync {
    /// Normal-layer, on-screen windows, frontmost first.
    fn list(&self) -> Vec<Win>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
    pub primary: bool,
}

/// The monitor that holds most of `bounds` (or, if none overlaps, the nearest one).
pub fn monitor_for<'a>(bounds: &Rect, monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    let best = monitors.iter().map(|m| (m.rect.overlap(bounds), m)).filter(|(a, _)| *a > 0.0).max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, m)) = best {
        return Some(m);
    }
    let (cx, cy) = bounds.centre();
    monitors.iter().min_by(|a, b| {
        let d = |m: &Monitor| {
            let (mx, my) = m.rect.centre();
            (mx - cx).powi(2) + (my - cy).powi(2)
        };
        d(a).total_cmp(&d(b))
    })
}

// --------------------------------------------------------------- processes

#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub tty: String,
    pub comm: String,
}

/// `ps -axo pid=,ppid=,uid=,tty=,comm=`: the command may contain spaces.
pub fn parse_ps(text: &str) -> HashMap<i32, Proc> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut rest = line.trim_start();
        let mut fields = Vec::new();
        for _ in 0..4 {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            fields.push(&rest[..end]);
            rest = rest[end..].trim_start();
        }
        let (Ok(pid), Ok(ppid), Ok(uid)) = (fields[0].parse(), fields.get(1).unwrap_or(&"").parse(), fields.get(2).unwrap_or(&"").parse()) else { continue };
        out.insert(pid, Proc { pid, ppid, uid, tty: fields.get(3).unwrap_or(&"").to_string(), comm: rest.trim_end().to_string() });
    }
    out
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The values of `keys` (any case) in a Windows environment block (`K=V` strings, each
/// ending in a NUL, the block in two). Everything else in the block is dropped here.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn env_values(block: &[u16], keys: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in block.split(|&c| c == 0) {
        if entry.is_empty() {
            break; // the block's end
        }
        let Some(eq) = entry.iter().skip(1).position(|&c| c == u16::from(b'=')).map(|i| i + 1) else { continue };
        let name = String::from_utf16_lossy(&entry[..eq]);
        if let Some(k) = keys.iter().find(|k| k.eq_ignore_ascii_case(&name)) {
            out.push((k.to_string(), String::from_utf16_lossy(&entry[eq + 1..])));
        }
    }
    out
}

/// The windows the locator considers on Windows, front to back, from `EnumWindows` as
/// `win.rs` lists it: not cloaked (another virtual desktop, a suspended app), not a tool
/// window (the strips are), not this process's, and at least 50 px each way unless
/// minimised. Owner is the exe name; bounds are DWM's, in physical pixels.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn usable_windows(list: Vec<crate::dump::Window>, own_pid: u32, minimized: impl Fn(u64) -> bool) -> Vec<Win> {
    list.into_iter()
        .filter(|w| !w.cloaked && !w.tool && w.pid != own_pid)
        .filter_map(|w| {
            let min = minimized(w.hwnd);
            let [l, t, r, b] = w.bounds;
            let bounds = Rect { x: l as f64, y: t as f64, w: (r - l) as f64, h: (b - t) as f64 };
            (min || (bounds.w >= 50.0 && bounds.h >= 50.0)).then(|| Win { pid: w.pid as i32, owner: w.exe, bounds, hwnd: w.hwnd, minimized: min })
        })
        .collect()
}

/// Whether a process's `comm` is the program `name`: `claude` matches `/x/claude` on macOS
/// and `claude.exe` (any case) on Windows.
fn is_program(comm: &str, name: &str) -> bool {
    let b = basename(comm);
    let b = if b.len() > 4 && b[b.len() - 4..].eq_ignore_ascii_case(".exe") { &b[..b.len() - 4] } else { b };
    b.eq_ignore_ascii_case(name)
}

/// The `.app` bundle a process's executable lives in, for `open -a`.
fn app_bundle(comm: &str) -> Option<String> {
    comm.find(".app/").map(|i| comm[..i + 4].to_string())
}

// ----------------------------------------------------------------- locator

pub struct Settings {
    pub strategies: Vec<String>,
    pub herdr_path: String,
    pub apple_script: bool,
    pub uid: u32,
}

pub struct Locator {
    runner: Arc<dyn Runner>,
    windows: Arc<dyn Windows>,
    cfg: Settings,
    /// Apps whose Automation permission was refused (AppleScript isn't tried again).
    denied: Mutex<HashSet<String>>,
    /// Messages already logged (each is logged once).
    logged: Mutex<HashSet<String>>,
    /// herdr sockets that timed out, and until when they're left alone.
    backoff: Mutex<HashMap<String, std::time::Instant>>,
    /// Checks a HERDR_SOCKET_PATH header (real: `check_socket`; tests: a fake).
    pub socket_check: Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    log: Box<dyn Fn(&str) + Send + Sync>,
    /// Windows' processes and raising (`with_native`); `None` on macOS.
    native: Option<Arc<dyn Native>>,
}

/// What one strategy found: where, how sure, and what a click should use.
struct Found {
    bounds: Option<Rect>,
    confidence: Confidence,
}

/// At most this many sockets are remembered as hung.
const BACKOFF_CAP: usize = 64;

/// How long a herdr socket that didn't answer is left alone.
const SOCKET_BACKOFF: std::time::Duration = std::time::Duration::from_secs(300);

/// A herdr id (`w1`, `w1:t2`, `w1:p3`) as accepted from a header or from herdr: 1 to 64
/// of `A-Z a-z 0-9 _ . : -`, not starting with "-".
pub fn valid_pane_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// HERDR_SOCKET_PATH on Windows (plan WD4): a named pipe `\\.\pipe\<name>` (the name 1 to
/// 256 of `A-Z a-z 0-9 _ . - \\`, no ".."), or an absolute drive path of at most 1024
/// bytes with no ".." to an existing file (an AF_UNIX socket shows as one). The pipe's
/// server isn't asked who it is: connecting to it would take one of herdr's pipe instances.
pub fn check_socket_windows(path: &str, exists: impl Fn(&str) -> bool) -> Result<(), String> {
    if let Some(name) = path.strip_prefix(r"\\.\pipe\") {
        let ok = (1..=256).contains(&name.len()) && !name.split('\\').any(|c| c == ".." || c.is_empty()) && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '\\'));
        return if ok { Ok(()) } else { Err("not a plain pipe name".into()) };
    }
    let b = path.as_bytes();
    let drive = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if !drive || path.len() > 1024 || path.split(['\\', '/']).any(|c| c == "..") || path.chars().any(|c| c.is_control()) {
        return Err("not a pipe name or an absolute path of at most 1024 bytes".into());
    }
    if !exists(path) {
        return Err("doesn't exist".into());
    }
    Ok(())
}

/// HERDR_SOCKET_PATH must be an absolute path of at most 1024 bytes, with no "..",
/// to an existing unix socket owned by this user, itself not a symlink (lstat).
pub fn check_socket(path: &str) -> Result<(), String> {
    if cfg!(windows) {
        return check_socket_windows(path, |p| std::path::Path::new(p).is_file());
    }
    if !path.starts_with('/') || path.len() > 1024 || path.split('/').any(|c| c == "..") || path.chars().any(|c| c.is_control()) {
        return Err("not an absolute path of at most 1024 bytes".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let m = std::fs::symlink_metadata(path).map_err(|e| format!("can't be read ({})", e.kind()))?;
        if m.file_type().is_symlink() {
            return Err("is a symlink".into());
        }
        if !m.file_type().is_socket() {
            return Err("is not a unix socket".into());
        }
        if m.uid() != unsafe { libc::getuid() } {
            return Err("belongs to another user".into());
        }
    }
    Ok(())
}

const KNOWN: [&str; 6] = ["herdr", "iterm2", "terminal-app", "vscode", "process-tree", "app-activate"];

impl Locator {
    pub fn new(runner: Arc<dyn Runner>, windows: Arc<dyn Windows>, cfg: Settings, log: Box<dyn Fn(&str) + Send + Sync>) -> Locator {
        let l = Locator { runner, windows, cfg, denied: Mutex::default(), logged: Mutex::default(), backoff: Mutex::default(), socket_check: Box::new(check_socket), log, native: None };
        // Strategies for other systems (or later milestones) are skipped quietly here.
        const ELSEWHERE: [&str; 4] = ["windows-terminal", "tmux", "hyprland", "x11-window"];
        for s in &l.cfg.strategies {
            if !KNOWN.contains(&s.as_str()) && !ELSEWHERE.contains(&s.as_str()) {
                l.once(&format!("locator: strategy {s:?} isn't built on this OS (built: {}); skipped", KNOWN.join(", ")));
            }
        }
        l
    }

    /// Windows: processes, environments and raising go through `native` (plan WD4), and
    /// the AppleScript strategies (`iterm2`, `terminal-app`) are skipped with one line.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn with_native(mut self, native: Arc<dyn Native>) -> Locator {
        let apple: Vec<String> = self.cfg.strategies.iter().filter(|s| matches!(s.as_str(), "iterm2" | "terminal-app")).cloned().collect();
        for s in &apple {
            self.once(&format!("locator: strategy {s:?} isn't built on this OS (it is AppleScript, macOS only); skipped"));
        }
        self.cfg.strategies.retain(|s| !apple.contains(s));
        self.native = Some(native);
        self
    }

    /// The chain in use, in order (after `with_native` dropped what isn't built here).
    pub fn strategies(&self) -> &[String] {
        &self.cfg.strategies
    }

    fn once(&self, msg: &str) {
        if self.logged.lock().unwrap().insert(msg.to_string()) {
            (self.log)(msg);
        }
    }

    /// Which window holds this terminal. Never fails: `none` means "use the primary
    /// monitor".
    pub fn locate(&self, t: &Terminal) -> Located {
        if t.is_empty() {
            return Located::none();
        }
        let mut fallback: Option<Located> = None;
        for name in &self.cfg.strategies {
            let found = match name.as_str() {
                "herdr" => self.herdr_locate(t),
                "iterm2" => self.iterm_locate(t),
                "terminal-app" => self.terminal_locate(t),
                "vscode" => self.vscode_locate(t),
                "process-tree" => self.tree_locate(t),
                "app-activate" => self.app_locate(t),
                _ => None,
            };
            let Some(f) = found else { continue };
            let located = Located { strategy: name.clone(), bounds: f.bounds, confidence: f.confidence };
            match f.confidence {
                Confidence::Exact => return located,
                Confidence::App if fallback.is_none() => fallback = Some(located),
                _ => {}
            }
        }
        fallback.unwrap_or_else(Located::none)
    }

    /// Brings this terminal to the front: the first strategy that can act does.
    pub fn focus(&self, t: &Terminal) -> Focus {
        if t.is_empty() {
            return Focus::Unavailable("no terminal headers".into());
        }
        for name in &self.cfg.strategies {
            let done = match name.as_str() {
                "herdr" => self.herdr_focus(t),
                "iterm2" => self.iterm_focus(t),
                "terminal-app" => self.terminal_focus(t),
                "vscode" | "process-tree" => self.tree_focus(t, name),
                "app-activate" => self.app_focus(t),
                _ => None,
            };
            if let Some(f) = done {
                return f;
            }
        }
        Focus::Unavailable("no strategy could reach this terminal".into())
    }

    // ---------------------------------------------------------- helpers

    fn ps(&self) -> HashMap<i32, Proc> {
        if let Some(n) = &self.native {
            return n.processes();
        }
        match self.runner.run("/bin/ps", &["-axo", "pid=,ppid=,uid=,tty=,comm="], &[]) {
            Ok(text) => parse_ps(&text),
            Err(e) => {
                self.once(&format!("locator: ps failed ({e:?}); process-based strategies are unavailable"));
                HashMap::new()
            }
        }
    }

    /// Walks up from `pid` to the first process that owns an on-screen window. On Windows
    /// the shell (`explorer.exe`, which owns File Explorer's windows and starts most apps)
    /// never counts: a walk that reaches it found no terminal.
    fn window_owner(&self, procs: &HashMap<i32, Proc>, wins: &[Win], pid: i32) -> Option<i32> {
        let shell = |w: &&Win| self.native.is_some() && is_program(&w.owner, "explorer");
        let owners: HashSet<i32> = wins.iter().filter(|w| !shell(w)).map(|w| w.pid).collect();
        let mut p = pid;
        for _ in 0..32 {
            if owners.contains(&p) {
                return Some(p);
            }
            p = procs.get(&p)?.ppid;
            if p <= 1 {
                return None;
            }
        }
        None
    }

    /// The owner's frontmost window, and `exact` only if it has just one.
    fn owner_window(wins: &[Win], pid: i32) -> Option<Found> {
        // A minimised window says nothing about where the terminal is.
        let mine: Vec<&Win> = wins.iter().filter(|w| w.pid == pid && !w.minimized).collect();
        let front = mine.first()?;
        Some(Found { bounds: Some(front.bounds), confidence: if mine.len() == 1 { Confidence::Exact } else { Confidence::App } })
    }

    fn activate(&self, procs: &HashMap<i32, Proc>, pid: i32) -> bool {
        if let Some(n) = &self.native {
            // The owner's frontmost window, minimised or not.
            let Some(w) = self.windows.list().into_iter().find(|w| w.pid == pid) else { return false };
            return self.raise(n.as_ref(), &w);
        }
        let Some(app) = procs.get(&pid).and_then(|p| app_bundle(&p.comm)) else { return false };
        self.runner.run("/usr/bin/open", &["-a", &app], &[]).is_ok()
    }

    /// Windows: raises `w`. A refused foreground is logged; the taskbar button flashing
    /// is then the answer to the click (so the chain doesn't try again).
    fn raise(&self, n: &dyn Native, w: &Win) -> bool {
        if let Err(why) = n.raise(w.hwnd) {
            (self.log)(&format!("locator: foreground refused for {} ({why}); its taskbar button flashes instead", w.owner));
        }
        true
    }

    /// AppleScript to `app` (only if allowed, the app is running, and it wasn't refused).
    fn osascript(&self, app: &str, owner: &str, script: &str) -> Option<String> {
        if !self.cfg.apple_script || self.denied.lock().unwrap().contains(app) {
            return None;
        }
        // `tell application` would launch an app that isn't running.
        if !self.windows.list().iter().any(|w| w.owner == owner) {
            return None;
        }
        match self.runner.run("/usr/bin/osascript", &["-e", script], &[]) {
            Ok(out) => Some(out.trim().to_string()),
            Err(RunError::Failed(err)) if err.contains("-1743") || err.contains("Not authorized") || err.contains("-1744") => {
                self.denied.lock().unwrap().insert(app.to_string());
                self.once(&format!(
                    "locator: Automation for {app} was refused (System Settings > Privacy & Security > Automation); placing by the process tree instead, and clicks only activate {app}"
                ));
                None
            }
            Err(e) => {
                self.once(&format!("locator: AppleScript to {app} failed ({e:?})"));
                None
            }
        }
    }

    fn parse_bounds(text: &str) -> Option<Rect> {
        let n: Vec<f64> = text.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (n.len() == 4 && n[2] > n[0] && n[3] > n[1]).then(|| Rect { x: n[0], y: n[1], w: n[2] - n[0], h: n[3] - n[1] })
    }

    // ------------------------------------------------------------- herdr

    /// `herdr pane get <pane>` (read-only): the pane's tab and workspace, and proof that
    /// the pane exists on that server.
    fn herdr_pane(&self, t: &Terminal) -> Option<(String, String, String)> {
        let pane = t.herdr_pane.as_deref()?;
        // Header values never reach herdr unchecked: an id that started with "-" would
        // be read as an option (herdr 0.9.0 has no `--`), and the socket must be a
        // real socket of ours, not a path to something else.
        if !valid_pane_id(pane) {
            self.once(&format!("locator: refusing herdr pane id {:?}: not a herdr id", pane.chars().take(80).collect::<String>()));
            return None;
        }
        let env = self.herdr_env(t)?;
        let key = t.herdr_socket.clone().unwrap_or_default();
        if let Some(until) = self.backoff.lock().unwrap().get(&key) {
            if std::time::Instant::now() < *until {
                return None; // this socket hung recently; don't stall everyone again
            }
        }
        let result = self.runner.run(&self.cfg.herdr_path, &["pane", "get", pane], &env);
        if let Err(RunError::Failed(e)) = &result {
            if e.contains("took longer than") {
                let now = std::time::Instant::now();
                let mut b = self.backoff.lock().unwrap();
                // Expired entries go; past the cap, the soonest to expire makes room.
                b.retain(|_, until| *until > now);
                while b.len() >= BACKOFF_CAP {
                    let Some(k) = b.iter().min_by_key(|(_, u)| **u).map(|(k, _)| k.clone()) else { break };
                    b.remove(&k);
                }
                b.insert(key.clone(), now + SOCKET_BACKOFF);
                drop(b);
                self.once(&format!("locator: herdr didn't answer on socket {key:?} within 4 s; not asking it again for 5 minutes"));
                return None;
            }
        }
        match result {
            Err(RunError::NotFound) => {
                self.once(&format!("locator: herdr CLI not found at {:?} (locator.herdrPath); the herdr strategy is unavailable", self.cfg.herdr_path));
                None
            }
            Err(e) => {
                self.once(&format!("locator: herdr pane get {pane} failed ({e:?})"));
                None
            }
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).ok()?;
                if let Some(err) = v.get("error") {
                    self.once(&format!("locator: herdr has no pane {pane}: {}", err.get("code").and_then(Value::as_str).unwrap_or("error")));
                    return None;
                }
                let p = v.pointer("/result/pane")?;
                // What herdr returns is checked like a header before it reaches a focus command.
                let s = |k: &str| p.get(k).and_then(Value::as_str).filter(|x| valid_pane_id(x)).map(str::to_string);
                Some((s("workspace_id")?, s("tab_id")?, s("pane_id")?))
            }
        }
    }

    /// herdr's host window: the attached `herdr` client processes (same user, on a tty;
    /// the server has none), each walked up to the process that owns a window. Doesn't
    /// use the environment, which newer herdr strips of the host's markers.
    fn herdr_host(&self) -> Option<(HashMap<i32, Proc>, Vec<Win>, i32, String, usize)> {
        let procs = self.ps();
        let wins = self.windows.list();
        let mut hosts: Vec<(usize, i32, String)> = Vec::new();
        // A client is told from the server by its terminal on macOS (the server has none).
        // Windows has no tty: there a client is a herdr.exe with no herdr.exe above it (the
        // server is started by a client, or outlives it with no window above it at all).
        let client = |p: &&Proc| {
            if self.native.is_none() {
                return p.tty != "??" && !p.tty.is_empty();
            }
            let mut q = p.ppid;
            for _ in 0..32 {
                match procs.get(&q) {
                    Some(up) if is_program(&up.comm, "herdr") => return false,
                    Some(up) if up.ppid != q && q > 4 => q = up.ppid,
                    _ => return true,
                }
            }
            true
        };
        for p in procs.values().filter(|p| is_program(&p.comm, "herdr") && p.uid == self.cfg.uid).filter(client) {
            if let Some(owner) = self.window_owner(&procs, &wins, p.pid) {
                let front = wins.iter().position(|w| w.pid == owner).unwrap_or(usize::MAX);
                hosts.push((front, owner, p.tty.clone()));
            }
        }
        hosts.sort();
        let count = hosts.len();
        let (_, owner, tty) = hosts.into_iter().next()?;
        Some((procs, wins, owner, tty, count))
    }

    fn herdr_locate(&self, t: &Terminal) -> Option<Found> {
        self.herdr_pane(t)?;
        let (_, wins, owner, tty, clients) = self.herdr_host()?;
        let owner_name = wins.iter().find(|w| w.pid == owner).map(|w| w.owner.clone()).unwrap_or_default();
        if owner_name == "iTerm2" {
            if let Some(b) = self.osascript("iTerm2", "iTerm2", &iterm_script_by_tty(&tty, false)).and_then(|o| Self::parse_bounds(&o)) {
                return Some(Found { bounds: Some(b), confidence: Confidence::Exact });
            }
        }
        let mut found = Self::owner_window(&wins, owner)?;
        if clients > 1 {
            found.confidence = Confidence::App; // several herdr clients: the frontmost host wins
        }
        Some(found)
    }

    /// HERDR_SOCKET_PATH for herdr's commands: none (herdr's default socket) if the
    /// header is absent; `None` (don't run herdr) if it isn't a socket we own.
    fn herdr_env<'a>(&self, t: &'a Terminal) -> Option<Vec<(&'static str, &'a str)>> {
        let Some(sock) = t.herdr_socket.as_deref() else { return Some(Vec::new()) };
        match (self.socket_check)(sock) {
            Ok(()) => Some(vec![("HERDR_SOCKET_PATH", sock)]),
            Err(why) => {
                self.once(&format!("locator: refusing herdr socket {:?}: {why}", sock.chars().take(120).collect::<String>()));
                None
            }
        }
    }

    fn herdr_focus(&self, t: &Terminal) -> Option<Focus> {
        let (ws, tab, pane) = self.herdr_pane(t)?;
        let env = self.herdr_env(t)?;
        let h = &self.cfg.herdr_path;
        let _ = self.runner.run(h, &["workspace", "focus", &ws], &env);
        let _ = self.runner.run(h, &["tab", "focus", &tab], &env);
        // `agent focus` takes the pane id hosting an agent (herdr --skill, 0.9.0).
        let pane_focused = self.runner.run(h, &["agent", "focus", &pane], &env).is_ok();
        if let Some((procs, wins, owner, tty, _)) = self.herdr_host() {
            let owner_name = wins.iter().find(|w| w.pid == owner).map(|w| w.owner.clone()).unwrap_or_default();
            let raised = owner_name == "iTerm2" && self.osascript("iTerm2", "iTerm2", &iterm_script_by_tty(&tty, true)).is_some();
            if !raised {
                self.activate(&procs, owner);
            }
        } else {
            (self.log)("locator: herdr: no attached herdr client has a window (detached?); the pane is selected in herdr, no window raised");
        }
        Some(Focus::Focused(format!("herdr ({tab}{})", if pane_focused { format!(", pane {pane}") } else { String::new() })))
    }

    // ------------------------------------------------------------ iTerm2

    fn iterm_uuid(t: &Terminal) -> Option<String> {
        let id = t.iterm_session.as_deref()?;
        let uuid = id.rsplit(':').next()?;
        // Only a UUID ever reaches AppleScript.
        (uuid.len() >= 8 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then(|| uuid.to_string())
    }

    fn iterm_locate(&self, t: &Terminal) -> Option<Found> {
        let uuid = Self::iterm_uuid(t)?;
        let b = self.osascript("iTerm2", "iTerm2", &iterm_script_by_session(&uuid, false)).and_then(|o| Self::parse_bounds(&o))?;
        Some(Found { bounds: Some(b), confidence: Confidence::Exact })
    }

    fn iterm_focus(&self, t: &Terminal) -> Option<Focus> {
        let uuid = Self::iterm_uuid(t)?;
        let out = self.osascript("iTerm2", "iTerm2", &iterm_script_by_session(&uuid, true))?;
        (!out.is_empty()).then(|| Focus::Focused("iterm2".into()))
    }

    // ---------------------------------------------------------- Terminal

    fn terminal_locate(&self, t: &Terminal) -> Option<Found> {
        if t.program.as_deref() != Some("Apple_Terminal") {
            return None;
        }
        let (procs, wins, claude) = self.claude_process(t)?;
        let tty = procs.get(&claude).map(|p| p.tty.clone()).filter(|x| x != "??")?;
        if let Some(b) = self.osascript("Terminal", "Terminal", &terminal_script(&tty, false)).and_then(|o| Self::parse_bounds(&o)) {
            return Some(Found { bounds: Some(b), confidence: Confidence::Exact });
        }
        let owner = self.window_owner(&procs, &wins, claude)?;
        Self::owner_window(&wins, owner)
    }

    fn terminal_focus(&self, t: &Terminal) -> Option<Focus> {
        if t.program.as_deref() != Some("Apple_Terminal") {
            return None;
        }
        let (procs, wins, claude) = self.claude_process(t)?;
        let tty = procs.get(&claude).map(|p| p.tty.clone()).unwrap_or_default();
        if self.osascript("Terminal", "Terminal", &terminal_script(&tty, true)).is_some_and(|o| !o.is_empty()) {
            return Some(Focus::Focused("terminal-app".into()));
        }
        let owner = self.window_owner(&procs, &wins, claude)?;
        self.activate(&procs, owner).then(|| Focus::AppActivated("terminal-app".into()))
    }

    // ------------------------------------------------------ process tree

    /// The session's `claude` process: same user, and every terminal marker in its
    /// environment matches. Only the marker variables are read; nothing else from
    /// another process's environment is kept or logged.
    fn claude_process(&self, t: &Terminal) -> Option<(HashMap<i32, Proc>, Vec<Win>, i32)> {
        if let Some(n) = &self.native {
            return self.claude_process_native(n.as_ref(), t);
        }
        let markers = t.markers();
        if markers.is_empty() {
            return None;
        }
        let procs = self.ps();
        let pids: Vec<String> = procs.values().filter(|p| basename(&p.comm) == "claude" && p.uid == self.cfg.uid).map(|p| p.pid.to_string()).collect();
        if pids.is_empty() {
            return None;
        }
        let list = pids.join(",");
        let text = self.runner.run("/bin/ps", &["-Eww", "-o", "pid=,command=", "-p", &list], &[]).ok()?;
        let matches: Vec<i32> = text
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                let (pid, rest) = line.split_once(char::is_whitespace)?;
                let tokens: HashSet<&str> = rest.split_whitespace().collect();
                markers.iter().all(|(k, v)| tokens.contains(format!("{k}={v}").as_str())).then(|| pid.parse().ok()).flatten()
            })
            .collect();
        if matches.len() != 1 {
            return None; // none, or several sessions look alike: not sure enough
        }
        let wins = self.windows.list();
        Some((procs, wins, matches[0]))
    }

    /// Windows: `claude.exe` (or `node.exe` running Claude Code) of this user whose
    /// environment has every marker. Inside herdr the markers are herdr's server's (its
    /// `WT_SESSION` is whichever tab started the server), so a herdr session never matches
    /// here: the `herdr` strategy, or `app-activate`, answers for it (plan WD4).
    fn claude_process_native(&self, n: &dyn Native, t: &Terminal) -> Option<(HashMap<i32, Proc>, Vec<Win>, i32)> {
        if t.herdr_pane.is_some() {
            return None;
        }
        let mut markers: Vec<(&str, &str)> = Vec::new();
        if let Some(v) = &t.wt_session {
            markers.push(("WT_SESSION", v.as_str()));
        }
        if let Some(v) = &t.tmux_pane {
            markers.push(("TMUX_PANE", v.as_str()));
        }
        if markers.is_empty() {
            return None;
        }
        let keys: Vec<&str> = markers.iter().map(|(k, _)| *k).collect();
        let procs = self.ps();
        let matches: Vec<i32> = procs
            .values()
            .filter(|p| p.uid == self.cfg.uid)
            .filter(|p| is_program(&p.comm, "claude") || (is_program(&p.comm, "node") && n.command_line_has(p.pid, "claude-code")))
            .filter(|p| {
                let Some(env) = n.environment(p.pid, &keys) else { return false };
                markers.iter().all(|(k, v)| env.iter().any(|(ek, ev)| ek.eq_ignore_ascii_case(k) && ev == v))
            })
            .map(|p| p.pid)
            .collect();
        if matches.len() != 1 {
            return None; // none, or several sessions look alike: not sure enough
        }
        let wins = self.windows.list();
        Some((procs, wins, matches[0]))
    }

    fn tree_locate(&self, t: &Terminal) -> Option<Found> {
        let (procs, wins, claude) = self.claude_process(t)?;
        let owner = self.window_owner(&procs, &wins, claude)?;
        Self::owner_window(&wins, owner)
    }

    fn tree_focus(&self, t: &Terminal, name: &str) -> Option<Focus> {
        if name == "vscode" && t.program.as_deref() != Some("vscode") {
            return None;
        }
        let (procs, wins, claude) = self.claude_process(t)?;
        let owner = self.window_owner(&procs, &wins, claude)?;
        if self.native.is_some() {
            // Windows raises the owner's own frontmost window (exact with one window).
            return self.activate(&procs, owner).then(|| Focus::Focused(name.to_string()));
        }
        self.activate(&procs, owner).then(|| Focus::AppActivated(name.to_string()))
    }

    fn vscode_locate(&self, t: &Terminal) -> Option<Found> {
        if t.program.as_deref() != Some("vscode") {
            return None;
        }
        let mut f = self.tree_locate(t).or_else(|| self.app_locate(t))?;
        f.confidence = Confidence::App; // one VS Code process owns every window
        Some(f)
    }

    // ------------------------------------------------------- app-activate

    /// The app a session lives in: (window owner name, `open` arguments). A session the
    /// Claude desktop app started has no terminal, so its entrypoint decides first.
    fn app_names(t: &Terminal) -> Option<(&'static str, [&'static str; 2])> {
        if matches!(t.entrypoint.as_deref(), Some("claude-desktop" | "claude-desktop-3p")) {
            return Some(("Claude", ["-b", "com.anthropic.claudefordesktop"]));
        }
        // TERM_PROGRAM -> (window owner name, app for `open -a`)
        let (owner, app) = match t.program.as_deref()? {
            "iTerm.app" => ("iTerm2", "iTerm"),
            "Apple_Terminal" => ("Terminal", "Terminal"),
            "vscode" => ("Code", "Visual Studio Code"),
            "ghostty" => ("Ghostty", "Ghostty"),
            "WezTerm" => ("wezterm-gui", "WezTerm"),
            "WarpTerminal" => ("Warp", "Warp"),
            _ => return None,
        };
        Some((owner, ["-a", app]))
    }

    /// Windows: the exe whose most recent window a session lives in (plan WD4's table).
    fn app_exe(t: &Terminal) -> Option<&'static str> {
        if matches!(t.entrypoint.as_deref(), Some("claude-desktop" | "claude-desktop-3p")) {
            return Some("claude.exe");
        }
        match t.program.as_deref() {
            Some("vscode") => return Some("Code.exe"),
            Some("WezTerm") => return Some("wezterm-gui.exe"),
            _ => {}
        }
        t.wt_session.as_ref().map(|_| "WindowsTerminal.exe")
    }

    fn app_locate(&self, t: &Terminal) -> Option<Found> {
        if self.native.is_some() {
            let exe = Self::app_exe(t)?;
            let w = self.windows.list().into_iter().find(|w| !w.minimized && w.owner.eq_ignore_ascii_case(exe))?;
            return Some(Found { bounds: Some(w.bounds), confidence: Confidence::App });
        }
        let (owner, _) = Self::app_names(t)?;
        let w = self.windows.list().into_iter().find(|w| w.owner == owner)?;
        Some(Found { bounds: Some(w.bounds), confidence: Confidence::App })
    }

    fn app_focus(&self, t: &Terminal) -> Option<Focus> {
        if let Some(n) = &self.native {
            let exe = Self::app_exe(t)?;
            let w = self.windows.list().into_iter().find(|w| w.owner.eq_ignore_ascii_case(exe))?;
            return self.raise(n.as_ref(), &w).then(|| Focus::AppActivated("app-activate".into()));
        }
        let (_, args) = Self::app_names(t)?;
        self.runner.run("/usr/bin/open", &args, &[]).ok().map(|_| Focus::AppActivated("app-activate".into()))
    }
}

// --------------------------------------------------------- AppleScripts

/// iTerm2: the window holding the session with this unique id. `select` also brings
/// it to the front. Returns "x1,y1,x2,y2" (or "" if no such session).
fn iterm_script_by_session(uuid: &str, select: bool) -> String {
    let act = if select { "select s\n            select t\n            select w\n            activate\n" } else { "" };
    format!(
        "tell application \"iTerm2\"\n  repeat with w in windows\n    repeat with t in tabs of w\n      repeat with s in sessions of t\n        if unique id of s is \"{uuid}\" then\n            {act}set b to bounds of w\n            return ((item 1 of b) as text) & \",\" & ((item 2 of b) as text) & \",\" & ((item 3 of b) as text) & \",\" & ((item 4 of b) as text)\n        end if\n      end repeat\n    end repeat\n  end repeat\nend tell\nreturn \"\""
    )
}

/// iTerm2: the window whose session runs on this tty (the herdr client's).
fn iterm_script_by_tty(tty: &str, select: bool) -> String {
    let tty = safe_tty(tty);
    let act = if select { "select s\n            select t\n            select w\n            activate\n" } else { "" };
    format!(
        "tell application \"iTerm2\"\n  repeat with w in windows\n    repeat with t in tabs of w\n      repeat with s in sessions of t\n        if tty of s is \"/dev/{tty}\" then\n            {act}set b to bounds of w\n            return ((item 1 of b) as text) & \",\" & ((item 2 of b) as text) & \",\" & ((item 3 of b) as text) & \",\" & ((item 4 of b) as text)\n        end if\n      end repeat\n    end repeat\n  end repeat\nend tell\nreturn \"\""
    )
}

/// Terminal.app: the window whose tab runs on this tty.
fn terminal_script(tty: &str, select: bool) -> String {
    let tty = safe_tty(tty);
    let act = if select { "set selected of t to true\n          set index of w to 1\n          activate\n" } else { "" };
    format!(
        "tell application \"Terminal\"\n  repeat with w in windows\n    repeat with t in tabs of w\n      if tty of t is \"/dev/{tty}\" then\n          {act}set b to bounds of w\n          return ((item 1 of b) as text) & \",\" & ((item 2 of b) as text) & \",\" & ((item 3 of b) as text) & \",\" & ((item 4 of b) as text)\n      end if\n    end repeat\n  end repeat\nend tell\nreturn \"\""
    )
}

/// A tty name as `ps` prints it (`ttys001`); anything else becomes "-" and matches nothing.
fn safe_tty(tty: &str) -> String {
    if !tty.is_empty() && tty.len() < 16 && tty.chars().all(|c| c.is_ascii_alphanumeric()) {
        tty.to_string()
    } else {
        "-".into()
    }
}

// -------------------------------------------------------------- real seams

/// Real commands, each with a time limit (a hung AppleScript or herdr must not stall
/// placement).
pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, RunError> {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        // Windows: a console program (herdr) started from the strip, which has no console,
        // would flash a console window of its own.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { RunError::NotFound } else { RunError::Failed(e.to_string()) })?;
        // Read both pipes while it runs: a big `ps` listing would otherwise fill the
        // pipe and block until the time limit.
        let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = pipe {
                    let _ = std::io::Read::read_to_end(&mut p, &mut buf);
                }
                buf
            })
        };
        let out_t = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let err_t = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let deadline = Instant::now() + Duration::from_secs(4);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::Failed(format!("{program} took longer than 4 s")));
                }
            }
        };
        let stdout = out_t.join().unwrap_or_default();
        let stderr = err_t.join().unwrap_or_default();
        if status.success() {
            Ok(String::from_utf8_lossy(&stdout).into_owned())
        } else {
            // herdr reports errors as JSON on stdout; keep both, short.
            let text = format!("{} {}", String::from_utf8_lossy(&stderr).trim(), String::from_utf8_lossy(&stdout).trim());
            Err(RunError::Failed(format!("{status}: {}", text.trim().chars().take(300).collect::<String>())))
        }
    }
}

/// The window list from CoreGraphics. Owner, pid and bounds need no permission
/// (window titles would, and are not read). Windows has its own: `winlocate::WinWindows`.
#[cfg(not(windows))]
pub struct RealWindows;

#[cfg(target_os = "macos")]
impl Windows for RealWindows {
    fn list(&self) -> Vec<Win> {
        cg::windows()
    }
}

/// Every on-screen window with its layer (macOS; empty elsewhere): what the full-screen
/// check in privacy.rs reads.
#[cfg(target_os = "macos")]
pub fn layered_windows() -> Vec<(i32, Win)> {
    cg::layered()
}

#[cfg(not(target_os = "macos"))]
pub fn layered_windows() -> Vec<(i32, Win)> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", windows)))]
impl Windows for RealWindows {
    fn list(&self) -> Vec<Win> {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod cg {
    use super::{Rect, Win};
    use std::ffi::c_void;
    type CFTypeRef = *const c_void;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> CFTypeRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowOwnerName: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
        static kCGWindowBounds: CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(d: CFTypeRef, k: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(n: CFTypeRef, kind: isize, out: *mut c_void) -> bool;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut u8, size: isize, encoding: u32) -> bool;
        fn CFRelease(x: CFTypeRef);
    }
    const ON_SCREEN_ONLY: u32 = 1; // kCGWindowListOptionOnScreenOnly
    const EXCLUDE_DESKTOP: u32 = 16; // kCGWindowListExcludeDesktopElements
    const SINT32: isize = 3; // kCFNumberSInt32Type
    const UTF8: u32 = 0x0800_0100;

    /// Ordinary windows (layer 0, at least 50 points each way), front to back.
    pub fn windows() -> Vec<Win> {
        layered().into_iter().filter(|(layer, w)| *layer == 0 && w.bounds.w >= 50.0 && w.bounds.h >= 50.0).map(|(_, w)| w).collect()
    }

    /// Every on-screen window with its layer, front to back (the Dock's, menus and
    /// overlays included).
    pub fn layered() -> Vec<(i32, Win)> {
        let mut out = Vec::new();
        unsafe {
            let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
            if list.is_null() {
                return out;
            }
            for i in 0..CFArrayGetCount(list) {
                let d = CFArrayGetValueAtIndex(list, i);
                let num = |k: CFTypeRef| {
                    let v = CFDictionaryGetValue(d, k);
                    let mut n: i32 = 0;
                    if !v.is_null() {
                        CFNumberGetValue(v, SINT32, &mut n as *mut i32 as *mut c_void);
                    }
                    n
                };
                let mut r = CGRect::default();
                let b = CFDictionaryGetValue(d, kCGWindowBounds);
                if b.is_null() || !CGRectMakeWithDictionaryRepresentation(b, &mut r) {
                    continue;
                }
                let mut buf = [0u8; 256];
                let name = CFDictionaryGetValue(d, kCGWindowOwnerName);
                let owner = if !name.is_null() && CFStringGetCString(name, buf.as_mut_ptr(), buf.len() as isize, UTF8) {
                    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
                    String::from_utf8_lossy(&buf[..len]).into_owned()
                } else {
                    String::new()
                };
                out.push((num(kCGWindowLayer), Win::new(num(kCGWindowOwnerPID), owner, Rect { x: r.x, y: r.y, w: r.w, h: r.h })));
            }
            CFRelease(list);
        }
        out
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Answers by the command line: "program arg arg…" -> result. Records every call.
    #[derive(Default)]
    pub struct FakeRunner {
        pub answers: HashMap<String, Result<String, String>>,
        pub missing: HashSet<String>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        pub fn on(mut self, cmd: &str, out: &str) -> Self {
            self.answers.insert(cmd.to_string(), Ok(out.to_string()));
            self
        }
        pub fn fail(mut self, cmd: &str, err: &str) -> Self {
            self.answers.insert(cmd.to_string(), Err(err.to_string()));
            self
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, RunError> {
            let mut line = std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
            if program.ends_with("osascript") {
                // Keyed by what the script targets, not its whole text.
                let s = args.get(1).copied().unwrap_or("");
                let target = ["unique id of s is \"", "tty of s is \"/dev/", "tty of t is \"/dev/"]
                    .iter()
                    .find_map(|p| s.find(p).map(|i| s[i + p.len()..].split('"').next().unwrap_or("").to_string()))
                    .unwrap_or_default();
                let app = if s.contains("\"iTerm2\"") { "iTerm2" } else { "Terminal" };
                line = format!("osascript {app} {target}{}", if s.contains("activate") { " select" } else { "" });
            }
            for (k, v) in env {
                line.push_str(&format!(" [{k}={v}]"));
            }
            self.calls.lock().unwrap().push(line.clone());
            if self.missing.contains(program) {
                return Err(RunError::NotFound);
            }
            match self.answers.get(&line) {
                Some(Ok(o)) => Ok(o.clone()),
                Some(Err(e)) => Err(RunError::Failed(e.clone())),
                None => Err(RunError::Failed(format!("no recorded answer for: {line}"))),
            }
        }
    }

    pub struct FakeWindows(pub Vec<Win>);
    impl Windows for FakeWindows {
        fn list(&self) -> Vec<Win> {
            self.0.clone()
        }
    }

    fn win(pid: i32, owner: &str, x: f64) -> Win {
        Win::new(pid, owner, Rect { x, y: 40.0, w: 1200.0, h: 800.0 })
    }

    fn locator(runner: FakeRunner, windows: Vec<Win>, strategies: &[&str], apple: bool) -> (Locator, Arc<FakeRunner>, Arc<Mutex<Vec<String>>>) {
        let runner = Arc::new(runner);
        let logs = Arc::new(Mutex::new(Vec::new()));
        let l2 = logs.clone();
        let mut loc = Locator::new(
            runner.clone(),
            Arc::new(FakeWindows(windows)),
            Settings { strategies: strategies.iter().map(|s| s.to_string()).collect(), herdr_path: "herdr".into(), apple_script: apple, uid: 501 },
            Box::new(move |m| l2.lock().unwrap().push(m.to_string())),
        );
        // Tests use made-up socket paths: only "/s/…" passes.
        loc.socket_check = Box::new(|p| if p.starts_with("/s/") { Ok(()) } else { Err("not a socket (fake)".into()) });
        (loc, runner, logs)
    }

    const ALL: [&str; 6] = ["herdr", "iterm2", "terminal-app", "vscode", "process-tree", "app-activate"];

    // An invented process table, shaped like `ps -axo pid=,ppid=,uid=,tty=,comm=`: herdr inside iTerm2, and a Terminal.app session.
    const PS: &str = "    1     0     0 ??       /sbin/launchd
 2100     1   501 ??       /Applications/iTerm.app/Contents/MacOS/iTerm2
 2101  2100   501 ??       /Users/example/Library/Application Support/iTerm2/iTermServer-3.0.0
 3100  2101     0 ttys003  login
 3101  3100   501 ttys003  -zsh
 4100  3101   501 ttys003  herdr
 4101  4100   501 ??       /Users/example/.local/bin/herdr
 5100  4101   501 ttys004  claude
70001     1   501 ??       /Applications/Utilities/Terminal.app/Contents/MacOS/Terminal
70002 70001     0 ttys009  login
70003 70002   501 ttys009  -zsh
70004 70003   501 ttys009  claude
";
    const PANE: &str = r#"{"id":"cli:pane:get","result":{"pane":{"agent":"claude","pane_id":"w4:p2","tab_id":"w4:t2","workspace_id":"w4","focused":true},"type":"pane_get"}}"#;
    const NO_PANE: &str = r#"{"error":{"code":"pane_not_found","message":"pane w9:p9 not found"},"id":"cli:pane:get"}"#;

    fn herdr_term() -> Terminal {
        // ITERM_SESSION_ID deliberately absent: the host is found through the client process.
        Terminal { herdr_pane: Some("w4:p2".into()), herdr_socket: Some("/s/herdr.sock".into()), program: Some("herdr".into()), ..Default::default() }
    }

    #[test]
    fn herdr_finds_its_host_through_the_client_process_without_any_host_marker() {
        let r = FakeRunner::default().on("herdr pane get w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", PANE).on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        let (loc, _, _) = locator(r, vec![win(2100, "iTerm2", 2000.0)], &ALL, false);
        let l = loc.locate(&herdr_term());
        assert_eq!(l.strategy, "herdr");
        assert_eq!(l.confidence, Confidence::Exact, "one client, one host window");
        assert_eq!(l.bounds.unwrap().x, 2000.0);
    }

    #[test]
    fn herdr_with_two_host_windows_is_app_confidence_unless_applescript_finds_the_tty() {
        let r = || FakeRunner::default().on("herdr pane get w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", PANE).on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        let wins = vec![win(2100, "iTerm2", 0.0), win(2100, "iTerm2", 2000.0)];
        let (loc, _, _) = locator(r(), wins.clone(), &["herdr"], false);
        assert_eq!(loc.locate(&herdr_term()).confidence, Confidence::App);
        let (loc, runner, _) = locator(r().on("osascript iTerm2 ttys003", "1800,30,3000,900"), wins, &["herdr"], true);
        let l = loc.locate(&herdr_term());
        assert_eq!((l.confidence, l.bounds.unwrap().x), (Confidence::Exact, 1800.0));
        assert!(runner.calls().iter().any(|c| c == "osascript iTerm2 ttys003"), "asked iTerm2 by the client's tty");
    }

    #[test]
    fn a_missing_herdr_cli_is_unavailable_and_the_chain_moves_on() {
        let mut r = FakeRunner::default().on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        r.missing.insert("herdr".into());
        let t = Terminal { program: Some("iTerm.app".into()), ..herdr_term() };
        let (loc, _, logs) = locator(r, vec![win(2100, "iTerm2", 0.0)], &ALL, false);
        let l = loc.locate(&t);
        assert_eq!(l.strategy, "app-activate");
        assert_eq!(l.confidence, Confidence::App);
        loc.locate(&t);
        let logs = logs.lock().unwrap();
        assert_eq!(logs.iter().filter(|m| m.contains("herdr CLI not found")).count(), 1, "logged once: {logs:?}");
    }

    #[test]
    fn a_pane_herdr_doesnt_know_falls_through() {
        let r = FakeRunner::default().on("herdr pane get w9:p9 [HERDR_SOCKET_PATH=/s/herdr.sock]", NO_PANE).on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        let t = Terminal { herdr_pane: Some("w9:p9".into()), ..herdr_term() };
        let (loc, _, logs) = locator(r, vec![win(2100, "iTerm2", 0.0)], &["herdr"], false);
        assert_eq!(loc.locate(&t).confidence, Confidence::None);
        assert!(logs.lock().unwrap()[0].contains("pane_not_found"));
    }

    #[test]
    fn herdr_focus_selects_workspace_tab_and_pane_then_raises_the_host() {
        let r = FakeRunner::default()
            .on("herdr pane get w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", PANE)
            .on("herdr workspace focus w4 [HERDR_SOCKET_PATH=/s/herdr.sock]", "{}")
            .on("herdr tab focus w4:t2 [HERDR_SOCKET_PATH=/s/herdr.sock]", "{}")
            .on("herdr agent focus w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", "{}")
            .on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS)
            .on("/usr/bin/open -a /Applications/iTerm.app", "");
        let (loc, runner, _) = locator(r, vec![win(2100, "iTerm2", 0.0)], &ALL, false);
        assert_eq!(loc.focus(&herdr_term()), Focus::Focused("herdr (w4:t2, pane w4:p2)".into()));
        let calls = runner.calls();
        let order: Vec<&str> = calls.iter().map(String::as_str).filter(|c| c.starts_with("herdr") || c.starts_with("/usr/bin/open")).collect();
        assert_eq!(&order[1..], &["herdr workspace focus w4 [HERDR_SOCKET_PATH=/s/herdr.sock]", "herdr tab focus w4:t2 [HERDR_SOCKET_PATH=/s/herdr.sock]", "herdr agent focus w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", "/usr/bin/open -a /Applications/iTerm.app"]);
    }

    #[test]
    fn iterm2_by_session_id_is_exact_and_a_refusal_is_logged_once_and_not_retried() {
        let t = Terminal { program: Some("iTerm.app".into()), iterm_session: Some("w0t1p0:0F0E0D0C-AAAA-4BBB-8CCC-000000000001".into()), ..Default::default() };
        let ok = FakeRunner::default().on("osascript iTerm2 0F0E0D0C-AAAA-4BBB-8CCC-000000000001", "100,50,1300,850");
        let (loc, _, _) = locator(ok, vec![win(2100, "iTerm2", 0.0)], &ALL, true);
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence), ("iterm2", Confidence::Exact));
        assert_eq!(l.bounds, Some(Rect { x: 100.0, y: 50.0, w: 1200.0, h: 800.0 }));

        let denied = FakeRunner::default()
            .fail("osascript iTerm2 0F0E0D0C-AAAA-4BBB-8CCC-000000000001", "execution error: Not authorized to send Apple events to iTerm2. (-1743)")
            .on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        let (loc, runner, logs) = locator(denied, vec![win(2100, "iTerm2", 0.0)], &ALL, true);
        let l = loc.locate(&t);
        assert_eq!(l.strategy, "app-activate", "falls back without Automation");
        loc.locate(&t);
        assert_eq!(runner.calls().iter().filter(|c| c.starts_with("osascript")).count(), 1, "not asked again");
        let logs = logs.lock().unwrap();
        assert_eq!(logs.iter().filter(|m| m.contains("Automation for iTerm2 was refused")).count(), 1);
    }

    #[test]
    fn applescript_is_never_sent_to_an_app_that_isnt_running_or_when_switched_off() {
        let t = Terminal { program: Some("iTerm.app".into()), iterm_session: Some("w0t1p0:0F0E0D0C-AAAA-4BBB-8CCC-000000000001".into()), ..Default::default() };
        let (loc, runner, _) = locator(FakeRunner::default(), vec![], &["iterm2"], true);
        assert_eq!(loc.locate(&t).confidence, Confidence::None);
        assert!(runner.calls().is_empty(), "iTerm2 isn't running: `tell application` would launch it");
        let (loc, runner, _) = locator(FakeRunner::default(), vec![win(2100, "iTerm2", 0.0)], &["iterm2"], false);
        loc.locate(&t);
        assert!(runner.calls().is_empty());
        // Only a hex UUID reaches a script.
        let bad = Terminal { iterm_session: Some("w0t1p0:x\" & do shell script \"rm".into()), ..t };
        let (loc, runner, _) = locator(FakeRunner::default(), vec![win(2100, "iTerm2", 0.0)], &["iterm2"], true);
        loc.locate(&bad);
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn process_tree_matches_the_claude_process_by_its_markers_and_walks_to_the_window_owner() {
        let t = Terminal { program: Some("Apple_Terminal".into()), term_session: Some("ABC-123".into()), ..Default::default() };
        let env = "5100 claude HERDR_PANE_ID=w4:p2 TERM_SESSION_ID=other\n70004 claude TERM_PROGRAM=Apple_Terminal TERM_SESSION_ID=ABC-123 SECRET=never-kept\n";
        let r = FakeRunner::default().on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS);
        let ps_env = |r: FakeRunner| {
            // pid order in the -p list follows the HashMap; record both orders.
            r.on("/bin/ps -Eww -o pid=,command= -p 5100,70004", env).on("/bin/ps -Eww -o pid=,command= -p 70004,5100", env)
        };
        let (loc, _, _) = locator(ps_env(r), vec![win(70001, "Terminal", 300.0)], &["process-tree"], false);
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence, l.bounds.unwrap().x), ("process-tree", Confidence::Exact, 300.0));
        // Terminal.app with AppleScript: the window of that tty.
        let r = FakeRunner::default().on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS).on("osascript Terminal ttys009", "10,20,810,620");
        let (loc, _, _) = locator(ps_env(r), vec![win(70001, "Terminal", 300.0), win(70001, "Terminal", 900.0)], &ALL, true);
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.bounds.unwrap().x), ("terminal-app", 10.0));
    }

    #[test]
    fn two_sessions_that_look_alike_are_not_guessed() {
        let t = Terminal { term_session: Some("SAME".into()), ..Default::default() };
        let env = "5100 claude TERM_SESSION_ID=SAME\n70004 claude TERM_SESSION_ID=SAME\n";
        let r = FakeRunner::default().on("/bin/ps -axo pid=,ppid=,uid=,tty=,comm=", PS).on("/bin/ps -Eww -o pid=,command= -p 5100,70004", env).on("/bin/ps -Eww -o pid=,command= -p 70004,5100", env);
        let (loc, _, _) = locator(r, vec![win(70001, "Terminal", 0.0)], &["process-tree"], false);
        assert_eq!(loc.locate(&t).confidence, Confidence::None);
    }

    #[test]
    fn no_terminal_headers_means_the_primary_monitor_and_a_click_just_hops() {
        let (loc, runner, _) = locator(FakeRunner::default(), vec![win(2100, "iTerm2", 0.0)], &ALL, true);
        assert_eq!(loc.locate(&Terminal::default()), Located::none());
        assert!(matches!(loc.focus(&Terminal::default()), Focus::Unavailable(_)));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn vscode_is_app_confidence_and_app_activate_opens_the_app() {
        let t = Terminal { program: Some("vscode".into()), ..Default::default() };
        let r = FakeRunner::default().on("/usr/bin/open -a Visual Studio Code", "");
        let (loc, _, _) = locator(r, vec![win(800, "Code", 500.0)], &ALL, false);
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence), ("vscode", Confidence::App));
        assert_eq!(loc.focus(&t), Focus::AppActivated("app-activate".into()));
    }

    #[test]
    fn a_claude_desktop_session_is_placed_on_and_focuses_the_claude_app() {
        // No terminal at all: only the entrypoint the desktop app sets.
        let t = Terminal { entrypoint: Some("claude-desktop".into()), ..Default::default() };
        let r = FakeRunner::default().on("/usr/bin/open -b com.anthropic.claudefordesktop", "");
        let (loc, runner, _) = locator(r, vec![win(900, "iTerm2", 0.0), win(901, "Claude", 2000.0)], &ALL, false);
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence, l.bounds.unwrap().x), ("app-activate", Confidence::App, 2000.0));
        assert_eq!(loc.focus(&t), Focus::AppActivated("app-activate".into()));
        assert!(!runner.calls().iter().any(|c| c.contains("/bin/ps") || c.starts_with("osascript")), "nothing terminal-specific is tried: {:?}", runner.calls());
    }

    #[test]
    fn a_terminal_session_with_the_cli_entrypoint_is_unchanged() {
        let t = Terminal { program: Some("ghostty".into()), entrypoint: Some("cli".into()), ..Default::default() };
        let r = FakeRunner::default().on("/usr/bin/open -a Ghostty", "");
        let (loc, _, _) = locator(r, vec![win(901, "Claude", 0.0), win(902, "Ghostty", 2000.0)], &ALL, false);
        assert_eq!(loc.locate(&t).bounds.unwrap().x, 2000.0);
        assert_eq!(loc.focus(&t), Focus::AppActivated("app-activate".into()));
        // An entrypoint alone that isn't the desktop app's reaches nothing, as before.
        let only = Terminal { entrypoint: Some("cli".into()), ..Default::default() };
        let (loc, _, _) = locator(FakeRunner::default(), vec![win(901, "Claude", 0.0)], &ALL, false);
        assert_eq!(loc.locate(&only).confidence, Confidence::None);
    }

    #[test]
    fn unknown_strategies_are_logged_once_and_skipped() {
        let (_, _, logs) = locator(FakeRunner::default(), vec![], &["tmux", "windows-terminal", "herdr", "itrem2"], false);
        let logs = logs.lock().unwrap();
        assert_eq!(logs.len(), 1, "other systems' strategies are quiet; a typo is not: {logs:?}");
        assert!(logs[0].contains("\"itrem2\" isn't built"));
    }

    #[test]
    fn herdr_ids_and_sockets_are_checked_before_herdr_runs() {
        for bad in ["--help", "-x", "", "w1:p1;rm", "w1 p1", "w1:p1\n", &"a".repeat(65)] {
            assert!(!valid_pane_id(bad), "{bad:?}");
        }
        for good in ["w1", "w4:t2", "w4:p2", "pane_1.2-x"] {
            assert!(valid_pane_id(good), "{good}");
        }
        let (loc, runner, logs) = locator(FakeRunner::default(), vec![], &["herdr"], false);
        loc.locate(&Terminal { herdr_pane: Some("--help".into()), herdr_socket: Some("/s/h".into()), ..Default::default() });
        loc.locate(&Terminal { herdr_pane: Some("w1:p1".into()), herdr_socket: Some("/tmp/other".into()), ..Default::default() });
        assert!(runner.calls().iter().all(|c| !c.starts_with("herdr")), "herdr never ran: {:?}", runner.calls());
        let logs = logs.lock().unwrap();
        assert!(logs.iter().any(|m| m.contains("refusing herdr pane id \"--help\"")));
        assert!(logs.iter().any(|m| m.contains("refusing herdr socket")));
    }

    #[test]
    fn ids_herdr_returns_are_checked_before_any_focus_command() {
        let bad = r#"{"result":{"pane":{"pane_id":"w4:p2","tab_id":"--evil","workspace_id":"w4"}}}"#;
        let r = FakeRunner::default().on("herdr pane get w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", bad);
        let (loc, runner, _) = locator(r, vec![], &["herdr"], false);
        assert!(matches!(loc.focus(&herdr_term()), Focus::Unavailable(_)));
        assert!(runner.calls().iter().all(|c| !c.contains("focus")), "{:?}", runner.calls());
    }

    #[test]
    fn the_back_off_list_drops_expired_entries_and_never_grows_past_its_cap() {
        let mut r = FakeRunner::default();
        for i in 0..100 {
            r = r.fail(&format!("herdr pane get w1:p1 [HERDR_SOCKET_PATH=/s/{i}]"), "herdr took longer than 4 s");
        }
        let (loc, _, _) = locator(r, vec![], &["herdr"], false);
        loc.backoff.lock().unwrap().insert("/s/old".into(), std::time::Instant::now() - std::time::Duration::from_secs(1));
        for i in 0..100 {
            loc.locate(&Terminal { herdr_pane: Some("w1:p1".into()), herdr_socket: Some(format!("/s/{i}")), ..Default::default() });
        }
        let b = loc.backoff.lock().unwrap();
        assert!(b.len() <= BACKOFF_CAP, "{} entries", b.len());
        assert!(!b.contains_key("/s/old"), "an expired entry was kept");
        assert!(b.contains_key("/s/99"), "the newest is kept");
    }

    #[test]
    fn a_socket_that_times_out_is_left_alone_for_five_minutes() {
        let r = FakeRunner::default().fail("herdr pane get w4:p2 [HERDR_SOCKET_PATH=/s/herdr.sock]", "herdr took longer than 4 s");
        let (loc, runner, logs) = locator(r, vec![], &["herdr"], false);
        loc.locate(&herdr_term());
        loc.locate(&herdr_term());
        assert_eq!(runner.calls().iter().filter(|c| c.starts_with("herdr pane get")).count(), 1, "asked once, then backed off");
        assert!(logs.lock().unwrap().iter().any(|m| m.contains("not asking it again for 5 minutes")));
    }

    #[cfg(unix)]
    #[test]
    fn the_real_socket_check_wants_our_own_socket_not_a_file_or_symlink() {
        let dir = std::env::temp_dir().join(format!("bellwether-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("h.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(check_socket(sock.to_str().unwrap()), Ok(()));
        let file = dir.join("file");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(check_socket(file.to_str().unwrap()), Err("is not a unix socket".into()));
        let link = dir.join("link");
        std::os::unix::fs::symlink(&sock, &link).unwrap();
        assert_eq!(check_socket(link.to_str().unwrap()), Err("is a symlink".into()));
        assert!(check_socket("relative/h.sock").is_err());
        assert!(check_socket("/tmp/../etc/x").is_err());
        assert!(check_socket(&format!("/{}", "a".repeat(1100))).is_err());
        assert!(check_socket(dir.join("missing").to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------ Windows (plan WD4, milestone W3)

    /// The PC's fixture (W1's dump, trimmed): herdr attached in Windows Terminal, the
    /// server under the client, five `claude.exe` under the server.
    const PC: &str = include_str!("../../../test/fixtures/windows/desktop-pc-herdr.json");
    const WT_HWND: u64 = 132162;

    struct FakeNative {
        procs: HashMap<i32, Proc>,
        env: HashMap<i32, Vec<(String, String)>>,
        node_claude: HashSet<i32>,
        refuse: bool,
        raised: Mutex<Vec<u64>>,
    }

    impl Native for FakeNative {
        fn processes(&self) -> HashMap<i32, Proc> {
            self.procs.clone()
        }
        fn environment(&self, pid: i32, keys: &[&str]) -> Option<Vec<(String, String)>> {
            let all = self.env.get(&pid)?;
            Some(all.iter().filter(|(k, _)| keys.iter().any(|w| w.eq_ignore_ascii_case(k))).cloned().collect())
        }
        fn command_line_has(&self, pid: i32, needle: &str) -> bool {
            needle == "claude-code" && self.node_claude.contains(&pid)
        }
        fn raise(&self, hwnd: u64) -> Result<(), String> {
            self.raised.lock().unwrap().push(hwnd);
            if self.refuse { Err("SetForegroundWindow".into()) } else { Ok(()) }
        }
    }

    /// The fixture's processes and windows, the windows through the real filter.
    fn pc(edit: impl FnOnce(&mut HashMap<i32, Proc>, &mut Vec<crate::dump::Window>)) -> (HashMap<i32, Proc>, Vec<Win>) {
        let v: Value = serde_json::from_str(PC).unwrap();
        let mut procs: HashMap<i32, Proc> = v["processes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let pid = p["pid"].as_i64().unwrap() as i32;
                (pid, Proc { pid, ppid: p["ppid"].as_i64().unwrap() as i32, uid: 0, tty: String::new(), comm: p["exe"].as_str().unwrap().to_string() })
            })
            .collect();
        let mut wins: Vec<crate::dump::Window> = v["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                let b: Vec<i32> = w["bounds"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as i32).collect();
                crate::dump::Window {
                    hwnd: w["hwnd"].as_u64().unwrap(),
                    pid: w["pid"].as_u64().unwrap() as u32,
                    exe: w["exe"].as_str().unwrap().into(),
                    class: w["class"].as_str().unwrap().into(),
                    bounds: [b[0], b[1], b[2], b[3]],
                    cloaked: w["cloaked"].as_bool().unwrap(),
                    tool: w["tool"].as_bool().unwrap(),
                    topmost: w["topmost"].as_bool().unwrap(),
                }
            })
            .collect();
        edit(&mut procs, &mut wins);
        let minimized: HashSet<u64> = wins.iter().filter(|w| w.bounds[0] <= -32000).map(|w| w.hwnd).collect();
        (procs, usable_windows(wins, 99999, |h| minimized.contains(&h)))
    }

    fn windows_locator(runner: FakeRunner, procs: HashMap<i32, Proc>, wins: Vec<Win>, env: &[(i32, &str, &str)], refuse: bool) -> (Locator, Arc<FakeRunner>, Arc<FakeNative>, Arc<Mutex<Vec<String>>>) {
        let runner = Arc::new(runner);
        let logs = Arc::new(Mutex::new(Vec::new()));
        let l2 = logs.clone();
        let mut envs: HashMap<i32, Vec<(String, String)>> = HashMap::new();
        for (pid, k, v) in env {
            envs.entry(*pid).or_default().push((k.to_string(), v.to_string()));
        }
        let native = Arc::new(FakeNative { procs, env: envs, node_claude: HashSet::new(), refuse, raised: Mutex::default() });
        let mut loc = Locator::new(
            runner.clone(),
            Arc::new(FakeWindows(wins)),
            Settings { strategies: ALL.iter().map(|s| s.to_string()).collect(), herdr_path: "herdr".into(), apple_script: true, uid: 0 },
            Box::new(move |m| l2.lock().unwrap().push(m.to_string())),
        )
        .with_native(native.clone());
        loc.socket_check = Box::new(|p| check_socket_windows(p, |_| false));
        (loc, runner, native, logs)
    }

    const WIN_PANE: &str = r#"{"result":{"pane":{"pane_id":"wH:p5","tab_id":"wH:t5","workspace_id":"wH"},"type":"pane_get"}}"#;

    fn win_herdr_term() -> Terminal {
        // As the PC's hooks sent it: herdr's pane, and a WT_SESSION inherited from the server.
        Terminal { herdr_pane: Some("wH:p5".into()), wt_session: Some("11111111-2222-3333-4444-555555555555".into()), ..Default::default() }
    }

    fn herdr_runner() -> FakeRunner {
        FakeRunner::default().on("herdr pane get wH:p5", WIN_PANE).on("herdr workspace focus wH", "").on("herdr tab focus wH:t5", "").on("herdr agent focus wH:p5", "")
    }

    #[test]
    fn windows_herdr_places_on_the_terminal_of_the_attached_client_and_a_click_raises_it() {
        let (procs, wins) = pc(|_, _| {});
        assert!(wins.iter().all(|w| !w.owner.eq_ignore_ascii_case("pwsh.exe")), "tool and empty windows are filtered");
        let (loc, runner, native, logs) = windows_locator(herdr_runner(), procs, wins, &[], false);
        let l = loc.locate(&win_herdr_term());
        assert_eq!((l.strategy.as_str(), l.confidence), ("herdr", Confidence::Exact), "{:?}", logs.lock().unwrap());
        assert_eq!(l.bounds, Some(Rect { x: 578.0, y: 432.0, w: 2041.0, h: 1547.0 }), "Windows Terminal's window, physical px");
        assert_eq!(loc.focus(&win_herdr_term()), Focus::Focused("herdr (wH:t5, pane wH:p5)".into()));
        assert_eq!(*native.raised.lock().unwrap(), vec![WT_HWND], "the click raises Windows Terminal");
        let calls = runner.calls();
        assert!(calls.iter().any(|c| c == "herdr agent focus wH:p5"), "{calls:?}");
        assert!(!calls.iter().any(|c| c.starts_with("/bin/ps") || c.contains("osascript") || c.starts_with("/usr/bin/open")), "no macOS command on Windows: {calls:?}");
        assert!(logs.lock().unwrap().iter().any(|m| m.contains("\"iterm2\" isn't built on this OS")));
        assert_eq!(loc.strategies(), ["herdr", "vscode", "process-tree", "app-activate"]);
    }

    #[test]
    fn windows_herdr_detached_finds_no_window_and_says_so_on_a_click() {
        // The client (14628) is gone; the server's parent no longer exists.
        let (procs, wins) = pc(|p, _| {
            p.remove(&14628);
        });
        let (loc, _, native, logs) = windows_locator(herdr_runner(), procs, wins, &[], false);
        let l = loc.locate(&win_herdr_term());
        assert_ne!(l.strategy, "herdr", "no host window");
        assert_ne!(l.strategy, "process-tree", "a herdr session never matches by its inherited WT_SESSION");
        assert_eq!(l.strategy, "app-activate", "Windows Terminal's window, app confidence");
        assert_eq!(l.confidence, Confidence::App);
        assert_eq!(loc.focus(&win_herdr_term()), Focus::Focused("herdr (wH:t5, pane wH:p5)".into()));
        assert!(native.raised.lock().unwrap().is_empty());
        assert!(logs.lock().unwrap().iter().any(|m| m.contains("no attached herdr client has a window")), "{:?}", logs.lock().unwrap());
    }

    #[test]
    fn windows_two_attached_herdr_clients_are_app_confidence() {
        let (procs, wins) = pc(|p, _| {
            p.insert(70000, Proc { pid: 70000, ppid: 38664, uid: 0, tty: String::new(), comm: "herdr.exe".into() });
        });
        let (loc, _, _, _) = windows_locator(herdr_runner(), procs, wins, &[], false);
        let l = loc.locate(&win_herdr_term());
        assert_eq!((l.strategy.as_str(), l.confidence), ("herdr", Confidence::App));
    }

    /// A plain session: claude.exe under pwsh under Windows Terminal, outside herdr.
    fn plain(p: &mut HashMap<i32, Proc>, w: &mut Vec<crate::dump::Window>, second_window: bool) {
        p.insert(80001, Proc { pid: 80001, ppid: 38664, uid: 0, tty: String::new(), comm: "claude.exe".into() });
        if second_window {
            let mut other = w.iter().find(|x| x.hwnd == WT_HWND).unwrap().clone();
            other.hwnd = 555;
            other.bounds = [5200, -900, 7200, 1000];
            w.insert(0, other); // in front
        }
    }

    const WT: &str = "aaaaaaaa-0000-0000-0000-000000000001";

    #[test]
    fn windows_process_tree_matches_wt_session_and_raises_the_terminal() {
        let (procs, wins) = pc(|p, w| plain(p, w, false));
        let (loc, _, native, _) = windows_locator(FakeRunner::default(), procs, wins, &[(80001, "WT_SESSION", WT), (28568, "WT_SESSION", "something-else")], false);
        let t = Terminal { wt_session: Some(WT.into()), ..Default::default() };
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence), ("process-tree", Confidence::Exact));
        assert_eq!(loc.focus(&t), Focus::Focused("process-tree".into()));
        assert_eq!(*native.raised.lock().unwrap(), vec![WT_HWND]);
    }

    #[test]
    fn windows_terminal_with_two_windows_is_app_confidence_and_two_lookalikes_are_not_guessed() {
        let (procs, wins) = pc(|p, w| plain(p, w, true));
        let (loc, _, _, _) = windows_locator(FakeRunner::default(), procs.clone(), wins.clone(), &[(80001, "WT_SESSION", WT)], false);
        let t = Terminal { wt_session: Some(WT.into()), ..Default::default() };
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence), ("process-tree", Confidence::App), "one WindowsTerminal.exe owns both windows");
        let (loc, _, _, _) = windows_locator(FakeRunner::default(), procs, wins, &[(80001, "WT_SESSION", WT), (28568, "WT_SESSION", WT)], false);
        assert_eq!(loc.locate(&t).strategy, "app-activate", "two processes match: process-tree doesn't guess");
    }

    #[test]
    fn windows_a_session_whose_walk_reaches_explorer_falls_back_to_app_activate() {
        // Started from a shortcut and handed to Windows Terminal (defterm): pwsh's parent is
        // explorer.exe, whose File Explorer windows never count.
        let (procs, wins) = pc(|p, _| {
            p.insert(81000, Proc { pid: 81000, ppid: 7428, uid: 0, tty: String::new(), comm: "pwsh.exe".into() });
            p.insert(81001, Proc { pid: 81001, ppid: 81000, uid: 0, tty: String::new(), comm: "claude.exe".into() });
        });
        let (loc, _, native, _) = windows_locator(FakeRunner::default(), procs, wins, &[(81001, "WT_SESSION", WT)], false);
        let t = Terminal { wt_session: Some(WT.into()), ..Default::default() };
        let l = loc.locate(&t);
        assert_eq!((l.strategy.as_str(), l.confidence), ("app-activate", Confidence::App));
        assert_eq!(loc.focus(&t), Focus::AppActivated("app-activate".into()));
        assert_eq!(*native.raised.lock().unwrap(), vec![WT_HWND]);
    }

    #[test]
    fn windows_a_minimised_terminal_is_raised_but_not_used_for_placement_and_a_refusal_is_logged() {
        let (procs, wins) = pc(|p, w| {
            plain(p, w, false);
            w.iter_mut().find(|x| x.hwnd == WT_HWND).unwrap().bounds = [-32000, -32000, -31840, -31972];
        });
        let (loc, _, native, logs) = windows_locator(FakeRunner::default(), procs, wins, &[(80001, "WT_SESSION", WT)], true);
        let t = Terminal { wt_session: Some(WT.into()), ..Default::default() };
        assert_eq!(loc.locate(&t).bounds, None, "a minimised window says nothing about a monitor");
        assert_eq!(loc.focus(&t), Focus::Focused("process-tree".into()));
        assert_eq!(*native.raised.lock().unwrap(), vec![WT_HWND], "raised once (it restores a minimised window); no second try");
        assert!(logs.lock().unwrap().iter().any(|m| m.contains("foreground refused for WindowsTerminal.exe")), "{:?}", logs.lock().unwrap());
    }

    #[test]
    fn windows_claude_run_by_node_counts_and_another_users_process_doesnt() {
        let t = Terminal { wt_session: Some(WT.into()), ..Default::default() };
        let (procs, wins) = pc(|p, _| {
            p.insert(82001, Proc { pid: 82001, ppid: 38664, uid: 0, tty: String::new(), comm: "node.exe".into() });
            p.insert(82002, Proc { pid: 82002, ppid: 38664, uid: u32::MAX, tty: String::new(), comm: "claude.exe".into() });
        });
        let env = [(82001, "WT_SESSION", WT), (82002, "WT_SESSION", WT)];
        let (loc, _, _, _) = windows_locator(FakeRunner::default(), procs.clone(), wins.clone(), &env, false);
        assert_eq!(loc.locate(&t).strategy, "app-activate", "neither: node.exe without Claude Code, and another user's claude.exe");
        let (loc, _, native, _) = windows_locator(FakeRunner::default(), procs, wins, &env, false);
        let native = Arc::new(FakeNative { node_claude: [82001].into_iter().collect(), procs: native.procs.clone(), env: native.env.clone(), refuse: false, raised: Mutex::default() });
        let loc = loc.with_native(native);
        assert_eq!(loc.locate(&t).strategy, "process-tree", "node.exe running Claude Code's cli.js is the session");
    }

    #[test]
    fn windows_app_activate_knows_the_claude_app_vscode_and_windows_terminal() {
        let t = |program: Option<&str>, wt: bool, entry: Option<&str>| Terminal { program: program.map(Into::into), wt_session: wt.then(|| WT.to_string()), entrypoint: entry.map(Into::into), ..Default::default() };
        assert_eq!(Locator::app_exe(&t(None, false, Some("claude-desktop"))), Some("claude.exe"));
        assert_eq!(Locator::app_exe(&t(Some("vscode"), true, None)), Some("Code.exe"));
        assert_eq!(Locator::app_exe(&t(Some("WezTerm"), false, None)), Some("wezterm-gui.exe"));
        assert_eq!(Locator::app_exe(&t(None, true, None)), Some("WindowsTerminal.exe"));
        assert_eq!(Locator::app_exe(&t(Some("herdr"), false, None)), None);
    }

    #[test]
    fn windows_environment_blocks_keep_only_the_marker_variables() {
        let block: Vec<u16> = "=C:=C:\\work\0PATH=C:\\bin\0wt_session=abc\0HERDR_PANE_ID=w1:p2\0SECRET=x\0\0junk=1\0".encode_utf16().collect();
        assert_eq!(env_values(&block, &["WT_SESSION", "HERDR_PANE_ID"]), vec![("WT_SESSION".to_string(), "abc".to_string()), ("HERDR_PANE_ID".to_string(), "w1:p2".to_string())]);
        assert!(env_values(&block, &["SECRET_NOT_ASKED"]).is_empty());
        assert!(env_values(&[], &["WT_SESSION"]).is_empty());
    }

    #[test]
    fn windows_herdr_sockets_are_pipe_names_or_existing_absolute_files() {
        let none = |_: &str| false;
        assert!(check_socket_windows(r"\\.\pipe\herdr-1000", none).is_ok());
        assert!(check_socket_windows(r"\\.\pipe\herdr\sock.v1", none).is_ok());
        for bad in [r"\\.\pipe\", r"\\.\pipe\..\x", r"\\.\pipe\a b", r"\\server\pipe\herdr", "herdr.sock", r"C:\x\..\y", r"\\?\C:\h"] {
            assert!(check_socket_windows(bad, |_| true).is_err(), "{bad}");
        }
        assert!(check_socket_windows(r"C:\Users\me\.herdr\herdr.sock", |_| true).is_ok());
        assert!(check_socket_windows(r"C:\Users\me\.herdr\herdr.sock", none).is_err(), "must exist");
    }

    #[test]
    fn program_names_match_with_or_without_exe_in_any_case() {
        assert!(is_program("claude", "claude"));
        assert!(is_program("/Users/x/.local/bin/herdr", "herdr"));
        assert!(is_program("CLAUDE.EXE", "claude"));
        assert!(is_program(r"C:\x\herdr.exe", "herdr"));
        assert!(!is_program("claude-helper.exe", "claude"));
        assert!(!is_program(".exe", "claude"));
    }

    #[test]
    fn ps_lines_with_spaces_in_the_command_parse() {
        let p = parse_ps(PS);
        assert_eq!(p[&2101].comm, "/Users/example/Library/Application Support/iTerm2/iTermServer-3.0.0");
        assert_eq!((p[&4100].ppid, p[&4100].tty.as_str()), (3101, "ttys003"));
        assert_eq!(app_bundle(&p[&2100].comm).as_deref(), Some("/Applications/iTerm.app"));
        assert_eq!(safe_tty("ttys003"), "ttys003");
        assert_eq!(safe_tty("ttys003\" & x"), "-");
    }

    #[test]
    fn monitors_by_overlap_then_nearest() {
        let mons = vec![
            Monitor { name: "Built-in".into(), rect: Rect { x: 0.0, y: 0.0, w: 1728.0, h: 1117.0 }, primary: true },
            Monitor { name: "DELL".into(), rect: Rect { x: 1728.0, y: -200.0, w: 2560.0, h: 1440.0 }, primary: false },
        ];
        let on = |x: f64| monitor_for(&Rect { x, y: 100.0, w: 1000.0, h: 600.0 }, &mons).unwrap().name.clone();
        assert_eq!(on(100.0), "Built-in");
        assert_eq!(on(2500.0), "DELL");
        assert_eq!(on(1400.0), "DELL", "most of it on the DELL");
        assert_eq!(on(9000.0), "DELL", "off every monitor: the nearest");
        assert!(monitor_for(&Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, &[]).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_window_list_reads_without_permission() {
        // Whatever is on screen (maybe nothing in CI); it must not crash or prompt.
        let wins = RealWindows.list();
        for w in wins {
            assert!(w.bounds.w >= 50.0 && w.bounds.h >= 50.0);
        }
    }
}
