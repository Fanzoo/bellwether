//! The onboarding window (distribution plan D6, milestone 7): a small, normal window
//! (label `onboarding`) that takes a new user from first launch to a sheep without a
//! terminal. It opens on the app's first launch (until its Done is clicked), from the
//! tray's "Set up Bellwether…", and from `open -b <id> --args --onboarding`.
//!
//! Its webview gets its own capability (capabilities/onboarding.json) with exactly the
//! eleven commands below, and each command checks the calling window's label again here,
//! so the strip's webview can't reach them even if a capability were widened by mistake.
//! Arguments are enums or opaque ids: never paths, file contents or tokens. The token is
//! masked in everything returned.
//!
//! Step 1 (milestone 8) is the guided move of an earlier install (D9): the same preview
//! `bellwether migrate --to-app` prints, then the move on Confirm, through
//! `bellwether_brain::toapp` on this app's host (`migration.rs`). For those users it
//! replaces steps 2–3, and while an earlier install's LaunchAgents are there, start at
//! login and installing hooks stay refused (deviation 63): two brains would fight over one
//! port, and the app's hooks would carry a token the old brain refuses.
//!
//! Bellwether.app only. A repository debug build opens it only when asked (`--onboarding`),
//! to test the window; a repository release build (the strip a repository LaunchAgent
//! runs) never does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use bellwether_core::hooks::{self as installer, Action, Plan};
use bellwether_core::lossless::Lj;
use bellwether_core::token as tok;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::log::log;
use crate::login_items::{self, Service, Status};
pub use crate::settings::Layout;

/// The window's label: the one window the onboarding capability and [`guard`] accept.
pub const LABEL: &str = "onboarding";

/// The eleven commands of D6's table, in its order. The capability test checks that
/// capabilities/onboarding.json grants exactly these, and build.rs declares them.
#[cfg(test)]
pub const COMMANDS: [&str; 11] = [
    "setup_status",
    "hooks_preview",
    "hooks_apply",
    "login_items_set",
    "permission_request",
    "pet_select",
    "pet_import",
    "migration_preview",
    "migration_apply",
    "cli_tool_install",
    "setup_close",
];

/// The second wall: a command answers only the onboarding window.
pub fn guard(label: &str) -> Result<(), String> {
    if label == LABEL {
        Ok(())
    } else {
        Err(format!("refused: the {label:?} window can't use the onboarding commands"))
    }
}

/// Where onboarding works, fixed when the app starts.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub layout: Layout,
    /// The app's files (or a repository's `data/`): hook-token, config.json, installs.json,
    /// backups/, pets/, shell-state.json.
    pub home: PathBuf,
    pub port: u16,
    /// `Contents/Resources` (bundled pets).
    pub resources: Option<PathBuf>,
    /// The user's home: `~/.claude/settings.json`, `~/.local/bin`, `~/Library/LaunchAgents`.
    pub user_home: PathBuf,
    /// BELLWETHER_CONFIG: a read-only config file in place of `<home>/config.json`.
    pub env_config: Option<String>,
    /// BELLWETHER_HOOK_TOKEN: a token from the environment (tests); never written anywhere.
    pub env_token: Option<String>,
}

impl Ctx {
    /// The settings file the window edits: the user's, as `bellwether hooks install` edits it.
    pub fn settings_file(&self) -> PathBuf {
        self.user_home.join(".claude").join("settings.json")
    }
}

/// Whether this layout may open the window at all.
pub fn allowed(layout: Layout) -> bool {
    layout == Layout::App || cfg!(debug_assertions)
}

/// The token a planned install will write: one already in use, or one made for this plan
/// (a new user's first install; written only when the plan is applied).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PlanToken {
    Existing(String),
    New(String),
    /// An uninstall carries none.
    NotNeeded,
}

struct Stored {
    plan: Plan,
    token: PlanToken,
}

/// Makes the host the migration runs on (the app's own; tests give a recorder).
pub type HostFactory = Box<dyn Fn(&Ctx) -> Result<Box<dyn bellwether_brain::toapp::Host + Send>, String> + Send + Sync>;

pub struct Onboarding {
    pub ctx: Ctx,
    host: HostFactory,
    /// The current migration preview: its id, and what it was worked out from.
    migration_plan: Mutex<Option<(String, String)>>,
    /// The one current preview: a new preview replaces it, and applying takes it.
    plans: Mutex<HashMap<String, Stored>>,
    next: AtomicU64,
    /// The Accessibility prompt was shown once already in this run: a second request
    /// opens the Settings pane instead (macOS shows its prompt only once per app).
    prompted_ax: AtomicBool,
}

impl Onboarding {
    pub fn new(ctx: Ctx) -> Onboarding {
        Onboarding::with_host(ctx, Box::new(|c: &Ctx| crate::migration::host_for(&c.user_home)))
    }

    pub fn with_host(ctx: Ctx, host: HostFactory) -> Onboarding {
        Onboarding { ctx, host, migration_plan: Mutex::new(None), plans: Mutex::new(HashMap::new()), next: AtomicU64::new(1), prompted_ax: AtomicBool::new(false) }
    }

    // ------------------------------------------------------------ the token

    /// The usable token, None when there is none yet; an error when one is there but
    /// can't be used (the user's to fix: `token init --force`).
    fn token_now(&self) -> Result<Option<String>, String> {
        match self.ctx.layout {
            Layout::App => match tok::read(&self.ctx.home, self.ctx.env_token.clone()) {
                Ok((t, _)) => Ok(Some(t)),
                Err(e) => {
                    let env_set = self.ctx.env_token.as_deref().is_some_and(|t| !t.is_empty());
                    match tok::read_file(&self.ctx.home) {
                        Ok(None) if !env_set => Ok(None),
                        _ => Err(e.message),
                    }
                }
            },
            // The repository's token lives in its .env; onboarding never makes one there.
            Layout::Repository => crate::settings::token_now().map(Some).ok_or_else(|| "no hook token: this is a repository build, whose token is in the checkout's .env (`./bellwether token init`)".to_string()),
        }
    }

    /// Makes the token file if there is no token yet (the app only). Returns whether it did.
    fn ensure_token(&self) -> Result<bool, String> {
        if self.token_now()?.is_some() {
            return Ok(false);
        }
        if self.ctx.layout != Layout::App {
            return Err("no hook token".into());
        }
        let t = tok::generate().map_err(|e| e.message)?;
        tok::write(&self.ctx.home, &t).map_err(|e| e.message)?;
        log(&format!("onboarding: made the hook token ({})", tok::file(&self.ctx.home).display()));
        Ok(true)
    }

    // ------------------------------------------------------------ an earlier install

    /// The repository version's LaunchAgents still on this Mac (D9's scan; launchd asked
    /// only when HOME is the real home).
    pub fn earlier_install(&self) -> Vec<bellwether_brain::legacy::Agent> {
        bellwether_brain::legacy::scan_here(&self.ctx.user_home)
    }

    fn refuse_while_earlier(&self, what: &str) -> Result<(), String> {
        let found = self.earlier_install();
        if found.is_empty() {
            return Ok(());
        }
        Err(format!(
            "refused: {what} waits until the earlier Bellwether on this Mac ({}) is moved into the app. Its brain uses the same port, so two brains would fight over it, and its hooks would get a token it refuses. Run `bellwether doctor` in a terminal for the details",
            bellwether_brain::legacy::summary(&found)
        ))
    }

    // ------------------------------------------------------------ step 1: the move (D9)

    fn migration_options(&self, write: bool) -> bellwether_brain::toapp::Options {
        bellwether_brain::toapp::Options { from: None, env_from: None, settings: self.ctx.settings_file(), write, force: false, home: self.ctx.home.clone(), user_home: self.ctx.user_home.clone() }
    }

    /// What a preview was worked out from: if any of it differs at Confirm, the user sees a
    /// fresh preview instead (as with the hooks).
    fn migration_fingerprint(&self, s: &bellwether_brain::toapp::Survey) -> String {
        let settings = std::fs::read(self.ctx.settings_file()).map(|b| installer::sha256_hex(&b)).unwrap_or_default();
        let agents: Vec<String> = s.agents.iter().map(|a| format!("{}:{}:{:?}", a.label, a.plist.is_some(), a.loaded)).collect();
        let copy: Vec<String> = s.copy.iter().map(|c| format!("{c:?}").chars().take(200).collect()).collect();
        format!("{settings}|{}|{:?}|{}|{:?}|{}", agents.join(","), s.token.as_ref().map(|(_, f)| f), copy.join(","), s.source.as_ref().map(|x| &x.data_dir), s.port)
    }

    /// D6 step 1's preview: `bellwether migrate --to-app`'s, token masked. Kept on the Rust
    /// side under a one-use id.
    pub fn migration_preview(&self) -> Result<Value, String> {
        if self.ctx.layout != Layout::App {
            return Err("moving an earlier install needs Bellwether.app (this is a repository build)".into());
        }
        let host = (self.host)(&self.ctx)?;
        let o = self.migration_options(false);
        let s = bellwether_brain::toapp::survey(&o, host.as_ref()).map_err(|e| e.message)?;
        let lines = bellwether_brain::toapp::preview(&o, &s, host.as_ref());
        let token = s.token.as_ref().map(|(t, _)| t.clone());
        let text = bellwether_core::mask::mask_alone(&lines.join("\n"), token.as_deref());
        if s.nothing() {
            *self.migration_plan.lock().unwrap() = None;
            return Ok(json!({ "found": false, "planId": Value::Null, "text": text, "message": "nothing to migrate: no earlier Bellwether on this Mac" }));
        }
        let id = format!("move-{}-{}", self.next.fetch_add(1, Ordering::SeqCst), &tok::generate().map_err(|e| e.message)?[..8]);
        *self.migration_plan.lock().unwrap() = Some((id.clone(), self.migration_fingerprint(&s)));
        log(&format!("onboarding: migration preview: {} agent(s), {} file(s) to copy", s.agents.len(), s.copy.len()));
        Ok(json!({ "found": true, "planId": id, "text": text, "agents": s.agents.iter().map(|a| a.label.clone()).collect::<Vec<_>>() }))
    }

    /// D6 step 1's Confirm: runs the previewed migration, only if nothing it was worked out
    /// from changed since (otherwise a fresh preview comes back, and nothing is done).
    pub fn migration_apply(&self, id: &str) -> Result<Value, String> {
        // Only the current preview's id takes it; a stale id leaves the current one in place.
        let fingerprint = {
            let mut plan = self.migration_plan.lock().unwrap();
            match plan.as_ref() {
                Some((current, _)) if current == id => plan.take().unwrap().1,
                _ => return Err("that preview is no longer the current one; show what moves again".into()),
            }
        };
        if bellwether_brain::migrate::is_real_home(&self.ctx.user_home) {
            if let Some(why) = crate::migration::real_write_refusal() {
                return Err(why);
            }
        }
        let mut host = (self.host)(&self.ctx)?;
        let o = self.migration_options(true);
        let s = bellwether_brain::toapp::survey(&o, host.as_ref()).map_err(|e| e.message)?;
        if self.migration_fingerprint(&s) != fingerprint {
            log("onboarding: migration: refused, something changed since the preview");
            return Ok(json!({ "outcome": "changed", "message": "something changed since the preview (the settings file, the old agents or the files to copy), so nothing was done. Here is a fresh preview; check it and confirm again.", "preview": self.migration_preview()? }));
        }
        let token = s.token.as_ref().map(|(t, _)| t.clone());
        let mask = |t: &str| bellwether_core::mask::mask_alone(t, token.as_deref());
        match bellwether_brain::toapp::to_app(&o, host.as_mut()) {
            Ok(out) => {
                log("onboarding: migration: done");
                Ok(json!({ "outcome": "applied", "text": mask(&out.lines.join("\n")) }))
            }
            Err(e) => {
                log(&format!("onboarding: migration: stopped: {}", e.message.lines().next().unwrap_or("")));
                Ok(json!({ "outcome": "failed", "text": mask(&e.message) }))
            }
        }
    }

    /// Whether step 1 has anything to offer: the old LaunchAgents, or anything else of an
    /// earlier install the migration would move (hook entries alone, D9).
    fn migration_found(&self, earlier: &[bellwether_brain::legacy::Agent]) -> bool {
        if !earlier.is_empty() {
            return true;
        }
        if self.ctx.layout != Layout::App {
            return false;
        }
        let Ok(host) = (self.host)(&self.ctx) else { return false };
        bellwether_brain::toapp::survey(&self.migration_options(false), host.as_ref()).is_ok_and(|s| s.found())
    }

    // ------------------------------------------------------------ hooks

    /// D6 step 3's preview: the diff `bellwether hooks install` (or `uninstall`) prints for
    /// the same file, token masked, kept here with the file's sha256 under a fresh id.
    pub fn hooks_preview(&self, action: Action) -> Result<Value, String> {
        if action == Action::Install {
            self.refuse_while_earlier("installing hooks")?;
        }
        let token = match action {
            Action::Install => match self.token_now()? {
                Some(t) => PlanToken::Existing(t),
                None => PlanToken::New(tok::generate().map_err(|e| e.message)?),
            },
            Action::Uninstall => PlanToken::NotNeeded,
        };
        let token_str = match &token {
            PlanToken::Existing(t) | PlanToken::New(t) => t.clone(),
            PlanToken::NotNeeded => String::new(),
        };
        let file = self.ctx.settings_file();
        let plan = installer::plan_edit(&file, action, self.ctx.port, &token_str, Some(&self.ctx.home)).map_err(|e| e.message)?;
        // The token to mask: the plan's, or (uninstall) the one in use.
        let mask_with = match &token {
            PlanToken::NotNeeded => self.token_now().ok().flatten(),
            _ => Some(token_str.clone()),
        };
        let mask = |t: &str| bellwether_core::mask::mask_alone(t, mask_with.as_deref());
        let before = installer::sum_groups(&installer::count_groups(&plan.before, self.ctx.port));
        let after = installer::sum_groups(&installer::count_groups(&plan.after, self.ctx.port));
        let mut warnings: Vec<String> = plan.warnings.iter().map(|w| mask(w)).collect();
        if plan.changed && !plan.layout_kept {
            warnings.push("this file isn't laid out the way Bellwether writes JSON, so installing also reformats existing lines (every value stays the same; the diff shows it, and a backup keeps the original)".into());
        }
        let mut out = json!({
            "action": action.word(),
            "file": file.display().to_string(),
            "changed": plan.changed,
            "groupsBefore": before,
            "groupsAfter": after,
            "layoutKept": plan.layout_kept,
            "othersUntouched": other_handlers(&plan.after, self.ctx.port),
            "warnings": warnings,
            "newToken": matches!(token, PlanToken::New(_)),
            "planId": Value::Null,
            "diffText": "",
        });
        if !plan.changed {
            out["message"] = Value::from(match action {
                Action::Install => format!("already installed, no changes ({after} bellwether matcher groups in {})", file.display()),
                Action::Uninstall => format!("nothing to remove: no bellwether entries in {}", file.display()),
            });
            self.plans.lock().unwrap().clear();
            return Ok(out);
        }
        out["diffText"] = Value::from(mask(&bellwether_brain::cli::hook_diff(&plan)));
        out["summary"] = Value::from(mask(&bellwether_brain::cli::hook_summary(&plan, self.ctx.port, &self.ctx.home.join("backups"))));
        let id = format!("plan-{}-{}", self.next.fetch_add(1, Ordering::SeqCst), &tok::generate().map_err(|e| e.message)?[..8]);
        let mut plans = self.plans.lock().unwrap();
        plans.clear();
        plans.insert(id.clone(), Stored { plan, token });
        out["planId"] = Value::from(id);
        Ok(out)
    }

    /// Applies the stored plan `id`, and only it. When the settings file (or the token)
    /// changed since the preview, nothing is written and a fresh preview comes back.
    pub fn hooks_apply(&self, id: &str) -> Result<Value, String> {
        let Some(Stored { mut plan, token }) = self.plans.lock().unwrap().remove(id) else {
            return Err("that preview is no longer the current one; show the changes again".into());
        };
        let action = plan.action;
        if action == Action::Install {
            self.refuse_while_earlier("installing hooks")?;
        }
        let file = plan.file.clone();
        let now = match std::fs::read(&file) {
            Ok(b) => Some(installer::sha256_hex(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("cannot read {}: {}", file.display(), bellwether_core::config::io_code(&e))),
        };
        let changed = |why: String| -> Result<Value, String> {
            log(&format!("onboarding: hooks {}: refused, {why}", action.word()));
            Ok(json!({ "outcome": "changed", "message": format!("{why}, so nothing was written. Here is a fresh preview; check it and confirm again."), "preview": self.hooks_preview(action)? }))
        };
        if now != plan.original_hash {
            return changed(format!("{} changed since the preview", file.display()));
        }
        let current = self.token_now();
        match &token {
            PlanToken::Existing(t) if current.as_ref().ok().and_then(|c| c.as_ref()) != Some(t) => return changed("the hook token changed since the preview".into()),
            PlanToken::New(_) if !matches!(current, Ok(None)) => return changed("a hook token was made since the preview".into()),
            _ => {}
        }
        if let PlanToken::New(t) = &token {
            tok::write(&self.ctx.home, t).map_err(|e| e.message)?;
            log(&format!("onboarding: made the hook token ({})", tok::file(&self.ctx.home).display()));
        }
        let shown = plan.warnings.len();
        match installer::apply_plan(&mut plan, &self.ctx.home.join("backups"), Some(&self.ctx.home), &bellwether_brain::cli::file_stamp(bellwether_brain::now_ms())) {
            Ok(backup) => {
                let mask_with = match &token {
                    PlanToken::Existing(t) | PlanToken::New(t) => Some(t.clone()),
                    PlanToken::NotNeeded => self.token_now().ok().flatten(),
                };
                let mask = |t: &str| bellwether_core::mask::mask_alone(t, mask_with.as_deref());
                let summary = mask(&format!("written: {}", bellwether_brain::cli::hook_summary(&plan, self.ctx.port, &self.ctx.home.join("backups"))));
                log(&format!("onboarding: hooks {}: {summary}", action.word()));
                Ok(json!({
                    "outcome": "applied",
                    "summary": summary,
                    "backup": backup.map(|b| b.display().to_string()),
                    "warnings": plan.warnings[shown..].iter().map(|w| mask(w)).collect::<Vec<_>>(),
                }))
            }
            // apply_plan's own last look found a change: the same answer as above.
            Err(e) if e.message.contains("changed while bellwether was editing it") => changed(format!("{} changed while it was being written", file.display())),
            Err(e) => Err(e.message),
        }
    }

    // ------------------------------------------------------------ login items

    /// D6 step 2: registers (or unregisters) both login items, brain first. Registering
    /// makes the hook token first if there is none: the brain agent reads only its file.
    pub fn login_items_set(&self, enabled: bool) -> Result<Value, String> {
        if self.ctx.layout != Layout::App {
            return Err("start at login needs Bellwether.app (this is a repository build)".into());
        }
        if enabled {
            self.refuse_while_earlier("turning on start at login")?;
            self.ensure_token()?;
        }
        let mut lines = Vec::new();
        for s in [Service::Brain, Service::MainApp] {
            let r = if enabled { login_items::register(s) } else { login_items::unregister(s) };
            let line = match r {
                Ok(st) => login_items::advice(s, st),
                Err(e) => format!("{}: {e}", s.name()),
            };
            log(&format!("onboarding: {line}"));
            lines.push(line);
        }
        Ok(json!({ "lines": lines, "login": login_json() }))
    }

    // ------------------------------------------------------------ permissions

    /// D6 steps 4 and 5 (and the Login Items pane for an item waiting for approval).
    pub fn permission_request(&self, kind: &str) -> Result<Value, String> {
        let (ok, message) = match kind {
            "accessibility" => {
                if crate::ax::trusted() {
                    (true, "Accessibility is already allowed: sheep stand on the Dock".to_string())
                } else if !self.prompted_ax.swap(true, Ordering::SeqCst) {
                    crate::ax::prompt();
                    (true, "macOS is asking: choose Open System Settings and turn Bellwether on under Accessibility. The sheep move onto the Dock within ten seconds".to_string())
                } else {
                    open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
                    (true, "opened System Settings > Privacy & Security > Accessibility: turn Bellwether on there".to_string())
                }
            }
            "automation-iterm2" => crate::actions::automation("com.googlecode.iterm2", "iTerm2"),
            "automation-terminal" => crate::actions::automation("com.apple.Terminal", "Terminal"),
            "login-items" => {
                login_items::open_settings();
                (true, "opened System Settings > General > Login Items & Extensions: turn Bellwether on under \"Allow in the Background\"".to_string())
            }
            other => return Err(format!("unknown permission {other:?}")),
        };
        log(&format!("onboarding: permission {kind}: {message}"));
        Ok(json!({ "ok": ok, "message": message }))
    }

    // ------------------------------------------------------------ pets

    /// Every pet the strip could draw: Bellwether's own (bundled, pinned by the compiled-in
    /// lock) first, in their fixed order (plan D8), then imported ones (pinned in
    /// pets.user.lock.json), or a repository's fetched ones; each with a still to show.
    /// `tint` is the first palette colour: the window tints each still's helmet with it, so
    /// no one sees the mask's magenta.
    pub fn pets(&self, config: &Value) -> Value {
        let mut list = Vec::new();
        // The app's Resources, or a repository's checkout: both keep bundled pets in pets/.
        if let Some(res) = &self.ctx.resources {
            for name in bellwether_brain::pets::BUNDLED {
                let bytes = std::fs::read(res.join("pets").join(name).join("animations.xml")).ok();
                if let Some(b) = bytes.filter(|b| crate::verify_pet(name, b).is_ok()) {
                    list.push(pet_entry(name, "bundled", &b));
                }
            }
        }
        match self.ctx.layout {
            Layout::App => {
                for name in bellwether_brain::pets::imported_names(&self.ctx.home) {
                    if bellwether_brain::pets::reserved(&name) || list.iter().any(|p: &Value| p["name"] == name.as_str()) {
                        continue;
                    }
                    match bellwether_brain::pets::load(&self.ctx.home, &name) {
                        Ok(b) => list.push(pet_entry(&name, "imported", &b)),
                        Err(e) => list.push(json!({ "name": name, "kind": "imported", "problem": e })),
                    }
                }
            }
            Layout::Repository => {
                for name in dir_names(&self.ctx.home.join("pets")) {
                    if bellwether_brain::pets::reserved(&name) {
                        continue;
                    }
                    let bytes = std::fs::read(self.ctx.home.join("pets").join(&name).join("animations.xml")).ok();
                    if let Some(b) = bytes.filter(|b| crate::verify_pet(&name, b).is_ok()) {
                        list.push(pet_entry(&name, "fetched", &b));
                    }
                }
            }
        }
        let current = config.pointer("/pets/default").and_then(Value::as_str).unwrap_or(bellwether_brain::pets::DEFAULT_PET).to_string();
        let tint = crate::sheep_palette(config.pointer("/pets/palette")).get(0).cloned().unwrap_or(Value::Null);
        json!({ "current": current, "list": list, "editable": self.config_editable().is_ok(), "tint": tint })
    }

    /// config.json can be changed here: the app's own file, not BELLWETHER_CONFIG's (read
    /// only) and not a repository's.
    fn config_editable(&self) -> Result<PathBuf, String> {
        if self.ctx.layout != Layout::App {
            return Err("this is a repository build: set pets.default in the checkout's config.json".into());
        }
        if self.ctx.env_config.as_deref().is_some_and(|c| !c.is_empty()) {
            return Err("BELLWETHER_CONFIG names another config file (read only), so the pet can't be changed here".into());
        }
        Ok(self.ctx.home.join("config.json"))
    }

    /// D6 step 6: makes `name` the strip's pet, in config.json's `pets.default`. An imported
    /// pet with no `pets.animationMap.<name>` gets the map `pets import` proposes, so it
    /// walks (deviation 64). Returns the config as the strip should now use it.
    pub fn pet_select(&self, name: &str, config: &Value) -> Result<Value, String> {
        let pets = self.pets(config);
        let entry = pets["list"].as_array().and_then(|l| l.iter().find(|p| p["name"] == name)).cloned();
        let Some(entry) = entry.filter(|e| e.get("problem").is_none()) else {
            return Err(format!("{name:?} isn't a pet Bellwether can load (bundled or imported)"));
        };
        let file = self.config_editable()?;
        let old = match std::fs::read_to_string(&file) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("cannot read {}: {}", file.display(), bellwether_core::config::io_code(&e))),
        };
        let map = (entry["kind"] == "imported" && config.pointer(&format!("/pets/animationMap/{name}")).is_none())
            .then(|| bellwether_brain::pets::load(&self.ctx.home, name).ok().and_then(|b| bellwether_brain::pets::check(&b).ok()).map(|p| bellwether_brain::pets::propose_map(&p)))
            .flatten();
        let text = set_pet_default(old.as_deref(), name, map.as_ref())?;
        // Checked as the brain and the strip will read it before it replaces the file.
        let parsed = bellwether_core::json::parse(&text).map_err(|e| format!("internal: the new config.json doesn't parse ({e})"))?;
        bellwether_core::config::Config::from_js(&parsed, "config.json").map_err(|e| format!("the new config.json would be refused ({}); nothing was written", e.message))?;
        bellwether_core::persist::create_private_dir(&self.ctx.home).map_err(|e| e.to_string())?;
        bellwether_core::persist::write_atomic(&file, &self.ctx.home.join(format!(".config.json.{}.tmp", std::process::id())), &text, 0o600).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        log(&format!("onboarding: pets.default is now {name} ({})", file.display()));
        let merged = bellwether_core::config::load(&self.ctx.home, None).map_err(|e| e.message)?;
        serde_json::from_str(&bellwether_core::json::stringify(&merged.merged)).map_err(|e| e.to_string())
    }

    /// D6 step 6's Import…: `pets import` on the file the user chose in the panel.
    pub fn pet_import_file(&self, source: &Path) -> Result<Value, String> {
        if self.ctx.layout != Layout::App {
            return Err("this is a repository build: pets come from `./bellwether pets fetch`".into());
        }
        let r = bellwether_brain::pets::import(&self.ctx.home, source, None, false).map_err(|e| e.message)?;
        log(&format!("onboarding: imported pet {} ({})", r.name, r.file.display()));
        Ok(json!({ "name": r.name, "title": bellwether_brain::pets::printable(&r.pet.title), "animations": r.pet.animations.len() }))
    }

    // ------------------------------------------------------------ the CLI tool

    /// D6 step 7: `~/.local/bin/bellwether`, a link to this app's executable.
    pub fn cli_tool_install(&self) -> Result<Value, String> {
        let exe = crate::identity::bundle_executable().ok_or("the command-line tool links to Bellwether.app; this is a repository build (use ./bellwether)")?;
        let state = install_cli_link(&self.ctx.user_home, &exe, &homebrew_links())?;
        log(&format!("onboarding: command-line tool: {}", state["state"]));
        Ok(state)
    }

    pub fn cli_tool_state(&self) -> Value {
        match crate::identity::bundle_executable() {
            Some(exe) => cli_state(&self.ctx.user_home, &exe, &homebrew_links()),
            None => json!({ "state": "unavailable", "detail": "a repository build: use ./bellwether in the checkout" }),
        }
    }

    // ------------------------------------------------------------ status

    /// Everything the window shows, read only. `doctor` is the last step's report.
    pub fn status(&self, config: &Value) -> Value {
        let earlier = self.earlier_install();
        let token = self.token_now();
        let file = self.ctx.settings_file();
        let hooks = match installer::plan_edit(&file, Action::Install, self.ctx.port, token.as_ref().ok().and_then(|t| t.as_deref()).unwrap_or("tokenNotMadeYet_0000000000"), None) {
            Ok(plan) => {
                let groups = installer::sum_groups(&installer::count_groups(&plan.before, self.ctx.port));
                json!({ "file": file.display().to_string(), "groups": groups, "events": installer::EVENTS.len(), "upToDate": !plan.changed && token.as_ref().is_ok_and(|t| t.is_some()), "problem": Value::Null })
            }
            Err(e) => json!({ "file": file.display().to_string(), "groups": 0, "events": installer::EVENTS.len(), "upToDate": false, "problem": e.message }),
        };
        json!({
            "layout": if self.ctx.layout == Layout::App { "app" } else { "repository" },
            "port": self.ctx.port,
            "onboarded": bellwether_core::shellstate::onboarded(&self.ctx.home),
            "token": match &token { Ok(Some(_)) => json!({"usable": true}), Ok(None) => json!({"usable": false, "missing": true}), Err(e) => json!({"usable": false, "problem": e}) },
            "hooks": hooks,
            "login": if self.ctx.layout == Layout::App { login_json() } else { Value::Null },
            "accessibility": crate::ax::trusted(),
            "automation": { "iterm2": automation_state("com.googlecode.iterm2"), "terminal": automation_state("com.apple.Terminal") },
            "pets": self.pets(config),
            "migration": { "found": self.migration_found(&earlier), "agents": earlier.iter().map(|a| a.short()).collect::<Vec<_>>() },
            "cliTool": self.cli_tool_state(),
        })
    }
}

/// Handlers in a settings file that aren't Bellwether's (other tools' hooks), which an
/// install or uninstall leaves exactly as they are.
pub fn other_handlers(settings: &Lj, port: u16) -> usize {
    let Some(hooks) = settings.get("hooks").and_then(Lj::as_obj) else { return 0 };
    hooks
        .iter()
        .filter_map(|(_, groups)| groups.as_arr())
        .flatten()
        .filter_map(|g| g.get("hooks").and_then(Lj::as_arr))
        .flatten()
        .filter(|h| !installer::is_bellwether_handler(h, port))
        .count()
}

/// config.json's text with `pets.default` set (and `pets.animationMap.<name>` when given),
/// the rest of the file as it was: the lossless writer keeps its layout.
pub fn set_pet_default(old: Option<&str>, name: &str, map: Option<&bellwether_core::json::Obj>) -> Result<String, String> {
    use bellwether_core::lossless::{self, LObj};
    let mut root = match old {
        Some(t) => lossless::parse(t).map_err(|e| format!("config.json isn't valid JSON ({e}); fix it first, nothing was written"))?,
        None => Lj::Obj(LObj::new()),
    };
    let obj = root.as_obj_mut().ok_or("config.json doesn't hold a JSON object; nothing was written")?;
    if obj.get("pets").is_none() {
        obj.set("pets", Lj::Obj(LObj::new()));
    }
    let pets = obj.get_mut("pets").and_then(Lj::as_obj_mut).ok_or("config.json's \"pets\" isn't an object; nothing was written")?;
    pets.set("default", Lj::str(name));
    if let Some(m) = map {
        if pets.get("animationMap").is_none() {
            pets.set("animationMap", Lj::Obj(LObj::new()));
        }
        let maps = pets.get_mut("animationMap").and_then(Lj::as_obj_mut).ok_or("config.json's \"pets.animationMap\" isn't an object; nothing was written")?;
        maps.set(name, Lj::from_js(&bellwether_core::json::Js::Obj(m.clone())));
    }
    Ok(match old {
        Some(t) => lossless::serialise(&root, t),
        None => lossless::stringify(&root, "  ") + "\n",
    })
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| bellwether_brain::pets::valid_name(n)).collect()).unwrap_or_default();
    names.sort();
    names
}

fn pet_entry(name: &str, kind: &str, bytes: &[u8]) -> Value {
    match bellwether_brain::pets::still(bytes) {
        Ok(s) => json!({ "name": name, "kind": kind, "title": s.title, "still": { "png": s.png, "tilesx": s.tilesx, "tilesy": s.tilesy, "frame": s.frame, "transparency": s.transparency } }),
        Err(e) => json!({ "name": name, "kind": kind, "problem": e }),
    }
}

fn login_json() -> Value {
    let session = crate::identity::session();
    if session == crate::identity::Session::Stale {
        return json!({ "stale": true, "brain": "unknown", "app": "unknown", "lines": [login_items::STALE_SESSION] });
    }
    let (b, a) = (login_items::status(Service::Brain), login_items::status(Service::MainApp));
    json!({
        "stale": false,
        "brain": b.word(),
        "app": a.word(),
        "waiting": b == Status::RequiresApproval || a == Status::RequiresApproval,
        "lines": [login_items::advice(Service::Brain, b), login_items::advice(Service::MainApp, a)],
    })
}

/// Opens a URL with LaunchServices (a System Settings pane), only on the user's click.
fn open_url(url: &str) {
    match std::process::Command::new("/usr/bin/open").arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status() {
        Ok(_) => {}
        Err(e) => log(&format!("warning: couldn't open {url}: {e}")),
    }
}

// ------------------------------------------------------------ Automation, read only

/// Whether this app may send Apple Events to a terminal, asked without a prompt
/// (`AEDeterminePermissionToAutomateTarget` with askUserIfNeeded false): "allowed",
/// "denied", "notAsked", "notRunning" or "unknown".
pub fn automation_state(bundle_id: &str) -> &'static str {
    automation_word(mac_ae::determine(bundle_id))
}

pub fn automation_word(code: Option<i32>) -> &'static str {
    match code {
        Some(0) => "allowed",
        Some(-1743) => "denied",
        Some(-1744) => "notAsked",
        Some(-600) => "notRunning",
        _ => "unknown",
    }
}

#[cfg(target_os = "macos")]
mod mac_ae {
    use std::ffi::c_void;

    #[repr(C)]
    struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, event_class: u32, event_id: u32, ask_user_if_needed: u8) -> i32;
    }

    const fn four(c: &[u8; 4]) -> u32 {
        (c[0] as u32) << 24 | (c[1] as u32) << 16 | (c[2] as u32) << 8 | c[3] as u32
    }

    /// The OSStatus, never asking the user.
    pub fn determine(bundle_id: &str) -> Option<i32> {
        let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
        let bytes = bundle_id.as_bytes();
        unsafe {
            if AECreateDesc(four(b"bund"), bytes.as_ptr() as *const c_void, bytes.len() as isize, &mut desc) != 0 {
                return None;
            }
            let r = AEDeterminePermissionToAutomateTarget(&desc, four(b"****"), four(b"****"), 0);
            AEDisposeDesc(&mut desc);
            Some(r)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod mac_ae {
    pub fn determine(_bundle_id: &str) -> Option<i32> {
        None
    }
}

// ------------------------------------------------------------ the CLI tool

/// Where Homebrew's cask puts its `bellwether` link.
fn homebrew_links() -> Vec<PathBuf> {
    vec![PathBuf::from("/opt/homebrew/bin/bellwether"), PathBuf::from("/usr/local/bin/bellwether")]
}

fn is_app_executable(p: &Path) -> bool {
    p.ends_with("Bellwether.app/Contents/MacOS/Bellwether")
}

/// The command-line tool's state: "homebrew" (a cask link exists: nothing to do),
/// "installed" (~/.local/bin/bellwether links to this app), "otherApp" (it links to another
/// copy of Bellwether.app: installing points it here), "foreign" (something else is there:
/// left alone), or "absent".
pub fn cli_state(user_home: &Path, exe: &Path, homebrew: &[PathBuf]) -> Value {
    let link = user_home.join(".local").join("bin").join("bellwether");
    let dir = link.parent().unwrap().to_path_buf();
    if let Some(h) = homebrew.iter().find(|h| h.canonicalize().is_ok_and(|c| is_app_executable(&c))) {
        return json!({ "state": "homebrew", "path": h.display().to_string(), "detail": format!("Homebrew already put `bellwether` on your PATH ({})", h.display()) });
    }
    let state = match std::fs::symlink_metadata(&link) {
        Err(_) => "absent",
        Ok(m) if m.file_type().is_symlink() => match link.canonicalize() {
            Ok(c) if c == exe => "installed",
            Ok(c) if is_app_executable(&c) => "otherApp",
            _ => match std::fs::read_link(&link) {
                Ok(t) if is_app_executable(&t) => "otherApp",
                _ => "foreign",
            },
        },
        Ok(_) => "foreign",
    };
    json!({ "state": state, "path": link.display().to_string(), "dir": dir.display().to_string() })
}

/// Creates (or repoints) `~/.local/bin/bellwether` at `exe`. Never replaces anything that
/// isn't a link to a Bellwether.app; skipped when Homebrew's link is there.
pub fn install_cli_link(user_home: &Path, exe: &Path, homebrew: &[PathBuf]) -> Result<Value, String> {
    let mut state = cli_state(user_home, exe, homebrew);
    let link = user_home.join(".local").join("bin").join("bellwether");
    match state["state"].as_str() {
        Some("homebrew") | Some("installed") => {}
        Some("foreign") => return Err(format!("{} is already there and isn't a link to Bellwether.app; it was left alone. Remove or rename it, then try again", link.display())),
        _ => {
            let dir = link.parent().unwrap();
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            if std::fs::symlink_metadata(&link).is_ok() {
                std::fs::remove_file(&link).map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(exe, &link).map_err(|e| format!("cannot create {}: {e}", link.display()))?;
            state = cli_state(user_home, exe, homebrew);
        }
    }
    if state["state"] == "installed" {
        let on_path = login_path().map(|p| path_has(&p, link.parent().unwrap()));
        state["onPath"] = on_path.map(Value::from).unwrap_or(Value::Null);
        state["detail"] = Value::from(match on_path {
            Some(true) => format!("`bellwether` is ready in new terminal windows ({})", link.display()),
            Some(false) => format!("made {}, but {} isn't on your PATH: add `export PATH=\"$HOME/.local/bin:$PATH\"` to your shell's startup file (~/.zshrc for zsh), then open a new terminal window", link.display(), link.parent().unwrap().display()),
            None => format!("made {}; if `bellwether` isn't found in a new terminal window, add ~/.local/bin to your PATH", link.display()),
        });
    }
    Ok(state)
}

pub fn path_has(path: &str, dir: &Path) -> bool {
    let want = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    path.split(':').filter(|p| !p.is_empty()).any(|p| {
        let p = PathBuf::from(p);
        p == dir || p.canonicalize().is_ok_and(|c| c == want)
    })
}

/// The PATH a new terminal window gets: the user's login shell, interactive, asked to
/// print it after a marker (3 s at most; the child is stopped by its own handle).
fn login_path() -> Option<String> {
    use std::process::{Command, Stdio};
    let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(&shell).args(["-ilc", "printf '\\n__BELLWETHER_PATH__%s\\n' \"$PATH\""]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("__BELLWETHER_PATH__").map(str::to_string))
}

// ------------------------------------------------------------ the window

/// Opens the window, or brings it forward if it is open.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    let Some(ob) = app.try_state::<Onboarding>() else {
        log("onboarding: not available in this build");
        return;
    };
    if !allowed(ob.ctx.layout) {
        log("onboarding: the setup window is Bellwether.app's; a repository install is set up with ./bellwether");
        return;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("onboarding.html".into()))
        .title("Set up Bellwether")
        .inner_size(520.0, 600.0)
        .min_inner_size(440.0, 480.0)
        .resizable(true)
        .center()
        .focused(true)
        .build();
    match built {
        Ok(w) => {
            // The app is an accessory (no Dock icon): bring it forward with its window.
            let _ = w.set_focus();
            log("onboarding: window opened");
        }
        Err(e) => log(&format!("warning: couldn't open the setup window: {e}")),
    }
}

/// The NSOpenPanel, run on the main thread (Rust only: the webview has no dialog
/// permission). The chosen file's path, or None when cancelled.
#[cfg(target_os = "macos")]
fn choose_pet_file<R: Runtime>(app: &AppHandle<R>) -> Result<Option<PathBuf>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
        use objc2_foundation::NSString;
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(Err("not on the main thread".to_string()));
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setTitle(Some(&NSString::from_str("Import a pet")));
        panel.setMessage(Some(&NSString::from_str("Choose a desktopPet pet file (animations.xml). It is checked before anything is copied.")));
        panel.setPrompt(Some(&NSString::from_str("Import")));
        let answer = panel.runModal();
        let path = if answer == NSModalResponseOK { panel.URL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())) } else { None };
        let _ = tx.send(Ok(path));
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "macos"))]
fn choose_pet_file<R: Runtime>(_app: &AppHandle<R>) -> Result<Option<PathBuf>, String> {
    Err("the file panel is macOS-only for now".into())
}

/// Reloads every strip window so it asks for its pet again.
fn reload_strips<R: Runtime>(app: &AppHandle<R>) {
    for (label, w) in app.webview_windows() {
        if crate::strips::is_strip(&label) {
            let _ = w.reload();
        }
    }
}

// ------------------------------------------------------------ the commands

type Out = Result<Value, String>;

fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Result<T, String>> {
    async move { tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string()) }
}

#[tauri::command]
pub async fn setup_status(window: tauri::WebviewWindow, app: AppHandle) -> Out {
    guard(window.label())?;
    log("onboarding: setup_status");
    blocking(move || {
        let ob = app.state::<Onboarding>();
        let config = app.state::<crate::Shared>().config.read().unwrap().clone();
        let mut s = ob.status(&config);
        let report = crate::cli::doctor_report(&ob.ctx.home, Some(ob.ctx.settings_file()), Some(ob.ctx.port), false);
        let token = ob.token_now().ok().flatten();
        s["doctor"] = Value::Array(report.checks.iter().map(|c| json!({ "status": c.status, "name": c.name, "detail": bellwether_core::mask::mask_alone(&c.detail, token.as_deref()) })).collect());
        log(&format!(
            "onboarding: setup_status answered: hooks {} of {} events, {} pet(s), earlier install {}, doctor {} check(s), {} failing",
            s["hooks"]["groups"], s["hooks"]["events"], s["pets"]["list"].as_array().map_or(0, Vec::len), s["migration"]["found"], report.checks.len(), report.checks.iter().filter(|c| c.status == "FAIL").count()
        ));
        s
    })
    .await
}

#[tauri::command]
pub async fn hooks_preview(window: tauri::WebviewWindow, app: AppHandle, action: String) -> Out {
    guard(window.label())?;
    let action = match action.as_str() {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        _ => return Err("action must be \"install\" or \"uninstall\"".into()),
    };
    blocking(move || app.state::<Onboarding>().hooks_preview(action)).await?
}

#[tauri::command]
pub async fn hooks_apply(window: tauri::WebviewWindow, app: AppHandle, plan_id: String) -> Out {
    guard(window.label())?;
    blocking(move || app.state::<Onboarding>().hooks_apply(&plan_id)).await?
}

#[tauri::command]
pub async fn login_items_set(window: tauri::WebviewWindow, app: AppHandle, enabled: bool) -> Out {
    guard(window.label())?;
    blocking(move || app.state::<Onboarding>().login_items_set(enabled)).await?
}

#[tauri::command]
pub async fn permission_request(window: tauri::WebviewWindow, app: AppHandle, kind: String) -> Out {
    guard(window.label())?;
    if !matches!(kind.as_str(), "accessibility" | "automation-iterm2" | "automation-terminal" | "login-items") {
        return Err(format!("unknown permission {kind:?}"));
    }
    // The Accessibility prompt is AppKit's: on the main thread. Automation waits for the
    // user's answer to its prompt: on a worker.
    if kind == "accessibility" {
        let (tx, rx) = std::sync::mpsc::channel();
        let a = app.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(a.state::<Onboarding>().permission_request("accessibility"));
        })
        .map_err(|e| e.to_string())?;
        return blocking(move || rx.recv().map_err(|e| e.to_string())).await??;
    }
    blocking(move || app.state::<Onboarding>().permission_request(&kind)).await?
}

#[tauri::command]
pub async fn pet_select(window: tauri::WebviewWindow, app: AppHandle, name: String) -> Out {
    guard(window.label())?;
    if !bellwether_brain::pets::valid_name(&name) {
        return Err("not a pet's name".into());
    }
    let a = app.clone();
    let n = name.clone();
    let config = blocking(move || {
        let current = a.state::<crate::Shared>().config.read().unwrap().clone();
        a.state::<Onboarding>().pet_select(&n, &current)
    })
    .await??;
    *app.state::<crate::Shared>().config.write().unwrap() = config.clone();
    reload_strips(&app);
    let pets = app.state::<Onboarding>().pets(&config);
    Ok(json!({ "current": name, "pets": pets }))
}

#[tauri::command]
pub async fn pet_import(window: tauri::WebviewWindow, app: AppHandle) -> Out {
    guard(window.label())?;
    if app.state::<Onboarding>().ctx.layout != Layout::App {
        return Err("this is a repository build: pets come from `./bellwether pets fetch`".into());
    }
    let a = app.clone();
    let chosen = blocking(move || choose_pet_file(&a)).await??;
    let Some(path) = chosen else { return Ok(json!({ "cancelled": true })) };
    blocking(move || app.state::<Onboarding>().pet_import_file(&path)).await?
}

#[tauri::command]
pub async fn migration_preview(window: tauri::WebviewWindow, app: AppHandle) -> Out {
    guard(window.label())?;
    blocking(move || app.state::<Onboarding>().migration_preview()).await?
}

#[tauri::command]
pub async fn migration_apply(window: tauri::WebviewWindow, app: AppHandle, plan_id: String) -> Out {
    guard(window.label())?;
    blocking(move || app.state::<Onboarding>().migration_apply(&plan_id)).await?
}

#[tauri::command]
pub async fn cli_tool_install(window: tauri::WebviewWindow, app: AppHandle) -> Out {
    guard(window.label())?;
    blocking(move || app.state::<Onboarding>().cli_tool_install()).await?
}

#[tauri::command]
pub async fn setup_close(window: tauri::WebviewWindow, app: AppHandle) -> Out {
    guard(window.label())?;
    let home = app.state::<Onboarding>().ctx.home.clone();
    bellwether_core::shellstate::set_onboarded(&home).map_err(|e| format!("cannot write {}: {e}", bellwether_core::shellstate::file(&home).display()))?;
    log("onboarding: done");
    let _ = window.close();
    Ok(json!({ "closed": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "testTokenNotReal_abcdefghijklmnop";

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bw-onboarding-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx(base: &Path, env_token: Option<&str>) -> Ctx {
        Ctx {
            layout: Layout::App,
            home: base.join("app"),
            port: 47431,
            resources: None,
            user_home: base.join("user"),
            env_config: None,
            env_token: env_token.map(str::to_string),
        }
    }

    #[test]
    fn only_the_onboarding_window_passes_the_guard() {
        assert!(guard("onboarding").is_ok());
        for other in ["strip-0", "strip-12", "Onboarding", "onboarding ", "", "main"] {
            let e = guard(other).unwrap_err();
            assert!(e.contains("can't use the onboarding commands"), "{e}");
        }
        // Every command checks it first (the second wall of D6).
        let src = include_str!("onboarding.rs");
        let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        for c in COMMANDS {
            let body = code.split(&format!("pub async fn {c}(")).nth(1).unwrap_or_else(|| panic!("{c} is not a command here"));
            let first = body.split('{').nth(1).unwrap().trim_start();
            assert!(first.starts_with("guard(window.label())?;"), "{c} must call guard first");
        }
        assert_eq!(code.matches(concat!("#[tauri::", "command]")).count(), COMMANDS.len());
    }

    #[test]
    fn the_preview_is_the_clis_diff_masked_and_install_applies_exactly_it() {
        let base = temp("preview");
        let ob = Onboarding::new(ctx(&base, Some(T)));
        let file = ob.ctx.settings_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"echo other\"\n          }\n        ]\n      }\n    ]\n  }\n}\n").unwrap();
        let p = ob.hooks_preview(Action::Install).unwrap();
        let diff = p["diffText"].as_str().unwrap();
        assert!(!diff.contains(T), "the token is masked");
        assert!(diff.contains("\"X-Bellwether-Token\": \"test…\""), "{diff}");
        assert!(diff.starts_with(&format!("--- {} (current)", file.display())), "{diff}");
        assert_eq!(p["othersUntouched"], 1);
        assert_eq!(p["groupsBefore"], 0);
        assert_eq!(p["groupsAfter"], 16);
        assert_eq!(p["newToken"], false);
        let id = p["planId"].as_str().unwrap().to_string();
        // Someone edits the file between the preview and Install: refused, re-previewed.
        let edited = std::fs::read_to_string(&file).unwrap().replace("echo other", "echo edited");
        std::fs::write(&file, &edited).unwrap();
        let r = ob.hooks_apply(&id).unwrap();
        assert_eq!(r["outcome"], "changed");
        assert!(r["message"].as_str().unwrap().contains("changed since the preview"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited, "nothing was written");
        assert!(ob.hooks_apply(&id).is_err(), "a plan is used once");
        let fresh = r["preview"]["planId"].as_str().unwrap().to_string();
        assert!(r["preview"]["diffText"].as_str().unwrap().contains("echo edited"));
        let done = ob.hooks_apply(&fresh).unwrap();
        assert_eq!(done["outcome"], "applied", "{done}");
        assert!(done["backup"].as_str().unwrap().contains("backups/settings-"));
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains(T) && written.contains("echo edited"));
        // Already installed: no plan.
        let again = ob.hooks_preview(Action::Install).unwrap();
        assert_eq!(again["changed"], false);
        assert!(again["planId"].is_null());
        // Uninstall: the same preview-then-confirm, back to the edited bytes.
        let u = ob.hooks_preview(Action::Uninstall).unwrap();
        assert!(!u["diffText"].as_str().unwrap().contains(T));
        let id = u["planId"].as_str().unwrap().to_string();
        assert_eq!(ob.hooks_apply(&id).unwrap()["outcome"], "applied");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_new_user_gets_a_token_with_the_first_install_and_not_before() {
        let base = temp("newtoken");
        let ob = Onboarding::new(ctx(&base, None));
        let p = ob.hooks_preview(Action::Install).unwrap();
        assert_eq!(p["newToken"], true);
        assert!(!tok::file(&ob.ctx.home).exists(), "the preview writes nothing");
        assert!(!ob.ctx.settings_file().exists());
        let id = p["planId"].as_str().unwrap().to_string();
        // A token appears meanwhile (token init in a terminal): refused, re-previewed with it.
        tok::write(&ob.ctx.home, T).unwrap();
        let r = ob.hooks_apply(&id).unwrap();
        assert_eq!(r["outcome"], "changed");
        assert_eq!(r["preview"]["newToken"], false);
        std::fs::remove_file(tok::file(&ob.ctx.home)).unwrap();
        let p = ob.hooks_preview(Action::Install).unwrap();
        let r = ob.hooks_apply(p["planId"].as_str().unwrap()).unwrap();
        assert_eq!(r["outcome"], "applied");
        let (t, _) = tok::read(&ob.ctx.home, None).unwrap();
        assert!(std::fs::read_to_string(ob.ctx.settings_file()).unwrap().contains(&t), "the file and the hooks carry one token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(ob.ctx.settings_file()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_window_never_installs_over_an_earlier_install_or_outside_the_app() {
        let base = temp("legacy");
        let ob = Onboarding::new(ctx(&base, Some(T)));
        let agents = ob.ctx.user_home.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("local.bellwether.brain.plist"), "<plist/>").unwrap();
        let e = ob.hooks_preview(Action::Install).unwrap_err();
        assert!(e.contains("earlier Bellwether") && e.contains("local.bellwether.brain"), "{e}");
        let e = ob.login_items_set(true).unwrap_err();
        assert!(e.contains("earlier Bellwether"), "{e}");
        assert!(ob.hooks_preview(Action::Uninstall).is_ok(), "removing hooks is never refused");
        assert_eq!(ob.status(&Value::Null)["migration"]["found"], true);
        let _ = std::fs::remove_dir_all(&base);
        let mut repo = ctx(&base, Some(T));
        repo.layout = Layout::Repository;
        let ob = Onboarding::new(repo);
        assert!(ob.login_items_set(true).unwrap_err().contains("Bellwether.app"));
        assert!(ob.pet_import_file(Path::new("/nonexistent")).unwrap_err().contains("repository"));
        assert!(ob.pet_select("esheep64", &Value::Null).is_err());
    }

    /// A migration host that records what it is asked and never runs launchctl or
    /// SMAppService (plan D9 in the window, milestone 8).
    struct Rec {
        calls: std::sync::Arc<Mutex<Vec<String>>>,
        app_agent: Option<i32>,
    }

    impl bellwether_brain::toapp::Host for Rec {
        fn acts(&self) -> bool {
            true
        }
        fn domain(&self) -> String {
            "gui/501".into()
        }
        fn loaded(&self, label: &str) -> bellwether_brain::legacy::Loaded {
            let booted = self.calls.lock().unwrap().iter().any(|c| c.ends_with(&format!("/{label}")));
            if label == "local.bellwether.brain" && !booted { bellwether_brain::legacy::Loaded::Running(4242) } else { bellwether_brain::legacy::Loaded::No }
        }
        fn app_agent(&self) -> Option<i32> {
            self.app_agent
        }
        fn launchctl(&mut self, args: &[String]) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("launchctl {}", args.join(" ")));
            Ok(())
        }
        fn register(&mut self) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push("register".into());
            Ok(vec!["brain agent: enabled".into(), "app at login: enabled".into()])
        }
        fn unregister(&mut self) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push("unregister".into());
            Ok(vec![])
        }
        fn port_holder(&self, port: u16) -> bellwether_brain::toapp::Holder {
            bellwether_brain::toapp::port_holder(port)
        }
        fn process_path(&self, _: i32) -> Option<PathBuf> {
            None
        }
        fn terminate(&mut self, _: i32) -> bool {
            false
        }
        fn doctor(&self, _: &Path, _: &Path, _: u16) -> Vec<String> {
            vec!["PASS  brain".into()]
        }
        fn verify_limit(&self) -> Duration {
            Duration::from_secs(3)
        }
    }

    #[test]
    fn step_one_previews_the_move_and_confirm_runs_exactly_it() {
        let base = temp("move");
        // A brain already answering, playing this app's own agent (a recorder never starts one).
        let bound = bellwether_brain::bind(0).unwrap();
        let port = bound.port;
        let opts = bellwether_brain::Options::new(T.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
        // (No stop handle without tokio here: it ends with the test process.)
        std::thread::spawn(move || {
            let _ = bellwether_brain::serve(bound, opts, None);
        });
        let mut c = ctx(&base, None);
        c.port = port;
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("data")).unwrap();
        std::fs::create_dir_all(repo.join("brain")).unwrap();
        std::fs::write(repo.join(".env"), format!("BELLWETHER_HOOK_TOKEN={T}\n")).unwrap();
        std::fs::write(repo.join("config.json"), format!("{{\"port\": {port}}}\n")).unwrap();
        let agents = c.user_home.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("local.bellwether.brain.plist"), format!("<plist><dict><key>Label</key><string>local.bellwether.brain</string><key>WorkingDirectory</key><string>{}</string></dict></plist>", repo.display())).unwrap();
        let settings = c.settings_file();
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let pasture = (bellwether_core::lossless::stringify(&installer::hook_block(port, T), "  ") + "\n").replace("X-Bellwether-", "X-Pasture-");
        std::fs::write(&settings, &pasture).unwrap();
        let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
        let shared = calls.clone();
        let me = std::process::id() as i32;
        let ob = Onboarding::with_host(c, Box::new(move |_| Ok(Box::new(Rec { calls: shared.clone(), app_agent: Some(me) }))));
        assert_eq!(ob.status(&Value::Null)["migration"]["found"], true);
        let p = ob.migration_preview().unwrap();
        assert_eq!(p["found"], true);
        let text = p["text"].as_str().unwrap();
        assert!(text.contains("- copy the token from") && text.contains("- rewrite 16 pasture entries") && text.contains("local.bellwether.brain.plist to"), "{text}");
        assert!(!text.contains(T), "masked");
        assert!(calls.lock().unwrap().is_empty(), "a preview calls nothing");
        // The settings file changes after the preview: a fresh preview, nothing done.
        std::fs::write(&settings, pasture.replacen("{", "{ ", 1)).unwrap();
        let r = ob.migration_apply(p["planId"].as_str().unwrap()).unwrap();
        assert_eq!(r["outcome"], "changed");
        assert!(calls.lock().unwrap().is_empty() && !tok::file(&ob.ctx.home).exists());
        assert!(ob.migration_apply(p["planId"].as_str().unwrap()).is_err(), "a preview is used once");
        let fresh = r["preview"]["planId"].as_str().unwrap().to_string();
        let done = ob.migration_apply(&fresh).unwrap();
        assert_eq!(done["outcome"], "applied", "{done}");
        assert_eq!(*calls.lock().unwrap(), ["launchctl bootout gui/501/local.bellwether.brain", "register"]);
        let text = done["text"].as_str().unwrap();
        assert!(text.contains("the new brain answers on port") && !text.contains(T), "{text}");
        assert!(!agents.join("local.bellwether.brain.plist").exists() && ob.ctx.home.join("legacy").join("local.bellwether.brain.plist").exists());
        assert!(!std::fs::read_to_string(&settings).unwrap().contains("X-Pasture-"));
        assert_eq!(ob.migration_preview().unwrap()["found"], false, "a second look: nothing to migrate");
        assert_eq!(ob.status(&Value::Null)["migration"]["found"], false);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn importing_a_doctype_pet_is_refused_and_nothing_is_written() {
        let base = temp("doctype");
        let ob = Onboarding::new(ctx(&base, Some(T)));
        let f = base.join("animations.xml");
        std::fs::write(&f, "<?xml version=\"1.0\"?><!DOCTYPE a [<!ENTITY x \"y\">]><animations/>").unwrap();
        let e = ob.pet_import_file(&f).unwrap_err();
        assert!(e.contains("DOCTYPE") && e.contains("Nothing was imported"), "{e}");
        assert!(!ob.ctx.home.join("pets").exists() && !bellwether_brain::pets::lock_file(&ob.ctx.home).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    const TINY_PET: &str = "<animations><header><title>tiny</title></header><image><tilesx>2</tilesx><tilesy>1</tilesy><png>iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==</png><transparency>Magenta</transparency></image><spawns><spawn id=\"1\"><x>0</x><y>0</y><next probability=\"100\">1</next></spawn></spawns><animations><animation id=\"1\"><name>walk</name><start><x>0</x><y>0</y><interval>200</interval></start><end><x>0</x><y>0</y><interval>200</interval></end><sequence repeat=\"0\" repeatfrom=\"0\"><frame>1</frame><frame>0</frame></sequence></animation></animations></animations>";

    /// Plan D8, milestone M10a: Bellwether's own pets first, in their fixed order, then the
    /// imported ones; the still's tint is the first palette colour; a bundled pet can be chosen.
    #[test]
    fn bundled_pets_are_listed_first_with_the_tint_and_can_be_chosen() {
        let base = temp("bundled");
        let mut c = ctx(&base, Some(T));
        c.resources = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap());
        let ob = Onboarding::new(c);
        let f = base.join("tiny.xml");
        std::fs::write(&f, TINY_PET).unwrap();
        ob.pet_import_file(&f).unwrap();
        let pets = ob.pets(&Value::Null);
        let names: Vec<&str> = pets["list"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["bellwether", "tiny"], "bundled first (brah joins after bellwether in M10b)");
        assert_eq!(pets["list"][0]["kind"], "bundled");
        assert_eq!(pets["list"][0]["title"], "Bellwether sheep");
        assert_eq!(pets["list"][0]["still"]["frame"], 0, "the first frame of walk");
        assert_eq!(pets["tint"], "#FF8C00", "the first default palette colour");
        assert_eq!(ob.pets(&json!({ "pets": { "palette": ["#123456"] } }))["tint"], "#123456");
        assert_eq!(pets["current"], "bellwether");
        let merged = ob.pet_select("tiny", &Value::Null).unwrap();
        assert_eq!(merged.pointer("/pets/default").and_then(Value::as_str), Some("tiny"));
        let merged = ob.pet_select("bellwether", &merged).unwrap();
        assert_eq!(merged.pointer("/pets/default").and_then(Value::as_str), Some("bellwether"));
        assert!(merged.pointer("/pets/animationMap/bellwether").is_none(), "a bundled pet brings its own map");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_imported_pet_is_listed_with_its_still_and_selecting_it_edits_only_pets() {
        let base = temp("pets");
        let ob = Onboarding::new(ctx(&base, Some(T)));
        let f = base.join("tiny.xml");
        std::fs::write(&f, TINY_PET).unwrap();
        let r = ob.pet_import_file(&f).unwrap();
        assert_eq!(r["name"], "tiny");
        let pets = ob.pets(&Value::Null);
        assert_eq!(pets["current"], "bellwether");
        let tiny = &pets["list"][0];
        assert_eq!(tiny["kind"], "imported");
        assert_eq!(tiny["still"]["frame"], 1, "the first frame of the first animation");
        assert_eq!(tiny["still"]["tilesx"], 2);
        assert_eq!(tiny["still"]["transparency"], "Magenta");
        assert!(ob.pet_select("nothere", &Value::Null).unwrap_err().contains("isn't a pet"));
        // A config.json of the user's own: only pets changes; the rest keeps its lines (4-space
        // indent here). An inline object would be laid out again, as the hook installer does.
        let config = ob.ctx.home.join("config.json");
        std::fs::write(&config, "{\n    \"strip\": {\n        \"heightPx\": 150\n    },\n    \"pets\": {\n        \"scale\": 2\n    }\n}\n").unwrap();
        let merged = ob.pet_select("tiny", &Value::Null).unwrap();
        assert_eq!(merged.pointer("/pets/default").and_then(Value::as_str), Some("tiny"));
        assert_eq!(merged.pointer("/pets/animationMap/tiny/walk").and_then(Value::as_str), Some("walk"), "the proposed map, so it walks");
        assert_eq!(merged.pointer("/strip/heightPx").and_then(Value::as_f64), Some(150.0));
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("{\n    \"strip\": {\n        \"heightPx\": 150\n    },\n    \"pets\": {\n        \"scale\": 2,\n        \"default\": \"tiny\","), "{text}");
        // A pet whose file changed after import is listed with its problem and can't be chosen.
        std::fs::write(bellwether_brain::pets::pet_file(&ob.ctx.home, "tiny"), TINY_PET.replace("tiny", "tinz")).unwrap();
        assert!(ob.pets(&Value::Null)["list"][0]["problem"].as_str().unwrap().contains("changed after it was imported"));
        assert!(ob.pet_select("tiny", &Value::Null).is_err());
        // BELLWETHER_CONFIG (read only) is never written.
        let mut ro = ctx(&base, Some(T));
        ro.env_config = Some("/tmp/elsewhere.json".into());
        assert!(Onboarding::new(ro).config_editable().unwrap_err().contains("read only"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_pet_default_writes_a_new_file_and_keeps_an_old_ones_layout() {
        assert_eq!(set_pet_default(None, "tiny", None).unwrap(), "{\n  \"pets\": {\n    \"default\": \"tiny\"\n  }\n}\n");
        let old = "{\n\t\"port\": 47431,\n\t\"pets\": {\n\t\t\"default\": \"esheep64\"\n\t}\n}\n";
        assert_eq!(set_pet_default(Some(old), "tiny", None).unwrap(), old.replace("esheep64", "tiny"));
        assert!(set_pet_default(Some("[1]"), "tiny", None).is_err());
        assert!(set_pet_default(Some("{\"pets\": 3}"), "tiny", None).is_err());
        assert!(set_pet_default(Some("{nope"), "tiny", None).is_err());
    }

    #[test]
    fn the_cli_tool_is_a_link_to_this_app_and_never_replaces_a_stranger() {
        let base = temp("cli");
        let app = base.join("Apps").join("Bellwether.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("Bellwether");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let exe = exe.canonicalize().unwrap();
        let user = base.join("user");
        std::fs::create_dir_all(&user).unwrap();
        let none: Vec<PathBuf> = vec![base.join("no-brew")];
        assert_eq!(cli_state(&user, &exe, &none)["state"], "absent");
        let r = install_cli_link(&user, &exe, &none).unwrap();
        assert_eq!(r["state"], "installed");
        let link = user.join(".local/bin/bellwether");
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        // Another copy of the app: repointed here.
        let other = base.join("Old").join("Bellwether.app/Contents/MacOS");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Bellwether"), "").unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(other.join("Bellwether"), &link).unwrap();
        assert_eq!(cli_state(&user, &exe, &none)["state"], "otherApp");
        assert_eq!(install_cli_link(&user, &exe, &none).unwrap()["state"], "installed");
        // Something else of the user's: left alone.
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "mine").unwrap();
        assert!(install_cli_link(&user, &exe, &none).unwrap_err().contains("left alone"));
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "mine");
        // Homebrew's link: nothing to do.
        let brew = base.join("brew-bellwether");
        std::os::unix::fs::symlink(&exe, &brew).unwrap();
        assert_eq!(cli_state(&user, &exe, &[brew])["state"], "homebrew");
        assert!(path_has("/usr/bin:/x/.local/bin", Path::new("/x/.local/bin")));
        assert!(!path_has("/usr/bin::/bin", Path::new("/x/.local/bin")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn other_tools_handlers_are_counted() {
        let s = bellwether_core::lossless::parse(r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"a"},{"type":"http","url":"http://127.0.0.1:47431/hook"}]}],"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"http","url":"http://127.0.0.1:9/hook"}]}]}}"#).unwrap();
        assert_eq!(other_handlers(&s, 47431), 2);
        assert_eq!(other_handlers(&bellwether_core::lossless::parse("{}").unwrap(), 47431), 0);
    }

    /// The shape test/onboarding.test.mjs gives the page (its `status()`), plus `doctor`,
    /// which the command adds.
    #[test]
    fn the_status_has_the_keys_the_page_reads() {
        let base = temp("status");
        let ob = Onboarding::new(ctx(&base, None));
        let s = ob.status(&Value::Null);
        let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
        let mut want = ["layout", "port", "onboarded", "token", "hooks", "login", "accessibility", "automation", "pets", "migration", "cliTool"];
        want.sort();
        let mut keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, want);
        assert_eq!(s["token"], json!({"usable": false, "missing": true}), "a new user: no token yet, and none made by looking");
        assert!(!tok::file(&ob.ctx.home).exists());
        assert_eq!(s["hooks"]["groups"], 0);
        assert_eq!(s["hooks"]["upToDate"], false);
        assert_eq!(s["migration"]["found"], false);
        assert_eq!(s["pets"]["list"], json!([]));
        assert!(!ob.ctx.home.exists(), "reading the status writes nothing");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn automation_codes_in_words() {
        assert_eq!(automation_word(Some(0)), "allowed");
        assert_eq!(automation_word(Some(-1743)), "denied");
        assert_eq!(automation_word(Some(-1744)), "notAsked");
        assert_eq!(automation_word(Some(-600)), "notRunning");
        assert_eq!(automation_word(None), "unknown");
    }
}
