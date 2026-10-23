//! `bellwether migrate [--from <dir>] [--env <file>] [--settings <path>] [--force]
//! [--write]` (distribution plan, milestone 4): today's repository migration
//! (brain/migrate.mjs), ported, with the app's files as the target. A preview unless
//! `--write`; nothing is changed without it.
//!
//! What it moves, in this order, into the app's files (`BELLWETHER_HOME`):
//! 1. the hook token: the old `.env`'s BELLWETHER_HOOK_TOKEN (a renamed repository
//!    install) or PASTURE_HOOK_TOKEN (before the rename), else the token in the existing
//!    hook entries, written to `hook-token` with the same value, so running sessions keep
//!    working;
//! 2. config.json, if the app has none (keys the app no longer reads are dropped, and the
//!    preview says which);
//! 3. state.json (each session's colour and slot, and mute; through the same checks the
//!    brain applies on load) and shell-state.json (Hide everything, Show labels), if the app
//!    has none;
//! 4. the repository's LaunchAgents are found by what they run and listed, never touched:
//!    the app never writes LaunchAgents (plan D5), and moving them to the app's login items
//!    is the app migration (`--to-app`, crates/brain/src/toapp.rs);
//! 5. only then, the hook entries pasture wrote (`X-Pasture-*` headers), rewritten as
//!    bellwether's with the token (a backup first), and only once a brain on the port
//!    accepts that token under the current header name.
//!
//! The source is `--from` (the old repository, or its data directory), else the
//! WorkingDirectory of the old brain's LaunchAgent. The token is never printed. Nothing
//! here calls launchctl or signals a process; the brain check runs only when HOME is the
//! real home directory (a test's temp HOME never reaches a live brain).

use std::path::{Path, PathBuf};
use std::time::Duration;

use bellwether_core::hooks::{self as installer, Action};
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::lossless::{self, Lj};
use bellwether_core::{cliargs, config as cfg, flock, outfile, persist, token as tok};

use crate::cli::{config, out, remember_token, user_path};
use crate::{client, exit, home, BrainError, BIND_HOST, DEFAULT_PORT};

type CmdResult = Result<i32, BrainError>;

fn bad(m: impl Into<String>) -> BrainError {
    BrainError::new(m, exit::BAD_CONFIG)
}

/// Where the old install is: its repository directory (None for a data directory alone),
/// and its data directory.
#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub root: Option<PathBuf>,
    pub data_dir: PathBuf,
}

/// `sourceOf(from)`: a repository directory, or a data directory.
pub fn source_of(from: &Path) -> Result<Source, BrainError> {
    let dir = installer::resolve(from);
    if !dir.is_dir() {
        return Err(bad(format!("--from {}: no such directory. Give the old install's directory, or its data directory. Nothing was done.", from.display())));
    }
    let looks_like_data = ["state.json", "shell-state.json", "brain.log", "installs.json"].iter().any(|f| dir.join(f).exists());
    if looks_like_data {
        let root = (dir.file_name().is_some_and(|n| n == "data")).then(|| dir.parent().map(Path::to_path_buf)).flatten();
        return Ok(Source { root, data_dir: dir });
    }
    if dir.join("data").exists() || dir.join("brain").exists() {
        return Ok(Source { data_dir: dir.join("data"), root: Some(dir) });
    }
    Err(bad(format!("--from {} doesn't look like a pasture or bellwether repository install (no data/ or brain/ in it, and no state.json). Nothing was done.", from.display())))
}

/// A LaunchAgent plist's Label, ProgramArguments and WorkingDirectory (XML, or binary
/// through `plutil`, read only).
#[derive(Debug, Default)]
pub struct Plist {
    pub label: Option<String>,
    pub args: Option<Vec<String>>,
    pub working_directory: Option<String>,
}

pub fn read_plist(file: &Path) -> Plist {
    let Ok(bytes) = std::fs::read(file) else { return Plist::default() };
    let text = if bytes.starts_with(b"bplist") {
        if !cfg!(target_os = "macos") {
            return Plist::default();
        }
        match std::process::Command::new("/usr/bin/plutil").args(["-convert", "xml1", "-o", "-"]).arg(file).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => return Plist::default(),
        }
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let un = |v: &str| v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    let string_after = |key: &str| -> Option<String> {
        let at = text.find(&format!("<key>{key}</key>"))?;
        let rest = text[at + key.len() + 11..].trim_start();
        let rest = rest.strip_prefix("<string>")?;
        Some(un(&rest[..rest.find("</string>")?]))
    };
    let args = text.find("<key>ProgramArguments</key>").and_then(|at| {
        let rest = text[at + 27..].trim_start().strip_prefix("<array>")?;
        let body = &rest[..rest.find("</array>")?];
        Some(body.split("<string>").skip(1).filter_map(|s| s.find("</string>").map(|e| un(&s[..e]))).collect())
    });
    Plist { label: string_after("Label"), args, working_directory: string_after("WorkingDirectory") }
}

/// A LaunchAgent of the repository versions: its label, file, and what it runs.
#[derive(Debug, PartialEq, Eq)]
pub struct OldAgent {
    pub label: String,
    pub file: PathBuf,
    pub kind: &'static str,
}

/// What a ProgramArguments array runs: a repository brain, a repository strip, or neither.
fn program_kind(args: &[String]) -> Option<&'static str> {
    let cli = args.iter().find(|a| a.ends_with("/brain/cli.mjs") || a.ends_with("\\brain\\cli.mjs"));
    if cli.is_some() && args.iter().any(|a| a == "serve") {
        return Some("brain");
    }
    let first = args.first()?;
    (first.ends_with("/pasture-shell") || first.ends_with("/bellwether-shell")).then_some("shell")
}

/// The repository versions' LaunchAgents under `home`, found by what they run rather than
/// by a fixed label (an install inside a hub used its own label prefix): a Label (or file
/// name) ending in `.pasture.brain`/`.shell` or `local.bellwether.brain`/`.shell` whose
/// ProgramArguments, when it has any, run that kind of program; or any plist whose
/// ProgramArguments point into the `from` install and run a brain or a strip.
pub fn find_old_agents(user_home: &Path, from: Option<&Path>) -> Vec<OldAgent> {
    let dir = user_home.join("Library").join("LaunchAgents");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut names: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".plist")).collect();
    names.sort();
    let from_dir = from.map(|f| format!("{}/", installer::resolve(f).display()));
    let mut out = Vec::new();
    for n in names {
        let file = dir.join(&n);
        let p = read_plist(&file);
        let label = p.label.clone().unwrap_or_else(|| n.trim_end_matches(".plist").to_string());
        let runs = p.args.as_deref().and_then(program_kind);
        let in_from = from_dir.as_ref().is_some_and(|f| p.args.as_ref().is_some_and(|a| a.iter().any(|x| x.starts_with(f.as_str()))));
        let named = ["brain", "shell"].into_iter().find(|k| label.ends_with(&format!(".pasture.{k}")) || label == format!("local.bellwether.{k}"));
        let kind = if in_from && runs.is_some() {
            runs
        } else if named.is_some() && (p.args.is_none() || runs == named) {
            named
        } else {
            None
        };
        if let Some(kind) = kind {
            out.push(OldAgent { label, file, kind });
        }
    }
    out
}

/// D9's search for the old install when there is no `--from`: the WorkingDirectory of the
/// old brain's LaunchAgent (local.bellwether.brain, else local.pasture.brain), else the
/// repository its ProgramArguments run.
pub fn discover(user_home: &Path) -> Option<PathBuf> {
    for label in ["local.bellwether.brain", "local.pasture.brain"] {
        let f = user_home.join("Library").join("LaunchAgents").join(format!("{label}.plist"));
        if !f.exists() {
            continue;
        }
        let p = read_plist(&f);
        if let Some(w) = p.working_directory.filter(|w| !w.is_empty()) {
            return Some(PathBuf::from(w));
        }
        if let Some(cli) = p.args.as_ref().and_then(|a| a.iter().find(|x| x.ends_with("/brain/cli.mjs"))) {
            return Path::new(cli).parent().and_then(Path::parent).map(Path::to_path_buf);
        }
    }
    None
}

/// The .env an old install read: its own, or (at agents/<name> in a hub) the hub's.
pub(crate) fn legacy_env_file(root: &Path) -> PathBuf {
    let own = root.join(".env");
    if own.exists() {
        return own;
    }
    if root.parent().and_then(Path::file_name).is_some_and(|n| n == "agents") {
        if let Some(hub) = root.parent().and_then(Path::parent) {
            let f = hub.join(".env");
            if f.exists() {
                return f;
            }
        }
    }
    own
}

pub(crate) fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => installer::resolve(a) == installer::resolve(b),
    }
}

/// Entries pasture wrote on the port (exact URL, X-Pasture-Token, no X-Bellwether-Token).
pub(crate) fn count_legacy(file: &Path, port: u16) -> usize {
    let Ok(bytes) = std::fs::read(file) else { return 0 };
    let Ok(s) = lossless::parse(&String::from_utf8_lossy(&bytes)) else { return 0 };
    let mut n = 0;
    if let Some(hooks) = s.get("hooks").and_then(Lj::as_obj) {
        for (_, groups) in hooks.iter() {
            for g in groups.as_arr().map(Vec::as_slice).unwrap_or(&[]) {
                for h in g.get("hooks").and_then(Lj::as_arr).map(Vec::as_slice).unwrap_or(&[]) {
                    if installer::is_legacy_handler(h, port) {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

/// A usable token in the hook entries on the port (D9: hooks alone are enough to start).
pub(crate) fn token_in_hooks(file: &Path, port: u16) -> Option<String> {
    let s = lossless::parse(&String::from_utf8_lossy(&std::fs::read(file).ok()?)).ok()?;
    for (_, groups) in s.get("hooks")?.as_obj()?.iter() {
        for g in groups.as_arr().map(Vec::as_slice).unwrap_or(&[]) {
            for h in g.get("hooks").and_then(Lj::as_arr).map(Vec::as_slice).unwrap_or(&[]) {
                if installer::is_bellwether_handler(h, port) {
                    if let Some(t) = installer::handler_token(h).and_then(Lj::as_str) {
                        if tok::problem(t).is_none() {
                            return Some(t.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Whether HOME is the real home directory of this user (the passwd entry), so a live
/// brain may be asked. A test's temp HOME never is.
pub fn is_real_home(user_home: &Path) -> bool {
    #[cfg(unix)]
    {
        let pw = unsafe { libc::getpwuid(libc::getuid()) };
        if pw.is_null() {
            return false;
        }
        let dir = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) }.to_string_lossy().into_owned();
        matches!((std::fs::canonicalize(user_home), std::fs::canonicalize(dir)), (Ok(a), Ok(b)) if a == b)
    }
    #[cfg(not(unix))]
    {
        let _ = user_home;
        false
    }
}

/// Whether a brain on the port takes this token under the current header name (a doctor
/// ping: counted by the brain, never shown).
fn brain_takes(port: u16, token: &str) -> Result<(), String> {
    let body = format!("{{\"hook_event_name\":\"{}\"}}", crate::DOCTOR_PING);
    match client::request(port, "POST", "/hook", &[("Content-Type", "application/json"), ("X-Bellwether-Token", token)], Some(body.as_bytes()), Duration::from_secs(5)) {
        Ok(r) if r.status == 204 => Ok(()),
        Ok(r) if r.status == 403 => Err(format!("the brain on {BIND_HOST}:{port} refuses the token under X-Bellwether-Token (403): it is an older brain, or has another token")),
        Ok(r) => Err(format!("the brain on {BIND_HOST}:{port} answered {}", r.status)),
        Err(client::Failure::Refused) => Err(format!("no brain answers on {BIND_HOST}:{port}")),
        Err(f) => Err(format!("the brain on {BIND_HOST}:{port} didn't answer ({})", f.code())),
    }
}

/// The options `migrate` runs with.
pub struct Options {
    pub from: Option<PathBuf>,
    pub env_from: Option<PathBuf>,
    pub settings: PathBuf,
    pub write: bool,
    pub force: bool,
    /// The app's files (BELLWETHER_HOME).
    pub home: PathBuf,
    /// The user's home directory.
    pub user_home: PathBuf,
    /// Whether a brain on the port must take the token before the hook entries move: the
    /// CLI sets it when HOME is the real home (a test's temp HOME never reaches a live
    /// brain); the cargo tests set it to drive that path against their own brain.
    pub check_brain: bool,
}

pub struct Outcome {
    pub lines: Vec<String>,
    pub wrote: bool,
}

/// The migration. Returns its lines; an error stops it with the message.
pub fn migrate(o: &Options) -> Result<Outcome, BrainError> {
    let from = match &o.from {
        Some(f) => f.clone(),
        None => discover(&o.user_home).ok_or_else(|| bad(format!("no earlier install found: no local.bellwether.brain or local.pasture.brain LaunchAgent in {}. Pass --from <the old repository's directory>. Nothing was done.", o.user_home.join("Library").join("LaunchAgents").display())))?,
    };
    let src = source_of(&from)?;
    let home = &o.home;
    let mut lines = vec![
        format!("From: {}, data in {}", src.root.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "(a data directory only)".into()), src.data_dir.display()),
        format!("To:   the app's files in {}", home.display()),
        String::new(),
    ];
    let mut did: Vec<&str> = Vec::new();
    let step = |lines: &mut Vec<String>, n: u32, title: &str| lines.push(format!("{n}. {title}"));
    let say = |lines: &mut Vec<String>, l: String| lines.push(format!("   {l}"));
    let would = |done: &str, todo: &str| if o.write { done.to_string() } else { todo.to_string() };

    // The port the hooks are on: the app's config, else the old install's, else 47417.
    let old_config = src.root.as_ref().map(|r| r.join("config.json"));
    let new_config = home.join("config.json");
    let port = [Some(new_config.clone()), old_config.clone()]
        .into_iter()
        .flatten()
        .find(|f| f.exists())
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| json::parse(&t).ok())
        .and_then(|v| v.get("port").as_num())
        .filter(|p| p.fract() == 0.0 && (1024.0..=65535.0).contains(p))
        .map(|p| p as u16)
        .unwrap_or(DEFAULT_PORT);

    // Before anything is written: whether the settings file can be rewritten at the end.
    let legacy = count_legacy(&o.settings, port);
    if o.write && legacy > 0 {
        installer::plan_edit(&o.settings, Action::Install, port, "precheck", None)?;
        for f in [o.settings.clone(), o.settings.parent().map(Path::to_path_buf).unwrap_or_default()] {
            if let Err(why) = writable(&f) {
                return Err(bad(format!("{} can't be written ({why}), so the hook entries couldn't be moved at the end. Nothing was changed. Fix the permission, then run this again.", f.display())));
            }
        }
    }

    // 1. The token.
    step(&mut lines, 1, "the hook token");
    let target = tok::file(home);
    let old_env = o.env_from.clone().or_else(|| src.root.as_deref().map(legacy_env_file));
    let old_token = match &old_env {
        Some(f) if f.exists() => tok::from_env_file(f)?,
        _ => None,
    };
    let have = tok::read_file(home)?.filter(|t| tok::problem(t).is_none());
    let mut token = have.clone();
    if let Some(have) = &have {
        let differs = old_token.as_ref().is_some_and(|t| t != have);
        say(&mut lines, format!("a hook token is already set in {}; left alone{}.", target.display(), if differs { ". It differs from the old token, so the hook entries are rewritten with it (step 5)" } else { "" }));
    } else if let Some(t) = &old_token {
        if let Some(why) = tok::problem(t) {
            say(&mut lines, format!("the token in {} isn't usable ({why}); run `bellwether token init` for a new one", old_env.as_ref().unwrap().display()));
        } else {
            token = Some(t.clone());
            say(&mut lines, format!("{} the token from {} into {} (same value, not shown; {} isn't changed)", would("copied", "would copy"), old_env.as_ref().unwrap().display(), target.display(), old_env.as_ref().unwrap().display()));
            if o.write {
                tok::write(home, t)?;
                did.push("token");
            }
        }
    } else if let Some(t) = token_in_hooks(&o.settings, port) {
        token = Some(t.clone());
        say(&mut lines, format!("{} the token of the hook entries in {} into {} (same value, not shown)", would("copied", "would copy"), o.settings.display(), target.display()));
        if o.write {
            tok::write(home, &t)?;
            did.push("token");
        }
    } else {
        say(&mut lines, format!("no BELLWETHER_HOOK_TOKEN or PASTURE_HOOK_TOKEN found{} (pass --env <file> to name the old .env), and none in the hook entries; run `bellwether token init` for a new one", old_env.as_ref().map(|f| format!(" in {}", f.display())).unwrap_or_default()));
    }

    // 2. config.json.
    step(&mut lines, 2, "config.json");
    match &old_config {
        Some(old) if old.exists() && !same(old, &new_config) => {
            if new_config.exists() && !o.force {
                say(&mut lines, format!("{} exists; kept (--force replaces it with {})", new_config.display(), old.display()));
            } else {
                let text = std::fs::read(old).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| bad(format!("can't read {} ({e})", old.display())))?;
                let (text, dropped) = drop_obsolete(&text);
                say(&mut lines, format!("{} {} to {}{}", would("copied", "would copy"), old.display(), new_config.display(), if dropped.is_empty() { String::new() } else { format!(", without {} (the app doesn't read {})", dropped.join(" and "), if dropped.len() == 1 { "it" } else { "them" }) }));
                if o.write {
                    home::create_private_dir(home).map_err(|e| BrainError::new(format!("can't create {} ({e})", home.display()), exit::UNEXPECTED))?;
                    persist::write_atomic(&new_config, &home.join(format!(".config.json.{}.tmp", std::process::id())), &text, 0o600).map_err(|e| BrainError::new(format!("can't write {} ({e})", new_config.display()), exit::UNEXPECTED))?;
                    did.push("config");
                }
            }
        }
        _ => say(&mut lines, "nothing to copy".into()),
    }

    // 3. Colours, slots, mute; the tray's settings.
    step(&mut lines, 3, "colours and slots (state.json), and the tray settings (shell-state.json)");
    if same(&src.data_dir, home) {
        say(&mut lines, "the data directory is the same; nothing to copy".into());
    } else {
        for name in ["state.json", "shell-state.json"] {
            let a = src.data_dir.join(name);
            let b = home.join(name);
            if !a.exists() {
                say(&mut lines, format!("{name}: none in the old data directory"));
                continue;
            }
            if b.exists() && !o.force {
                say(&mut lines, format!("{name}: {} exists; kept (--force replaces it)", b.display()));
                continue;
            }
            if name == "state.json" {
                // Through the same checks the brain applies on load, so a bad file can't get in.
                let (clean, warnings) = clean_state(&a);
                for w in warnings {
                    say(&mut lines, format!("warning: {w}"));
                }
                let Some(clean) = clean else { continue };
                let count = clean.get("identities").as_obj().map(|o| o.len()).unwrap_or(0);
                say(&mut lines, format!("{name}: {} {count} session colour(s) and slot(s){}", would("copied", "would copy"), if clean.get("muted").truthy() { ", and mute" } else { "" }));
                if o.write {
                    persist::save_state(&b, &clean).map_err(|e| BrainError::new(format!("can't write {} ({e})", b.display()), exit::UNEXPECTED))?;
                    did.push("state");
                }
            } else {
                let v = std::fs::read_to_string(&a).ok().and_then(|t| json::parse(&t).ok());
                let Some(Js::Obj(v)) = v else {
                    say(&mut lines, format!("{name}: not valid JSON; skipped"));
                    continue;
                };
                // The strips write `showLabels` (privacy.rs, brain/privacy.mjs); `labels` is read
                // too, in case a file ever had it. Written by the strip's own writer, as Node now
                // writes it with privacy.mjs (the maintainer's decision on deviation 82).
                let on = |k: &str| matches!(v.get(k), Some(Js::Bool(true)));
                let (hidden, show_labels) = (on("hidden"), on("showLabels") || on("labels"));
                let text = json::stringify(&Js::Obj(Obj::new().with("hidden", hidden).with("showLabels", show_labels)));
                say(&mut lines, format!("{name}: {} {text}", would("copied", "would copy")));
                if o.write {
                    bellwether_core::shellstate::write(home, hidden, show_labels).map_err(|e| BrainError::new(format!("can't write {} ({e})", b.display()), exit::UNEXPECTED))?;
                    did.push("shell-state");
                }
            }
        }
    }

    // 4. LaunchAgents: found and listed, never touched (plan D5; the swap is `--to-app`).
    step(&mut lines, 4, "start at login (LaunchAgents)");
    let agents_dir = o.user_home.join("Library").join("LaunchAgents");
    let found = find_old_agents(&o.user_home, src.root.as_deref());
    if found.is_empty() {
        say(&mut lines, format!("no repository LaunchAgents in {}", agents_dir.display()));
    } else {
        for a in &found {
            say(&mut lines, format!("the repository's LaunchAgent {} ({}, {}) is left as it is", a.label, a.kind, a.file.display()));
        }
        say(&mut lines, "the app doesn't write or remove LaunchAgents: moving them to the app's login items is `bellwether migrate --to-app`. Until then the old brain keeps its port".into());
    }

    // 5. Hook entries, last: only once a brain that reads them is answering.
    step(&mut lines, 5, &format!("hook entries in {}", o.settings.display()));
    let entries = |n: usize| format!("{n} pasture entr{}", if n == 1 { "y" } else { "ies" });
    if legacy == 0 {
        say(&mut lines, format!("no pasture entries (on port {port}); nothing to replace"));
    } else if token.is_none() {
        say(&mut lines, format!("{} found, but there is no token to write into the new ones: set one (step 1), then run this again", entries(legacy)));
    } else if !o.write {
        say(&mut lines, format!("would replace {} with bellwether's, once a brain on port {port} accepts the token under the new header (`bellwether hooks install` shows the full diff)", entries(legacy)));
    } else {
        let token = token.unwrap();
        let checked = if o.check_brain { brain_takes(port, &token) } else { Ok(()) };
        match checked {
            Err(detail) => say(&mut lines, format!("left as they are: {detail}. Start a brain that reads them (`bellwether serve`, or the app), then run this again")),
            Ok(()) => {
                let mut plan = installer::plan_edit(&o.settings, Action::Install, port, &token, Some(home))?;
                if plan.changed {
                    let backup = installer::apply_plan(&mut plan, &home.join("backups"), Some(home), &crate::cli::file_stamp(crate::now_ms()))?;
                    say(&mut lines, format!("replaced {} with bellwether's", entries(legacy)));
                    if let Some(b) = backup {
                        say(&mut lines, format!("backup of the previous file: {}", b.display()));
                    }
                    for w in &plan.warnings {
                        say(&mut lines, format!("warning: {w}"));
                    }
                    did.push("hooks");
                }
            }
        }
    }

    lines.push(String::new());
    lines.push(if o.write { "Done. `bellwether doctor` checks the result.".into() } else { "Preview only; nothing was changed. Re-run with --write to apply.".into() });
    Ok(Outcome { lines, wrote: o.write && !did.is_empty() })
}

/// An old state.json through the checks the brain applies on load (each session's colour,
/// slots and label, and mute): None when there is nothing usable in it; and the warnings.
pub(crate) fn clean_state(file: &Path) -> (Option<Js>, Vec<String>) {
    let (persisted, warnings) = persist::load_state(file);
    if matches!(persisted, Js::Null) {
        return (None, warnings);
    }
    let merged = cfg::Config::defaults().merged;
    (Some(flock::Flock::new(flock::FlockConfig::from_merged(&merged), &persisted, crate::now_ms() as f64).persistable()), warnings)
}

/// The config without the keys the app no longer reads (`brain.nodePath`, `autostart`),
/// every other line as it was (`lossless::edit`, M13); and which were dropped.
pub(crate) fn drop_obsolete(text: &str) -> (String, Vec<&'static str>) {
    let Ok(mut v) = lossless::parse(text) else { return (text.to_string(), Vec::new()) };
    let mut dropped = Vec::new();
    if let Some(o) = v.as_obj_mut() {
        if o.remove("autostart").is_some() {
            dropped.push("autostart");
        }
        if let Some(brain) = o.get_mut("brain").and_then(Lj::as_obj_mut) {
            if brain.remove("nodePath").is_some() {
                dropped.push("brain.nodePath");
            }
        }
    }
    if dropped.is_empty() {
        return (text.to_string(), dropped);
    }
    (lossless::edit(text, &v), dropped)
}

/// `fs.accessSync(f, W_OK)`, by asking the file system.
pub(crate) fn writable(f: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(f.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        if unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        Err(cfg::io_code(&e))
    }
    #[cfg(not(unix))]
    {
        match std::fs::metadata(f) {
            Ok(m) if m.permissions().readonly() => Err("EACCES".into()),
            Ok(_) => Ok(()),
            Err(e) => Err(cfg::io_code(&e)),
        }
    }
}

/// The CLI command.
pub fn command(args: &[String]) -> CmdResult {
    let a = cliargs::parse(args, &["write", "force"], &["from", "env", "settings"], 0)?;
    let home = home::dir();
    remember_token(&home);
    // A bad config is the user's to fix before anything moves.
    let _ = config(&home)?;
    let user_home = bellwether_core::paths::user_home().unwrap_or_default();
    let o = Options {
        from: a.value("from").map(user_path),
        env_from: a.value("env").map(user_path),
        settings: a.value("settings").map(user_path).unwrap_or_else(outfile::user_settings),
        write: a.has("write"),
        force: a.has("force"),
        home,
        check_brain: is_real_home(&user_home),
        user_home,
    };
    let r = migrate(&o)?;
    out(&(r.lines.join("\n") + "\n"));
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bw-migrate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sources_and_obsolete_keys() {
        let d = temp("src");
        std::fs::create_dir_all(d.join("old").join("data")).unwrap();
        std::fs::write(d.join("old").join("data").join("state.json"), "{}").unwrap();
        assert_eq!(source_of(&d.join("old")).unwrap(), Source { root: Some(d.join("old")), data_dir: d.join("old").join("data") });
        assert_eq!(source_of(&d.join("old").join("data")).unwrap(), Source { root: Some(d.join("old")), data_dir: d.join("old").join("data") });
        assert!(source_of(&d.join("nope")).unwrap_err().message.contains("no such directory"));
        assert!(source_of(&d).unwrap_err().message.contains("doesn't look like"));
        let (text, dropped) = drop_obsolete("{\n  \"port\": 47417,\n  \"brain\": {\n    \"nodePath\": \"/x/node\"\n  },\n  \"autostart\": true\n}\n");
        assert_eq!(dropped, ["autostart", "brain.nodePath"]);
        assert_eq!(text, "{\n  \"port\": 47417,\n  \"brain\": {}\n}\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_agents_are_found_by_what_they_run() {
        let h = temp("agents");
        let dir = h.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&dir).unwrap();
        let plist = |label: &str, args: &[&str]| format!("<plist><dict><key>Label</key>\n<string>{label}</string><key>ProgramArguments</key><array>{}</array><key>WorkingDirectory</key><string>/r/bellwether</string></dict></plist>", args.iter().map(|a| format!("<string>{a}</string>")).collect::<String>());
        std::fs::write(dir.join("local.example-hub.pasture.brain.plist"), plist("local.example-hub.pasture.brain", &["/n/node", "/h/agents/pasture/brain/cli.mjs", "serve"])).unwrap();
        std::fs::write(dir.join("local.bellwether.shell.plist"), plist("local.bellwether.shell", &["/r/bellwether/shell/src-tauri/target/release/bellwether-shell", "--no-spawn-brain"])).unwrap();
        std::fs::write(dir.join("com.someone.pasture.brain.plist"), plist("com.someone.pasture.brain", &["/usr/local/bin/other"])).unwrap();
        std::fs::write(dir.join("local.bellwether.brain.plist"), plist("local.bellwether.brain", &["/n/node", "/r/bellwether/brain/cli.mjs", "serve"])).unwrap();
        let found: Vec<(String, &str)> = find_old_agents(&h, None).into_iter().map(|a| (a.label, a.kind)).collect();
        assert_eq!(found, [("local.bellwether.brain".to_string(), "brain"), ("local.bellwether.shell".to_string(), "shell"), ("local.example-hub.pasture.brain".to_string(), "brain")]);
        assert_eq!(discover(&h), Some(PathBuf::from("/r/bellwether")));
        let _ = std::fs::remove_dir_all(&h);
    }
}
