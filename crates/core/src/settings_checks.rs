//! doctor's checks of Claude Code settings files (a port of `checkSettingsFile`,
//! `checkSettings`, `checkOtherLevels` and `globMatch` in brain/doctor.mjs). Read only:
//! nothing here writes a settings file.
//!
//! One check is new with the app (plan D3): a bellwether entry carrying
//! `"onFailure": "block"` is a FAIL, because with the brain down it would block every
//! tool call on that event. bellwether never writes `onFailure`.

use std::path::Path;

use crate::hooks::{bellwether_like_port, handler_token, hook_url, is_bellwether_handler, is_legacy_handler, shape_problems, LikePort, ENV_HEADERS, EVENTS, HTTP_UNSUPPORTED_EVENTS, MAX_TIMEOUT_SEC};
use crate::lossless::{self, Lj};

pub const OK: i32 = 0;
pub const UNEXPECTED: i32 = 1;
pub const NOT_RUNNING: i32 = 2;
pub const BAD_CONFIG: i32 = 3;

/// One doctor line.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: &'static str,
    pub detail: String,
    /// The exit code this check asks for: its own when it FAILs, else 0.
    pub exit_code: i32,
    /// For "hooks installed": how many bellwether entries were found.
    pub found: Option<usize>,
}

impl Check {
    pub fn new(name: impl Into<String>, status: &'static str, detail: impl Into<String>, exit_code: i32) -> Check {
        Check { name: name.into(), status, detail: detail.into(), exit_code: if status == "FAIL" { exit_code } else { OK }, found: None }
    }
}

/// `String(v)` for a value a settings file holds (doctor's messages quote some of them).
fn js_string(v: &Lj) -> String {
    match v {
        Lj::Null => "null".into(),
        Lj::Bool(b) => b.to_string(),
        Lj::Num(_) => crate::json::number_to_string(v.as_f64().unwrap_or(f64::NAN)),
        Lj::Str(s) => crate::jsstr::to_string_lossy(s),
        Lj::Arr(a) => a.iter().map(|x| if matches!(x, Lj::Null) { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(","),
        Lj::Obj(_) => "[object Object]".into(),
    }
}

fn lossy(s: &str) -> String {
    crate::jsstr::to_string_lossy(s)
}

/// Reads a settings file and reports on bellwether's entries in it.
pub fn check_settings_file(file: &Path, port: u16, token: Option<&str>) -> Vec<Check> {
    let shown = file.display().to_string();
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            let why = if e.kind() == std::io::ErrorKind::NotFound { "no such file".to_string() } else { crate::config::io_code(&e) };
            return vec![Check::new("hooks installed", "FAIL", format!("{shown}: {why}. Run `bellwether hooks install`"), NOT_RUNNING)];
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let (bom, body) = lossless::split_bom(&text);
    let mut out = match lossless::parse(body) {
        Ok(v) => check_settings(&v, port, token, &shown),
        Err(e) => vec![Check::new("hooks installed", "FAIL", format!("{shown} is not valid JSON ({e}); Claude Code will ignore it"), BAD_CONFIG)],
    };
    // Bellwether reads past a BOM and keeps it (W2-6), but whether Claude Code reads such a
    // file isn't confirmed for every version (W2-19): said, not failed.
    if !bom.is_empty() {
        out.push(Check::new("settings BOM", "INFO", format!("{shown} starts with a byte order mark (UTF-8 with BOM, as Notepad and PowerShell 5.1 save it). Bellwether reads past it, but some Claude Code versions may ignore a settings file that has one: if no sheep appear, save it as UTF-8 without BOM"), OK));
    }
    out
}

struct Handler<'a> {
    event: String,
    h: &'a Lj,
    matcher: Option<&'a Lj>,
}

pub fn check_settings(settings: &Lj, port: u16, token: Option<&str>, file: &str) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    let add = |out: &mut Vec<Check>, name: &str, status: &'static str, detail: String| out.push(Check::new(name, status, detail, BAD_CONFIG));

    let problems = shape_problems(settings);
    if !problems.is_empty() {
        add(&mut out, "settings shape", "FAIL", format!("{file}: {}. Claude Code rejects a file shaped like this; fix it by hand (bellwether won't edit it)", problems.iter().take(3).cloned().collect::<Vec<_>>().join("; ")));
        return out;
    }

    let mut per_event: Vec<(String, usize)> = Vec::new();
    let mut handlers: Vec<Handler> = Vec::new();
    let mut strays: Vec<(String, LikePort, bool)> = Vec::new();
    if let Some(hooks) = settings.get("hooks").and_then(Lj::as_obj) {
        for (event, groups) in hooks.iter() {
            let Some(groups) = groups.as_arr() else { continue };
            for g in groups {
                for h in g.get("hooks").and_then(Lj::as_arr).map(Vec::as_slice).unwrap_or(&[]) {
                    if is_bellwether_handler(h, port) {
                        match per_event.iter_mut().find(|(e, _)| e == event) {
                            Some((_, n)) => *n += 1,
                            None => per_event.push((event.clone(), 1)),
                        }
                        handlers.push(Handler { event: lossy(event), h, matcher: g.get("matcher") });
                    } else if let Some(like) = bellwether_like_port(h) {
                        strays.push((h.get("url").map(js_string).unwrap_or_default(), like, handler_token(h).is_some()));
                    }
                }
            }
        }
    }
    let count = |e: &str| per_event.iter().find(|(x, _)| x == e).map(|(_, n)| *n).unwrap_or(0);
    let missing: Vec<&str> = EVENTS.iter().copied().filter(|e| count(e) == 0).collect();
    let dupes: Vec<String> = per_event.iter().filter(|(_, n)| *n > 1).map(|(e, _)| lossy(e)).collect();
    if handlers.is_empty() {
        out.push(Check::new("hooks installed", "FAIL", format!("no bellwether entries in {file}. Run `bellwether hooks install` to see the diff, then add --write"), NOT_RUNNING));
    } else if !missing.is_empty() {
        out.push(Check::new("hooks installed", "FAIL", format!("{}/{} events; missing {}. Re-run `bellwether hooks install --write`", EVENTS.len() - missing.len(), EVENTS.len(), missing.join(", ")), NOT_RUNNING));
    } else {
        out.push(Check::new("hooks installed", "PASS", format!("{0}/{0} events in {file}", EVENTS.len()), NOT_RUNNING));
    }
    out.last_mut().unwrap().found = Some(handlers.len());
    if !dupes.is_empty() {
        out.push(Check::new("hooks duplicated", "FAIL", format!("more than one bellwether entry on {}. Run `bellwether hooks install --write` to collapse them", dupes.join(", ")), BAD_CONFIG));
    }
    let dead: Vec<String> = per_event.iter().filter(|(e, _)| HTTP_UNSUPPORTED_EVENTS.contains(&e.as_str())).map(|(e, _)| e.clone()).collect();
    if !dead.is_empty() {
        add(&mut out, "hooks unsupported", "WARN", format!("bellwether entries on {} never fire: Claude Code skips HTTP hooks there. `bellwether hooks install --write` removes them", dead.join(", ")));
    }
    if !strays.is_empty() {
        let unique = |list: &[&(String, LikePort, bool)]| {
            let mut urls: Vec<String> = Vec::new();
            for (u, _, _) in list {
                if !urls.contains(u) {
                    urls.push(u.clone());
                }
            }
            urls.join(", ")
        };
        let fix = |list: &[&(String, LikePort, bool)]| {
            let mut ports: Vec<u16> = Vec::new();
            for (_, p, _) in list {
                if let LikePort::Port(p) = p {
                    if !ports.contains(p) {
                        ports.push(*p);
                    }
                }
            }
            if ports.is_empty() {
                String::new()
            } else {
                format!(" To remove them: {}.", ports.iter().map(|p| format!("`bellwether hooks uninstall --port {p} --write`")).collect::<Vec<_>>().join(", "))
            }
        };
        let like: Vec<&(String, LikePort, bool)> = strays.iter().filter(|s| s.2).collect();
        let foreign: Vec<&(String, LikePort, bool)> = strays.iter().filter(|s| !s.2).collect();
        if !like.is_empty() {
            add(&mut out, "hooks stray", "WARN", format!("bellwether-like entries (they carry X-Bellwether-Token) that aren't on port {port}'s exact URL: {}. bellwether leaves them alone (exact-URL ownership).{}", unique(&like), fix(&like)));
        }
        if !foreign.is_empty() {
            add(&mut out, "hooks foreign", "INFO", format!("another tool's hook on a bellwether-style URL (no X-Bellwether-Token): {}. Not bellwether's; nothing to do unless it's a leftover of yours (`hooks uninstall --port <port>` would remove it).", unique(&foreign)));
        }
    }
    let legacy = handlers.iter().filter(|x| is_legacy_handler(x.h, port)).count();
    if legacy > 0 {
        // The app's brain refuses X-Pasture-* headers (test/DIVERGENCES.md 4), so these
        // entries fail every call: a FAIL here, where the Node doctor only warns.
        out.push(Check::new(
            "hooks from pasture",
            "FAIL",
            format!(
                "{legacy} entr{} written by pasture, before the rename (X-Pasture-* headers). The app's brain refuses them (403); `bellwether hooks install --write` (or `bellwether migrate --write`) replaces them",
                if legacy == 1 { "y was" } else { "ies were" }
            ),
            BAD_CONFIG,
        ));
    }
    let tokenless: Vec<&str> = handlers.iter().filter(|x| handler_token(x.h).is_none()).map(|x| x.event.as_str()).collect();
    if !tokenless.is_empty() {
        add(
            &mut out,
            "hooks foreign",
            "WARN",
            format!("an http hook on bellwether's exact URL with no X-Bellwether-Token header ({}): probably another tool's hook on bellwether's port. install and uninstall treat it as bellwether's (exact-URL rule), and the brain refuses its requests (403)", tokenless.join(", ")),
        );
    }

    if !handlers.is_empty() {
        let bad: Vec<String> = handlers
            .iter()
            .filter(|x| match x.h.get("timeout") {
                Some(t @ Lj::Num(_)) => t.as_f64().is_none_or(|n| n.is_nan() || n <= 0.0 || n > MAX_TIMEOUT_SEC),
                _ => true,
            })
            .map(|x| format!("{} (timeout {})", x.event, x.h.get("timeout").filter(|t| !matches!(t, Lj::Null)).map(js_string).unwrap_or_else(|| "unset".into())))
            .collect();
        if !bad.is_empty() {
            add(&mut out, "hook timeout", "FAIL", format!("{}: every bellwether entry needs \"timeout\" of at most {} s, or a hung brain stalls tool calls for up to 600 s. Re-run `bellwether hooks install --write`", bad.join(", "), MAX_TIMEOUT_SEC));
        } else {
            add(&mut out, "hook timeout", "PASS", format!("every entry has timeout ≤ {MAX_TIMEOUT_SEC} s"));
        }
        let blocking: Vec<&str> = handlers.iter().filter(|x| x.h.get("onFailure").and_then(Lj::as_str) == Some("block")).map(|x| x.event.as_str()).collect();
        if !blocking.is_empty() {
            add(
                &mut out,
                "hook onFailure",
                "FAIL",
                format!("bellwether entries on {} set \"onFailure\": \"block\": with the brain down, Claude Code would block every tool call there. bellwether never writes onFailure; `bellwether hooks install --write` replaces them", blocking.join(", ")),
            );
        }
        let matchers: Vec<&str> = handlers.iter().filter(|x| x.matcher.is_some_and(|m| !matches!(m.as_str(), Some("") | Some("*")))).map(|x| x.event.as_str()).collect();
        if !matchers.is_empty() {
            add(&mut out, "hook matcher", "WARN", format!("matcher set on {}; bellwether expects every event unfiltered", matchers.join(", ")));
        }
        // Entries written before a header was added (X-Bellwether-Entrypoint, say) still
        // work, but those sessions can't be placed or focused.
        let old: Vec<&str> = handlers
            .iter()
            .filter(|x| !is_legacy_handler(x.h, port) && !ENV_HEADERS.iter().all(|(name, _)| x.h.get("headers").is_some_and(|hs| hs.get(name).is_some())))
            .map(|x| x.event.as_str())
            .collect();
        if !old.is_empty() {
            add(&mut out, "hook headers", "WARN", format!("bellwether entries on {} were written by an older version and miss some terminal headers, so a click on a sheep can't always find its window (sessions in the Claude desktop app, for one). Re-run `bellwether hooks install --write`", if old.len() == EVENTS.len() { "every event".to_string() } else { old.join(", ") }));
        }
        if let Some(token) = token {
            let stale = handlers.iter().filter(|x| handler_token(x.h).and_then(Lj::as_str) != Some(token)).count();
            if stale > 0 {
                add(&mut out, "hook token", "FAIL", format!("the token in {stale} entr{} differs from BELLWETHER_HOOK_TOKEN, the hook token in use, so the brain will refuse them (403). Re-run `bellwether hooks install --write`", if stale == 1 { "y" } else { "ies" }));
            } else {
                add(&mut out, "hook token", "PASS", "entries carry the hook token in use".into());
            }
        }
    }

    if matches!(settings.get("disableAllHooks"), Some(Lj::Bool(true))) {
        out.push(Check::new("disableAllHooks", "FAIL", format!("{file} sets \"disableAllHooks\": true, so no hook runs (bellwether's or anyone else's)"), BAD_CONFIG));
    }
    if let Some(Lj::Arr(allow)) = settings.get("allowedHttpHookUrls") {
        let url = hook_url(port);
        if !allow.iter().any(|p| glob_match(&js_string(p), &url)) {
            out.push(Check::new("allowedHttpHookUrls", "FAIL", format!("{file} sets allowedHttpHookUrls without a pattern matching {url}; Claude Code will skip bellwether's hooks. Add \"{url}\""), BAD_CONFIG));
        } else {
            out.push(Check::new("allowedHttpHookUrls", "PASS", format!("allows {url}"), BAD_CONFIG));
        }
    }
    if let Some(Lj::Arr(vars)) = settings.get("httpHookAllowedEnvVars") {
        let absent: Vec<&str> = ENV_HEADERS.iter().map(|(_, v)| *v).filter(|v| !vars.iter().any(|x| x.as_str() == Some(v))).collect();
        if !absent.is_empty() {
            add(&mut out, "httpHookAllowedEnvVars", "WARN", format!("does not allow {}; those terminal headers will arrive empty and click-to-focus will degrade", absent.join(", ")));
        }
    }
    out
}

/// `*` matches any run of characters; everything else is literal.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !value.starts_with(first) || value.len() < first.len() + last.len() || !value.ends_with(last) {
        return false;
    }
    let mut rest = &value[first.len()..value.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// Claude Code merges user, project, local and managed settings. Hooks in the user file
/// can still be switched off from the other levels, so doctor reads those too (never
/// writes them): `disableAllHooks`, `allowedHttpHookUrls` (a merged allowlist),
/// `httpHookAllowedEnvVars`, and managed `allowManagedHooksOnly`.
pub fn check_other_levels(project_dir: Option<&Path>, managed_file: Option<&Path>, port: u16, user_settings: Option<&Lj>) -> Vec<Check> {
    check_other_levels_with(project_dir, managed_file, &[], &[], port, user_settings)
}

/// [`check_other_levels`], with more managed levels: files (Windows' `managed-settings.d\`
/// drop-ins) and settings given as text with a name (Windows' registry policy values,
/// `HKLM` and `HKCU\SOFTWARE\Policies\ClaudeCode` `Settings`; Windows plan W2).
pub fn check_other_levels_with(project_dir: Option<&Path>, managed_file: Option<&Path>, managed_files: &[std::path::PathBuf], managed_texts: &[(String, String)], port: u16, user_settings: Option<&Lj>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut levels: Vec<(&str, std::path::PathBuf)> = Vec::new();
    if let Some(p) = project_dir {
        levels.push(("project", p.join(".claude").join("settings.json")));
        levels.push(("local", p.join(".claude").join("settings.local.json")));
    }
    if let Some(m) = managed_file {
        levels.push(("managed", m.to_path_buf()));
    }
    levels.extend(managed_files.iter().map(|f| ("managed", f.clone())));
    // Text levels are named by a path-like label (a registry value), read from `texts`.
    let texts: std::collections::HashMap<std::path::PathBuf, &str> = managed_texts.iter().map(|(name, text)| (std::path::PathBuf::from(name), text.as_str())).collect();
    levels.extend(managed_texts.iter().map(|(name, _)| ("managed", std::path::PathBuf::from(name))));
    let mut found: Vec<String> = Vec::new();
    let (mut allow, mut allow_defined): (Vec<String>, bool) = (Vec::new(), false);
    let (mut env_allow, mut env_defined): (Vec<String>, bool) = (Vec::new(), false);
    if let Some(Lj::Arr(a)) = user_settings.and_then(|u| u.get("allowedHttpHookUrls")) {
        allow_defined = true;
        allow.extend(a.iter().map(js_string));
    }
    for (label, file) in &levels {
        let bytes = match texts.get(file) {
            Some(text) => text.as_bytes().to_vec(),
            None => match std::fs::read(file) {
                Ok(b) => b,
                Err(_) => continue,
            },
        };
        let Ok(s) = lossless::parse(lossless::split_bom(&String::from_utf8_lossy(&bytes)).1) else {
            out.push(Check::new(format!("{label} settings"), "WARN", format!("{} is not valid JSON; Claude Code ignores it, and doctor can't check it", file.display()), BAD_CONFIG));
            continue;
        };
        if !s.is_obj() {
            continue;
        }
        found.push(format!("{label} ({})", file.display()));
        if matches!(s.get("disableAllHooks"), Some(Lj::Bool(true))) {
            out.push(Check::new(format!("{label} settings"), "FAIL", format!("{} sets \"disableAllHooks\": true, so no hook runs in that {}, bellwether's included", file.display(), if *label == "managed" { "machine" } else { "project" }), BAD_CONFIG));
        }
        if *label == "managed" && matches!(s.get("allowManagedHooksOnly"), Some(Lj::Bool(true))) {
            out.push(Check::new("managed settings", "FAIL", format!("{} sets \"allowManagedHooksOnly\": true, so hooks from ~/.claude/settings.json (bellwether's) never run", file.display()), BAD_CONFIG));
        }
        // A bellwether entry with onFailure "block" here blocks tool calls just the same (D3).
        let mut blocking: Vec<String> = Vec::new();
        if let Some(hooks) = s.get("hooks").and_then(Lj::as_obj) {
            for (event, groups) in hooks.iter() {
                for g in groups.as_arr().map(Vec::as_slice).unwrap_or(&[]) {
                    for h in g.get("hooks").and_then(Lj::as_arr).map(Vec::as_slice).unwrap_or(&[]) {
                        if crate::hooks::is_bellwether_handler(h, port) && h.get("onFailure").and_then(Lj::as_str) == Some("block") {
                            blocking.push(lossy(event));
                        }
                    }
                }
            }
        }
        if !blocking.is_empty() {
            out.push(Check::new(
                "hook onFailure",
                "FAIL",
                format!("bellwether entries on {} in {} ({label} settings) set \"onFailure\": \"block\": with the brain down, Claude Code would block every tool call there. bellwether never writes onFailure; remove the entries there (`bellwether hooks install` only edits ~/.claude/settings.json)", blocking.join(", "), file.display()),
                BAD_CONFIG,
            ));
        }
        if let Some(Lj::Arr(a)) = s.get("allowedHttpHookUrls") {
            allow_defined = true;
            allow.extend(a.iter().map(js_string));
        }
        if let Some(Lj::Arr(a)) = s.get("httpHookAllowedEnvVars") {
            env_defined = true;
            env_allow.extend(a.iter().filter_map(|x| x.as_str().map(lossy)));
        }
    }
    if !levels.is_empty() {
        let detail = if !found.is_empty() { format!("also read: {}", found.join(", ")) } else { format!("no project, local or managed settings file found{}", project_dir.map(|p| format!(" for {}", p.display())).unwrap_or_default()) };
        out.push(Check::new("other settings levels", "INFO", detail, BAD_CONFIG));
    }
    if !found.is_empty() && allow_defined {
        let url = hook_url(port);
        if !allow.iter().any(|p| glob_match(p, &url)) {
            out.push(Check::new("allowedHttpHookUrls (merged)", "FAIL", format!("the merged allowedHttpHookUrls across settings levels has no pattern matching {url}; Claude Code will skip bellwether's hooks"), BAD_CONFIG));
        }
    }
    if !found.is_empty() && env_defined {
        let absent: Vec<&str> = ENV_HEADERS.iter().map(|(_, v)| *v).filter(|v| !env_allow.iter().any(|x| x == v)).collect();
        if !absent.is_empty() {
            out.push(Check::new("httpHookAllowedEnvVars (merged)", "WARN", format!("the merged list doesn't allow {}; those terminal headers arrive empty, and sheep identity and click-to-focus degrade", absent.join(", ")), BAD_CONFIG));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::hook_block;

    const T: &str = "testTokenNotReal_abcdefghijklmnop";

    #[test]
    fn a_full_install_passes_and_on_failure_block_fails() {
        let block = hook_block(47417, T);
        let checks = check_settings(&block, 47417, Some(T), "s");
        assert!(checks.iter().all(|c| c.status == "PASS"), "{checks:?}");
        let mut text = lossless::stringify(&block, "");
        text = text.replacen("\"timeout\":1,", "\"timeout\":1,\"onFailure\":\"block\",", 1);
        let checks = check_settings(&lossless::parse(&text).unwrap(), 47417, Some(T), "s");
        let c = checks.iter().find(|c| c.name == "hook onFailure").expect("an onFailure check");
        assert_eq!((c.status, c.exit_code), ("FAIL", BAD_CONFIG));
        assert!(c.detail.contains("SessionEnd") && c.detail.contains("block every tool call"), "{}", c.detail);
    }

    /// Entries from before X-Bellwether-Entrypoint still work: a warning, not a failure.
    #[test]
    fn entries_missing_a_header_are_a_warning_to_reinstall() {
        let text = lossless::stringify(&hook_block(47417, T), "  ").replace("X-Bellwether-Entrypoint", "X-Some-Other-Header");
        assert!(!text.contains("X-Bellwether-Entrypoint"), "the header was taken out");
        let checks = check_settings(&lossless::parse(&text).unwrap(), 47417, Some(T), "s");
        let c = checks.iter().find(|c| c.name == "hook headers").expect("a hook headers check");
        assert_eq!((c.status, c.exit_code), ("WARN", OK));
        assert!(c.detail.contains("every event") && c.detail.contains("hooks install --write"), "{}", c.detail);
        assert!(checks.iter().all(|c| c.status != "FAIL"), "{checks:?}");
    }

    /// A settings file saved with a UTF-8 BOM (Notepad, W2) is read past it, not called
    /// invalid JSON.
    #[test]
    fn a_bom_is_read_past() {
        let f = std::env::temp_dir().join(format!("bw-bom-{}.json", std::process::id()));
        std::fs::write(&f, format!("\u{feff}{}", lossless::stringify(&hook_block(47417, T), "  ").replace('\n', "\r\n"))).unwrap();
        let checks = check_settings_file(&f, 47417, Some(T));
        assert!(checks.iter().filter(|c| c.name != "settings BOM").all(|c| c.status == "PASS"), "{checks:?}");
        let note = checks.iter().find(|c| c.name == "settings BOM").expect("the BOM is said");
        assert_eq!(note.status, "INFO", "said, never failed");
        assert!(note.detail.contains("without BOM"), "{}", note.detail);
        std::fs::write(&f, lossless::stringify(&hook_block(47417, T), "  ")).unwrap();
        assert!(!check_settings_file(&f, 47417, Some(T)).iter().any(|c| c.name == "settings BOM"), "no BOM, no line");
        let _ = std::fs::remove_file(&f);
    }

    /// Windows' registry policy values (W2) are managed levels like the file: read from the
    /// text given, named by the value, checked by the same rules.
    #[test]
    fn registry_policy_text_is_a_managed_level() {
        let name = "HKLM\\SOFTWARE\\Policies\\ClaudeCode\\Settings".to_string();
        let checks = check_other_levels_with(None, None, &[], &[(name.clone(), r#"{"allowManagedHooksOnly": true}"#.into())], 47417, None);
        let c = checks.iter().find(|c| c.name == "managed settings").expect("the managed check");
        assert_eq!(c.status, "FAIL");
        assert!(c.detail.starts_with(&name), "{}", c.detail);
        let info = checks.iter().find(|c| c.name == "other settings levels").unwrap();
        assert!(info.detail.contains(&format!("managed ({name})")), "{}", info.detail);
        let none = check_other_levels_with(None, None, &[], &[], 47417, None);
        assert!(none.is_empty(), "no levels, no lines: {none:?}");
    }

    #[test]
    fn globs() {
        assert!(glob_match("http://127.0.0.1:47417/hook", "http://127.0.0.1:47417/hook"));
        assert!(glob_match("http://127.0.0.1:*", "http://127.0.0.1:47417/hook"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "abc"));
        assert!(!glob_match("a*b*c", "acb"));
        assert!(!glob_match("http://127.0.0.1:47417", "http://127.0.0.1:47417/hook"));
        assert!(!glob_match("ab*ba", "aba"));
    }
}
