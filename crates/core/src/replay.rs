//! `bellwether replay`'s pure half (a port of brain/replay.mjs): reading a recording,
//! rebuilding each line's payload and headers, and the synthetic `--burst` load. The HTTP
//! half (posting on schedule, timing /hook, counting snapshots) is in crates/brain.
//!
//! Recordings hold field names, not values, so each payload is rebuilt: every field the
//! normalised event has goes back under its Claude Code name; tool_input is rebuilt from
//! toolSummary through the per-tool allowlist; every other recorded field name is sent
//! with a marker value «replay:<name>», which must never reach a snapshot. A field the
//! normaliser reads never gets a marker.

use crate::config::{io_code, ConfigError};
use crate::json::{self, Js, Obj};
use crate::normalise::{READ_FIELDS, SUMMARY_FIELDS, TERMINAL_HEADERS};
use crate::status::js_string;

const FROM_NORMALISED: [(&str, &str); 16] = [
    ("sessionId", "session_id"),
    ("agentId", "agent_id"),
    ("agentType", "agent_type"),
    ("toolName", "tool_name"),
    ("toolUseId", "tool_use_id"),
    ("notificationType", "notification_type"),
    ("source", "source"),
    ("reason", "reason"),
    ("trigger", "trigger"),
    ("mcpServer", "mcp_server_name"),
    ("mode", "mode"),
    ("permissionMode", "permission_mode"),
    ("cwd", "cwd"),
    ("message", "message"),
    ("isInterrupt", "is_interrupt"),
    ("stopHookActive", "stop_hook_active"),
];

/// The names every JS object answers `in` for (Object.prototype's).
const PROTOTYPE_NAMES: [&str; 12] = ["constructor", "__defineGetter__", "__defineSetter__", "hasOwnProperty", "__lookupGetter__", "__lookupSetter__", "isPrototypeOf", "propertyIsEnumerable", "toString", "valueOf", "__proto__", "toLocaleString"];

/// `k in o` for a plain object built from JSON.
fn has(o: &Obj, k: &str) -> bool {
    o.contains(k) || PROTOTYPE_NAMES.contains(&k)
}

pub fn marker(name: &str) -> String {
    format!("«replay:{name}»")
}

/// The recording's lines, each checked for `{normalised: {event, receivedAt}}`.
pub fn read_recording(file: &std::path::Path) -> Result<Vec<Js>, ConfigError> {
    let text = std::fs::read(file).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| ConfigError::new(format!("cannot read {}: {}", file.display(), io_code(&e))))?;
    let mut out = Vec::new();
    for (i, l) in text.split('\n').filter(|l| !crate::jsstr::trim(l).is_empty()).enumerate() {
        let o = json::parse(l).map_err(|_| ConfigError::new(format!("{}:{} is not JSON; replay takes a bellwether recording (.jsonl)", file.display(), i + 1)))?;
        let n = o.get("normalised");
        if !n.get("event").truthy() || n.get("receivedAt").as_num().is_none() {
            return Err(ConfigError::new(format!("{}:{} is not a recording line ({{normalised, rawKeys, headersPresent}})", file.display(), i + 1)));
        }
        out.push(o);
    }
    Ok(out)
}

/// A payload and the X-Bellwether-* headers from one recording line. With a `tag`, the
/// line plays as another session in another pane (session ids get `-<tag>`, the terminal
/// becomes herdr pane `<tag>`).
pub fn rebuild(line: &Js, tag: Option<&str>) -> (Js, Obj) {
    let mut n = line.get("normalised").as_obj().cloned().unwrap_or_default();
    if let Some(tag) = tag {
        let sid = n.get("sessionId").cloned().unwrap_or(Js::Undefined);
        n.set("sessionId", if sid.truthy() { Js::Str(format!("{}-{tag}", js_string(&sid))) } else { sid });
        n.set("terminal", Obj::new().with("program", "iTerm.app").with("herdrPane", tag));
    }
    let get = |k: &str| n.get(k).cloned().unwrap_or(Js::Undefined);
    let mut payload = Obj::new().with("hook_event_name", get("event"));
    for (from, to) in FROM_NORMALISED {
        let v = get(from);
        if !v.is_undefined() {
            payload.set(to, v);
        }
    }
    if get("event").as_str() == Some("StopFailure") && get("errorType").truthy() {
        payload.set("error", get("errorType"));
    }
    if get("backgroundTaskCount").is_integer() {
        let count = get("backgroundTaskCount").as_num().unwrap().max(0.0) as usize;
        payload.set("background_tasks", Js::Arr((0..count).map(|i| Js::Obj(Obj::new().with("id", marker(&format!("background_tasks.{i}"))))).collect()));
    }
    let raw = line.get("rawKeys");
    let top: Vec<String> = raw.get("top").as_arr().map(|a| a.iter().map(js_string).collect()).unwrap_or_default();
    let tool_keys: Vec<String> = raw.get("tool_input").as_arr().map(|a| a.iter().map(js_string).collect()).unwrap_or_default();
    let has_input = top.iter().any(|k| k == "tool_input");
    if (has_input || get("toolName").truthy()) && (has_input || !get("toolSummary").is_undefined() || !get("toolFingerprint").is_undefined()) {
        payload.set("tool_input", rebuild_tool_input(&n, &tool_keys));
    }
    for k in &top {
        if !has(&payload, k) && !READ_FIELDS.contains(&k.as_str()) {
            payload.set(k.clone(), marker(k));
        }
    }
    let terminal = n.get("terminal").cloned().unwrap_or(Js::Undefined);
    let mut headers = Obj::new();
    for (field, header) in TERMINAL_HEADERS {
        let v = terminal.get(field);
        headers.set(header, if matches!(v, Js::Undefined | Js::Null) { Js::str("") } else { v.clone() });
    }
    (Js::Obj(payload), headers)
}

fn rebuild_tool_input(n: &Obj, keys: &[String]) -> Js {
    let mut input = Obj::new();
    if let Some(raw) = n.get("toolSummary").filter(|s| !s.is_undefined()) {
        // `input.command = s` keeps the value as it is; a template literal makes it text.
        let s = js_string(raw);
        let cwd = n.get("cwd").filter(|c| !matches!(c, Js::Undefined | Js::Null)).map(js_string).unwrap_or_else(|| "/replay".into());
        match n.get("toolName").and_then(Js::as_str) {
            Some("Bash") => input.set("command", raw.clone()),
            Some("Edit" | "Write" | "Read") => input.set("file_path", format!("{cwd}/{s}")),
            Some("NotebookEdit") => input.set("notebook_path", format!("{cwd}/{s}")),
            Some("WebFetch") => input.set("url", format!("https://{s}/")),
            Some("WebSearch") => input.set("query", raw.clone()),
            Some("Grep" | "Glob") => input.set("pattern", raw.clone()),
            Some("Task" | "Agent") => input.set("subagent_type", raw.clone()),
            _ => {}
        }
    }
    for k in keys {
        if !has(&input, k) && !SUMMARY_FIELDS.contains(&k.as_str()) {
            input.set(k.clone(), marker(&format!("tool_input.{k}")));
        }
    }
    Js::Obj(input)
}

/// A synthetic load: PreToolUse/PostToolUse pairs at `rate` events per second.
pub fn burst(rate: f64, seconds: f64) -> Vec<Js> {
    let total = ((rate * seconds).round() as i64).max(2) as usize;
    let terminal = Obj::new().with("herdrPane", "w9:p9").with("program", "iTerm.app");
    let top = ["cwd", "duration_ms", "hook_event_name", "session_id", "tool_input", "tool_name", "tool_use_id"];
    (0..total)
        .map(|i| {
            let pre = i % 2 == 0;
            let normalised = Obj::new()
                .with("receivedAt", ((i as f64) * 1000.0 / rate).round())
                .with("event", if pre { "PreToolUse" } else { "PostToolUse" })
                .with("sessionId", "burst-session")
                .with("toolName", "Read")
                .with("toolUseId", format!("toolu_burst_{}", i / 2))
                .with("cwd", "/replay/burst")
                .with("toolSummary", format!("file{}.txt", i % 7))
                .with("terminal", terminal.clone());
            let raw = Obj::new().with("top", Js::Arr(top.iter().map(|k| Js::str(k)).collect())).with("tool_input", Js::Arr(vec![Js::str("file_path")]));
            Js::Obj(Obj::new().with("normalised", normalised).with("rawKeys", raw).with("headersPresent", Js::Arr(vec![])))
        })
        .collect()
}
