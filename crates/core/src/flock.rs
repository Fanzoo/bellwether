//! The flock: sheep (sessions) and lambs (subagents), their calm states and their alerts.
//! A port of brain/flock.mjs and brain/identity.mjs, pinned to them by the 21 golden
//! fixtures (byte-identical transition lines; see [`crate::fixture`]) and the flock tests,
//! which run against both brains.
//!
//! A pure reducer: nothing here reads the clock. `apply(ev)` takes the time from
//! `ev.receivedAt` and `tick(t)` re-evaluates at `t`. Times are f64 milliseconds, as in
//! JS, NaN included (an event without a time compares false everywhere, as in Node).
//!
//! Calm states: arriving, busy (flavour running | digging | reading | thinking), done,
//! idle, waiting, sleeping, compacting, error (flavour = error kind), leaving. Each is held
//! for at least `behaviour.minDwellMs`. Alerts are an overlay with absolute priority.
//!
//! The order of everything that is a JS `Map` (sheep, lambs, identities, tools in flight)
//! is kept: an IndexMap, removed from with `shift_remove`, behaves like one.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::json::{number_to_string, Js, Obj};
use crate::jsstr;
use crate::normalise::{HookEvent, AGENT_TYPE_CHARS};
use crate::redact::{clean_label, truncate};

const PENDING_TTL_MS: f64 = 15.0 * 60_000.0;
const PENDING_CAP: usize = 256;
const PROGRESS_GUARD_MS: f64 = 2000.0;
const NOTIFY_AFTER_CLEAR_GUARD_MS: f64 = 2000.0;
const EFFECT_MS: f64 = 1500.0;
const IDENTITY_TTL_MS: f64 = 7.0 * 24.0 * 3600_000.0;

/// Bounds on what a token holder (or a hand-edited state.json) can make the brain keep.
pub mod limits {
    pub const IDENTITIES: usize = 2000;
    pub const MONITORS_PER_IDENTITY: usize = 16;
    pub const MONITOR_NAME_CHARS: usize = 4096;
    pub const IDENTITY_KEY_CHARS: usize = 1024;
    pub const LABEL_CHARS: usize = 200;
    pub const LAMBS_PER_SHEEP: usize = 64;
    pub const SHEEP: usize = 200;
}

/// LIMITS as Node exports it.
pub fn limits_js() -> Js {
    use limits::*;
    Js::Obj(
        Obj::new()
            .with("identities", IDENTITIES as f64)
            .with("monitorsPerIdentity", MONITORS_PER_IDENTITY as f64)
            .with("monitorNameChars", MONITOR_NAME_CHARS as f64)
            .with("identityKeyChars", IDENTITY_KEY_CHARS as f64)
            .with("labelChars", LABEL_CHARS as f64)
            .with("lambsPerSheep", LAMBS_PER_SHEEP as f64)
            .with("sheep", SHEEP as f64),
    )
}

/// The flavour a tool gives a busy actor. Only these names: a tool called `constructor`
/// is thinking here (test/DIVERGENCES.md, Object.prototype names).
fn category_of(tool: Option<&str>) -> &'static str {
    match tool {
        Some("Bash" | "BashOutput" | "KillShell") => "running",
        Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit") => "digging",
        Some("Read" | "Grep" | "Glob" | "LS" | "WebFetch" | "WebSearch") => "reading",
        _ => "thinking",
    }
}

/// `Math.max`, NaN-propagating like JS.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// A comparator's number as an ordering, NaN as equal (what `Array.prototype.sort` does).
fn order(d: f64) -> std::cmp::Ordering {
    if d < 0.0 {
        std::cmp::Ordering::Less
    } else if d > 0.0 {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

/// The flock's view of the config.
#[derive(Clone, Debug)]
pub struct FlockConfig {
    pub busy_window_ms: f64,
    pub graze_after_ms: f64,
    pub sleep_after_ms: f64,
    pub min_dwell_ms: f64,
    pub reap_after_ms: f64,
    pub lamb_timeout_ms: f64,
    pub tool_timeout_ms: f64,
    pub thinking_timeout_ms: f64,
    pub max_lambs_shown: usize,
    pub palette_size: usize,
    pub detail_full: bool,
    pub max_chars: usize,
}

impl FlockConfig {
    /// From the merged config (crate::config::with_defaults).
    pub fn from_merged(c: &Js) -> FlockConfig {
        let b = c.get("behaviour");
        let n = |k: &str| b.get(k).as_num().unwrap_or(0.0);
        FlockConfig {
            busy_window_ms: n("busyWindowSec") * 1000.0,
            graze_after_ms: n("grazeAfterSec") * 1000.0,
            sleep_after_ms: n("sleepAfterMin") * 60_000.0,
            min_dwell_ms: n("minDwellMs"),
            reap_after_ms: n("reapAfterHours") * 3600_000.0,
            lamb_timeout_ms: n("lambTimeoutMin") * 60_000.0,
            tool_timeout_ms: n("toolTimeoutMin") * 60_000.0,
            thinking_timeout_ms: n("thinkingTimeoutSec") * 1000.0,
            max_lambs_shown: n("maxLambsShown") as usize,
            palette_size: c.get("pets").get("palette").as_arr().map_or(1, Vec::len),
            detail_full: c.get("bubble").get("detail").as_str() != Some("tool"),
            max_chars: c.get("bubble").get("maxChars").as_num().unwrap_or(60.0) as usize,
        }
    }
}

// ------------------------------------------------------------------ identity

/// The sheep identity key and whether it comes from the terminal (identityOf). The
/// innermost multiplexer wins: herdr pane, tmux pane, iTerm2 tab (the uuid part), Terminal.app
/// (only with TERM_PROGRAM=Apple_Terminal), Windows Terminal, else the session.
pub fn identity_of(terminal: &Obj, session_id: Option<&str>) -> (String, bool) {
    let t = |k: &str| terminal.get(k).and_then(Js::as_str).filter(|s| !s.is_empty());
    if let Some(pane) = t("herdrPane") {
        return (format!("herdr:{}#{pane}", terminal.get("herdrSocket").and_then(Js::as_str).unwrap_or("")), true);
    }
    if let Some(pane) = t("tmuxPane") {
        return (format!("tmux:{pane}"), true);
    }
    if let Some(iterm) = t("itermSession") {
        return (format!("iterm:{}", strip_iterm_prefix(iterm)), true);
    }
    if let (Some(term), Some("Apple_Terminal")) = (t("termSession"), terminal.get("program").and_then(Js::as_str)) {
        return (format!("term:{term}"), true);
    }
    if let Some(wt) = t("wtSession") {
        return (format!("wt:{wt}"), true);
    }
    (format!("session:{}", session_id.unwrap_or("unknown")), false)
}

/// `replace(/^w\d+t\d+p\d+:/, '')`: the wNtNpN prefix moves with the tab, the uuid doesn't.
fn strip_iterm_prefix(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    for letter in [b'w', b't', b'p'] {
        if b.get(i) != Some(&letter) {
            return s;
        }
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return s;
        }
    }
    if b.get(i) == Some(&b':') {
        &s[i + 1..]
    } else {
        s
    }
}

/// A short, stable, opaque actor id for the snapshot.
pub fn sheep_id(identity_key: &str) -> String {
    let d = Sha256::digest(jsstr::utf8_bytes(identity_key));
    let hex: String = d.iter().take(5).map(|b| format!("{b:02x}")).collect();
    format!("s-{hex}")
}

// ------------------------------------------------------------------ actors

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub kind: &'static str,
    pub text: String,
    pub since: f64,
    pub tool_use_id: Option<String>,
    pub fingerprint: Option<String>,
    pub confirmed: bool,
}

#[derive(Clone, Debug)]
struct Shown {
    state: &'static str,
    flavour: Option<String>,
    since: f64,
}

#[derive(Clone, Debug)]
struct InFlight {
    tool_name: Option<String>,
    at: f64,
}

#[derive(Clone, Debug)]
struct Actor {
    lamb: bool,
    id: String,
    created: f64,
    last_event_at: f64,
    last_activity_at: f64,
    last_tool_at: f64,
    last_tool_name: Option<String>,
    turn_active: bool,
    in_flight: IndexMap<String, InFlight>,
    stopped_at: Option<f64>,
    waiting_since: Option<f64>,
    error: Option<String>,
    leaving_since: Option<f64>,
    alert: Option<Alert>,
    effect: Option<(&'static str, f64)>,
    shown: Shown,
    /// Sheep only (a lamb's is always false).
    compacting: bool,
    /// Lambs only.
    alive: bool,
}

impl Actor {
    fn new(lamb: bool, id: String, t: f64) -> Actor {
        Actor {
            lamb,
            id,
            created: t,
            last_event_at: t,
            last_activity_at: t,
            last_tool_at: f64::NEG_INFINITY,
            last_tool_name: None,
            turn_active: false,
            in_flight: IndexMap::new(),
            stopped_at: None,
            waiting_since: None,
            error: None,
            leaving_since: None,
            alert: None,
            effect: None,
            shown: Shown { state: "arriving", flavour: None, since: t },
            compacting: false,
            alive: lamb,
        }
    }
}

#[derive(Clone, Debug)]
struct Lamb {
    a: Actor,
    agent_id: String,
    agent_type: String,
}

#[derive(Clone, Debug)]
struct Pending {
    tool_use_id: Option<String>,
    fingerprint: Option<String>,
    agent_id: Option<String>,
    actor_id: String,
    at: f64,
    tool_name: Option<String>,
    tool_summary: Option<String>,
}

#[derive(Clone, Debug)]
struct Sheep {
    a: Actor,
    identity_key: String,
    terminal_based: bool,
    session_id: Option<String>,
    label: String,
    terminal: Obj,
    lambs: IndexMap<String, Lamb>,
    pending: Vec<Pending>,
    colour_index: i64,
    slot: i64,
    last_permission_clear_at: f64,
}

/// Which actor of a sheep: the sheep itself, or the lamb with this agent id.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sel {
    Sheep,
    Lamb(String),
}

impl Sheep {
    fn actor(&mut self, sel: &Sel) -> &mut Actor {
        match sel {
            Sel::Sheep => &mut self.a,
            Sel::Lamb(id) => &mut self.lambs.get_mut(id).expect("a lamb of this sheep").a,
        }
    }

    /// `[s, ...s.lambs.values()].filter((a) => a.alert)`
    fn alerting(&self) -> Vec<Sel> {
        let mut out = Vec::new();
        if self.a.alert.is_some() {
            out.push(Sel::Sheep);
        }
        for (id, l) in &self.lambs {
            if l.a.alert.is_some() {
                out.push(Sel::Lamb(id.clone()));
            }
        }
        out
    }

    /// An actor of this sheep by snapshot id: events never reach across sessions.
    fn actor_in(&self, id: &str) -> Option<Sel> {
        if self.a.id == id {
            return Some(Sel::Sheep);
        }
        self.lambs.iter().find(|(_, l)| l.a.id == id).map(|(k, _)| Sel::Lamb(k.clone()))
    }

    fn clear(&mut self, sel: &Sel, t: f64) {
        if self.actor(sel).alert.as_ref().is_some_and(|a| a.kind == "permission") {
            self.last_permission_clear_at = t;
        }
        self.actor(sel).alert = None;
    }

    fn reset_turn(&mut self) {
        let a = &mut self.a;
        a.turn_active = false;
        a.in_flight.clear();
        a.stopped_at = None;
        a.waiting_since = None;
        a.error = None;
        a.compacting = false;
    }

    fn leave(&mut self, t: f64) {
        self.a.leaving_since.get_or_insert(t);
        self.a.alert = None;
        for l in self.lambs.values_mut() {
            l.a.alert = None;
            l.a.leaving_since.get_or_insert(t);
        }
    }
}

/// One persisted identity: colour, slots per monitor, label, last seen.
#[derive(Clone, Debug)]
struct Ident {
    colour_index: Option<i64>,
    slots: Obj,
    label: Option<String>,
    last_seen: f64,
}

// ------------------------------------------------------------------ the flock

pub struct Flock {
    cfg: FlockConfig,
    sheep: IndexMap<String, Sheep>,
    sessions: HashMap<String, String>,
    pub muted: bool,
    identities: IndexMap<String, Ident>,
    pub persist_dirty: bool,
}

/// What `/control` answers: `{ok: true}` or `{ok: false, error}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlResult {
    pub ok: bool,
    pub error: Option<String>,
}

impl ControlResult {
    fn ok() -> ControlResult {
        ControlResult { ok: true, error: None }
    }
    fn err(e: impl Into<String>) -> ControlResult {
        ControlResult { ok: false, error: Some(e.into()) }
    }
    pub fn to_js(&self) -> Js {
        let mut o = Obj::new().with("ok", self.ok);
        if let Some(e) = &self.error {
            o.set("error", e.as_str());
        }
        Js::Obj(o)
    }
}

/// The handlers the reducer has, by event name (its own names only).
const HANDLED: [&str; 16] = [
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "Notification",
    "Elicitation",
    "ElicitationResult",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionDenied",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

impl Flock {
    /// `new Flock({config, persisted, t})`. `persisted` is state.json's contents (or
    /// null); every field is checked for type and range, and anything else is dropped.
    pub fn new(cfg: FlockConfig, persisted: &Js, t: f64) -> Flock {
        let muted = persisted.get("muted") == &Js::Bool(true);
        let mut loaded: Vec<(String, Ident)> = Vec::new();
        if let Js::Obj(raw) = persisted.get("identities") {
            for (k, v) in raw.iter() {
                if let Some(ident) = sanitise_identity(k, v, t, cfg.palette_size) {
                    loaded.push((k.clone(), ident));
                }
            }
        }
        loaded.sort_by(|a, b| order(b.1.last_seen - a.1.last_seen));
        let identities = loaded.into_iter().take(limits::IDENTITIES).collect();
        Flock { cfg, sheep: IndexMap::new(), sessions: HashMap::new(), muted, identities, persist_dirty: false }
    }

    // ---------------------------------------------------------------- events

    /// Applies one normalised HookEvent. Returns true if anything may have changed.
    pub fn apply(&mut self, ev: &HookEvent) -> bool {
        let t = ev.received_at;
        let Some(session) = ev.session_id().filter(|s| !s.is_empty()) else { return false };
        if ev.event == "unknown" || ev.event.is_empty() || !HANDLED.contains(&ev.event.as_str()) {
            return false;
        }
        // A session that ends before we ever saw it (brain started late) gets no sheep.
        if ev.event == "SessionEnd" && !self.sessions.contains_key(session) {
            // Except the session after a /clear or a resume, which can end before its first
            // prompt (Claude Code sends no hook at a session's start): the sheep its pane kept
            // for it leaves, or it would stay until the brain restarts.
            let (key, terminal_based) = identity_of(&ev.terminal, Some(session));
            match self.sheep.get_mut(&key).filter(|s| terminal_based && s.session_id.is_none()) {
                Some(s) => {
                    s.leave(t);
                    self.evaluate(t);
                    return true;
                }
                None => return false,
            }
        }
        let key = self.sheep_for(ev, session, t);
        self.handle(&key, ev, t);
        self.evaluate(t);
        true
    }

    /// Re-evaluates every actor at time t: states, effects, leaving, reaping.
    pub fn tick(&mut self, t: f64) {
        let cfg = self.cfg.clone();
        let keys: Vec<String> = self.sheep.keys().cloned().collect();
        for key in keys {
            let s = self.sheep.get_mut(&key).unwrap();
            if t - s.a.last_event_at > cfg.reap_after_ms {
                self.forget(&key);
                continue;
            }
            s.pending.retain(|p| t - p.at < PENDING_TTL_MS);
            // A tool whose PostToolUse never came, and a lamb whose SubagentStop never
            // came, time out. An alert keeps a lamb: there is no timeout on alerts.
            s.a.in_flight.retain(|_, f| !(t - f.at >= cfg.tool_timeout_ms));
            for l in s.lambs.values_mut() {
                l.a.in_flight.retain(|_, f| !(t - f.at >= cfg.tool_timeout_ms));
            }
            for l in s.lambs.values_mut() {
                if l.a.leaving_since.is_none() && l.a.alert.is_none() && t - l.a.last_event_at >= cfg.lamb_timeout_ms {
                    l.a.alive = false;
                    l.a.in_flight.clear();
                    l.a.leaving_since = Some(t);
                }
            }
        }
        self.evaluate(t);
    }

    /// `/control` actions from the shell or the CLI.
    pub fn control(&mut self, msg: &Js, t: f64) -> ControlResult {
        match msg.get("action").as_str() {
            Some(a @ ("mute" | "unmute")) => {
                self.muted = a == "mute";
                self.persist_dirty = true;
                ControlResult::ok()
            }
            Some("dismiss") => {
                let Some(id) = msg.get("actorId").as_str() else { return ControlResult::err("no such actor") };
                for s in self.sheep.values_mut() {
                    if let Some(sel) = s.actor_in(id) {
                        s.actor(&sel).alert = None;
                        return ControlResult::ok();
                    }
                }
                ControlResult::err("no such actor")
            }
            Some("placed") => {
                let bad = "placed needs a known identityKey, a monitor name and a slot from 0 to 10000";
                let Some(ident) = msg.get("identityKey").as_str().and_then(|k| self.identities.get_mut(k)) else { return ControlResult::err(bad) };
                let monitor = msg.get("monitor");
                let slot = msg.get("slot");
                let Some(m) = monitor.as_str().filter(|m| !m.is_empty()) else { return ControlResult::err(bad) };
                if !slot.is_integer() || !(0.0..=10_000.0).contains(&slot.as_num().unwrap()) {
                    return ControlResult::err(bad);
                }
                if jsstr::len(m) > limits::MONITOR_NAME_CHARS {
                    return ControlResult::err(format!("monitor name longer than {} characters", limits::MONITOR_NAME_CHARS));
                }
                if !ident.slots.contains(m) && ident.slots.len() >= limits::MONITORS_PER_IDENTITY {
                    return ControlResult::err(format!("at most {} monitors per sheep", limits::MONITORS_PER_IDENTITY));
                }
                ident.slots.set(m, slot.clone());
                ident.last_seen = t;
                self.persist_dirty = true;
                ControlResult::ok()
            }
            _ => ControlResult::err("unknown action"),
        }
    }

    // -------------------------------------------------------------- snapshot

    /// One mark per open alert and whether it is confirmed: a mark that wasn't there
    /// before an event means a new or newly confirmed alert (published at once).
    pub fn alert_marks(&self) -> HashSet<String> {
        let mark = |a: &Alert| format!("{}\0{}\0{}", a.kind, number_to_string(a.since), a.confirmed);
        let mut marks = HashSet::new();
        for (key, s) in &self.sheep {
            if let Some(a) = &s.a.alert {
                marks.insert(format!("{key}\0\0{}", mark(a)));
            }
            for (id, l) in &s.lambs {
                if let Some(a) = &l.a.alert {
                    marks.insert(format!("{key}\0{id}\0{}", mark(a)));
                }
            }
        }
        marks
    }

    /// The shell's view: `{muted, flock}`, already truncated and redacted text only.
    pub fn snapshot(&self) -> Js {
        let mut sheep: Vec<&Sheep> = self.sheep.values().collect();
        sheep.sort_by(|a, b| {
            let d = (a.slot - b.slot) as f64;
            if d != 0.0 {
                order(d)
            } else {
                order(a.a.created - b.a.created)
            }
        });
        let flock: Vec<Js> = sheep
            .into_iter()
            .map(|s| {
                let mut lambs: Vec<&Lamb> = s.lambs.values().collect();
                lambs.sort_by(|a, b| {
                    let d = (b.a.alert.is_some() as i32 - a.a.alert.is_some() as i32) as f64;
                    if d != 0.0 {
                        order(d)
                    } else {
                        order(a.a.created - b.a.created)
                    }
                });
                let shown: Vec<&&Lamb> = lambs.iter().take(self.cfg.max_lambs_shown).collect();
                let slots = self.identities.get(&s.identity_key).map(|i| i.slots.clone()).unwrap_or_default();
                let mut o = Obj::new()
                    .with("id", s.a.id.as_str())
                    .with("identityKey", s.identity_key.as_str())
                    .with("label", s.label.as_str())
                    .with("colourIndex", s.colour_index as f64)
                    .with("slot", s.slot as f64)
                    .with("slots", Js::Obj(slots))
                    .with("state", s.a.shown.state)
                    .with("flavour", s.a.shown.flavour.as_deref())
                    .with("since", s.a.shown.since)
                    .with("alert", alert_view(&s.a.alert))
                    .with("effect", effect_view(&s.a.effect))
                    .with("terminal", Js::Obj(s.terminal.clone()));
                if lambs.iter().any(|l| l.a.alert.is_some()) {
                    o.set("lambAlert", true);
                }
                let lamb_views: Vec<Js> = shown
                    .iter()
                    .map(|l| {
                        let lo = Obj::new()
                            .with("id", l.a.id.as_str())
                            .with("agentType", l.agent_type.as_str())
                            .with("label", l.agent_type.as_str())
                            .with("state", l.a.shown.state)
                            .with("flavour", l.a.shown.flavour.as_deref())
                            .with("alert", alert_view(&l.a.alert))
                            .with("effect", effect_view(&l.a.effect));
                        clean(lo)
                    })
                    .collect();
                o.set("lambs", Js::Arr(lamb_views));
                o.set("hiddenLambCount", (lambs.len() - shown.len()) as f64);
                clean(o)
            })
            .collect();
        Js::Obj(Obj::new().with("muted", self.muted).with("flock", Js::Arr(flock)))
    }

    /// What state.json keeps between runs. `with_undefined` writes an identity's missing
    /// colour or label as `Js::Undefined`, the way Node's object holds them (the test
    /// bridge needs it); `stringify` leaves them out either way.
    pub fn persistable(&self) -> Js {
        let mut ids = Obj::new();
        for (k, i) in &self.identities {
            ids.set(
                k.clone(),
                Obj::new().with("colourIndex", i.colour_index.map(|c| c as f64)).with("slots", Js::Obj(i.slots.clone())).with("label", i.label.as_deref()).with("lastSeen", i.last_seen),
            );
        }
        Js::Obj(Obj::new().with("version", 1.0).with("muted", self.muted).with("identities", Js::Obj(ids)))
    }

    /// The identity keys, in their Map order (tests).
    pub fn identity_keys(&self) -> Vec<String> {
        self.identities.keys().cloned().collect()
    }

    // -------------------------------------------------------------- internals

    fn sheep_for(&mut self, ev: &HookEvent, session: &str, t: f64) -> String {
        let (key, terminal_based) = identity_of(&ev.terminal, Some(session));
        if let Some(prev) = self.sessions.get(session).cloned() {
            if prev != key {
                // The same session now reports another terminal (resumed in another tab).
                if let Some(old) = self.sheep.get_mut(&prev) {
                    if old.session_id.as_deref() == Some(session) {
                        old.leave(t);
                    }
                }
            }
        }
        if !self.sheep.contains_key(&key) {
            self.cap_sheep();
            let s = self.new_sheep(&key, terminal_based, t);
            self.sheep.insert(key.clone(), s);
        }
        let Flock { sheep, sessions, identities, .. } = self;
        let s = sheep.get_mut(&key).unwrap();
        if s.session_id.as_deref() != Some(session) {
            // A newer session in the same pane takes the sheep over (after /clear, or a new
            // `claude` in the same tab): nothing of the old session's alerts or lambs survives.
            if let Some(old) = s.session_id.take() {
                sessions.remove(&old);
            }
            s.session_id = Some(session.to_string());
            s.a.alert = None;
            s.lambs.clear();
            s.pending.clear();
            s.reset_turn();
            s.a.leaving_since = None;
        }
        sessions.insert(session.to_string(), key.clone());
        s.a.last_event_at = t;
        if let Some(label) = ev.project_label.as_deref().and_then(|l| clean_label(l, limits::LABEL_CHARS)) {
            s.label = label;
        }
        if !ev.terminal.is_empty() {
            s.terminal = ev.terminal.clone();
        }
        let ident = identities.get_mut(&key).expect("a present sheep's identity is kept");
        ident.last_seen = t;
        ident.label = Some(jsstr::prefix(&s.label, limits::LABEL_CHARS).to_string());
        self.persist_dirty = true;
        key
    }

    /// At most LIMITS.sheep present: the least recently seen without an alert is forgotten.
    fn cap_sheep(&mut self) {
        if self.sheep.len() < limits::SHEEP {
            return;
        }
        let mut all: Vec<(&String, &Sheep)> = self.sheep.iter().collect();
        all.sort_by(|a, b| order(a.1.a.last_event_at - b.1.a.last_event_at));
        let victim = all.iter().find(|(_, s)| s.a.alert.is_none() && !s.lambs.values().any(|l| l.a.alert.is_some())).or(all.first()).map(|(k, _)| (*k).clone());
        if let Some(k) = victim {
            self.forget(&k);
        }
    }

    /// At most LIMITS.identities: the least recently seen that isn't present goes.
    fn cap_identities(&mut self, keep: &str) {
        if self.identities.len() <= limits::IDENTITIES {
            return;
        }
        let mut candidates: Vec<(&String, f64)> = self.identities.iter().filter(|(k, _)| k.as_str() != keep && !self.sheep.contains_key(*k)).map(|(k, i)| (k, i.last_seen)).collect();
        candidates.sort_by(|a, b| order(a.1 - b.1));
        let n = self.identities.len() - limits::IDENTITIES;
        let gone: Vec<String> = candidates.into_iter().take(n).map(|(k, _)| k.clone()).collect();
        for k in gone {
            self.identities.shift_remove(&k);
        }
    }

    fn new_sheep(&mut self, key: &str, terminal_based: bool, t: f64) -> Sheep {
        if !self.identities.contains_key(key) {
            self.identities.insert(key.to_string(), Ident { colour_index: None, slots: Obj::new(), label: None, last_seen: t });
            self.cap_identities(key);
        }
        let used_colours: HashSet<i64> = self.sheep.values().map(|s| s.colour_index).collect();
        let used_slots: HashSet<i64> = self.sheep.values().map(|s| s.slot).collect();
        let present = self.sheep.len();
        let palette = self.cfg.palette_size;
        let ident = self.identities.get_mut(key).unwrap();
        let colour_index = match ident.colour_index {
            Some(c) if (c as usize) < palette && !used_colours.contains(&c) => c,
            _ => (0..palette as i64).find(|i| !used_colours.contains(i)).unwrap_or((present % palette.max(1)) as i64),
        };
        ident.colour_index = Some(colour_index);
        let slot = match ident.slots.get("primary") {
            Some(v) if v.is_integer() && !used_slots.contains(&(v.as_num().unwrap() as i64)) => v.as_num().unwrap() as i64,
            _ => (0..).find(|i| !used_slots.contains(i)).unwrap(),
        };
        ident.slots.set("primary", slot as f64);
        let label = ident.label.clone().unwrap_or_else(|| key.to_string());
        Sheep {
            a: Actor::new(false, sheep_id(key), t),
            identity_key: key.to_string(),
            terminal_based,
            session_id: None,
            label,
            terminal: Obj::new(),
            lambs: IndexMap::new(),
            pending: Vec::new(),
            colour_index,
            slot,
            last_permission_clear_at: f64::NEG_INFINITY,
        }
    }

    /// The actor an event belongs to: its lamb, or the sheep. None means ignore it.
    fn actor_for(&mut self, key: &str, ev: &HookEvent, t: f64, create: bool) -> Option<Sel> {
        let s = self.sheep.get_mut(key).unwrap();
        let Some(agent) = ev.agent_id().filter(|a| !a.is_empty()) else { return Some(Sel::Sheep) };
        if let Some(l) = s.lambs.get_mut(agent) {
            l.a.last_event_at = t;
            return Some(Sel::Lamb(agent.to_string()));
        }
        // A lamb first seen mid-flight appears on its first event, but only with a real
        // agent type: the internal helper agent (agent_type "") never becomes a lamb.
        let agent_type = ev.agent_type().filter(|a| !a.is_empty())?;
        if !create {
            return None;
        }
        if s.lambs.len() >= limits::LAMBS_PER_SHEEP {
            let mut free: Vec<&Lamb> = s.lambs.values().filter(|l| l.a.alert.is_none()).collect();
            free.sort_by(|a, b| order(a.a.last_event_at - b.a.last_event_at));
            let oldest = free.first()?.agent_id.clone();
            s.lambs.shift_remove(&oldest);
        }
        let id = format!("l-{}-{agent}", s.a.id.strip_prefix("s-").unwrap_or(&s.a.id));
        s.lambs.insert(agent.to_string(), Lamb { a: Actor::new(true, id, t), agent_id: agent.to_string(), agent_type: truncate(agent_type, AGENT_TYPE_CHARS) });
        Some(Sel::Lamb(agent.to_string()))
    }

    fn forget(&mut self, key: &str) {
        if let Some(s) = self.sheep.shift_remove(key) {
            if let Some(sid) = &s.session_id {
                if self.sessions.get(sid).map(String::as_str) == Some(key) {
                    self.sessions.remove(sid);
                }
            }
        }
    }

    fn evaluate(&mut self, t: f64) {
        let cfg = self.cfg.clone();
        let keys: Vec<String> = self.sheep.keys().cloned().collect();
        for key in keys {
            let s = self.sheep.get_mut(&key).unwrap();
            let ids: Vec<String> = s.lambs.keys().cloned().collect();
            for id in ids {
                let l = s.lambs.get_mut(&id).unwrap();
                if l.a.leaving_since.is_some_and(|since| t - since >= cfg.min_dwell_ms) {
                    s.lambs.shift_remove(&id);
                    continue;
                }
                let target = target(&l.a, t, &cfg);
                settle(&mut l.a, target, t, cfg.min_dwell_ms);
                if l.a.effect.is_some_and(|(_, at)| t - at >= EFFECT_MS) {
                    l.a.effect = None;
                }
            }
            if s.a.leaving_since.is_some_and(|since| t - since >= cfg.min_dwell_ms) {
                self.forget(&key);
                continue;
            }
            let target = target(&s.a, t, &cfg);
            settle(&mut s.a, target, t, cfg.min_dwell_ms);
            if s.a.effect.is_some_and(|(_, at)| t - at >= EFFECT_MS) {
                s.a.effect = None;
            }
        }
    }

    fn bubble(&self, tool: Option<&str>, summary: Option<&str>) -> String {
        let Some(tool) = tool.filter(|t| !t.is_empty()) else { return truncate("Permission needed?", self.cfg.max_chars) };
        let text = match summary.filter(|s| !s.is_empty()) {
            Some(s) if self.cfg.detail_full => format!("{tool}: {s}?"),
            _ => format!("{tool}?"),
        };
        truncate(&text, self.cfg.max_chars)
    }

    fn handle(&mut self, key: &str, ev: &HookEvent, t: f64) {
        let max_chars = self.cfg.max_chars;
        match ev.event.as_str() {
            "UserPromptSubmit" => {
                let s = self.sheep.get_mut(key).unwrap();
                s.clear(&Sel::Sheep, t);
                // A new turn: whatever the last one left in flight is over.
                s.a.in_flight.clear();
                s.a.turn_active = true;
                s.a.stopped_at = None;
                s.a.error = None;
                s.a.waiting_since = None;
                s.a.last_activity_at = t;
            }
            "PreToolUse" => {
                let Some(sel) = self.actor_for(key, ev, t, true) else { return };
                let s = self.sheep.get_mut(key).unwrap();
                // Progress means the prompt was answered: a different tool call from the
                // same actor, more than 2 s after the alert.
                let progressed = s.actor(&sel).alert.as_ref().is_some_and(|a| t - a.since > PROGRESS_GUARD_MS && ev.tool_use_id() != a.tool_use_id.as_deref());
                if progressed {
                    s.clear(&sel, t);
                }
                let actor_id = s.actor(&sel).id.clone();
                s.pending.push(Pending {
                    tool_use_id: ev.tool_use_id().map(str::to_string),
                    fingerprint: ev.tool_fingerprint.clone(),
                    agent_id: ev.agent_id().map(str::to_string),
                    actor_id,
                    at: t,
                    tool_name: ev.tool_name().map(str::to_string),
                    tool_summary: ev.tool_summary.clone(),
                });
                if s.pending.len() > PENDING_CAP {
                    s.pending.remove(0);
                }
                let a = s.actor(&sel);
                let flight = ev.tool_use_id().map(str::to_string).unwrap_or_else(|| format!("anon-{}", number_to_string(t)));
                a.in_flight.insert(flight, InFlight { tool_name: ev.tool_name().map(str::to_string), at: t });
                a.last_tool_name = ev.tool_name().map(str::to_string);
                a.last_tool_at = t;
                a.last_activity_at = t;
                a.waiting_since = None;
                if sel == Sel::Sheep {
                    s.a.turn_active = true;
                    s.a.stopped_at = None;
                }
            }
            "PermissionRequest" => {
                // Correlate with the PreToolUse it belongs to: same fingerprint, same agent
                // if the request names one, most recent first, and not already alerted.
                let s = self.sheep.get(key).unwrap();
                let alerted: HashSet<String> = std::iter::once(&s.a).chain(s.lambs.values().map(|l| &l.a)).filter_map(|a| a.alert.as_ref()?.tool_use_id.clone()).filter(|u| !u.is_empty()).collect();
                let m = s
                    .pending
                    .iter()
                    .rev()
                    .find(|p| p.fingerprint.as_deref().is_some_and(|f| !f.is_empty() && Some(f) == ev.tool_fingerprint.as_deref()) && (ev.agent_id().is_none_or(str::is_empty) || p.agent_id.as_deref() == ev.agent_id()) && !p.tool_use_id.as_ref().is_some_and(|u| alerted.contains(u)))
                    .cloned();
                let mut sel = m.as_ref().and_then(|m| s.actor_in(&m.actor_id));
                if sel.is_none() {
                    sel = Some(self.actor_for(key, ev, t, true).unwrap_or(Sel::Sheep));
                }
                let sel = sel.unwrap();
                let text = self.bubble(ev.tool_name().or(m.as_ref().and_then(|m| m.tool_name.as_deref())), ev.tool_summary.as_deref().or(m.as_ref().and_then(|m| m.tool_summary.as_deref())));
                let s = self.sheep.get_mut(key).unwrap();
                let a = s.actor(&sel);
                a.alert = Some(Alert { kind: "permission", text, since: t, tool_use_id: m.and_then(|m| m.tool_use_id), fingerprint: ev.tool_fingerprint.clone(), confirmed: false });
                a.waiting_since = None;
            }
            "Notification" => match ev.notification_type() {
                Some("permission_prompt") => {
                    let s = self.sheep.get_mut(key).unwrap();
                    let open: Vec<(Sel, Alert)> = s.alerting().into_iter().map(|sel| {
                        let a = s.actor(&sel).alert.clone().unwrap();
                        (sel, a)
                    }).filter(|(_, a)| a.kind == "permission").collect();
                    let mut unconfirmed: Vec<&(Sel, Alert)> = open.iter().filter(|(_, a)| !a.confirmed).collect();
                    unconfirmed.sort_by(|a, b| order(a.1.since - b.1.since));
                    if let Some((sel, _)) = unconfirmed.first() {
                        s.actor(sel).alert.as_mut().unwrap().confirmed = true;
                    } else if open.is_empty() && t - s.last_permission_clear_at > NOTIFY_AFTER_CLEAR_GUARD_MS {
                        // No PermissionRequest came first (sandbox network prompts): the
                        // notification is the only signal, so it raises the alert itself.
                        s.a.alert = Some(Alert { kind: "permission", text: truncate(ev.message.as_deref().unwrap_or("Permission needed?"), max_chars), since: t, tool_use_id: None, fingerprint: None, confirmed: true });
                    }
                }
                Some("idle_prompt") => {
                    let s = self.sheep.get_mut(key).unwrap();
                    s.a.turn_active = false;
                    s.a.waiting_since = Some(t);
                }
                Some("elicitation_dialog" | "elicitation_url_dialog") => {
                    let sel = self.actor_for(key, ev, t, true).unwrap_or(Sel::Sheep);
                    let s = self.sheep.get_mut(key).unwrap();
                    let a = s.actor(&sel);
                    if a.alert.as_ref().is_some_and(|x| x.kind == "elicitation") {
                        a.alert.as_mut().unwrap().confirmed = true;
                    } else {
                        a.alert = Some(Alert { kind: "elicitation", text: truncate(ev.message.as_deref().unwrap_or("Input requested"), max_chars), since: t, tool_use_id: None, fingerprint: None, confirmed: true });
                    }
                }
                Some("elicitation_complete" | "elicitation_response") => self.clear_kind(key, "elicitation", t),
                Some("agent_needs_input") => {
                    let s = self.sheep.get_mut(key).unwrap();
                    s.a.alert = Some(Alert { kind: "input", text: truncate(ev.message.as_deref().unwrap_or("Needs your input"), max_chars), since: t, tool_use_id: None, fingerprint: None, confirmed: false });
                }
                _ => {}
            },
            "Elicitation" => {
                let sel = self.actor_for(key, ev, t, true).unwrap_or(Sel::Sheep);
                let parts: Vec<&str> = [ev.mcp_server(), ev.message.as_deref()].into_iter().flatten().filter(|p| !p.is_empty()).collect();
                let text = if parts.is_empty() { "Input requested".to_string() } else { parts.join(": ") };
                let s = self.sheep.get_mut(key).unwrap();
                s.actor(&sel).alert = Some(Alert { kind: "elicitation", text: truncate(&text, max_chars), since: t, tool_use_id: None, fingerprint: None, confirmed: false });
            }
            "ElicitationResult" => self.clear_kind(key, "elicitation", t),
            "PostToolUse" => {
                self.tool_done(key, ev, t);
            }
            "PostToolUseFailure" => {
                if let Some(sel) = self.tool_done(key, ev, t) {
                    self.sheep.get_mut(key).unwrap().actor(&sel).effect = Some(("trip", t));
                }
            }
            "PermissionDenied" => {
                if let Some(sel) = self.tool_done(key, ev, t) {
                    self.sheep.get_mut(key).unwrap().actor(&sel).waiting_since = Some(t);
                }
            }
            "Stop" | "StopFailure" => {
                if ev.agent_id().is_some_and(|a| !a.is_empty()) {
                    return;
                }
                let s = self.sheep.get_mut(key).unwrap();
                s.clear(&Sel::Sheep, t);
                s.a.turn_active = false;
                s.a.stopped_at = Some(t);
                s.a.in_flight.clear();
                if ev.event == "Stop" {
                    s.a.waiting_since = None;
                    s.a.last_activity_at = t;
                } else {
                    s.a.error = Some(ev.error_type.clone().unwrap_or_else(|| "error".into()));
                }
            }
            "SubagentStart" => {
                if let Some(sel @ Sel::Lamb(_)) = self.actor_for(key, ev, t, true) {
                    self.sheep.get_mut(key).unwrap().actor(&sel).last_activity_at = t;
                }
            }
            "SubagentStop" => {
                // Only a lamb we already know can stop; an unknown agent is never created.
                let Some(sel @ Sel::Lamb(_)) = self.actor_for(key, ev, t, false) else { return };
                let s = self.sheep.get_mut(key).unwrap();
                s.clear(&sel, t);
                let a = s.actor(&sel);
                a.alive = false;
                a.in_flight.clear();
                a.leaving_since = Some(t);
            }
            "PreCompact" => self.sheep.get_mut(key).unwrap().a.compacting = true,
            "PostCompact" => {
                let s = self.sheep.get_mut(key).unwrap();
                s.a.compacting = false;
                s.a.last_activity_at = t;
            }
            "SessionEnd" => {
                // Every alert in the session goes, lambs included, and the lambs leave.
                let Flock { sheep, sessions, .. } = self;
                let s = sheep.get_mut(key).unwrap();
                for sel in s.alerting() {
                    s.clear(&sel, t);
                }
                for l in s.lambs.values_mut() {
                    l.a.leaving_since.get_or_insert(t);
                }
                if let Some(sid) = &s.session_id {
                    sessions.remove(sid);
                }
                if matches!(ev.reason(), Some("clear" | "resume")) && s.terminal_based {
                    // Same tab or pane, new session next: the sheep stays.
                    s.reset_turn();
                    s.a.stopped_at = Some(t);
                    s.session_id = None;
                } else {
                    s.leave(t);
                }
            }
            _ => {}
        }
    }

    fn clear_kind(&mut self, key: &str, kind: &str, t: f64) {
        let s = self.sheep.get_mut(key).unwrap();
        for sel in s.alerting() {
            if s.actor(&sel).alert.as_ref().is_some_and(|a| a.kind == kind) {
                s.clear(&sel, t);
            }
        }
    }

    fn tool_done(&mut self, key: &str, ev: &HookEvent, t: f64) -> Option<Sel> {
        let s = self.sheep.get_mut(key).unwrap();
        // Clear by tool_use_id wherever the alert sits (the PreToolUse decided the actor).
        if let Some(id) = ev.tool_use_id().filter(|i| !i.is_empty()) {
            for sel in s.alerting() {
                if s.actor(&sel).alert.as_ref().and_then(|a| a.tool_use_id.as_deref()) == Some(id) {
                    s.clear(&sel, t);
                }
            }
        }
        let id = ev.tool_use_id().map(str::to_string);
        s.pending.retain(|p| p.tool_use_id != id);
        let sel = self.actor_for(key, ev, t, false)?;
        let s = self.sheep.get_mut(key).unwrap();
        let a = s.actor(&sel);
        if let Some(id) = &id {
            a.in_flight.shift_remove(id);
        }
        a.last_activity_at = t;
        Some(sel)
    }
}

fn target(a: &Actor, t: f64, c: &FlockConfig) -> (&'static str, Option<String>) {
    if a.leaving_since.is_some() {
        return ("leaving", None);
    }
    if t - a.created < c.min_dwell_ms {
        return ("arriving", None);
    }
    if let Some(e) = a.error.as_ref().filter(|e| !e.is_empty()) {
        return ("error", Some(e.clone()));
    }
    if a.compacting {
        return ("compacting", None);
    }
    if let Some((_, f)) = a.in_flight.last() {
        return ("busy", Some(category_of(f.tool_name.as_deref()).into()));
    }
    let recent = if t - a.last_tool_at < c.busy_window_ms { category_of(a.last_tool_name.as_deref()) } else { "thinking" };
    if a.lamb && a.alive {
        return ("busy", Some(recent.into()));
    }
    if a.turn_active && t - a.last_event_at < c.thinking_timeout_ms {
        return ("busy", Some(recent.into()));
    }
    if let Some(w) = a.waiting_since {
        return (if t - w >= c.sleep_after_ms { "sleeping" } else { "waiting" }, None);
    }
    if let Some(s) = a.stopped_at {
        if t - s < c.graze_after_ms {
            return ("done", None);
        }
    }
    let quiet = t - js_max(a.last_activity_at, a.stopped_at.unwrap_or(f64::NEG_INFINITY));
    (if quiet >= c.sleep_after_ms { "sleeping" } else { "idle" }, None)
}

/// Hysteresis: a calm state changes only after minDwellMs, except into `leaving`.
fn settle(a: &mut Actor, (state, flavour): (&'static str, Option<String>), t: f64, min_dwell_ms: f64) {
    if a.shown.state == state && a.shown.flavour == flavour {
        return;
    }
    if state != "leaving" && t - a.shown.since < min_dwell_ms {
        return;
    }
    a.shown = Shown { state, flavour, since: t };
}

fn alert_view(a: &Option<Alert>) -> Js {
    match a {
        Some(a) => Js::Obj(Obj::new().with("kind", a.kind).with("text", a.text.as_str()).with("since", a.since).with("confirmed", a.confirmed)),
        None => Js::Undefined,
    }
}

fn effect_view(e: &Option<(&'static str, f64)>) -> Js {
    match e {
        Some((name, at)) => Js::Obj(Obj::new().with("name", *name).with("at", *at)),
        None => Js::Undefined,
    }
}

/// Drops undefined fields, so the snapshot JSON stays small and stable.
fn clean(o: Obj) -> Js {
    let mut out = Obj::new();
    for (k, v) in o.iter() {
        if !v.is_undefined() {
            out.set(k.clone(), v.clone());
        }
    }
    Js::Obj(out)
}

/// One persisted identity, checked field by field; None if it can't be used at all.
fn sanitise_identity(key: &str, v: &Js, t: f64, palette_size: usize) -> Option<Ident> {
    if key.is_empty() || jsstr::len(key) > limits::IDENTITY_KEY_CHARS {
        return None;
    }
    let o = v.as_obj()?;
    let last_seen = match o.get("lastSeen") {
        Some(Js::Num(n)) if n.is_finite() && *n <= t + 86_400_000.0 => *n,
        _ => t,
    };
    if t - last_seen >= IDENTITY_TTL_MS {
        return None;
    }
    let colour_index = match o.get("colourIndex") {
        Some(c) if c.is_integer() && c.as_num().unwrap() >= 0.0 && (c.as_num().unwrap() as usize) < palette_size => Some(c.as_num().unwrap() as i64),
        _ => None,
    };
    let mut slots = Obj::new();
    if let Some(Js::Obj(raw)) = o.get("slots") {
        for (m, slot) in raw.iter() {
            if slots.len() >= limits::MONITORS_PER_IDENTITY {
                break;
            }
            if !m.is_empty() && jsstr::len(m) <= limits::MONITOR_NAME_CHARS && slot.is_integer() && (0.0..=10_000.0).contains(&slot.as_num().unwrap()) {
                slots.set(m.clone(), slot.clone());
            }
        }
    }
    // Cleaned like bubble text, then capped.
    let label = o.get("label").and_then(Js::as_str).and_then(|l| clean_label(jsstr::prefix(l, 4 * limits::LABEL_CHARS), limits::LABEL_CHARS));
    Some(Ident { colour_index, slots, label, last_seen })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterm_prefix() {
        assert_eq!(strip_iterm_prefix("w0t1p0:UUID"), "UUID");
        assert_eq!(strip_iterm_prefix("w12t3p45:X:Y"), "X:Y");
        assert_eq!(strip_iterm_prefix("wt1p0:UUID"), "wt1p0:UUID");
        assert_eq!(strip_iterm_prefix("UUID"), "UUID");
    }

    fn pane_event(t: f64, event: &str, session: &str, reason: Option<&str>) -> HookEvent {
        let mut terminal = Obj::new();
        terminal.set("wtSession", "11111111-2222-4333-8444-555555555555");
        let mut scalars = vec![("sessionId", session.to_string())];
        if let Some(r) = reason {
            scalars.push(("reason", r.to_string()));
        }
        HookEvent { received_at: t, event: event.into(), scalars, project_label: Some("meadow".into()), terminal, ..HookEvent::default() }
    }

    fn states(f: &Flock) -> Vec<String> {
        f.snapshot().get("flock").as_arr().map(|a| a.iter().filter_map(|s| s.get("state").as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    #[test]
    fn the_session_after_a_clear_that_ends_unprompted_takes_its_sheep_away() {
        let mut f = Flock::new(FlockConfig::from_merged(&crate::config::Config::defaults().merged), &Js::Null, 0.0);
        f.apply(&pane_event(1000.0, "UserPromptSubmit", "a", None));
        f.apply(&pane_event(2000.0, "Stop", "a", None));
        // /clear: the sheep stays for the next session in the same tab.
        assert!(f.apply(&pane_event(10_000.0, "SessionEnd", "a", Some("clear"))));
        assert_eq!(states(&f).len(), 1);
        assert_ne!(states(&f)[0], "leaving");
        // That session is closed before its first prompt: its only hook is SessionEnd.
        assert!(f.apply(&pane_event(20_000.0, "SessionEnd", "b", Some("prompt_input_exit"))));
        assert_eq!(states(&f), vec!["leaving".to_string()]);
    }

    #[test]
    fn an_unknown_session_ending_leaves_a_live_sheep_in_its_pane_alone() {
        let mut f = Flock::new(FlockConfig::from_merged(&crate::config::Config::defaults().merged), &Js::Null, 0.0);
        f.apply(&pane_event(1000.0, "UserPromptSubmit", "a", None));
        // Some other session the brain never saw, with the same tab id: not this sheep's.
        assert!(!f.apply(&pane_event(5000.0, "SessionEnd", "z", Some("prompt_input_exit"))));
        assert_ne!(states(&f)[0], "leaving");
    }

    #[test]
    fn object_prototype_names_are_not_handlers() {
        let mut f = Flock::new(FlockConfig::from_merged(&crate::config::Config::defaults().merged), &Js::Null, 0.0);
        for name in ["toString", "constructor", "__proto__", "hasOwnProperty", "valueOf"] {
            let ev = HookEvent { received_at: 1.0, event: name.into(), scalars: vec![("sessionId", "s".into())], ..HookEvent::default() };
            assert!(!f.apply(&ev), "{name}");
        }
        assert_eq!(crate::json::stringify(&f.snapshot()), r#"{"muted":false,"flock":[]}"#);
    }
}
