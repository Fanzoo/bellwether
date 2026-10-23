//! bellwether's hook block, and the installer that merges it into a Claude Code settings
//! file (a port of brain/hooks.mjs, over the lossless JSON of [`crate::lossless`]).
//!
//! The settings file already holds other tools' hooks. The installer only ever appends its
//! own matcher groups and removes its own handlers. A handler is bellwether's if, and only
//! if, it is an http handler whose url is exactly `http://127.0.0.1:<port>/hook` (the
//! exact-URL rule). Nothing else is touched, reordered or deduplicated, and
//! [`assert_others_preserved`] checks that before any write. Entries written by pasture
//! (the name before the rename) are on the same URL, so they are bellwether's too and get
//! replaced. Entries left on another port after a port change are bellwether-like but not
//! bellwether's: doctor flags them, and `hooks uninstall --port <old>` removes them.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::{io_code, ConfigError, BAD_CONFIG};
use crate::json::{self, Js, Obj};
use crate::lossless::{self, deep_equal, LObj, Lj};

/// Exit 1: an unexpected failure.
pub const UNEXPECTED: i32 = 1;

/// The subscribed events. SessionStart is absent on purpose: Claude Code skips HTTP hooks
/// on SessionStart and Setup (measured against Claude Code 2.1.295).
pub const EVENTS: [&str; 16] = [
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "Elicitation",
    "ElicitationResult",
];

/// Events on which Claude Code silently skips every HTTP hook.
pub const HTTP_UNSUPPORTED_EVENTS: [&str; 2] = ["SessionStart", "Setup"];

/// Header name and the environment variable Claude Code interpolates into it, in order.
pub const ENV_HEADERS: [(&str, &str); 7] = [
    ("X-Bellwether-Term-Program", "TERM_PROGRAM"),
    ("X-Bellwether-Iterm-Session", "ITERM_SESSION_ID"),
    ("X-Bellwether-Term-Session", "TERM_SESSION_ID"),
    ("X-Bellwether-Wt-Session", "WT_SESSION"),
    ("X-Bellwether-Tmux-Pane", "TMUX_PANE"),
    ("X-Bellwether-Herdr-Pane", "HERDR_PANE_ID"),
    ("X-Bellwether-Herdr-Socket", "HERDR_SOCKET_PATH"),
];

/// Above this, a hung brain would stall tool calls noticeably. Doctor enforces it.
pub const MAX_TIMEOUT_SEC: f64 = 2.0;

pub fn hook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/hook")
}

/// The one handler bellwether writes on every event. It never sets `onFailure` (plan D3).
pub fn hook_handler(port: u16, token: &str) -> Lj {
    let mut headers = LObj::new().with("X-Bellwether-Token", Lj::str(token));
    for (h, env) in ENV_HEADERS {
        headers.set(h, Lj::Str(format!("${env}")));
    }
    Lj::Obj(
        LObj::new()
            .with("type", Lj::str("http"))
            .with("url", Lj::Str(hook_url(port)))
            .with("timeout", Lj::int(1))
            .with("headers", Lj::Obj(headers))
            .with("allowedEnvVars", Lj::Arr(ENV_HEADERS.iter().map(|(_, e)| Lj::str(e)).collect())),
    )
}

/// A settings object holding only bellwether's hooks: what `hooks print` emits.
pub fn hook_block(port: u16, token: &str) -> Lj {
    let mut hooks = LObj::new();
    for e in EVENTS {
        hooks.set(e, Lj::Arr(vec![Lj::Obj(LObj::new().with("hooks", Lj::Arr(vec![hook_handler(port, token)])))]));
    }
    Lj::Obj(LObj::new().with("hooks", Lj::Obj(hooks)))
}

pub fn is_bellwether_handler(h: &Lj, port: u16) -> bool {
    h.get("type").and_then(Lj::as_str) == Some("http") && h.get("url").and_then(Lj::as_str) == Some(hook_url(port).as_str())
}

/// The token header's value: X-Bellwether-Token, or X-Pasture-Token on an entry written
/// before the rename (`??`: a null X-Bellwether-Token falls through too).
pub fn handler_token(h: &Lj) -> Option<&Lj> {
    let headers = h.get("headers")?;
    if !headers.is_obj() {
        return None;
    }
    match headers.get("X-Bellwether-Token") {
        Some(v) if !matches!(v, Lj::Null) => Some(v),
        _ => headers.get("X-Pasture-Token"),
    }
}

/// Where a handler looks like bellwether's on some port.
#[derive(Debug, PartialEq, Eq)]
pub enum LikePort {
    /// The exact shape on this port.
    Port(u16),
    /// Another URL, but it carries a bellwether token header (Node's NaN).
    NoPort,
}

/// `bellwetherLikePort`: the port of a handler that looks like bellwether's, else None.
pub fn bellwether_like_port(h: &Lj) -> Option<LikePort> {
    if h.get("type").and_then(Lj::as_str) != Some("http") {
        return None;
    }
    let url = h.get("url").and_then(Lj::as_str)?;
    if let Some(rest) = url.strip_prefix("http://127.0.0.1:") {
        if let Some(digits) = rest.strip_suffix("/hook") {
            if (1..=5).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                let n: u32 = digits.parse().unwrap();
                return Some(if n <= u16::MAX as u32 { LikePort::Port(n as u16) } else { LikePort::NoPort });
            }
        }
    }
    handler_token(h).map(|_| LikePort::NoPort)
}

/// An entry written by pasture, before the rename: on the exact URL, with X-Pasture-Token
/// and not X-Bellwether-Token.
pub fn is_legacy_handler(h: &Lj, port: u16) -> bool {
    is_bellwether_handler(h, port) && h.get("headers").is_some_and(|hs| hs.is_obj() && hs.get("X-Pasture-Token").is_some() && hs.get("X-Bellwether-Token").is_none())
}

fn is_bellwether_group(g: &Lj, port: u16) -> bool {
    g.get("hooks").and_then(Lj::as_arr).is_some_and(|hs| hs.iter().any(|h| is_bellwether_handler(h, port)))
}

/// Groups with bellwether's handlers taken out; a group bellwether's removal emptied goes.
/// The flag says whether anything was bellwether's.
fn remove_from_groups(groups: &[Lj], port: u16) -> (Vec<Lj>, bool) {
    let mut kept = Vec::new();
    let mut touched = false;
    for g in groups {
        if !is_bellwether_group(g, port) {
            kept.push(g.clone());
            continue;
        }
        touched = true;
        let rest: Vec<Lj> = g.get("hooks").and_then(Lj::as_arr).map(|hs| hs.iter().filter(|h| !is_bellwether_handler(h, port)).cloned().collect()).unwrap_or_default();
        if !rest.is_empty() {
            let mut copy = g.as_obj().cloned().unwrap_or_default();
            copy.set("hooks", Lj::Arr(rest));
            kept.push(Lj::Obj(copy));
        }
    }
    (kept, touched)
}

fn groups_of(v: &Lj) -> &[Lj] {
    v.as_arr().map(Vec::as_slice).unwrap_or(&[])
}

/// Where the settings shape is something Claude Code itself would reject, the first
/// places that are wrong. Used by the installer (refuse, exit 3) and by doctor (report).
pub fn shape_problems(settings: &Lj) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(o) = settings.as_obj() else { return vec!["the file does not hold a JSON object".into()] };
    let Some(hooks) = o.get("hooks") else { return problems };
    let Some(hooks) = hooks.as_obj() else { return vec!["\"hooks\" is not an object".into()] };
    for (event, groups) in hooks.iter() {
        let event = crate::jsstr::to_string_lossy(event);
        let Some(groups) = groups.as_arr() else {
            problems.push(format!("\"hooks.{event}\" is not an array"));
            continue;
        };
        for (i, g) in groups.iter().enumerate() {
            match g.as_obj() {
                None => problems.push(format!("\"hooks.{event}[{i}]\" is not an object")),
                Some(g) if !matches!(g.get("hooks"), Some(Lj::Arr(_))) => problems.push(format!("\"hooks.{event}[{i}].hooks\" is not an array")),
                _ => {}
            }
        }
    }
    problems
}

fn assert_shape(settings: &Lj) -> Result<(), ConfigError> {
    let problems = shape_problems(settings);
    if problems.is_empty() {
        return Ok(());
    }
    Err(ConfigError::new(format!(
        "the settings file has a shape Claude Code would reject: {}. Fix it first; nothing was written.",
        problems.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
    )))
}

/// A settings object with exactly one bellwether matcher group per subscribed event. An
/// event whose only bellwether entry is already a lone, current handler is left alone, so
/// a second install is a no-op. Otherwise stale bellwether handlers on that event are
/// removed and a fresh group is appended at the end.
pub fn install(settings: &Lj, port: u16, token: &str) -> Result<Lj, ConfigError> {
    assert_shape(settings)?;
    let mut out = settings.clone();
    let wanted = hook_handler(port, token);
    let root = out.as_obj_mut().expect("checked by assert_shape");
    if !root.contains("hooks") {
        root.set("hooks", Lj::Obj(LObj::new()));
    }
    let hooks = root.get_mut("hooks").and_then(Lj::as_obj_mut).expect("checked by assert_shape");
    for event in EVENTS {
        let groups: Vec<Lj> = hooks.get(event).map(|g| groups_of(g).to_vec()).unwrap_or_default();
        let ours: Vec<&Lj> = groups.iter().filter(|g| is_bellwether_group(g, port)).collect();
        let current = ours.len() == 1 && ours[0].as_obj().is_some_and(|o| o.len() == 1) && ours[0].get("hooks").and_then(Lj::as_arr).is_some_and(|hs| hs.len() == 1 && deep_equal(&hs[0], &wanted));
        if current {
            continue;
        }
        let (mut kept, _) = remove_from_groups(&groups, port);
        kept.push(Lj::Obj(LObj::new().with("hooks", Lj::Arr(vec![wanted.clone()]))));
        hooks.set(event, Lj::Arr(kept));
    }
    // Bellwether entries on events it no longer subscribes to (SessionStart, say) go.
    let others: Vec<String> = hooks.keys().into_iter().filter(|e| !EVENTS.contains(&e.as_str())).cloned().collect();
    for event in others {
        let groups = groups_of(hooks.get(&event).unwrap()).to_vec();
        let (kept, touched) = remove_from_groups(&groups, port);
        if !touched {
            continue;
        }
        if kept.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.set(event, Lj::Arr(kept));
        }
    }
    Ok(out)
}

/// What an install created, from the install record: which event keys, and whether the
/// hooks object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub events: Vec<String>,
    pub hooks: bool,
}

/// Removes every bellwether handler from every event. A matcher group is removed only when
/// bellwether's removal emptied it. An event key, or the hooks object, that the removal
/// emptied is removed only if bellwether created it (`created`, from the install record);
/// without a record, an emptied key is assumed to be bellwether's and removed.
pub fn uninstall(settings: &Lj, port: u16, created: Option<&Created>) -> Result<Lj, ConfigError> {
    assert_shape(settings)?;
    let mut out = settings.clone();
    let root = out.as_obj_mut().expect("checked by assert_shape");
    let Some(hooks) = root.get_mut("hooks").and_then(Lj::as_obj_mut) else { return Ok(out) };
    let mut emptied = false;
    let events: Vec<String> = hooks.keys().into_iter().cloned().collect();
    for event in events {
        let groups = groups_of(hooks.get(&event).unwrap()).to_vec();
        let (kept, touched) = remove_from_groups(&groups, port);
        if !touched {
            continue;
        }
        if !kept.is_empty() || created.is_some_and(|c| !c.events.contains(&event)) {
            hooks.set(event, Lj::Arr(kept));
        } else {
            hooks.remove(&event);
            emptied = true;
        }
    }
    if emptied && hooks.is_empty() && created.is_none_or(|c| c.hooks) {
        root.remove("hooks");
    }
    Ok(out)
}

/// The settings with bellwether's handlers taken out and empty containers dropped.
fn others_only(settings: &Lj, port: u16) -> Lj {
    let mut out = settings.clone();
    let Some(root) = out.as_obj_mut() else { return out };
    let Some(hooks) = root.get_mut("hooks").and_then(Lj::as_obj_mut) else { return out };
    let events: Vec<String> = hooks.keys().into_iter().cloned().collect();
    for event in events {
        let (kept, _) = remove_from_groups(groups_of(hooks.get(&event).unwrap()), port);
        if kept.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.set(event, Lj::Arr(kept));
        }
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    out
}

/// The guard behind every write: with bellwether's handlers removed from both versions,
/// and empty containers ignored on both sides, they must be deep-equal.
pub fn assert_others_preserved(before: &Lj, after: &Lj, port: u16) -> Result<(), ConfigError> {
    if deep_equal(&others_only(before, port), &others_only(after, port)) {
        return Ok(());
    }
    Err(ConfigError { message: "internal check failed: the edit would change a non-bellwether setting. Nothing was written.".into(), code: UNEXPECTED })
}

/// bellwether matcher groups per event (events with none left out). Tolerates any shape.
pub fn count_groups(settings: &Lj, port: u16) -> Vec<(String, usize)> {
    let Some(hooks) = settings.get("hooks").and_then(Lj::as_obj) else { return Vec::new() };
    hooks
        .iter()
        .filter_map(|(e, groups)| groups.as_arr().map(|gs| (e.clone(), gs.iter().filter(|g| is_bellwether_group(g, port)).count())))
        .filter(|(_, n)| *n > 0)
        .collect()
}

pub fn sum_groups(counts: &[(String, usize)]) -> usize {
    counts.iter().map(|(_, n)| n).sum()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ the install record

/// `<data>/installs.json`: for each settings file bellwether installed into (by its real
/// path), which event keys the install created and whether it created the hooks object.
/// Advisory: anything malformed in it is ignored with a warning, and failing to write it
/// never fails a command whose settings write already happened.
pub fn read_install_records(data_dir: &Path) -> (Obj, Option<String>) {
    let f = data_dir.join("installs.json");
    let raw = match std::fs::read(&f) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Obj::new(), None),
        Err(e) => return (Obj::new(), Some(format!("could not read {} ({}); treating it as empty", f.display(), io_code(&e)))),
    };
    let mut all = Obj::new();
    let parsed = match json::parse(&raw) {
        Ok(Js::Obj(o)) => o,
        _ => return (all, Some(format!("{} is not a valid install record; ignoring it (uninstall falls back to removing event keys bellwether emptied)", f.display()))),
    };
    let mut dropped = 0;
    for (k, r) in parsed.iter() {
        match record_of(r) {
            Some(c) => all.set(k.clone(), Js::Obj(Obj::new().with("events", Js::Arr(c.events.iter().map(|e| Js::Str(e.clone())).collect())).with("hooks", c.hooks))),
            None => dropped += 1,
        }
    }
    let warning = (dropped > 0).then(|| format!("{}: ignored {dropped} malformed entr{}", f.display(), if dropped == 1 { "y" } else { "ies" }));
    (all, warning)
}

fn record_of(r: &Js) -> Option<Created> {
    let events = r.get("events").as_arr()?;
    let hooks = r.get("hooks").as_bool()?;
    if !r.is_plain_object() {
        return None;
    }
    let events: Option<Vec<String>> = events.iter().map(|e| e.as_str().map(str::to_string)).collect();
    Some(Created { events: events?, hooks })
}

/// The record key of a settings file: its real path, else the absolute path.
pub fn record_key(file: &Path) -> String {
    let p = std::fs::canonicalize(file).unwrap_or_else(|_| resolve(file));
    crate::jsstr::from_str(&p.to_string_lossy())
}

/// `path.resolve(p)`: absolute, with `.` and `..` folded.
pub fn resolve(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn read_install_record(data_dir: &Path, file: &Path) -> (Option<Created>, Option<String>) {
    let (all, warning) = read_install_records(data_dir);
    (all.get(&record_key(file)).and_then(record_of), warning)
}

// ------------------------------------------------------------------ plan and apply

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Uninstall => "uninstall",
        }
    }
}

/// A planned edit of a settings file (`planEdit`): read only; [`apply_plan`] writes it.
pub struct Plan {
    pub file: PathBuf,
    pub action: Action,
    pub exists: bool,
    pub original_hash: Option<String>,
    pub original_bytes: Option<Vec<u8>>,
    pub before: Lj,
    pub after: Lj,
    pub changed: bool,
    pub old_text: String,
    pub new_text: String,
    /// Whether the file was already laid out exactly as bellwether writes JSON. When it
    /// isn't (key spacing, inline arrays, mixed line endings), a write reformats existing
    /// lines too, although every value stays the same.
    pub layout_kept: bool,
    pub record: Option<Created>,
    pub warnings: Vec<String>,
}

/// Plans an install or uninstall. A missing file counts as `{}` (Claude Code treats it the
/// same). `data_dir` holds the install record (None: neither read nor written).
pub fn plan_edit(file: &Path, action: Action, port: u16, token: &str, data_dir: Option<&Path>) -> Result<Plan, ConfigError> {
    let bytes = match std::fs::read(file) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ConfigError::new(format!("cannot read {}: {}", file.display(), io_code(&e)))),
    };
    let old_text = bytes.as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    let before = match &bytes {
        None => Lj::Obj(LObj::new()),
        Some(_) => {
            let v = lossless::parse(lossless::split_bom(&old_text).1).map_err(|e| ConfigError::new(format!("{} is not valid JSON ({e}). Fix it first; nothing was written.", file.display())))?;
            if !v.is_obj() {
                return Err(ConfigError::new(format!("{} does not hold a JSON object; nothing was written.", file.display())));
            }
            v
        }
    };
    let (previous, record_warning) = match data_dir {
        Some(d) => read_install_record(d, file),
        None => (None, None),
    };
    let after = match action {
        Action::Install => install(&before, port, token)?,
        Action::Uninstall => uninstall(&before, port, previous.as_ref())?,
    };
    assert_others_preserved(&before, &after, port)?;
    let changed = !deep_equal(&before, &after);
    // CRLF and the indent are the file's own (serialise); a BOM is kept too (W2).
    let (bom, body) = lossless::split_bom(&old_text);
    let new_text = if changed { format!("{bom}{}", lossless::serialise(&after, body)) } else { old_text.clone() };
    let record = match action {
        Action::Uninstall => None,
        Action::Install => {
            let empty = LObj::new();
            let had = before.get("hooks").and_then(Lj::as_obj).unwrap_or(&empty);
            let prev_events: Vec<String> = previous.as_ref().map(|p| p.events.clone()).unwrap_or_default();
            let events = EVENTS
                .iter()
                .filter(|e| match had.get(e) {
                    None => true,
                    Some(groups) => prev_events.iter().any(|p| p == *e) && groups.as_arr().is_some_and(|gs| !gs.is_empty() && gs.iter().all(|g| is_bellwether_group(g, port))),
                })
                .map(|e| e.to_string())
                .collect();
            let hooks = before.get("hooks").is_none() || (previous.as_ref().is_some_and(|p| p.hooks) && had.keys().iter().all(|e| prev_events.contains(e)));
            Some(Created { events, hooks })
        }
    };
    let layout_kept = bytes.is_none() || format!("{bom}{}", lossless::serialise(&before, body)) == old_text;
    Ok(Plan {
        file: file.to_path_buf(),
        action,
        exists: bytes.is_some(),
        original_hash: bytes.as_deref().map(sha256_hex),
        original_bytes: bytes,
        before,
        after,
        changed,
        old_text,
        new_text,
        layout_kept,
        record,
        warnings: record_warning.into_iter().collect(),
    })
}

/// Writes a planned edit: back up the original bytes, re-read and compare the hash
/// (another tool may have edited the file meanwhile), then replace the file atomically
/// through a temp file in the same directory. Backups hold the token, so they are 0600;
/// a settings file bellwether creates is 0600 too. Returns the backup's path.
pub fn apply_plan(plan: &mut Plan, backup_dir: &Path, data_dir: Option<&Path>, stamp: &str) -> Result<Option<PathBuf>, ConfigError> {
    if !plan.changed {
        return Ok(None);
    }
    let fail = |what: String| ConfigError { message: what, code: UNEXPECTED };
    // Decided before writing: the new install record's content, from valid entries only.
    let record_text = data_dir.map(|d| {
        let (mut all, _) = read_install_records(d);
        let key = record_key(&plan.file);
        match (&plan.action, &plan.record) {
            (Action::Install, Some(r)) => all.set(key, Js::Obj(Obj::new().with("events", Js::Arr(r.events.iter().map(|e| Js::Str(e.clone())).collect())).with("hooks", r.hooks))),
            _ => {
                all.remove(&key);
            }
        }
        json::stringify_pretty(&Js::Obj(all), "  ") + "\n"
    });
    let target = if plan.exists { std::fs::canonicalize(&plan.file).map_err(|e| fail(format!("cannot resolve {}: {}", plan.file.display(), io_code(&e))))? } else { plan.file.clone() };
    let mut backup = None;
    if plan.exists {
        #[cfg(windows)]
        let fresh = !backup_dir.exists();
        std::fs::create_dir_all(backup_dir).map_err(|e| fail(format!("cannot create {}: {}", backup_dir.display(), io_code(&e))))?;
        // Windows (W2): the backups hold the token, so their folder is the user's only
        // (acl.rs), as their 0600 is on Unix. Best effort; the backup file below is too.
        #[cfg(windows)]
        if fresh {
            let _ = crate::acl::make_private(backup_dir, true);
        }
        let b = backup_dir.join(format!("settings-{stamp}.json"));
        write_backup(&b, plan.original_bytes.as_deref().unwrap_or(&[])).map_err(|e| fail(format!("cannot write the backup {}: {}", b.display(), io_code(&e))))?;
        let now = std::fs::read(&target).map_err(|e| fail(format!("cannot read {}: {}", target.display(), io_code(&e))))?;
        if Some(sha256_hex(&now)) != plan.original_hash {
            return Err(fail(format!("{} changed while bellwether was editing it. Nothing was written; run the command again.", plan.file.display())));
        }
        backup = Some(b);
    }
    let mode = if plan.exists { file_mode(&target).unwrap_or(0o600) } else { 0o600 };
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("cannot create {}: {}", dir.display(), io_code(&e))))?;
    let base = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{base}.bellwether-{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let written = write_new(&tmp, plan.new_text.as_bytes(), mode).and_then(|_| set_mode(&tmp, mode)).and_then(|_| crate::persist::replace_file(&tmp, &target));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(write_failure(&plan.file, &e, crate::persist::rename_is_transient(&e)));
    }
    // The settings file is written. From here on nothing may fail the command: a problem
    // with the advisory record becomes a warning.
    if let (Some(text), Some(d)) = (record_text, data_dir) {
        let rf = d.join("installs.json");
        let r = std::fs::create_dir_all(d).and_then(|_| write_replace(&rf, text.as_bytes(), 0o600)).and_then(|_| set_mode(&rf, 0o600));
        if let Err(e) = r {
            plan.warnings.push(format!("could not update {} ({}); a later uninstall falls back to removing event keys bellwether emptied", rf.display(), io_code(&e)));
        }
    }
    Ok(backup)
}

fn file_mode(p: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        Some(0o600)
    }
}

pub fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (p, mode);
        Ok(())
    }
}

/// Creates a file that must not exist yet (`flag: 'wx'`), with `mode`.
fn write_new(p: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    o.open(p)?.write_all(bytes)
}

/// Why the settings file couldn't be replaced, named by the path the user gave (not the
/// resolved `\\?\C:\…` form Windows' canonicalize returns). `in_use`: Windows still
/// refused after the retry (another program has it open; W2-15): exit 3, as the other
/// "nothing was written" refusals, with what to do. Anything else stays exit 1.
pub fn write_failure(file: &Path, e: &std::io::Error, in_use: bool) -> ConfigError {
    if in_use {
        return ConfigError { message: format!("{} is in use by another program (an editor, or a virus scan), so it wasn't changed. Close it there and run the command again", file.display()), code: BAD_CONFIG };
    }
    ConfigError { message: format!("cannot write {}: {}", file.display(), io_code(e)), code: UNEXPECTED }
}

/// A settings backup: a new file, 0600 on Unix; on Windows the user's only (a protected
/// DACL, acl.rs) before anything is written to it (W2).
fn write_backup(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(p)?;
    #[cfg(windows)]
    let _ = crate::acl::make_private(p, false);
    f.write_all(bytes)
}

/// `writeFileSync(p, bytes, { mode })`: created with `mode`, or truncated and rewritten.
pub fn write_replace(p: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    o.open(p)?.write_all(bytes)
}

/// An exit-3 error.
pub fn bad_config(message: impl Into<String>) -> ConfigError {
    ConfigError { message: message.into(), code: BAD_CONFIG }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W2-15 (BR-3K's D2): a settings file another program still holds after the retry is
    /// named as the user gave it, says what to do, and is exit 3; other failures stay exit 1.
    #[test]
    fn a_file_in_use_says_so_and_is_exit_3() {
        let given = Path::new("C:\\Users\\me\\scratch.json");
        let e = write_failure(given, &std::io::Error::from_raw_os_error(32), true);
        assert_eq!(e.code, BAD_CONFIG);
        assert!(e.message.starts_with("C:\\Users\\me\\scratch.json is in use by another program") && e.message.contains("run the command again"), "{}", e.message);
        assert!(!e.message.contains("\\\\?\\") && !e.message.contains("EACCES"));
        let other = write_failure(given, &std::io::Error::from(std::io::ErrorKind::NotFound), false);
        assert_eq!(other.code, UNEXPECTED);
        assert!(other.message.starts_with("cannot write C:\\Users\\me\\scratch.json"));
    }

    const T: &str = "testTokenNotReal_abcdefghijklmnop";

    #[test]
    fn install_twice_then_uninstall_gives_back_the_original() {
        let original = lossless::parse(r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"k"}]}],"SessionStart":[{"hooks":[{"type":"http","url":"http://127.0.0.1:47555/hook"}]}]}}"#).unwrap();
        let once = install(&original, 47555, T).unwrap();
        assert_eq!(sum_groups(&count_groups(&once, 47555)), 16);
        assert!(once.get("hooks").unwrap().get("SessionStart").is_none(), "the dead SessionStart entry went");
        let twice = install(&once, 47555, T).unwrap();
        assert!(deep_equal(&once, &twice));
        let back = uninstall(&twice, 47555, None).unwrap();
        assert_eq!(count_groups(&back, 47555), vec![]);
        assert_eq!(lossless::stringify(&back, ""), r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"k"}]}]}}"#);
        // A foreign handler in bellwether's group stays.
        let mixed = lossless::parse(r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"},{"type":"http","url":"http://127.0.0.1:47555/hook"}]}]}}"#).unwrap();
        assert_eq!(lossless::stringify(&uninstall(&mixed, 47555, None).unwrap(), ""), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#);
    }

    #[test]
    fn shapes_claude_code_rejects_are_named() {
        let bad = lossless::parse(r#"{"hooks":{"Stop":[{"hooks":"x"}],"A":5,"B":[3]}}"#).unwrap();
        assert_eq!(shape_problems(&bad), ["\"hooks.Stop[0].hooks\" is not an array", "\"hooks.A\" is not an array", "\"hooks.B[0]\" is not an object"]);
        assert!(install(&bad, 47555, T).unwrap_err().message.starts_with("the settings file has a shape Claude Code would reject"));
        assert_eq!(shape_problems(&lossless::parse("[]").unwrap()), ["the file does not hold a JSON object"]);
    }

    #[test]
    fn a_file_changed_between_plan_and_write_is_not_overwritten() {
        let d = std::env::temp_dir().join(format!("bw-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("settings.json");
        std::fs::write(&f, "{\n  \"theme\": \"dark\"\n}\n").unwrap();
        let mut plan = plan_edit(&f, Action::Install, 47555, T, Some(&d.join("data"))).unwrap();
        std::fs::write(&f, "{\n  \"theme\": \"light\"\n}\n").unwrap();
        let e = apply_plan(&mut plan, &d.join("backups"), Some(&d.join("data")), "2026-01-01T00-00-00-000Z").unwrap_err();
        assert!(e.message.contains("changed while bellwether was editing it") && e.code == UNEXPECTED, "{}", e.message);
        assert!(std::fs::read_to_string(&f).unwrap().contains("light"), "the other tool's edit stays");
        assert!(!d.join("data").join("installs.json").exists(), "no install record for a write that didn't happen");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ownership_is_the_exact_url() {
        for (url, ours) in [("http://127.0.0.1:47555/hook", true), ("http://127.0.0.1:47555/hooks", false), ("http://localhost:47555/hook", false), ("http://127.0.0.1:47555/hook?x=1", false)] {
            let h = lossless::parse(&format!(r#"{{"type":"http","url":"{url}"}}"#)).unwrap();
            assert_eq!(is_bellwether_handler(&h, 47555), ours, "{url}");
        }
        let like = lossless::parse(r#"{"type":"http","url":"http://127.0.0.1:47500/hook"}"#).unwrap();
        assert_eq!(bellwether_like_port(&like), Some(LikePort::Port(47500)));
        let tokened = lossless::parse(r#"{"type":"http","url":"http://x/y","headers":{"X-Bellwether-Token":"t"}}"#).unwrap();
        assert_eq!(bellwether_like_port(&tokened), Some(LikePort::NoPort));
    }
}
