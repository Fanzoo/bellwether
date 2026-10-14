//! The internal HookEvent, built from an allowlist of payload fields (a port of
//! brain/normalise.mjs, pinned to it by test/fixtures/normalise-vectors.json and the
//! differential). A field Claude Code adds in a future version is dropped because nothing
//! here names it.
//!
//! Free text is kept only when a cleaner is given (redact, then truncate), and only from
//! two places: `toolSummary`, from one allowlisted `tool_input` field per tool, and
//! `message`, from Notification and Elicitation. The recorder passes no cleaner, so
//! recordings never hold free text.
//!
//! Strings are unit strings and lengths are code units, as in Node (see [`crate::jsstr`]).

use sha2::{Digest, Sha256};

use crate::json::{self, Js, Obj};
use crate::jsstr;
use crate::redact::{self, Cleaner};

/// Terminal identity headers set by bellwether's own hook block: (HookEvent field, header).
pub const TERMINAL_HEADERS: [(&str, &str); 7] = [
    ("program", "x-bellwether-term-program"),
    ("itermSession", "x-bellwether-iterm-session"),
    ("termSession", "x-bellwether-term-session"),
    ("wtSession", "x-bellwether-wt-session"),
    ("tmuxPane", "x-bellwether-tmux-pane"),
    ("herdrPane", "x-bellwether-herdr-pane"),
    ("herdrSocket", "x-bellwether-herdr-socket"),
];

/// Payload field -> HookEvent field. Every value must be a short identifier or enum.
pub const SCALARS: [(&str, &str); 12] = [
    ("session_id", "sessionId"),
    ("agent_id", "agentId"),
    ("agent_type", "agentType"),
    ("tool_name", "toolName"),
    ("tool_use_id", "toolUseId"),
    ("notification_type", "notificationType"),
    ("source", "source"),
    ("reason", "reason"),
    ("trigger", "trigger"),
    ("mcp_server_name", "mcpServer"),
    ("mode", "mode"),
    ("permission_mode", "permissionMode"),
];

pub const BOOLEANS: [(&str, &str); 2] = [("is_interrupt", "isInterrupt"), ("stop_hook_active", "stopHookActive")];

/// The tool_input fields `toolSummary` reads, for any tool.
pub const SUMMARY_FIELDS: [&str; 7] = ["command", "file_path", "notebook_path", "url", "query", "pattern", "subagent_type"];

/// Every top-level payload field this module reads. Replay never fills these with a marker.
pub const READ_FIELDS: [&str; 20] = [
    "hook_event_name",
    "session_id",
    "agent_id",
    "agent_type",
    "tool_name",
    "tool_use_id",
    "notification_type",
    "source",
    "reason",
    "trigger",
    "mcp_server_name",
    "mode",
    "permission_mode",
    "is_interrupt",
    "stop_hook_active",
    "cwd",
    "error",
    "background_tasks",
    "tool_input",
    "message",
];

const MAX_SCALAR: usize = 200;

/// A lamb's label (its agent type) is cut to this many characters, ending in "…".
pub const AGENT_TYPE_CHARS: usize = 24;

/// One normalised hook event. Field names and their order are the Node HookEvent's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookEvent {
    /// ms; NaN when an event built by hand has none (as `undefined` is in Node).
    pub received_at: f64,
    pub event: String,
    /// The SCALARS, in their order, by HookEvent field name.
    pub scalars: Vec<(&'static str, String)>,
    pub is_interrupt: Option<bool>,
    pub stop_hook_active: Option<bool>,
    pub cwd: Option<String>,
    pub project_label: Option<String>,
    pub error_type: Option<String>,
    pub background_task_count: Option<f64>,
    pub tool_fingerprint: Option<String>,
    pub tool_summary: Option<String>,
    pub message: Option<String>,
    pub terminal: Obj,
}

impl HookEvent {
    pub fn scalar(&self, field: &str) -> Option<&str> {
        self.scalars.iter().find(|(k, _)| *k == field).map(|(_, v)| v.as_str())
    }

    fn set_scalar(&mut self, field: &'static str, v: Option<String>) {
        match v {
            Some(v) => match self.scalars.iter_mut().find(|(k, _)| *k == field) {
                Some(slot) => slot.1 = v,
                None => self.scalars.push((field, v)),
            },
            None => self.scalars.retain(|(k, _)| *k != field),
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.scalar("sessionId")
    }
    pub fn agent_id(&self) -> Option<&str> {
        self.scalar("agentId")
    }
    pub fn agent_type(&self) -> Option<&str> {
        self.scalar("agentType")
    }
    pub fn tool_name(&self) -> Option<&str> {
        self.scalar("toolName")
    }
    pub fn tool_use_id(&self) -> Option<&str> {
        self.scalar("toolUseId")
    }
    pub fn notification_type(&self) -> Option<&str> {
        self.scalar("notificationType")
    }
    pub fn reason(&self) -> Option<&str> {
        self.scalar("reason")
    }
    pub fn mcp_server(&self) -> Option<&str> {
        self.scalar("mcpServer")
    }

    /// The HookEvent as Node writes it (recordings, `__normalise`).
    pub fn to_js(&self) -> Js {
        let mut o = Obj::new().with("receivedAt", self.received_at).with("event", self.event.as_str());
        for (_, field) in SCALARS {
            if let Some(v) = self.scalar(field) {
                o.set(field, v);
            }
        }
        if let Some(b) = self.is_interrupt {
            o.set("isInterrupt", b);
        }
        if let Some(b) = self.stop_hook_active {
            o.set("stopHookActive", b);
        }
        for (k, v) in [("cwd", &self.cwd), ("projectLabel", &self.project_label), ("errorType", &self.error_type)] {
            if let Some(v) = v {
                o.set(k, v.as_str());
            }
        }
        if let Some(n) = self.background_task_count {
            o.set("backgroundTaskCount", n);
        }
        for (k, v) in [("toolFingerprint", &self.tool_fingerprint), ("toolSummary", &self.tool_summary), ("message", &self.message)] {
            if let Some(v) = v {
                o.set(k, v.as_str());
            }
        }
        o.set("terminal", Js::Obj(self.terminal.clone()));
        Js::Obj(o)
    }

    /// An event given as a JS object (a recording line's `normalised`, or one a test built
    /// by hand). Fields the reducer reads are taken when they are strings (booleans,
    /// numbers where Node has those); anything else is as if absent.
    pub fn from_js(v: &Js) -> HookEvent {
        let s = |k: &str| v.get(k).as_str().map(str::to_string);
        let mut ev = HookEvent { received_at: v.get("receivedAt").as_num().unwrap_or(f64::NAN), event: s("event").unwrap_or_default(), ..HookEvent::default() };
        for (_, field) in SCALARS {
            if let Some(x) = s(field) {
                ev.scalars.push((field, x));
            }
        }
        ev.is_interrupt = v.get("isInterrupt").as_bool();
        ev.stop_hook_active = v.get("stopHookActive").as_bool();
        ev.cwd = s("cwd");
        ev.project_label = s("projectLabel");
        ev.error_type = s("errorType");
        ev.background_task_count = v.get("backgroundTaskCount").as_num();
        ev.tool_fingerprint = s("toolFingerprint");
        ev.tool_summary = s("toolSummary");
        ev.message = s("message");
        ev.terminal = v.get("terminal").as_obj().cloned().unwrap_or_default();
        ev
    }
}

/// `str(v, max)`: a non-empty string, cut to `max` code units.
fn str_of(v: &Js, max: usize) -> Option<String> {
    let s = v.as_str().filter(|s| !s.is_empty())?;
    Some(jsstr::prefix(s, max).to_string())
}

/// `^[A-Za-z0-9_.:@-]+$`
fn is_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '-'))
}

/// Node's `path.posix.basename`: trailing slashes ignored, then what follows the last `/`.
pub fn basename(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => trimmed[i + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

pub fn normalise(payload: &Js, headers: &Obj, received_at: f64, clean: Option<&Cleaner>) -> HookEvent {
    let mut ev = HookEvent { received_at, ..HookEvent::default() };
    // The event name is written to logs and recordings, so it gets the identifier check too.
    ev.event = str_of(payload.get("hook_event_name"), 64).filter(|n| is_identifier(n)).unwrap_or_else(|| "unknown".into());
    for (from, to) in SCALARS {
        if let Some(v) = str_of(payload.get(from), MAX_SCALAR).filter(|v| is_identifier(v)) {
            ev.scalars.push((to, v));
        }
    }
    // The agent type becomes a lamb's label and bubble prefix: free text as far as the
    // screen is concerned, so it gets the bubble cleaner and a short cap.
    if let Some(t) = ev.agent_type().map(str::to_string) {
        let label = redact::truncate(&clean.map(|c| c.clean(&t)).unwrap_or(t), AGENT_TYPE_CHARS);
        ev.set_scalar("agentType", (!label.is_empty()).then_some(label));
    }
    for (from, to) in BOOLEANS {
        if let Some(b) = payload.get(from).as_bool() {
            if to == "isInterrupt" {
                ev.is_interrupt = Some(b);
            } else {
                ev.stop_hook_active = Some(b);
            }
        }
    }
    if let Some(cwd) = str_of(payload.get("cwd"), 4096) {
        // A folder name is free text too: control, bidi and zero-width characters out.
        ev.project_label = redact::clean_label(&basename(&cwd), 200);
        ev.cwd = Some(cwd);
    }
    // StopFailure carries an error enum; PostToolUseFailure's `error` is free text and is dropped.
    if ev.event == "StopFailure" {
        ev.error_type = str_of(payload.get("error"), MAX_SCALAR).filter(|e| e.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }
    if let Some(a) = payload.get("background_tasks").as_arr() {
        ev.background_task_count = Some(a.len() as f64);
    }
    let input = payload.get("tool_input");
    if !input.is_undefined() {
        if let Some(name) = ev.tool_name() {
            ev.tool_fingerprint = Some(fingerprint(name, input));
        }
    }
    if let Some(c) = clean {
        let summary = ev.tool_name().and_then(|n| tool_summary(n, input));
        ev.tool_summary = summary.map(|s| c.clean(&s)).filter(|s| !s.is_empty());
        if ev.event == "Notification" || ev.event == "Elicitation" {
            ev.message = payload.get("message").as_str().map(|m| c.clean(m)).filter(|m| !m.is_empty());
        }
    }
    ev.terminal = terminal_from(headers);
    ev
}

/// The one tool_input field each tool contributes to its bubble, as raw text before
/// cleaning. Every other tool_input field is never read.
pub fn tool_summary(tool_name: &str, input: &Js) -> Option<String> {
    // /^mcp__(.+?)__(.+)$/
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        let chars: Vec<char> = rest.chars().collect();
        for k in 1..chars.len() {
            if chars[k] == '_' && chars.get(k + 1) == Some(&'_') && chars.len() > k + 2 {
                let a: String = chars[..k].iter().collect();
                let b: String = chars[k + 2..].iter().collect();
                if !a.chars().chain(b.chars()).any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')) {
                    return Some(format!("{a}:{b}"));
                }
            }
        }
    }
    let o = input.as_obj()?;
    let field = |k: &str| o.get(k).and_then(Js::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    match tool_name {
        "Bash" => field("command"),
        "Edit" | "Write" | "Read" | "NotebookEdit" => field("file_path").or_else(|| field("notebook_path")).map(|p| basename(&p.replace('\\', "/"))),
        "WebFetch" => field("url").and_then(|u| url_host(&u)),
        "WebSearch" => field("query"),
        "Grep" | "Glob" => field("pattern"),
        "Task" | "Agent" => field("subagent_type"),
        _ => None,
    }
}

/// `new URL(u).host || undefined`: the WHATWG host, with a port that isn't the scheme's
/// default. A lone surrogate is U+FFFD to the URL parser, as in JS.
fn url_host(u: &str) -> Option<String> {
    let parsed = url::Url::parse(&jsstr::to_string_lossy(u)).ok()?;
    let host = parsed.host_str().filter(|h| !h.is_empty())?;
    let s = match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    Some(jsstr::from_str(&s))
}

/// herdr values are later handed to the herdr CLI: a pane id is 1 to 64 of
/// `A-Z a-z 0-9 _ . : -`, not starting with "-".
pub fn valid_herdr_pane(v: &str) -> bool {
    let n = jsstr::len(v);
    (1..=64).contains(&n) && !v.starts_with('-') && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// A socket path is absolute, at most 1024 bytes, with no ".." and no control characters.
pub fn valid_herdr_socket(p: &str) -> bool {
    p.starts_with('/') && jsstr::utf8_len(p) <= 1024 && !p.split('/').any(|seg| seg == "..") && !p.chars().any(|c| matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'))
}

pub fn terminal_from(headers: &Obj) -> Obj {
    let mut t = Obj::new();
    for (field, header) in TERMINAL_HEADERS {
        let raw = headers.get(header).unwrap_or(&Js::Undefined);
        // herdr values are checked whole: a long one is dropped, never cut to fit.
        let v = if field.starts_with("herdr") { raw.as_str().filter(|s| !s.is_empty()).map(str::to_string) } else { str_of(raw, 512) };
        let Some(v) = v else { continue };
        if field == "herdrPane" && !valid_herdr_pane(&v) {
            continue;
        }
        if field == "herdrSocket" && !valid_herdr_socket(&v) {
            continue;
        }
        t.set(field, v);
    }
    t
}

/// sha256 over the tool name and its input, canonicalised (keys sorted, compact): how a
/// PermissionRequest (which has no tool_use_id) is matched to its PreToolUse.
pub fn fingerprint(tool_name: &str, input: &Js) -> String {
    let text = format!("{}\n{}", jsstr::to_string_lossy(tool_name), canonical(input));
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Keys sorted (by code unit, as `Array.prototype.sort` does), compact, as real text.
pub fn canonical(v: &Js) -> String {
    match v {
        Js::Arr(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Js::Obj(o) => {
            let mut keys = o.keys();
            keys.sort_by(|a, b| jsstr::cmp(a, b));
            format!("{{{}}}", keys.iter().map(|k| format!("{}:{}", json::quote(k), canonical(o.get(k).unwrap()))).collect::<Vec<_>>().join(","))
        }
        Js::Undefined => "null".into(),
        other => json::stringify(other),
    }
}

/// Field names only, never values: the payload's top-level keys and tool_input's, sorted.
pub fn raw_keys(payload: &Js) -> Js {
    let sorted = |o: &Obj| {
        let mut k: Vec<String> = o.keys().into_iter().cloned().collect();
        k.sort_by(|a, b| jsstr::cmp(a, b));
        Js::Arr(k.into_iter().map(Js::Str).collect())
    };
    let mut out = Obj::new();
    if let Some(o) = payload.as_obj() {
        out.set("top", sorted(o));
        if let Some(ti) = o.get("tool_input").and_then(Js::as_obj) {
            out.set("tool_input", sorted(ti));
        }
    }
    Js::Obj(out)
}

/// Names of the X-Bellwether-* headers that arrived non-empty (the token's excepted).
pub fn headers_present(headers: &Obj) -> Js {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(h, v)| h.starts_with("x-bellwether-") && *h != "x-bellwether-token" && !matches!(v, Js::Str(s) if s.is_empty()))
        .map(|(h, _)| h.clone())
        .collect();
    names.sort_by(|a, b| jsstr::cmp(a, b));
    Js::Arr(names.into_iter().map(Js::Str).collect())
}

/// HTTP header values as Node gives them: bytes read as Latin-1, a repeated header joined
/// with ", ", names in lower case.
pub fn headers_from_http<'a>(pairs: impl Iterator<Item = (&'a str, &'a [u8])>) -> Obj {
    let mut out = Obj::new();
    for (name, value) in pairs {
        let v: String = value.iter().map(|b| *b as char).collect();
        let name = name.to_ascii_lowercase();
        match out.get_mut(&name) {
            Some(Js::Str(prev)) => {
                prev.push_str(", ");
                prev.push_str(&v);
            }
            _ => out.set(name, v),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basenames_like_node() {
        for (p, want) in [("/a/b", "b"), ("/a/b/", "b"), ("/", ""), ("", ""), ("a", "a"), ("//x//", "x")] {
            assert_eq!(basename(p), want, "{p}");
        }
    }

    #[test]
    fn mcp_summaries() {
        assert_eq!(tool_summary("mcp__github__create_issue", &Js::Null).as_deref(), Some("github:create_issue"));
        assert_eq!(tool_summary("mcp__a___b", &Js::Null).as_deref(), Some("a:_b"));
        assert_eq!(tool_summary("mcp_____x", &Js::Null).as_deref(), Some("_:x"));
        assert_eq!(tool_summary("mcp____x", &Js::Null), None);
    }

    #[test]
    fn fingerprints_ignore_key_order() {
        let a = json::parse(r#"{"a":1,"b":[1,{"c":2,"d":3}]}"#).unwrap();
        let b = json::parse(r#"{"b":[1,{"d":3,"c":2}],"a":1}"#).unwrap();
        assert_eq!(fingerprint("Bash", &a), fingerprint("Bash", &b));
        assert_eq!(fingerprint("Bash", &a).len(), 16);
    }
}
