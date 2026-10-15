//! `bellwether migrate --to-app [--from <dir>] [--env <file>] [--settings <path>] [--force]
//! [--write]` and `bellwether migrate --undo [--write]` (distribution plan D9, milestone 8):
//! an earlier install, the renamed repository version (`local.bellwether.*` LaunchAgents,
//! `.env` with BELLWETHER_HOOK_TOKEN, `X-Bellwether-Token` hooks) or the pre-rename one
//! (`local.pasture.*`, PASTURE_HOOK_TOKEN, `X-Pasture-*`), or hook entries alone, moved
//! into the app in one step. A preview unless `--write`; nothing changes without it.
//!
//! The order on `--write` is D9's, so the hooks always have a brain:
//! 1. the token (from the old `.env`, else the hook entries) into `hook-token`, same value;
//! 2. the data, each item only if the app has none yet (`--force` replaces): config.json
//!    without the obsolete keys, state.json (through the brain's checks), shell-state.json,
//!    installs.json, backups/ (0600), and fetched pets as imported pets;
//! 3. the port: free, or held by the old brain; anything else stops the run, saying what
//!    holds it (also checked before step 1, so such a run changes nothing at all);
//! 4. the swap: boot out the old agents, stop a strip `shell start` left (by its exact pid,
//!    from `shell.pid`, only while that pid still runs that checkout's strip), move the
//!    plists to `<home>/legacy/`, register the app's login items, and wait up to 10 s for a
//!    new brain to answer `/health` and take the token on `/state`. If it doesn't, all of
//!    step 4 is undone (unregister, plists back, old agents bootstrapped) and the run stops;
//! 5. the hook entries rewritten to the current header names (a backup first);
//! 6. a word on the old strip's Accessibility entry;
//! 7. doctor.
//!
//! The repository itself is never changed. `<home>/legacy/migration.json` records what
//! steps 4 and 5 did, for `--undo`: unregister, plists back and bootstrapped (those that
//! were loaded), and the settings file restored from step 5's backup, only if it is still
//! byte for byte what step 5 wrote. Application Support stays.
//!
//! Everything launchd, SMAppService and processes is behind [`Host`]: the app's own (the
//! shell crate, `migration.rs`) acts only for the real HOME; under any other HOME it acts
//! only through test stubs, and the cargo tests here use a recorder. The token is never
//! printed (the hook diff masks it).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bellwether_core::hooks::{self as installer, Action};
use bellwether_core::json::{self, Js};
use bellwether_core::{persist, token as tok};
use serde_json::{json, Value};

use crate::legacy::{self, Loaded};
use crate::migrate::{self, Source};
use crate::{client, exit, home, BrainError, BIND_HOST, DEFAULT_PORT};

/// How long step 4 waits for the new brain (D9).
pub const VERIFY_LIMIT: Duration = Duration::from_secs(10);

/// Who holds the brain's port (read only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    Free,
    /// A bellwether or pasture brain, by its `/health`.
    Brain { name: String, pid: Option<i32> },
    /// Something else: the listening pid and its command line, when they could be read.
    Other { pid: Option<i32>, command: Option<String> },
}

/// What the migration asks of the machine. The app's implementation is in the shell crate
/// (`migration.rs`); tests use a recorder.
pub trait Host {
    /// Whether launchd and the login items are acted on. When false (a temp HOME without
    /// test stubs) the files still move, but launchctl and SMAppService aren't called and
    /// no new brain is waited for.
    fn acts(&self) -> bool;
    /// Why not, in a few words, when [`Host::acts`] is false.
    fn why_not(&self) -> String {
        "HOME is not your real home directory (a test)".into()
    }
    /// `gui/<uid>`.
    fn domain(&self) -> String;
    /// What launchd says about a label in this user's GUI domain (read only).
    fn loaded(&self, label: &str) -> Loaded;
    /// The pid of this app's own brain agent, when launchd runs it.
    fn app_agent(&self) -> Option<i32>;
    /// `launchctl bootout …` / `bootstrap …`.
    fn launchctl(&mut self, args: &[String]) -> Result<(), String>;
    /// Registers the brain agent and the app at login; the lines to show. An item left
    /// waiting for approval is an error: its brain won't start until the user acts.
    fn register(&mut self) -> Result<Vec<String>, String>;
    fn unregister(&mut self) -> Result<Vec<String>, String>;
    /// Whether this app's login items are both registered (enabled); None when that can't
    /// be told (no stand-in answers, or SMAppService is unavailable).
    fn registered(&self) -> Option<bool> {
        None
    }
    /// Who holds the port.
    fn port_holder(&self, port: u16) -> Holder;
    /// The executable a pid runs (read only).
    fn process_path(&self, pid: i32) -> Option<PathBuf>;
    /// SIGTERM to exactly this pid, then a short wait; whether it is gone.
    fn terminate(&mut self, pid: i32) -> bool;
    /// `bellwether doctor`'s lines for the app's files.
    fn doctor(&self, home: &Path, settings: &Path, port: u16) -> Vec<String>;
    /// How long step 4 waits for the new brain.
    fn verify_limit(&self) -> Duration {
        VERIFY_LIMIT
    }
}

/// One of the old LaunchAgents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldAgent {
    pub label: String,
    /// "brain" or "shell".
    pub kind: &'static str,
    /// `~/Library/LaunchAgents/<label>.plist`, when it is there.
    pub plist: Option<PathBuf>,
    pub loaded: Loaded,
}

impl OldAgent {
    fn pid(&self) -> Option<i32> {
        match self.loaded {
            Loaded::Running(p) => Some(p),
            _ => None,
        }
    }
}

/// The old agents: the four labels of D5 (a plist, or loaded in launchd), and any other
/// LaunchAgent that runs a repository brain or strip (a hub's own label prefix).
pub fn old_agents(user_home: &Path, root: Option<&Path>, host: &dyn Host) -> Vec<OldAgent> {
    let mut out: Vec<OldAgent> = legacy::scan(user_home, &|l| host.loaded(l))
        .into_iter()
        .map(|a| OldAgent { label: a.label.to_string(), kind: if a.label.ends_with(".brain") { "brain" } else { "shell" }, plist: a.plist, loaded: a.loaded })
        .collect();
    for a in migrate::find_old_agents(user_home, root) {
        if !out.iter().any(|o| o.label == a.label) {
            let loaded = host.loaded(&a.label);
            out.push(OldAgent { label: a.label, kind: a.kind, plist: Some(a.file), loaded });
        }
    }
    out
}

/// The old install's repository, from the first brain agent's plist (D9: its
/// WorkingDirectory, else the checkout its ProgramArguments run).
fn discover(agents: &[OldAgent], moved_brains: &[PathBuf]) -> Option<PathBuf> {
    // A brain plist already moved to legacy/ by a run that stopped part-way counts too, so the
    // rerun still finds the repository (and its token and port).
    let files: Vec<&PathBuf> = agents.iter().filter(|a| a.kind == "brain").filter_map(|a| a.plist.as_ref()).chain(moved_brains.iter()).collect();
    files.into_iter().find_map(|f| {
        let p = migrate::read_plist(f);
        if let Some(w) = p.working_directory.filter(|w| !w.is_empty()) {
            return Some(PathBuf::from(w));
        }
        let cli = p.args.as_ref()?.iter().find(|x| x.ends_with("/brain/cli.mjs"))?.clone();
        Path::new(&cli).parent().and_then(Path::parent).map(Path::to_path_buf)
    })
}

/// The options both commands run with.
pub struct Options {
    pub from: Option<PathBuf>,
    pub env_from: Option<PathBuf>,
    pub settings: PathBuf,
    pub write: bool,
    pub force: bool,
    /// The app's files (BELLWETHER_HOME, or Application Support).
    pub home: PathBuf,
    /// The user's home directory.
    pub user_home: PathBuf,
}

/// Where the token comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenFrom {
    /// The app has a usable one already.
    Kept,
    EnvFile(PathBuf),
    Hooks,
}

/// One item of step 2.
#[derive(Clone, Debug)]
pub enum Item {
    Config { from: PathBuf, text: String, dropped: Vec<&'static str> },
    State { from: PathBuf, clean: Js, count: usize, muted: bool },
    ShellState { from: PathBuf, hidden: bool, show_labels: bool },
    Installs { from: PathBuf, text: String },
    Backup { from: PathBuf, name: String },
    Pet { from: PathBuf, name: String },
}

impl Item {
    fn target(&self, home: &Path) -> PathBuf {
        match self {
            Item::Config { .. } => home.join("config.json"),
            Item::State { .. } => home.join("state.json"),
            Item::ShellState { .. } => home.join("shell-state.json"),
            Item::Installs { .. } => home.join("installs.json"),
            Item::Backup { name, .. } => home.join("backups").join(name),
            Item::Pet { name, .. } => crate::pets::pet_file(home, name),
        }
    }

    fn describe(&self) -> String {
        match self {
            Item::Config { from, dropped, .. } => format!("config.json from {}{}", from.display(), if dropped.is_empty() { String::new() } else { format!(", without {} (the app doesn't read {})", dropped.join(" and "), if dropped.len() == 1 { "it" } else { "them" }) }),
            Item::State { count, muted, .. } => format!("state.json: {count} session colour(s) and slot(s){}", if *muted { ", and mute" } else { "" }),
            Item::ShellState { hidden, show_labels, .. } => format!("shell-state.json: Hide everything {}, Show labels {}", if *hidden { "on" } else { "off" }, if *show_labels { "on" } else { "off" }),
            Item::Installs { .. } => "installs.json (which events the hook install created)".into(),
            Item::Backup { name, .. } => format!("backups/{name} (0600)"),
            Item::Pet { name, .. } => format!("the pet {name}, as an imported pet (checked and pinned as `pets import` does)"),
        }
    }
}

/// Everything the migration would do, worked out without changing anything.
pub struct Survey {
    pub agents: Vec<OldAgent>,
    pub source: Option<Source>,
    /// Said about the source: a repository that is gone, or none found.
    pub source_note: Option<String>,
    pub port: u16,
    pub token: Option<(String, TokenFrom)>,
    /// The app has a token of its own, different from the one running sessions carry: step 1
    /// replaces it (keeping a copy in `legacy/`).
    pub token_replaces: bool,
    /// Step 2: to copy, and kept (the app has one already).
    pub copy: Vec<Item>,
    pub kept: Vec<String>,
    pub warnings: Vec<String>,
    /// Step 5: the planned edit, when there are Bellwether entries on the port.
    pub hooks: Option<installer::Plan>,
    pub legacy_entries: usize,
    /// A strip `shell start` left running: its pid.
    pub stray_strip: Option<i32>,
    /// A migration of this home that didn't finish (killed during step 4, or the app's login
    /// items or brain gone since): why. A run finishes it (D9).
    pub unfinished: Option<String>,
}

impl Survey {
    /// D9's "nothing to migrate": no old agents, no token to write, no data to copy, and the
    /// hook entries already current.
    pub fn nothing(&self) -> bool {
        self.agents.is_empty() && matches!(self.token, None | Some((_, TokenFrom::Kept))) && !self.token_replaces && self.copy.is_empty() && !self.hooks.as_ref().is_some_and(|p| p.changed) && self.unfinished.is_none()
    }

    /// Whether anything at all of an earlier install was found (the setup window's step 1).
    pub fn found(&self) -> bool {
        !self.nothing()
    }
}

fn bad(m: impl Into<String>) -> BrainError {
    BrainError::new(m, exit::BAD_CONFIG)
}

fn io(what: &str, p: &Path, e: impl std::fmt::Display) -> BrainError {
    BrainError::new(format!("can't {what} {} ({e})", p.display()), exit::UNEXPECTED)
}

/// Works out the migration (read only).
pub fn survey(o: &Options, host: &dyn Host) -> Result<Survey, BrainError> {
    let mut warnings = Vec::new();
    let pre = old_agents(&o.user_home, None, host);
    let (source, source_note) = match &o.from {
        Some(f) => (Some(migrate::source_of(f)?), None),
        None => match discover(&pre, &moved_plists(o).into_iter().filter(|m| m.label.ends_with(".brain")).map(|m| m.to).collect::<Vec<_>>()) {
            Some(root) if root.is_dir() => match migrate::source_of(&root) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e.message)),
            },
            Some(root) => (None, Some(format!("the old agent's repository, {}, is gone: its colours and settings can't be copied (pass --from to name another copy)", root.display()))),
            None => (None, None),
        },
    };
    let root = source.as_ref().and_then(|s| s.root.clone());
    // With the repository known, agents of a hub's own label that run it count too.
    let agents = if root.is_some() { old_agents(&o.user_home, root.as_deref(), host) } else { pre };

    // The port the hooks are on: the app's config, else the old install's, else 47417.
    let old_config = root.as_ref().map(|r| r.join("config.json"));
    let port = [Some(o.home.join("config.json")), old_config.clone()]
        .into_iter()
        .flatten()
        .find(|f| f.exists())
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| json::parse(&t).ok())
        .and_then(|v| v.get("port").as_num())
        .filter(|p| p.fract() == 0.0 && (1024.0..=65535.0).contains(p))
        .map(|p| p as u16)
        .unwrap_or(DEFAULT_PORT);

    // 1. The token: the one running sessions carry (the old `.env`'s, which the old brain
    //    takes, else the hook entries'). An app token that differs is replaced by it, or
    //    those sessions would be refused (D9 step 1); one that is already the same is kept.
    let env = o.env_from.clone().or_else(|| root.as_deref().map(migrate::legacy_env_file)).filter(|f| f.exists());
    let from_env = match &env {
        Some(f) => tok::from_env_file(f)?,
        None => None,
    };
    let old = match (from_env, &env) {
        (Some(t), Some(f)) if tok::problem(&t).is_none() => Some((t, TokenFrom::EnvFile(f.clone()))),
        (Some(t), Some(f)) => {
            warnings.push(format!("the token in {} can't be used ({})", f.display(), tok::problem(&t).unwrap_or("")));
            migrate::token_in_hooks(&o.settings, port).map(|t| (t, TokenFrom::Hooks))
        }
        _ => migrate::token_in_hooks(&o.settings, port).map(|t| (t, TokenFrom::Hooks)),
    };
    let have = tok::read_file(&o.home)?;
    let mut token_replaces = false;
    let token = match (have, old) {
        (Some(a), Some((t, _))) if a == t => Some((a, TokenFrom::Kept)),
        (Some(_), Some(old)) => {
            token_replaces = true;
            Some(old)
        }
        (Some(a), None) if tok::problem(&a).is_none() => Some((a, TokenFrom::Kept)),
        (Some(_), None) => return Err(bad(format!("{} holds a token that can't be used; fix it first (`bellwether token init --force`). Nothing was changed.", tok::file(&o.home).display()))),
        (None, old) => old,
    };

    // 2. The data.
    let mut copy = Vec::new();
    let mut kept = Vec::new();
    let mut add = |item: Item, kept: &mut Vec<String>| {
        if item.target(&o.home).exists() && !o.force {
            kept.push(format!("{}: the app has one ({}); kept (--force replaces it)", item.describe(), item.target(&o.home).display()));
        } else {
            copy.push(item);
        }
    };
    if let Some(src) = &source {
        if let Some(old) = old_config.as_ref().filter(|f| f.exists() && !migrate::same(f, &o.home.join("config.json"))) {
            let text = std::fs::read(old).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| io("read", old, e))?;
            let (text, dropped) = migrate::drop_obsolete(&text);
            add(Item::Config { from: old.clone(), text, dropped }, &mut kept);
        }
        if !migrate::same(&src.data_dir, &o.home) {
            let d = &src.data_dir;
            if d.join("state.json").exists() {
                let (clean, w) = migrate::clean_state(&d.join("state.json"));
                warnings.extend(w);
                if let Some(clean) = clean {
                    let count = clean.get("identities").as_obj().map(|o| o.len()).unwrap_or(0);
                    let muted = clean.get("muted").truthy();
                    add(Item::State { from: d.join("state.json"), clean, count, muted }, &mut kept);
                }
            }
            if let Ok(t) = std::fs::read_to_string(d.join("shell-state.json")) {
                match json::parse(&t) {
                    Ok(Js::Obj(v)) => {
                        // The strips write `showLabels` (brain/privacy.mjs, privacy.rs); an older
                        // file may say `labels`.
                        let on = |k: &str| matches!(v.get(k), Some(Js::Bool(true)));
                        add(Item::ShellState { from: d.join("shell-state.json"), hidden: on("hidden"), show_labels: on("showLabels") || on("labels") }, &mut kept);
                    }
                    _ => warnings.push(format!("{}: not valid JSON; not copied", d.join("shell-state.json").display())),
                }
            }
            if let Ok(t) = std::fs::read_to_string(d.join("installs.json")) {
                match json::parse(&t) {
                    Ok(Js::Obj(_)) => add(Item::Installs { from: d.join("installs.json"), text: t }, &mut kept),
                    _ => warnings.push(format!("{}: not valid JSON; not copied", d.join("installs.json").display())),
                }
            }
            let mut backups: Vec<String> = std::fs::read_dir(d.join("backups"))
                .map(|r| r.filter_map(|e| e.ok()).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect())
                .unwrap_or_default();
            backups.sort();
            for name in backups {
                add(Item::Backup { from: d.join("backups").join(&name), name }, &mut kept);
            }
            let mut pets: Vec<String> = std::fs::read_dir(d.join("pets")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| crate::pets::valid_name(n)).collect()).unwrap_or_default();
            pets.sort();
            for name in pets {
                let file = d.join("pets").join(&name).join("animations.xml");
                if file.is_file() {
                    add(Item::Pet { from: file, name }, &mut kept);
                }
            }
        }
    }

    // 5. The hook entries: only when there are Bellwether entries on the port to rewrite.
    let legacy_entries = migrate::count_legacy(&o.settings, port);
    let ours = has_entries(&o.settings, port);
    let hooks = match (&token, ours) {
        (Some((t, _)), true) => Some(installer::plan_edit(&o.settings, Action::Install, port, t, Some(&o.home))?),
        _ => None,
    };

    let stray_strip = source.as_ref().and_then(|s| stray_strip(s, host));
    let unfinished = unfinished(o, port, token.as_ref().map(|(t, _)| t.as_str()), host);
    let s = Survey { agents, source, source_note, port, token, token_replaces, copy, kept, warnings, hooks, legacy_entries, stray_strip, unfinished };
    if !s.nothing() && s.token.is_none() {
        return Err(bad(format!(
            "an earlier install was found, but no hook token: none in {} (BELLWETHER_HOOK_TOKEN or PASTURE_HOOK_TOKEN), and none in the hook entries of {}. Pass --env <the old .env>, or run `bellwether token init` for a new one (then `bellwether hooks install --write`). Nothing was changed.",
            s.source.as_ref().and_then(|x| x.root.as_ref()).map(|r| migrate::legacy_env_file(r).display().to_string()).unwrap_or_else(|| "the old .env (no repository found)".into()),
            o.settings.display()
        )));
    }
    Ok(s)
}

/// Whether an earlier migration of this home is unfinished, and why: its record says step 4
/// started and never ended (the run was killed), or, for a finished one, the app's login
/// items are no longer registered or no brain takes the token on the port.
fn unfinished(o: &Options, port: u16, token: Option<&str>, host: &dyn Host) -> Option<String> {
    if let Some(m) = stranded(o).first() {
        return Some(format!("{} is in {} but not in the migration's record (a run stopped while moving it)", m.label, legacy_dir(&o.home).display()));
    }
    let r: Value = serde_json::from_str(&std::fs::read_to_string(record_file(&o.home)).ok()?).ok()?;
    if r["swap"] == "started" {
        return Some("it stopped during step 4, before the app's brain was confirmed".into());
    }
    if r["swap"] != "done" || !host.acts() {
        return None;
    }
    if host.registered() == Some(false) {
        return Some("the app's login items aren't registered".into());
    }
    let answers = token.is_some_and(|t| matches!(client::request(port, "GET", "/state?wait=0", &[("X-Bellwether-Token", t)], None, Duration::from_millis(800)), Ok(r) if r.status == 200));
    (!answers).then(|| format!("no brain takes the hook token on port {port}"))
}

/// Whether a process is alive (signal 0; a process of another user counts as alive).
fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
    }
    #[cfg(not(unix))]
    true
}

/// Plists of known labels sitting in `<home>/legacy/` (`<label>.plist`, a file or a link)
/// that the record doesn't list: moved by a run that stopped before recording them. Each is
/// treated as one the migration moved (from `~/Library/LaunchAgents`, and loaded, as an agent
/// whose plist was there loads at login).
fn stranded(o: &Options) -> Vec<Moved> {
    let recorded: Vec<String> = std::fs::read_to_string(record_file(&o.home))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|r| r["plists"].as_array().map(|a| a.iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    let dir = legacy_dir(&o.home);
    let mut names: Vec<String> = std::fs::read_dir(&dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            let label = n.strip_suffix(".plist")?.to_string();
            let meta = std::fs::symlink_metadata(dir.join(&n)).ok()?;
            (known_label(&label) && !recorded.contains(&label) && (meta.is_file() || meta.file_type().is_symlink())).then(|| Moved { from: o.user_home.join("Library").join("LaunchAgents").join(&n), to: dir.join(&n), label, loaded: true })
        })
        .collect()
}

/// Every plist the migration moved: the record's (as written; checked by undo) and the
/// stranded ones.
fn moved_plists(o: &Options) -> Vec<Moved> {
    let mut out: Vec<Moved> = std::fs::read_to_string(record_file(&o.home))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|r| r["plists"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| Some(Moved { label: x["label"].as_str()?.to_string(), from: PathBuf::from(x["from"].as_str()?), to: PathBuf::from(x["to"].as_str()?), loaded: x["loaded"].as_bool().unwrap_or(false) }))
        .filter(|m| known_label(&m.label) && m.to.parent() == Some(legacy_dir(&o.home).as_path()))
        .collect();
    out.extend(stranded(o));
    out
}

/// Whether the settings file has any Bellwether entry on the port (current or pasture's).
fn has_entries(file: &Path, port: u16) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else { return false };
    let Ok(s) = bellwether_core::lossless::parse(&text) else { return false };
    let Some(hooks) = s.get("hooks").and_then(|h| h.as_obj()) else { return false };
    let any = hooks.iter().filter_map(|(_, g)| g.as_arr()).flatten().filter_map(|g| g.get("hooks").and_then(|h| h.as_arr())).flatten().any(|h| installer::is_bellwether_handler(h, port));
    any
}

/// The strip `bellwether shell start` started from that checkout, by the pid in its
/// `shell.pid`, only while that pid still runs a strip binary of that checkout.
fn stray_strip(src: &Source, host: &dyn Host) -> Option<i32> {
    let root = std::fs::canonicalize(src.root.as_ref()?).ok()?;
    let pid: i32 = std::fs::read_to_string(src.data_dir.join("shell.pid")).ok()?.trim().parse().ok()?;
    if pid <= 1 {
        return None;
    }
    let exe = host.process_path(pid)?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let named = exe.file_name().is_some_and(|n| n == "bellwether-shell" || n == "pasture-shell");
    (named && exe.starts_with(&root)).then_some(pid)
}

// ---------------------------------------------------------------- the preview

fn line(lines: &mut Vec<String>, l: impl Into<String>) {
    lines.push(l.into());
}

/// Step 3's verdict: Ok with what to say, or Err with what holds the port.
pub fn port_verdict(port: u16, holder: &Holder, agents: &[OldAgent], app_agent: Option<i32>) -> Result<String, String> {
    match holder {
        Holder::Free => Ok(format!("port {port} is free")),
        Holder::Brain { pid: Some(p), .. } if agents.iter().any(|a| a.kind == "brain" && a.pid() == Some(*p)) => {
            let a = agents.iter().find(|a| a.pid() == Some(*p)).unwrap();
            Ok(format!("port {port} is held by the old brain ({}, pid {p}); it is stopped in step 4", a.label))
        }
        Holder::Brain { pid: Some(p), .. } if app_agent == Some(*p) => Ok(format!("port {port} is held by this app's own brain agent (pid {p})")),
        Holder::Brain { name, pid } => Err(format!("a {name} brain{}, not one started by the old LaunchAgents or by this app", pid.map(|p| format!(" (pid {p})")).unwrap_or_default())),
        // Not asked at all (a temp HOME on the live port): the reason as it was given.
        Holder::Other { pid: None, command: Some(c) } => Err(c.clone()),
        Holder::Other { pid, command } => Err(format!(
            "{}, which isn't a bellwether or pasture brain",
            match (pid, command) {
                (Some(p), Some(c)) => format!("pid {p} ({})", exe_name(c)),
                (Some(p), None) => format!("pid {p}"),
                _ => "a process that couldn't be identified (`lsof -nP -iTCP:<port> -sTCP:LISTEN` names it)".into(),
            }
        )),
    }
}

/// Who holds the port, read only: `/health` for a brain (bellwether's or pasture's, with its
/// pid), else the listening pid and its command line from `lsof` and `ps` (Node's
/// porthold.mjs). Nothing is signalled.
pub fn port_holder(port: u16) -> Holder {
    match client::request(port, "GET", "/health", &[], None, Duration::from_millis(1500)) {
        Err(client::Failure::Refused) => return Holder::Free,
        Ok(r) if r.status == 200 => {
            let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            if let Some(name) = v["name"].as_str().filter(|n| *n == "bellwether" || *n == "pasture") {
                return Holder::Brain { name: name.to_string(), pid: v["pid"].as_i64().map(|p| p as i32) };
            }
        }
        _ => {}
    }
    let (pid, command) = listener(port);
    Holder::Other { pid, command }
}

/// The pid listening on 127.0.0.1:<port>, and its command line (lsof, ps; read only, each
/// bounded to 5 s).
fn listener(port: u16) -> (Option<i32>, Option<String>) {
    let run = |prog: &str, args: &[String]| -> Option<String> {
        let mut cmd = std::process::Command::new(prog);
        cmd.args(args);
        match legacy::run_output(cmd, Duration::from_secs(5)) {
            Ok((true, out, _)) => Some(out),
            _ => None,
        }
    };
    let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"].into_iter().find(|p| Path::new(p).exists());
    let Some(lsof) = lsof else { return (None, None) };
    let out = run(lsof, &["-nP".into(), format!("-iTCP@{BIND_HOST}:{port}"), "-sTCP:LISTEN".into(), "-Fp".into()]).unwrap_or_default();
    let pid = out.lines().find_map(|l| l.strip_prefix('p')).and_then(|p| p.trim().parse::<i32>().ok());
    let Some(pid) = pid else { return (None, None) };
    let command = run("/bin/ps", &["-o".into(), "command=".into(), "-p".into(), pid.to_string()]).map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    (Some(pid), command)
}

/// The program of a command line, without its directory.
fn exe_name(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or(command);
    Path::new(first).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| first.to_string())
}

fn port_refusal(port: u16, who: &str) -> BrainError {
    BrainError::new(format!("port {port} is held by {who}. Nothing was changed: the old setup keeps running. Stop that process yourself (or change \"port\" in config.json), then run this again."), exit::NOT_RUNNING)
}

fn legacy_dir(home: &Path) -> PathBuf {
    home.join("legacy")
}

/// While step 4 runs: `<home>/legacy/swapping`, holding when it started (ms) and the pid of
/// the run. A strip of this home starts no brain of its own meanwhile (shell brain.rs), or
/// it could take the port in the gap before the app's login agent does.
pub fn swap_marker(home: &Path) -> PathBuf {
    legacy_dir(home).join("swapping")
}

/// How long a swap marker can count at most: step 4 takes about 10 s.
pub const SWAP_MARKER_LIFE: Duration = Duration::from_secs(60);

/// Whether a migration is swapping the brains for this home right now: a marker under a
/// minute old whose run is still alive. One left by a killed run stops counting at once,
/// so the app can start a brain again (BR-3K's M8 D1).
pub fn swapping(home: &Path) -> bool {
    let Ok(t) = std::fs::read_to_string(swap_marker(home)) else { return false };
    let mut parts = t.split_whitespace();
    let Some(at) = parts.next().and_then(|a| a.parse::<u64>().ok()) else { return false };
    let Some(pid) = parts.next().and_then(|p| p.parse::<i32>().ok()) else { return false };
    let now = crate::now_ms();
    now >= at && now - at < SWAP_MARKER_LIFE.as_millis() as u64 && alive(pid)
}

/// Only one migration (or undo) writes at a time: an exclusive `flock` on
/// `<home>/legacy/migrate.lock`, which also holds the pid of the run that has it. The kernel
/// drops the lock when that run ends, however it ends, so there is no stale lock to take over
/// and no window in which a second run could remove a fresh one (BR-3K's re-check R2). The
/// file itself stays (removing it would let a run lock a new file beside an old one).
pub struct Lock {
    // Held open: the flock lasts as long as this file is open.
    _file: std::fs::File,
}

impl Lock {
    pub fn take(home: &Path) -> Result<Lock, BrainError> {
        let dir = legacy_dir(home);
        home::create_private_dir(home).and_then(|_| home::create_private_dir(&dir)).map_err(|e| io("create", &dir, e))?;
        let f = dir.join("migrate.lock");
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut file = o.open(&f).map_err(|e| io("open", &f, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    let pid = std::fs::read_to_string(&f).ok().and_then(|t| t.trim().parse::<i32>().ok()).map(|p| format!("pid {p}, ")).unwrap_or_default();
                    return Err(BrainError::new(format!("another migration is running ({pid}{}); nothing was changed. Wait for it to finish, then run this again", f.display()), exit::NOT_RUNNING));
                }
                return Err(io("lock", &f, err));
            }
        }
        use std::io::{Seek, Write};
        let _ = file.set_len(0);
        let _ = file.seek(std::io::SeekFrom::Start(0));
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();
        Ok(Lock { _file: file })
    }
}


/// Where step 1 keeps the app's own token when it replaces it.
pub fn replaced_token_file(home: &Path) -> PathBuf {
    legacy_dir(home).join("hook-token.before-migration")
}

/// The record of a migration, for `--undo`.
pub fn record_file(home: &Path) -> PathBuf {
    legacy_dir(home).join("migration.json")
}

fn mask(text: &str, token: Option<&str>) -> String {
    bellwether_core::mask::mask_alone(text, token)
}

/// The preview's lines. Each action is a line starting with "   - "; what is only said
/// starts with "   = ".
pub fn preview(o: &Options, s: &Survey, host: &dyn Host) -> Vec<String> {
    let mut l = Vec::new();
    let token = s.token.as_ref().map(|(t, _)| t.as_str());
    if s.nothing() {
        line(&mut l, format!("Nothing to migrate: no earlier Bellwether found (no local.bellwether.* or local.pasture.* LaunchAgents{}, and the hook entries in {} are current or absent).", if s.source.is_some() { ", nothing left to copy" } else { "" }, o.settings.display()));
        return l;
    }
    match &s.source {
        Some(src) => line(&mut l, format!("Earlier install: {}, data in {}", src.root.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "(a data directory only)".into()), src.data_dir.display())),
        None if s.agents.is_empty() && s.unfinished.is_some() => line(&mut l, format!("Earlier install: already moved by a migration that didn't finish ({})", record_file(&o.home).display())),
        None => line(&mut l, format!("Earlier install: {}", if s.agents.is_empty() { "hook entries only (no repository found)" } else { "LaunchAgents (no repository found)" })),
    }
    if let Some(why) = &s.unfinished {
        line(&mut l, format!("Unfinished: the migration recorded in {} didn't finish: {why}. This run finishes it (`bellwether migrate --undo --write` puts the old setup back instead).", record_file(&o.home).display()));
    }
    if let Some(n) = &s.source_note {
        line(&mut l, format!("Note: {n}"));
    }
    line(&mut l, format!("Into:            the app's files in {}", o.home.display()));
    line(&mut l, "");

    line(&mut l, "1. The hook token");
    let source = |from: &TokenFrom| match from {
        TokenFrom::EnvFile(f) => format!("the token from {}", f.display()),
        _ => format!("the token of the hook entries in {}", o.settings.display()),
    };
    match &s.token {
        Some((_, TokenFrom::Kept)) => line(&mut l, format!("   = the app has one ({}), the same running sessions carry; kept", tok::file(&o.home).display())),
        Some((_, from)) if s.token_replaces => {
            line(&mut l, format!("   - replace the app's own hook token ({}) with {}: running sessions carry that one, and with the app's they would be refused (403). Same value as theirs, not shown; the app's old token is kept as {}", tok::file(&o.home).display(), source(from), replaced_token_file(&o.home).display()));
        }
        Some((_, TokenFrom::EnvFile(f))) => line(&mut l, format!("   - copy the token from {} into {} (same value, not shown, so running sessions keep working)", f.display(), tok::file(&o.home).display())),
        Some((_, TokenFrom::Hooks)) => line(&mut l, format!("   - copy the token of the hook entries in {} into {} (same value, not shown)", o.settings.display(), tok::file(&o.home).display())),
        None => line(&mut l, "   = none found"),
    }

    line(&mut l, "2. Files (each only if the app has none yet)");
    for item in &s.copy {
        line(&mut l, format!("   - copy {}", item.describe()));
    }
    for k in &s.kept {
        line(&mut l, format!("   = {k}"));
    }
    if s.copy.is_empty() && s.kept.is_empty() {
        line(&mut l, "   = nothing to copy");
    }
    for w in &s.warnings {
        line(&mut l, format!("   = warning: {w}"));
    }

    line(&mut l, format!("3. Port {}", s.port));
    match port_verdict(s.port, &host.port_holder(s.port), &s.agents, host.app_agent()) {
        Ok(v) => line(&mut l, format!("   = {v}")),
        Err(who) => line(&mut l, format!("   = STOP: port {} is held by {who}. --write would stop here and change nothing; stop that process first", s.port)),
    }

    line(&mut l, "4. Start at login");
    let domain = host.domain();
    for a in s.agents.iter().filter(|a| a.loaded.is_loaded()) {
        line(&mut l, format!("   - stop {} (launchctl bootout {domain}/{})", a.label, a.label));
    }
    if let Some(pid) = s.stray_strip {
        line(&mut l, format!("   - stop the strip `bellwether shell start` left running (pid {pid}, from shell.pid)"));
    }
    for a in &s.agents {
        if let Some(p) = &a.plist {
            line(&mut l, format!("   - move {} to {}/", p.display(), legacy_dir(&o.home).display()));
        }
    }
    line(&mut l, "   - register the brain agent and the app at login");
    line(&mut l, format!("   - wait up to {} s for the new brain to answer on port {} and take the token", host.verify_limit().as_secs(), s.port));
    line(&mut l, "   = if it doesn't, everything in this step is undone and the old setup keeps running");
    if s.agents.iter().any(|a| a.kind == "brain" && a.loaded.is_loaded()) {
        line(&mut l, "   = between the old brain stopping and the new one answering (under a second) a hook may print one \"hook error\"; nothing else is lost");
    }
    if !host.acts() {
        line(&mut l, format!("   = launchctl and the login items are left alone here: {}", host.why_not()));
    }

    line(&mut l, format!("5. Hook entries in {}", o.settings.display()));
    match &s.hooks {
        Some(p) if p.changed => {
            let what = if s.legacy_entries > 0 { format!("{} pasture entr{} (X-Pasture-*)", s.legacy_entries, if s.legacy_entries == 1 { "y" } else { "ies" }) } else { "Bellwether's entries".into() };
            line(&mut l, format!("   - rewrite {what} with the current header names, a backup first (diff below, token masked)"));
        }
        Some(_) => line(&mut l, "   = already current: nothing changes in this file"),
        None => line(&mut l, format!("   = no Bellwether entries on port {}; none are added (`bellwether hooks install --write` adds them)", s.port)),
    }

    line(&mut l, "6. Accessibility");
    line(&mut l, "   = the old strip's entry (bellwether-shell or pasture-shell) may stay in System Settings > Privacy & Security > Accessibility: remove it there if you like. Bellwether.app asks for its own");
    line(&mut l, "7. `bellwether doctor` runs at the end");
    line(&mut l, "");
    match s.source.as_ref().and_then(|x| x.root.as_ref()) {
        Some(r) => line(&mut l, format!("Stays as it is: the repository at {} (its .env, config.json and data/ are never changed or deleted).", r.display())),
        None => line(&mut l, "Stays as it is: everything not listed above."),
    }
    if let Some(p) = s.hooks.as_ref().filter(|p| p.changed) {
        line(&mut l, "");
        for d in mask(&crate::cli::hook_diff(p), token).lines() {
            line(&mut l, d.to_string());
        }
    }
    l
}

// ---------------------------------------------------------------- the run

/// What a run printed, and whether it changed anything.
pub struct Outcome {
    pub lines: Vec<String>,
    pub wrote: bool,
}

/// `migrate --to-app`: the preview, or (with `write`) the migration.
pub fn to_app(o: &Options, host: &mut dyn Host) -> Result<Outcome, BrainError> {
    let s = survey(o, host)?;
    let mut lines = preview(o, &s, host);
    if !o.write || s.nothing() {
        if !s.nothing() {
            lines.push(String::new());
            lines.push("Preview only; nothing was changed. Re-run with --write to apply.".into());
        }
        return Ok(Outcome { lines, wrote: false });
    }
    let lines = apply(o, s, host)?;
    Ok(Outcome { lines, wrote: true })
}

fn stamp() -> String {
    crate::cli::file_stamp(crate::now_ms())
}

/// The run itself, D9's order. An error stops it with its message; step 4 undoes itself.
fn apply(o: &Options, s: Survey, host: &mut dyn Host) -> Result<Vec<String>, BrainError> {
    let mut l = Vec::new();
    let (token, from) = s.token.clone().expect("survey refuses a migration without a token");
    // Before anything is written: the port (step 3 again below), and whether the settings
    // file can be rewritten at the end.
    port_verdict(s.port, &host.port_holder(s.port), &s.agents, host.app_agent()).map_err(|who| port_refusal(s.port, &who))?;
    if s.hooks.as_ref().is_some_and(|p| p.changed) {
        for f in [o.settings.clone(), o.settings.parent().map(Path::to_path_buf).unwrap_or_default()] {
            if let Err(why) = migrate::writable(&f) {
                return Err(bad(format!("{} can't be written ({why}), so the hook entries couldn't be moved at the end. Nothing was changed. Fix the permission, then run this again.", f.display())));
            }
        }
    }
    // One run at a time (the CLI and the setup window could both try).
    let _lock = Lock::take(&o.home)?;
    line(&mut l, format!("Into the app's files in {}", o.home.display()));

    // 1. The token.
    line(&mut l, "1. The hook token");
    home::create_private_dir(&o.home).map_err(|e| io("create", &o.home, e))?;
    let replaced = if s.token_replaces { tok::read_file(&o.home)? } else { None };
    if let Some(old) = &replaced {
        let keep = replaced_token_file(&o.home);
        home::create_private_dir(&legacy_dir(&o.home)).map_err(|e| io("create", &legacy_dir(&o.home), e))?;
        installer::write_replace(&keep, format!("{old}\n").as_bytes(), 0o600).and_then(|_| installer::set_mode(&keep, 0o600)).map_err(|e| io("write", &keep, e))?;
    }
    match &from {
        TokenFrom::Kept => line(&mut l, "   kept the app's token (running sessions carry the same)"),
        TokenFrom::EnvFile(f) => {
            tok::write(&o.home, &token)?;
            line(&mut l, format!("   {} the token from {} (same value, not shown)", if replaced.is_some() { "replaced the app's own token with" } else { "copied" }, f.display()));
        }
        TokenFrom::Hooks => {
            tok::write(&o.home, &token)?;
            line(&mut l, format!("   {} the token of the hook entries in {} (same value, not shown)", if replaced.is_some() { "replaced the app's own token with" } else { "copied" }, o.settings.display()));
        }
    }
    if let Some(old) = &replaced {
        line(&mut l, format!("   the app's previous token is kept in {}", replaced_token_file(&o.home).display()));
        // This app's own brain, already running with the replaced token, reads the file again.
        if let Holder::Brain { pid: Some(p), .. } = host.port_holder(s.port) {
            if host.app_agent() == Some(p) {
                match crate::cli::tell_brain(s.port, old) {
                    crate::cli::Switch::Switched => line(&mut l, "   the app's running brain switched to it"),
                    _ => line(&mut l, "   warning: the app's running brain didn't switch to it; it does when it restarts"),
                }
            }
        }
    }

    // 2. The data. A copy that fails stops the run here, before any agent is touched.
    line(&mut l, "2. Files");
    for item in &s.copy {
        match copy_item(&o.home, item, o.force) {
            Ok(()) => line(&mut l, format!("   copied {}", item.describe())),
            Err(e) => {
                line(&mut l, format!("   not copied: {}: {}", item.describe(), e.message));
                return Err(BrainError::new(
                    format!("{}\n\nStopped at step 2: {} couldn't be copied ({}). The old setup keeps running: no agent, login item or hook entry was touched. What was copied before stays in the app's folder. Fix the cause, then run this again.", l.join("\n"), item.describe(), e.message),
                    exit::UNEXPECTED,
                ));
            }
        }
    }
    if s.copy.is_empty() {
        line(&mut l, "   nothing to copy");
    }

    // 3. The port, again: it may have changed while the files were copied.
    line(&mut l, format!("3. Port {}", s.port));
    let v = port_verdict(s.port, &host.port_holder(s.port), &s.agents, host.app_agent()).map_err(|who| port_refusal(s.port, &who))?;
    line(&mut l, format!("   {v}"));

    // 4. The swap.
    line(&mut l, "4. Start at login");
    swap(o, &s, &token, host, &mut l)?;

    // 5. The hook entries.
    line(&mut l, format!("5. Hook entries in {}", o.settings.display()));
    match installer::plan_edit(&o.settings, Action::Install, s.port, &token, Some(&o.home)) {
        Ok(mut plan) if plan.changed && has_entries(&o.settings, s.port) => {
            // The record names the rewrite (its backup, and the bytes it will write) before the
            // file is touched, so a run killed in between still leaves undo what it needs.
            let stamp = stamp();
            let backup_at = plan.exists.then(|| o.home.join("backups").join(format!("settings-{stamp}.json")));
            let after = installer::sha256_hex(plan.new_text.as_bytes());
            update_record(&o.home, |r| {
                r["settings"] = json!({ "file": o.settings.display().to_string(), "backup": backup_at.as_ref().map(|b| b.display().to_string()), "after": after, "existed": plan.exists });
            })?;
            match installer::apply_plan(&mut plan, &o.home.join("backups"), Some(&o.home), &stamp) {
                Ok(backup) => {
                    line(&mut l, "   rewrote Bellwether's entries with the current header names");
                    if let Some(b) = &backup {
                        line(&mut l, format!("   backup of the previous file: {}", b.display()));
                    }
                    for w in &plan.warnings {
                        line(&mut l, format!("   warning: {}", mask(w, Some(&token))));
                    }
                }
                Err(e) => {
                    let _ = update_record(&o.home, |r| r["settings"] = Value::Null);
                    line(&mut l, format!("   not rewritten: {}. The new brain is running; `bellwether hooks install --write` rewrites them", e.message));
                }
            }
        }
        Ok(_) if !has_entries(&o.settings, s.port) => line(&mut l, format!("   no Bellwether entries on port {}; none added", s.port)),
        Ok(_) => line(&mut l, "   already current: unchanged"),
        Err(e) => line(&mut l, format!("   not rewritten: {}. `bellwether hooks install --write` rewrites them once the file is fixed", e.message)),
    }

    // 6. Accessibility.
    line(&mut l, "6. Accessibility");
    line(&mut l, "   the old strip's entry (bellwether-shell or pasture-shell) may still be listed in System Settings > Privacy & Security > Accessibility; remove it there if you like. Bellwether.app asks for its own (the setup window's Dock step, or `open -a Bellwether --args --ax-prompt`)");

    // 7. Doctor.
    line(&mut l, "7. Doctor");
    for d in host.doctor(&o.home, &o.settings, s.port) {
        line(&mut l, format!("   {}", mask(&d, Some(&token))));
    }
    line(&mut l, "");
    line(&mut l, "Done. `bellwether migrate --undo` shows how to put the old setup back.");
    Ok(l)
}

fn copy_item(home: &Path, item: &Item, force: bool) -> Result<(), BrainError> {
    let target = item.target(home);
    let tmp = |name: &str| home.join(format!(".{name}.{}.tmp", std::process::id()));
    match item {
        Item::Config { text, .. } => persist::write_atomic(&target, &tmp("config.json"), text, 0o600).map_err(|e| io("write", &target, e)),
        Item::State { clean, .. } => persist::save_state(&target, clean).map_err(|e| io("write", &target, e)),
        // The strip's own writer: its layout, and the app's `onboarded` flag kept if --force
        // replaces the app's file.
        Item::ShellState { hidden, show_labels, .. } => bellwether_core::shellstate::write(home, *hidden, *show_labels).map_err(|e| io("write", &target, e)),
        Item::Installs { text, .. } => persist::write_atomic(&target, &tmp("installs.json"), text, 0o600).map_err(|e| io("write", &target, e)),
        Item::Backup { from, .. } => {
            let bytes = std::fs::read(from).map_err(|e| io("read", from, e))?;
            let dir = home.join("backups");
            home::create_private_dir(&dir).map_err(|e| io("create", &dir, e))?;
            installer::write_replace(&target, &bytes, 0o600).and_then(|_| installer::set_mode(&target, 0o600)).map_err(|e| io("write", &target, e))
        }
        Item::Pet { from, name } => crate::pets::import(home, from, Some(name), force).map(|_| ()),
    }
}

/// Reads the record (an empty object when there is none), changes it, writes it back
/// (0600, atomically).
fn update_record(home: &Path, change: impl FnOnce(&mut Value)) -> Result<(), BrainError> {
    let f = record_file(home);
    let mut r: Value = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({ "version": 1 }));
    change(&mut r);
    let dir = legacy_dir(home);
    home::create_private_dir(&dir).map_err(|e| io("create", &dir, e))?;
    let text = serde_json::to_string_pretty(&r).unwrap_or_default() + "\n";
    persist::write_atomic(&f, &dir.join(format!(".migration.json.{}.tmp", std::process::id())), &text, 0o600).map_err(|e| io("write", &f, e))
}

/// Moves a file, also across file systems (copy, then remove).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc_exdev()) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

fn libc_exdev() -> i32 {
    18 // EXDEV on macOS, Linux and the BSDs
}

/// Step 4: old agents out, the app's login items in, the new brain verified; undone on
/// failure.
fn swap(o: &Options, s: &Survey, token: &str, host: &mut dyn Host, l: &mut Vec<String>) -> Result<(), BrainError> {
    let dir = legacy_dir(&o.home);
    home::create_private_dir(&dir).map_err(|e| io("create", &dir, e))?;
    let marker = swap_marker(&o.home);
    installer::write_replace(&marker, format!("{} {}", crate::now_ms(), std::process::id()).as_bytes(), 0o600).map_err(|e| io("write", &marker, e))?;
    // Recorded before anything moves: a run killed from here on is seen as unfinished.
    update_record(&o.home, |r| r["swap"] = Value::from("started"))?;
    let r = swap_inner(o, s, token, host, l);
    let _ = std::fs::remove_file(&marker);
    r
}

fn swap_inner(o: &Options, s: &Survey, token: &str, host: &mut dyn Host, l: &mut Vec<String>) -> Result<(), BrainError> {
    let domain = host.domain();
    let acts = host.acts();
    let old_pids: Vec<i32> = s.agents.iter().filter_map(OldAgent::pid).collect();
    let mut done: Vec<String> = Vec::new();
    let mut moved: Vec<(OldAgent, PathBuf, PathBuf)> = Vec::new();
    let mut booted: Vec<OldAgent> = Vec::new();

    for a in s.agents.iter().filter(|a| a.loaded.is_loaded()) {
        if acts {
            match host.launchctl(&["bootout".into(), format!("{domain}/{}", a.label)]) {
                Ok(()) => {
                    done.push(format!("stopped {} (launchctl bootout)", a.label));
                    booted.push(a.clone());
                }
                Err(e) => done.push(format!("launchctl bootout {domain}/{} failed ({e}); going on", a.label)),
            }
        }
    }
    if let Some(pid) = s.stray_strip {
        if host.terminate(pid) {
            done.push(format!("stopped the strip `bellwether shell start` left running (pid {pid})"));
        } else {
            done.push(format!("the strip (pid {pid}) didn't stop within 5 s; quit it from its menu-bar sheep"));
        }
    }
    let dir = legacy_dir(&o.home);
    let mut move_error = None;
    // Plists a run that stopped part-way already moved, missing from the record: recorded now,
    // so undo can put them back (BR-3K's re-check R1).
    let stranded_now = stranded(o);
    if !stranded_now.is_empty() {
        update_record(&o.home, |r| {
            let mut list: Vec<Value> = r["plists"].as_array().cloned().unwrap_or_default();
            for m in &stranded_now {
                list.push(json!({ "label": m.label, "from": m.from.display().to_string(), "to": m.to.display().to_string(), "loaded": m.loaded }));
                done.push(format!("recorded {} (moved by the run that stopped)", m.to.display()));
            }
            r["plists"] = Value::Array(list);
        })?;
    }
    for a in &s.agents {
        let Some(p) = &a.plist else { continue };
        let to = dir.join(p.file_name().unwrap_or_default());
        // The record names the move before it happens: a run killed between the rename and
        // the record can't leave a plist undo doesn't know about. (Undo finds a recorded
        // plist still in LaunchAgents "back already".)
        let entry = json!({ "label": a.label, "from": p.display().to_string(), "to": to.display().to_string(), "loaded": a.loaded.is_loaded() });
        update_record(&o.home, |r| {
            let mut list: Vec<Value> = r["plists"].as_array().cloned().unwrap_or_default();
            list.retain(|x| x["label"] != a.label.as_str());
            list.push(entry.clone());
            r["plists"] = Value::Array(list);
        })?;
        match move_file(p, &to) {
            Ok(()) => {
                done.push(format!("moved {} to {}", p.display(), to.display()));
                moved.push((a.clone(), p.clone(), to));
            }
            Err(e) => {
                let _ = update_record(&o.home, |r| {
                    if let Some(list) = r["plists"].as_array_mut() {
                        list.retain(|x| x["label"] != a.label.as_str());
                    }
                });
                move_error = Some(format!("couldn't move {} ({e})", p.display()));
                break;
            }
        }
    }

    let mut registered = false;
    let result: Result<String, String> = (|| {
        if let Some(e) = &move_error {
            return Err(e.clone());
        }
        if !acts {
            return Ok(format!("launchctl and the login items were left alone: {}", host.why_not()));
        }
        let lines = host.register()?;
        registered = true;
        for x in lines {
            done.push(x);
        }
        wait_for_new_brain(s.port, token, &old_pids, host.verify_limit())
    })();

    match result {
        Ok(v) => {
            for d in done {
                line(l, format!("   {d}"));
            }
            line(l, format!("   {v}"));
            update_record(&o.home, |r| {
                r["swap"] = Value::from("done");
                if acts {
                    r["loginItems"] = Value::Bool(true);
                }
            })?;
            Ok(())
        }
        Err(why) => {
            // Undo all of step 4: the old setup comes back as it was.
            let mut undo = Vec::new();
            if registered {
                match host.unregister() {
                    Ok(_) => undo.push("unregistered the app's login items".to_string()),
                    Err(e) => undo.push(format!("couldn't unregister the app's login items ({e}): `bellwether autostart uninstall --write`")),
                }
            }
            let mut manual = Vec::new();
            for (a, from, to) in &moved {
                match move_file(to, from) {
                    Ok(()) => undo.push(format!("moved {} back", from.display())),
                    Err(e) => {
                        undo.push(format!("couldn't move {} back ({e})", to.display()));
                        manual.push(format!("mv '{}' '{}'", to.display(), from.display()));
                    }
                }
                let _ = a;
            }
            let back: Vec<String> = moved.iter().map(|(a, _, _)| a.label.clone()).collect();
            let _ = update_record(&o.home, |r| {
                if let Some(list) = r["plists"].as_array_mut() {
                    list.retain(|x| !back.iter().any(|b| x["label"] == b.as_str()));
                }
                r["swap"] = Value::from("undone");
            });
            for a in &booted {
                let plist = moved.iter().find(|(m, _, _)| m.label == a.label).map(|(_, f, _)| f.clone()).or_else(|| a.plist.clone());
                match plist.filter(|p| p.exists()) {
                    Some(p) => match host.launchctl(&["bootstrap".into(), domain.clone(), p.display().to_string()]) {
                        Ok(()) => undo.push(format!("started {} again (launchctl bootstrap)", a.label)),
                        Err(e) => {
                            undo.push(format!("couldn't start {} again ({e})", a.label));
                            manual.push(format!("launchctl bootstrap {domain} '{}'", p.display()));
                        }
                    },
                    None => undo.push(format!("{} had no plist to start it from; it starts again at your next login only if its plist is back", a.label)),
                }
            }
            let mut msg = vec![format!("the move to the app's login items didn't work: {why}"), "What changed, and was undone:".to_string()];
            msg.extend(done.iter().map(|d| format!("  - {d}")));
            msg.extend(undo.iter().map(|d| format!("  - {d}")));
            msg.push("The hook entries were not touched, so the old setup keeps working as before. The token and files copied in steps 1 and 2 stay in the app's folder (harmless; a later run keeps them).".into());
            if manual.is_empty() {
                msg.push("The old LaunchAgents are back as they were.".into());
            } else {
                msg.push(format!("Not everything could be put back. By hand:\n{}", manual.iter().map(|m| format!("  {m}")).collect::<Vec<_>>().join("\n")));
            }
            msg.push("Fix the cause, then run the same command again; it picks up where this left off.".into());
            Err(BrainError::new(msg.join("\n"), exit::UNEXPECTED))
        }
    }
}

/// Polls `/health` every 100 ms for a bellwether brain that isn't one of the old ones, then
/// asks `/state` with the token.
pub fn wait_for_new_brain(port: u16, token: &str, old_pids: &[i32], limit: Duration) -> Result<String, String> {
    let start = Instant::now();
    let mut last = format!("no brain answered on {BIND_HOST}:{port}");
    loop {
        match client::request(port, "GET", "/health", &[], None, Duration::from_millis(800)) {
            Ok(r) if r.status == 200 => {
                let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
                let pid = v["pid"].as_i64().map(|p| p as i32);
                if v["name"] != "bellwether" {
                    last = format!("what answers on port {port} isn't a bellwether brain");
                } else if pid.is_some_and(|p| old_pids.contains(&p)) {
                    last = format!("the old brain (pid {}) still answers on port {port}", pid.unwrap());
                } else {
                    match client::request(port, "GET", "/state?wait=0", &[("X-Bellwether-Token", token)], None, Duration::from_millis(800)) {
                        Ok(st) if st.status == 200 => {
                            return Ok(format!("the new brain answers on port {port}{} and takes the hook token ({} ms)", pid.map(|p| format!(" (pid {p})")).unwrap_or_default(), start.elapsed().as_millis()));
                        }
                        Ok(st) => last = format!("the new brain on port {port} refuses the hook token ({})", st.status),
                        Err(f) => last = format!("the new brain on port {port} didn't answer /state ({})", f.code()),
                    }
                }
            }
            Ok(r) => last = format!("port {port} answered /health with {}", r.status),
            Err(client::Failure::Refused) => last = format!("no brain answers on {BIND_HOST}:{port} yet"),
            Err(f) => last = format!("port {port} didn't answer ({})", f.code()),
        }
        if start.elapsed() >= limit {
            return Err(format!("{last} after {} s", limit.as_secs_f64()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------- undo

/// One plist the record says step 4 moved.
#[derive(Clone, Debug)]
struct Moved {
    label: String,
    from: PathBuf,
    to: PathBuf,
    loaded: bool,
}

/// A path that exists as a file, a directory or a symlink (dangling or not).
fn present(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A label the migration moves: one of D5's four, or a hub's own `<prefix>.pasture.brain|shell`.
fn known_label(label: &str) -> bool {
    legacy::LABELS.contains(&label) || ((label.ends_with(".pasture.brain") || label.ends_with(".pasture.shell")) && label.len() <= 200 && label.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')))
}

/// The record, checked before anything is done with it: only plists of the known labels,
/// only between `~/Library/LaunchAgents` and the app's `legacy/` (same file name, plain
/// names), and only this settings file restored from the app's own `backups/`. Anything
/// else is refused: undo is never more than putting back what the migration moved.
fn read_record(o: &Options) -> Result<Option<(Value, Vec<Moved>, Option<(PathBuf, Option<PathBuf>, String, bool)>)>, BrainError> {
    let f = record_file(&o.home);
    let text = match std::fs::read_to_string(&f) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io("read", &f, e)),
    };
    let refuse = |why: String| bad(format!("{} doesn't look like a record this migration wrote ({why}); nothing was changed", f.display()));
    let r: Value = serde_json::from_str(&text).map_err(|e| bad(format!("{} isn't valid JSON ({e}); nothing was changed", f.display())))?;
    if !r.is_object() {
        return Err(refuse("not an object".into()));
    }
    let agents_dir = o.user_home.join("Library").join("LaunchAgents");
    let legacy = legacy_dir(&o.home);
    let plain = |p: &Path| p.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::RootDir));
    let mut moved = Vec::new();
    match &r["plists"] {
        Value::Null => {}
        Value::Array(list) => {
            for x in list {
                let (Some(label), Some(from), Some(to)) = (x["label"].as_str(), x["from"].as_str(), x["to"].as_str()) else { return Err(refuse("a plist entry without label, from and to".into())) };
                let (from, to) = (PathBuf::from(from), PathBuf::from(to));
                if !known_label(label) {
                    return Err(refuse(format!("{label:?} isn't one of the repository's LaunchAgent labels")));
                }
                let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if !plain(&from) || !plain(&to) || from.parent() != Some(agents_dir.as_path()) || to.parent() != Some(legacy.as_path()) || to.file_name() != from.file_name() || !name.ends_with(".plist") {
                    return Err(refuse(format!("{label}: only a plist moved between {} and {} can be put back", agents_dir.display(), legacy.display())));
                }
                moved.push(Moved { label: label.to_string(), from, to, loaded: x["loaded"].as_bool().unwrap_or(false) });
            }
        }
        _ => return Err(refuse("\"plists\" isn't a list".into())),
    }
    let settings = match &r["settings"] {
        Value::Null => None,
        v => {
            let (Some(file), Some(after)) = (v["file"].as_str(), v["after"].as_str()) else { return Err(refuse("a settings entry without file and after".into())) };
            let file = PathBuf::from(file);
            if file != o.settings && !migrate::same(&file, &o.settings) {
                return Err(refuse(format!("it names {}, not {}", file.display(), o.settings.display())));
            }
            let backup = v["backup"].as_str().map(PathBuf::from);
            if let Some(b) = &backup {
                let name = b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if !plain(b) || b.parent() != Some(o.home.join("backups").as_path()) || !name.starts_with("settings-") {
                    return Err(refuse(format!("its settings backup {} isn't in {}", b.display(), o.home.join("backups").display())));
                }
            }
            Some((file, backup, after.to_string(), v["existed"].as_bool().unwrap_or(true)))
        }
    };
    Ok(Some((r, moved, settings)))
}

/// `migrate --undo`: the preview, or (with `write`) the old setup put back. Any plist that
/// can't be put back stops it before the app's login items are touched; anything left
/// undone keeps the record, exits non-zero, and a rerun picks up from there.
pub fn undo(o: &Options, host: &mut dyn Host) -> Result<Outcome, BrainError> {
    let f = record_file(&o.home);
    let stranded = stranded(o);
    let (moved, settings) = match read_record(o)? {
        Some((_, mut moved, settings)) => {
            moved.extend(stranded);
            (moved, settings)
        }
        None if !stranded.is_empty() => (stranded, None),
        None => return Ok(Outcome { lines: vec![format!("Nothing to undo: no migration is recorded in {}.", f.display())], wrote: false }),
    };
    let _lock = if o.write { Some(Lock::take(&o.home)?) } else { None };
    let domain = host.domain();
    let acts = host.acts();
    let w = o.write;
    let mut l = vec![format!("Undo the migration recorded in {}", f.display()), String::new()];
    let mut left: Vec<String> = Vec::new();

    // Each plist: back already, to move back, or lost.
    let back: Vec<&Moved> = moved.iter().filter(|m| present(&m.from)).collect();
    let to_move: Vec<&Moved> = moved.iter().filter(|m| !present(&m.from) && present(&m.to)).collect();
    let lost: Vec<&Moved> = moved.iter().filter(|m| !present(&m.from) && !present(&m.to)).collect();
    let agents_dir = o.user_home.join("Library").join("LaunchAgents");
    let blocked: Option<String> = if !lost.is_empty() {
        Some(format!("{} gone from both {} and {}: put the plist back in {} (a copy you kept, for example), then run this again", lost.iter().map(|m| m.label.as_str()).collect::<Vec<_>>().join(" and "), agents_dir.display(), legacy_dir(&o.home).display(), legacy_dir(&o.home).display()))
    } else if !to_move.is_empty() {
        let probe = if agents_dir.exists() { agents_dir.clone() } else { o.user_home.join("Library") };
        migrate::writable(&probe).err().map(|why| format!("{} can't be written ({why}), so the plists can't be moved back: fix its permissions (`chmod u+w`), then run this again", probe.display()))
    } else {
        None
    };

    line(&mut l, "1. The old LaunchAgents' plists");
    if moved.is_empty() {
        line(&mut l, "   = none were moved");
    }
    for m in &back {
        line(&mut l, format!("   = {} is back already", m.from.display()));
    }
    if let Some(why) = &blocked {
        for m in &to_move {
            line(&mut l, format!("   = {} is still in {}", m.label, m.to.display()));
        }
        line(&mut l, format!("   STOP: {why}"));
        line(&mut l, "");
        line(&mut l, "Nothing was changed: the app's login items, its brain and the settings file are as they were, and the record is kept, so this can run again.".to_string());
        return Err(BrainError::new(l.join("\n"), exit::UNEXPECTED));
    }
    if !w {
        for m in &to_move {
            line(&mut l, format!("   - move {} back to {}", m.to.display(), m.from.display()));
        }
    } else {
        // All or nothing: one that can't be moved puts the others back in legacy/.
        let _ = std::fs::create_dir_all(&agents_dir);
        let mut done: Vec<&Moved> = Vec::new();
        let mut failed = None;
        for m in &to_move {
            match move_file(&m.to, &m.from) {
                Ok(()) => done.push(m),
                Err(e) => {
                    failed = Some(format!("couldn't move {} back ({e})", m.to.display()));
                    break;
                }
            }
        }
        if let Some(why) = failed {
            for m in done.iter().rev() {
                let _ = move_file(&m.from, &m.to);
            }
            line(&mut l, format!("   STOP: {why}"));
            line(&mut l, "");
            line(&mut l, "Nothing was changed: the plists are in legacy/ as before, the app's login items and brain are as they were, and the record is kept. Fix the cause, then run this again.".to_string());
            return Err(BrainError::new(l.join("\n"), exit::UNEXPECTED));
        }
        for m in &done {
            line(&mut l, format!("   moved {} back", m.from.display()));
        }
    }

    line(&mut l, "2. The app's login items");
    if !acts {
        line(&mut l, format!("   = left alone: {}", host.why_not()));
    } else if w {
        match host.unregister() {
            Ok(lines) => lines.into_iter().for_each(|x| line(&mut l, format!("   {x}"))),
            Err(e) => {
                line(&mut l, format!("   couldn't unregister them ({e})"));
                left.push("the app's login items are still registered: `bellwether autostart uninstall --write`, then run this again (the old agents are started after that)".into());
            }
        }
    } else {
        line(&mut l, "   - unregister the brain agent and the app at login (the app's brain stops)");
    }

    line(&mut l, "3. Start the old agents that were running");
    let loaded: Vec<&Moved> = moved.iter().filter(|m| m.loaded).collect();
    if loaded.is_empty() {
        line(&mut l, "   = none were running");
    }
    for m in loaded {
        if !w {
            line(&mut l, format!("   - start {} (launchctl bootstrap {domain} {})", m.label, m.from.display()));
        } else if !acts {
            line(&mut l, format!("   = {}: launchctl left alone", m.label));
        } else if !left.is_empty() {
            line(&mut l, format!("   = {}: not started while the app's login items are registered", m.label));
        } else if host.loaded(&m.label).is_loaded() {
            line(&mut l, format!("   = {} is running already", m.label));
        } else {
            match host.launchctl(&["bootstrap".into(), domain.clone(), m.from.display().to_string()]) {
                Ok(()) => line(&mut l, format!("   started {} (launchctl bootstrap)", m.label)),
                Err(e) => {
                    line(&mut l, format!("   couldn't start {} ({e})", m.label));
                    left.push(format!("{} isn't running: `launchctl bootstrap {domain} '{}'`, or run this again", m.label, m.from.display()));
                }
            }
        }
    }

    line(&mut l, "4. The hook entries");
    let settings_now = settings.as_ref().map(|(file, _, after, _)| std::fs::read(file).ok().map(|b| installer::sha256_hex(&b)).as_deref() == Some(after.as_str()));
    match (&settings, settings_now) {
        (None, _) => line(&mut l, "   = the migration didn't change the settings file; nothing to restore"),
        (Some((file, _, _, _)), Some(false)) => line(&mut l, format!("   = {} isn't what the migration wrote (changed since, or restored already); left as it is", file.display())),
        (Some((file, Some(backup), _, _)), _) if !backup.exists() => {
            line(&mut l, format!("   = the backup {} is gone; {} left as it is", backup.display(), file.display()));
            left.push(format!("{} wasn't restored: its backup is gone", file.display()));
        }
        (Some((file, Some(backup), _, _)), _) => {
            if w {
                match std::fs::read(backup).map_err(|e| io("read", backup, e)).and_then(|b| restore(file, &b)) {
                    Ok(()) => line(&mut l, format!("   restored {} from {}", file.display(), backup.display())),
                    Err(e) => {
                        line(&mut l, format!("   couldn't restore {} ({})", file.display(), e.message));
                        left.push(format!("{} wasn't restored; run this again", file.display()));
                    }
                }
            } else {
                line(&mut l, format!("   - restore {} from {} (it is still exactly what the migration wrote)", file.display(), backup.display()));
            }
        }
        (Some((file, None, _, existed)), _) => {
            if *existed {
                line(&mut l, format!("   = no backup was taken; {} left as it is", file.display()));
            } else if w {
                match std::fs::remove_file(file) {
                    Ok(()) => line(&mut l, format!("   removed {} (the migration created it)", file.display())),
                    Err(e) => left.push(format!("{} wasn't removed ({e})", file.display())),
                }
            } else {
                line(&mut l, format!("   - remove {} (the migration created it)", file.display()));
            }
        }
    }
    line(&mut l, "");
    line(&mut l, format!("Stays: the app's files in {} (the token, config, colours, backups).", o.home.display()));
    if !w {
        line(&mut l, "Preview only; nothing was changed. Re-run with --write to apply.");
        return Ok(Outcome { lines: l, wrote: false });
    }
    if !left.is_empty() {
        line(&mut l, "Not finished; the record is kept, so `bellwether migrate --undo --write` can run again:");
        for x in &left {
            line(&mut l, format!("  - {x}"));
        }
        return Err(BrainError::new(l.join("\n"), exit::UNEXPECTED));
    }
    if f.exists() {
        let done = legacy_dir(&o.home).join(format!("migration-undone-{}.json", stamp()));
        std::fs::rename(&f, &done).map_err(|e| io("rename", &f, e))?;
        line(&mut l, format!("Done. The record is kept as {}.", done.display()));
    } else {
        line(&mut l, "Done.");
    }
    Ok(Outcome { lines: l, wrote: true })
}

/// Puts a settings file's earlier bytes back, atomically, keeping its mode.
fn restore(file: &Path, bytes: &[u8]) -> Result<(), BrainError> {
    let target = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&target).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600)
    };
    #[cfg(not(unix))]
    let mode = 0o600;
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let tmp = dir.join(format!(".{}.bellwether-undo-{}.tmp", target.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let r = installer::write_replace(&tmp, bytes, mode).and_then(|_| installer::set_mode(&tmp, mode)).and_then(|_| std::fs::rename(&tmp, &target));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(io("write", &target, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const T: &str = "testTokenNotReal_abcdefghijklmnop";

    /// A host that records every call and answers from fixed state; it never runs launchctl.
    struct Rec {
        calls: RefCell<Vec<String>>,
        loaded: Vec<(String, Loaded)>,
        holder: Holder,
        register_fails: bool,
    }

    impl Host for Rec {
        fn acts(&self) -> bool {
            true
        }
        fn domain(&self) -> String {
            "gui/501".into()
        }
        fn loaded(&self, label: &str) -> Loaded {
            self.loaded.iter().find(|(l, _)| l == label).map(|(_, x)| x.clone()).unwrap_or(Loaded::No)
        }
        fn app_agent(&self) -> Option<i32> {
            None
        }
        fn launchctl(&mut self, args: &[String]) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("launchctl {}", args.join(" ")));
            Ok(())
        }
        fn register(&mut self) -> Result<Vec<String>, String> {
            self.calls.borrow_mut().push("register".into());
            if self.register_fails {
                Err("brain agent: waiting for your approval".into())
            } else {
                Ok(vec!["brain agent: enabled".into()])
            }
        }
        fn unregister(&mut self) -> Result<Vec<String>, String> {
            self.calls.borrow_mut().push("unregister".into());
            Ok(vec!["brain agent: off".into()])
        }
        fn port_holder(&self, _port: u16) -> Holder {
            self.holder.clone()
        }
        fn process_path(&self, _pid: i32) -> Option<PathBuf> {
            None
        }
        fn terminate(&mut self, pid: i32) -> bool {
            self.calls.borrow_mut().push(format!("terminate {pid}"));
            true
        }
        fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> {
            vec!["PASS  brain".into()]
        }
        fn verify_limit(&self) -> Duration {
            Duration::from_millis(300)
        }
    }

    fn temp(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("bw-toapp-{}-{name}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A renamed repository install with both plists, on a port nothing listens on.
    fn world(base: &Path, port: u16) -> Options {
        let user = base.join("user");
        let repo = base.join("repo");
        let agents = user.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::create_dir_all(repo.join("data")).unwrap();
        std::fs::create_dir_all(repo.join("brain")).unwrap();
        std::fs::write(repo.join(".env"), format!("BELLWETHER_HOOK_TOKEN={T}\n")).unwrap();
        std::fs::write(repo.join("config.json"), format!("{{\n  \"port\": {port},\n  \"brain\": {{\n    \"nodePath\": \"/n/node\"\n  }}\n}}\n")).unwrap();
        for (label, args) in [("local.bellwether.brain", format!("<string>/n/node</string><string>{}/brain/cli.mjs</string><string>serve</string>", repo.display())), ("local.bellwether.shell", format!("<string>{}/shell/src-tauri/target/release/bellwether-shell</string>", repo.display()))] {
            std::fs::write(agents.join(format!("{label}.plist")), format!("<plist><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{args}</array><key>WorkingDirectory</key><string>{}</string></dict></plist>", repo.display())).unwrap();
        }
        let settings = user.join(".claude").join("settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let block = bellwether_core::lossless::stringify(&installer::hook_block(port, T), "  ") + "\n";
        std::fs::write(&settings, block.replace("X-Bellwether-Token", "X-Pasture-Token")).unwrap();
        Options { from: None, env_from: None, settings, write: false, force: false, home: base.join("app"), user_home: user }
    }

    fn free_port() -> u16 {
        loop {
            let p = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            if p != DEFAULT_PORT {
                return p;
            }
        }
    }

    #[test]
    fn the_preview_lists_the_actions_and_changes_nothing() {
        let base = temp("preview");
        let port = free_port();
        let o = world(&base, port);
        let before = std::fs::read(&o.settings).unwrap();
        let mut h = Rec { calls: RefCell::new(vec![]), loaded: vec![("local.bellwether.brain".into(), Loaded::Running(4242))], holder: Holder::Free, register_fails: false };
        let out = to_app(&o, &mut h).unwrap();
        assert!(!out.wrote);
        let actions: Vec<&String> = out.lines.iter().filter(|l| l.starts_with("   - ")).collect();
        let a = |s: &str| actions.iter().any(|l| l.contains(s));
        assert!(a("copy the token from") && a("copy config.json from") && a("without brain.nodePath"), "{actions:#?}");
        assert!(a("stop local.bellwether.brain (launchctl bootout gui/501/local.bellwether.brain)"), "{actions:#?}");
        assert!(!a("stop local.bellwether.shell"), "a plist that isn't loaded is only moved");
        assert!(a("local.bellwether.shell.plist to"), "{actions:#?}");
        assert!(a("rewrite 16 pasture entries"), "{actions:#?}");
        let text = out.lines.join("\n");
        assert!(!text.contains(T), "the token is masked in the diff");
        assert!(text.contains("\"X-Bellwether-Token\": \"test…\""), "{text}");
        assert!(text.contains("Preview only"));
        assert!(h.calls.borrow().is_empty(), "a preview calls nothing: {:?}", h.calls.borrow());
        assert_eq!(std::fs::read(&o.settings).unwrap(), before);
        assert!(!o.home.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_held_port_stops_before_anything_changes() {
        let base = temp("held");
        let port = free_port();
        let mut o = world(&base, port);
        o.write = true;
        let mut h = Rec { calls: RefCell::new(vec![]), loaded: vec![("local.bellwether.brain".into(), Loaded::Running(4242))], holder: Holder::Other { pid: Some(777), command: Some("/usr/bin/python3 -m http.server".into()) }, register_fails: false };
        let e = to_app(&o, &mut h).err().unwrap();
        assert_eq!(e.code, exit::NOT_RUNNING);
        assert!(e.message.contains(&format!("port {port} is held by pid 777 (python3)")) && e.message.contains("the old setup keeps running"), "{}", e.message);
        assert!(h.calls.borrow().is_empty());
        assert!(!o.home.exists(), "nothing written");
        assert!(o.user_home.join("Library/LaunchAgents/local.bellwether.brain.plist").exists());
        // A brain that isn't the old agent's is refused too; the old agent's is not.
        let agents = old_agents(&o.user_home, None, &h);
        assert!(port_verdict(port, &Holder::Brain { name: "bellwether".into(), pid: Some(9) }, &agents, None).is_err());
        assert!(port_verdict(port, &Holder::Brain { name: "bellwether".into(), pid: Some(4242) }, &agents, None).unwrap().contains("the old brain"));
        assert!(port_verdict(port, &Holder::Brain { name: "bellwether".into(), pid: Some(9) }, &agents, Some(9)).unwrap().contains("this app's own"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_new_brain_that_never_answers_puts_the_old_setup_back() {
        let base = temp("rollback");
        let port = free_port();
        let mut o = world(&base, port);
        o.write = true;
        let settings_before = std::fs::read(&o.settings).unwrap();
        let mut h = Rec { calls: RefCell::new(vec![]), loaded: vec![("local.bellwether.brain".into(), Loaded::Running(4242)), ("local.bellwether.shell".into(), Loaded::Running(4243))], holder: Holder::Free, register_fails: false };
        let e = to_app(&o, &mut h).err().unwrap();
        assert!(e.message.contains("didn't work") && e.message.contains("back as they were"), "{}", e.message);
        let calls = h.calls.borrow().clone();
        assert_eq!(
            calls,
            [
                "launchctl bootout gui/501/local.bellwether.brain",
                "launchctl bootout gui/501/local.bellwether.shell",
                "register",
                "unregister",
                &format!("launchctl bootstrap gui/501 {}", o.user_home.join("Library/LaunchAgents/local.bellwether.brain.plist").display()),
                &format!("launchctl bootstrap gui/501 {}", o.user_home.join("Library/LaunchAgents/local.bellwether.shell.plist").display()),
            ]
        );
        assert!(o.user_home.join("Library/LaunchAgents/local.bellwether.brain.plist").exists());
        assert!(o.user_home.join("Library/LaunchAgents/local.bellwether.shell.plist").exists());
        assert_eq!(std::fs::read(&o.settings).unwrap(), settings_before, "the hooks were not touched");
        let r: Value = serde_json::from_str(&std::fs::read_to_string(record_file(&o.home)).unwrap()).unwrap();
        assert_eq!(r["plists"], json!([]), "the record no longer lists moved plists");
        // Approval pending: the same undo, at once.
        let mut h2 = Rec { calls: RefCell::new(vec![]), loaded: vec![], holder: Holder::Free, register_fails: true };
        let e = to_app(&o, &mut h2).err().unwrap();
        assert!(e.message.contains("waiting for your approval"), "{}", e.message);
        assert!(!h2.calls.borrow().contains(&"unregister".to_string()), "nothing was registered");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_swap_marker_is_there_only_during_step_4() {
        let base = temp("marker");
        let port = free_port();
        let mut o = world(&base, port);
        o.write = true;
        assert!(!swapping(&o.home));
        struct Watch(Rec, PathBuf, RefCell<Vec<bool>>);
        impl Host for Watch {
            fn acts(&self) -> bool { true }
            fn domain(&self) -> String { self.0.domain() }
            fn loaded(&self, l: &str) -> Loaded { self.0.loaded(l) }
            fn app_agent(&self) -> Option<i32> { None }
            fn launchctl(&mut self, a: &[String]) -> Result<(), String> { self.2.borrow_mut().push(swapping(&self.1)); self.0.launchctl(a) }
            fn register(&mut self) -> Result<Vec<String>, String> { self.2.borrow_mut().push(swapping(&self.1)); self.0.register() }
            fn unregister(&mut self) -> Result<Vec<String>, String> { self.0.unregister() }
            fn port_holder(&self, p: u16) -> Holder { self.0.port_holder(p) }
            fn process_path(&self, _: i32) -> Option<PathBuf> { None }
            fn terminate(&mut self, _: i32) -> bool { true }
            fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> { vec![] }
            fn verify_limit(&self) -> Duration { Duration::from_millis(200) }
        }
        let rec = Rec { calls: RefCell::new(vec![]), loaded: vec![("local.bellwether.brain".into(), Loaded::Running(4242))], holder: Holder::Free, register_fails: false };
        let mut h = Watch(rec, o.home.clone(), RefCell::new(vec![]));
        assert!(to_app(&o, &mut h).is_err(), "no brain answers on the free port: undone");
        assert_eq!(*h.2.borrow(), [true, true, true], "set while booting out, registering and (undoing) bootstrapping");
        assert!(!swapping(&o.home) && !swap_marker(&o.home).exists(), "gone afterwards, failure or not");
        // It counts only while its run is alive, and for a minute at most: a marker left by a
        // killed run never keeps the app from starting a brain (BR-3K's M8 D1).
        let me = std::process::id();
        std::fs::write(swap_marker(&o.home), format!("{} {me}", crate::now_ms())).unwrap();
        assert!(swapping(&o.home));
        std::fs::write(swap_marker(&o.home), format!("{} {me}", crate::now_ms() - 61_000)).unwrap();
        assert!(!swapping(&o.home), "too old");
        let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        std::fs::write(swap_marker(&o.home), format!("{} {dead}", crate::now_ms())).unwrap();
        assert!(!swapping(&o.home), "its run is gone");
        std::fs::write(swap_marker(&o.home), crate::now_ms().to_string()).unwrap();
        assert!(!swapping(&o.home), "an old-format marker without a pid doesn't count");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn rec(loaded: Vec<(String, Loaded)>) -> Rec {
        Rec { calls: RefCell::new(vec![]), loaded, holder: Holder::Free, register_fails: false }
    }

    #[test]
    fn undo_puts_back_only_what_the_migration_moved() {
        let base = temp("record");
        let o = world(&base, free_port());
        let agents = o.user_home.join("Library").join("LaunchAgents");
        let legacy = legacy_dir(&o.home);
        std::fs::create_dir_all(&legacy).unwrap();
        let victim = base.join("victim.txt");
        std::fs::write(&victim, "keep me").unwrap();
        let write = |r: Value| std::fs::write(record_file(&o.home), r.to_string()).unwrap();
        let entry = |label: &str, from: &Path, to: &Path| json!({ "version": 1, "plists": [{ "label": label, "from": from.display().to_string(), "to": to.display().to_string(), "loaded": true }] });
        for (why, r) in [
            ("an unknown label", entry("com.evil.agent", &agents.join("com.evil.agent.plist"), &legacy.join("com.evil.agent.plist"))),
            ("from outside LaunchAgents", entry("local.bellwether.brain", &base.join("local.bellwether.brain.plist"), &legacy.join("local.bellwether.brain.plist"))),
            ("to outside legacy/", entry("local.bellwether.brain", &agents.join("local.bellwether.brain.plist"), &victim)),
            ("a .. in the path", entry("local.bellwether.brain", &agents.join("..").join("local.bellwether.brain.plist"), &legacy.join("local.bellwether.brain.plist"))),
            ("another file name", entry("local.bellwether.brain", &agents.join("local.bellwether.brain.plist"), &legacy.join("other.plist"))),
            ("a settings file that isn't this one", json!({ "settings": { "file": victim.display().to_string(), "backup": o.home.join("backups/settings-x.json").display().to_string(), "after": "0" } })),
            ("a backup outside backups/", json!({ "settings": { "file": o.settings.display().to_string(), "backup": victim.display().to_string(), "after": "0" } })),
        ] {
            write(r);
            for w in [false, true] {
                let mut h = rec(vec![]);
                let e = undo(&Options { write: w, ..world_opts(&o) }, &mut h).err().unwrap_or_else(|| panic!("{why} was accepted"));
                assert!(e.message.contains("doesn't look like a record this migration wrote") && e.message.contains("nothing was changed"), "{why}: {}", e.message);
                assert!(h.calls.borrow().is_empty(), "{why}");
            }
        }
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        // A hub's own label is one of ours (deviation 85).
        assert!(known_label("local.example-hub.pasture.brain") && !known_label("local.bellwether.brainx") && !known_label("x/../.pasture.brain"));
        let _ = std::fs::remove_dir_all(&base);
    }

    fn world_opts(o: &Options) -> Options {
        Options { from: o.from.clone(), env_from: o.env_from.clone(), settings: o.settings.clone(), write: o.write, force: o.force, home: o.home.clone(), user_home: o.user_home.clone() }
    }

    #[test]
    #[cfg(unix)]
    fn a_dangling_symlink_plist_moves_and_comes_back() {
        let base = temp("dangling");
        let mut o = world(&base, free_port());
        let agents = o.user_home.join("Library").join("LaunchAgents");
        let plist = agents.join("local.bellwether.shell.plist");
        std::fs::remove_file(&plist).unwrap();
        std::os::unix::fs::symlink("../dotfiles/local.bellwether.shell.plist", &plist).unwrap();
        // A brain on the port takes the token, playing the app's agent the recorder registers.
        let bound = crate::bind(0).unwrap();
        let port = bound.port;
        std::fs::write(base.join("repo/config.json"), format!("{{\"port\": {port}}}")).unwrap();
        let opts = crate::Options::new(T.into(), crate::Mode::Normal, crate::Logger::stderr_only());
        std::thread::spawn(move || {
            let _ = crate::serve(bound, opts, None);
        });
        o.write = true;
        struct Own(Rec, i32);
        impl Host for Own {
            fn acts(&self) -> bool { true }
            fn domain(&self) -> String { self.0.domain() }
            fn loaded(&self, l: &str) -> Loaded { self.0.loaded(l) }
            fn app_agent(&self) -> Option<i32> { Some(self.1) }
            fn launchctl(&mut self, a: &[String]) -> Result<(), String> { self.0.launchctl(a) }
            fn register(&mut self) -> Result<Vec<String>, String> { self.0.register() }
            fn unregister(&mut self) -> Result<Vec<String>, String> { self.0.unregister() }
            fn port_holder(&self, p: u16) -> Holder { port_holder(p) }
            fn process_path(&self, _: i32) -> Option<PathBuf> { None }
            fn terminate(&mut self, _: i32) -> bool { true }
            fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> { vec![] }
        }
        let mut h = Own(rec(vec![]), std::process::id() as i32);
        to_app(&o, &mut h).unwrap();
        assert!(std::fs::symlink_metadata(legacy_dir(&o.home).join("local.bellwether.shell.plist")).unwrap().file_type().is_symlink());
        assert!(!present(&plist));
        let out = undo(&o, &mut h).unwrap();
        assert!(out.lines.last().unwrap().starts_with("Done."), "{:?}", out.lines);
        assert_eq!(std::fs::read_link(&plist).unwrap(), PathBuf::from("../dotfiles/local.bellwether.shell.plist"), "the link itself is back");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn one_migration_at_a_time() {
        let base = temp("lock");
        let home = base.join("app");
        // A run holds the lock: a second one is refused, naming it.
        let first = Lock::take(&home).unwrap();
        assert_eq!(std::fs::read_to_string(legacy_dir(&home).join("migrate.lock")).unwrap(), std::process::id().to_string());
        let e = Lock::take(&home).err().unwrap();
        assert_eq!(e.code, exit::NOT_RUNNING);
        assert!(e.message.contains(&format!("another migration is running (pid {},", std::process::id())), "{}", e.message);
        // Released when the run ends (dropped here; the kernel does it for a killed run).
        drop(first);
        let again = Lock::take(&home).unwrap();
        drop(again);
        // A file left by a dead run (its pid, or garbage) isn't a lock at all.
        std::fs::write(legacy_dir(&home).join("migrate.lock"), "999999999 garbage").unwrap();
        let taken = Lock::take(&home).unwrap();
        assert_eq!(std::fs::read_to_string(legacy_dir(&home).join("migrate.lock")).unwrap(), std::process::id().to_string(), "rewritten with this run's pid");
        drop(taken);
        // No check-then-remove: the takeover is the kernel's (BR-3K's re-check R2).
        let src = include_str!("toapp.rs");
        let take = src.split("pub fn take(home: &Path)").nth(1).unwrap().split("\n    }\n}").next().unwrap();
        assert!(take.contains("libc::flock") && !take.contains("remove_file"), "the lock is flock, never removed and recreated");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_failed_copy_stops_before_any_agent_is_touched() {
        let base = temp("copyfail");
        let mut o = world(&base, free_port());
        std::fs::create_dir_all(base.join("repo/data")).unwrap();
        std::fs::write(base.join("repo/data/installs.json"), "{}").unwrap();
        // installs.json can't be written: a directory stands where the file goes.
        std::fs::create_dir_all(o.home.join("installs.json").join("x")).unwrap();
        o.force = true;
        o.write = true;
        let mut h = rec(vec![("local.bellwether.brain".into(), Loaded::Running(4242))]);
        let e = to_app(&o, &mut h).err().unwrap();
        assert_eq!(e.code, exit::UNEXPECTED);
        assert!(e.message.contains("Stopped at step 2") && e.message.contains("no agent, login item or hook entry was touched"), "{}", e.message);
        assert!(h.calls.borrow().is_empty(), "{:?}", h.calls.borrow());
        assert!(o.user_home.join("Library/LaunchAgents/local.bellwether.brain.plist").exists());
        drop(Lock::take(&o.home).expect("the lock is released"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_app_token_gives_way_to_the_one_running_sessions_carry() {
        let base = temp("token");
        let o = world(&base, free_port());
        std::fs::create_dir_all(&o.home).unwrap();
        let h = rec(vec![]);
        tok::write(&o.home, "appTokenDifferent_0123456789abcdefghij").unwrap();
        let s = survey(&o, &h).unwrap();
        assert!(s.token_replaces && matches!(&s.token, Some((t, TokenFrom::EnvFile(_))) if t == T));
        assert!(!s.nothing());
        let text = preview(&o, &s, &h).join("\n");
        assert!(text.contains("- replace the app's own hook token") && text.contains("would be refused (403)"), "{text}");
        // The same token: kept, nothing replaced.
        tok::write(&o.home, T).unwrap();
        let s = survey(&o, &h).unwrap();
        assert!(!s.token_replaces && matches!(&s.token, Some((_, TokenFrom::Kept))));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A recorder whose `app_agent` is this process: an in-process brain plays the app's agent.
    struct AppIsMe(Rec);
    impl Host for AppIsMe {
        fn acts(&self) -> bool { true }
        fn domain(&self) -> String { self.0.domain() }
        fn loaded(&self, l: &str) -> Loaded { self.0.loaded(l) }
        fn app_agent(&self) -> Option<i32> { Some(std::process::id() as i32) }
        fn launchctl(&mut self, a: &[String]) -> Result<(), String> { self.0.launchctl(a) }
        fn register(&mut self) -> Result<Vec<String>, String> { self.0.register() }
        fn unregister(&mut self) -> Result<Vec<String>, String> { self.0.unregister() }
        fn port_holder(&self, p: u16) -> Holder { port_holder(p) }
        fn process_path(&self, _: i32) -> Option<PathBuf> { None }
        fn terminate(&mut self, _: i32) -> bool { true }
        fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> { vec![] }
    }

    /// BR-3K's re-check R1 (its `recheck.sh sim`, section sim2): a run killed after it moved the
    /// brain plist and before the record listed it, on a port that isn't the default. The agents
    /// are booted out, the record says only `swap: started`, the brain plist sits in legacy/
    /// and the shell plist is still in LaunchAgents. The rerun finds the repository through the
    /// moved plist (its token and port) and finishes; undo, run instead, puts both back.
    #[test]
    fn a_run_killed_between_the_plist_moves_is_recovered_by_a_rerun_or_an_undo() {
        let sim = |name: &str| -> (PathBuf, Options, u16) {
            let base = temp(name);
            let bound = crate::bind(0).unwrap();
            let port = bound.port;
            let mut o = world(&base, port);
            assert_ne!(port, DEFAULT_PORT);
            let opts = crate::Options::new(T.into(), crate::Mode::Normal, crate::Logger::stderr_only());
            std::thread::spawn(move || {
                let _ = crate::serve(bound, opts, None);
            });
            let agents = o.user_home.join("Library/LaunchAgents");
            std::fs::create_dir_all(legacy_dir(&o.home)).unwrap();
            std::fs::rename(agents.join("local.bellwether.brain.plist"), legacy_dir(&o.home).join("local.bellwether.brain.plist")).unwrap();
            std::fs::write(record_file(&o.home), "{\"version\": 1, \"swap\": \"started\"}\n").unwrap();
            o.write = false;
            (base, o, port)
        };
        let brain_back = |o: &Options| std::fs::read_to_string(o.user_home.join("Library/LaunchAgents/local.bellwether.brain.plist")).unwrap();

        // The rerun.
        let (base, mut o, port) = sim("r1-rerun");
        let original = std::fs::read_to_string(legacy_dir(&o.home).join("local.bellwether.brain.plist")).unwrap();
        let mut h = AppIsMe(rec(vec![]));
        let p = to_app(&o, &mut h).unwrap();
        let text = p.lines.join("\n");
        assert!(!text.starts_with("Nothing to migrate") && text.contains("didn't finish"), "{text}");
        assert!(text.contains(&format!("Earlier install: {}", base.join("repo").display())), "found through the moved plist: {text}");
        assert!(text.contains(&format!("3. Port {port}")), "{text}");
        o.write = true;
        let done = to_app(&o, &mut h).unwrap().lines.join("\n");
        assert!(done.contains("recorded") && done.contains("the new brain answers"), "{done}");
        let r: Value = serde_json::from_str(&std::fs::read_to_string(record_file(&o.home)).unwrap()).unwrap();
        let labels: Vec<&str> = r["plists"].as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
        assert_eq!(labels, ["local.bellwether.brain", "local.bellwether.shell"], "both recorded");
        assert_eq!(r["swap"], "done");
        o.write = false;
        assert!(to_app(&o, &mut h).unwrap().lines[0].starts_with("Nothing to migrate"));
        o.write = true;
        undo(&o, &mut h).unwrap();
        assert_eq!(brain_back(&o), original, "undo puts the brain plist back too");
        assert!(o.user_home.join("Library/LaunchAgents/local.bellwether.shell.plist").exists());
        let _ = std::fs::remove_dir_all(&base);

        // Undo straight from the stopped state.
        let (base, mut o, _) = sim("r1-undo");
        let mut h = AppIsMe(rec(vec![]));
        let preview = undo(&o, &mut h).unwrap().lines.join("\n");
        assert!(preview.contains("- move") && preview.contains("local.bellwether.brain.plist back"), "{preview}");
        o.write = true;
        let out = undo(&o, &mut h).unwrap().lines.join("\n");
        assert!(out.contains("Done."), "{out}");
        assert!(brain_back(&o).contains("local.bellwether.brain"));
        assert!(!legacy_dir(&o.home).join("local.bellwether.brain.plist").exists());
        assert!(undo(&o, &mut h).unwrap().lines[0].starts_with("Nothing to undo"));
        let _ = std::fs::remove_dir_all(&base);

        // The order that prevents it: the record names each move before the rename, and step 5's
        // rewrite before the file is written.
        let src = include_str!("toapp.rs");
        let body = src.split("fn swap_inner(").nth(1).unwrap();
        assert!(body.find("list.push(entry.clone())").unwrap() < body.find("match move_file(p, &to)").unwrap());
        let apply = src.split("\nfn apply(").nth(1).unwrap();
        assert!(apply.find("r[\"settings\"] = json!").unwrap() < apply.find("installer::apply_plan(").unwrap());
    }

    #[test]
    fn nothing_found_is_nothing_to_migrate() {
        let base = temp("nothing");
        let o = Options { from: None, env_from: None, settings: base.join("settings.json"), write: true, force: false, home: base.join("app"), user_home: base.join("user") };
        let mut h = Rec { calls: RefCell::new(vec![]), loaded: vec![], holder: Holder::Free, register_fails: false };
        let out = to_app(&o, &mut h).unwrap();
        assert!(!out.wrote);
        assert!(out.lines[0].starts_with("Nothing to migrate"), "{:?}", out.lines);
        assert!(!o.home.exists());
        let u = undo(&o, &mut h).unwrap();
        assert!(u.lines[0].starts_with("Nothing to undo"));
        assert!(h.calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stray_strip_is_only_one_of_that_checkout() {
        struct P(Option<PathBuf>);
        impl Host for P {
            fn acts(&self) -> bool {
                false
            }
            fn domain(&self) -> String {
                "gui/501".into()
            }
            fn loaded(&self, _: &str) -> Loaded {
                Loaded::NotAsked
            }
            fn app_agent(&self) -> Option<i32> {
                None
            }
            fn launchctl(&mut self, _: &[String]) -> Result<(), String> {
                unreachable!()
            }
            fn register(&mut self) -> Result<Vec<String>, String> {
                unreachable!()
            }
            fn unregister(&mut self) -> Result<Vec<String>, String> {
                unreachable!()
            }
            fn port_holder(&self, _: u16) -> Holder {
                Holder::Free
            }
            fn process_path(&self, _: i32) -> Option<PathBuf> {
                self.0.clone()
            }
            fn terminate(&mut self, _: i32) -> bool {
                unreachable!()
            }
            fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> {
                vec![]
            }
        }
        let base = temp("stray");
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("data")).unwrap();
        std::fs::create_dir_all(repo.join("shell/src-tauri/target/release")).unwrap();
        std::fs::write(repo.join("shell/src-tauri/target/release/bellwether-shell"), "").unwrap();
        std::fs::write(repo.join("data/shell.pid"), "4321\n").unwrap();
        let src = migrate::source_of(&repo).unwrap();
        assert_eq!(stray_strip(&src, &P(Some(repo.join("shell/src-tauri/target/release/bellwether-shell")))), Some(4321));
        assert_eq!(stray_strip(&src, &P(Some(PathBuf::from("/elsewhere/target/release/bellwether-shell")))), None, "another checkout's strip");
        assert_eq!(stray_strip(&src, &P(Some(repo.join("shell/src-tauri/target/release/other")))), None, "not a strip");
        assert_eq!(stray_strip(&src, &P(None)), None, "not running");
        std::fs::write(repo.join("data/shell.pid"), "1\n").unwrap();
        assert_eq!(stray_strip(&src, &P(Some(repo.join("shell/src-tauri/target/release/bellwether-shell")))), None, "never pid 1");
        let _ = std::fs::remove_dir_all(&base);
    }
}
