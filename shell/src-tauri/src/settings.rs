//! Where the strip's settings come from. Read-only: the shell never writes config.json,
//! the token or a `.env`. Two layouts (distribution plan M5, deviation 45):
//!
//! - **App**: the executable is inside an app bundle (`Bellwether.app/Contents/MacOS`).
//!   Its home is `BELLWETHER_HOME`, else a test build's bundled home, else
//!   `~/Library/Application Support/Bellwether`: `config.json` there (every key optional,
//!   checked as the Rust brain checks it), the token from `BELLWETHER_HOOK_TOKEN` or the
//!   home's `hook-token`, `shell-state.json`, imported pets. It logs to
//!   `~/Library/Logs/Bellwether/shell.log` (`<home>/logs/` for any other home).
//! - **Repository**: any binary outside an app bundle: `target/release/bellwether-shell`,
//!   which a repository install's LaunchAgent runs with `--no-spawn-brain` next to the Node
//!   brain, and the `target-dev/` builds. It keeps the checkout's layout until milestone 8
//!   moves repository installs into the app: `config.json` of the checkout, the token
//!   from the environment or the `.env` (the Node CLI's rule), `data/`, `data/shell.log`.
//!
//! A test build (`scripts/build-app.mjs --test-port/--test-home`) carries its port and
//! home in the bundled brain agent's plist. The strip takes them from there too, so a test
//! build's strip never reads the live install's files or port, however it is started.

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    App,
    Repository,
}

pub struct Settings {
    pub layout: Layout,
    /// Repository only: the checkout.
    pub agent_dir: Option<PathBuf>,
    /// The app's home, or the checkout's `data/`: shell-state.json, imported pets.
    pub data_dir: PathBuf,
    pub log_file: PathBuf,
    pub port: u16,
    pub token: Option<String>,
    pub config: Value,
    /// What a brain the strip starts gets in its environment, besides
    /// BELLWETHER_PARENT_PID: the same home (and, for a repository, config).
    pub brain_env: Vec<(String, PathBuf)>,
    /// Repository only: the brain the strip starts gets the token in its environment
    /// (the checkout's `.env` is not the app's `hook-token`).
    pub brain_gets_token: bool,
    /// App only: `Contents/Resources`, where bundled pets live.
    pub resources: Option<PathBuf>,
}

/// Where the token comes from, for `token_now`.
enum TokenSource {
    App(PathBuf),
    Repository(PathBuf),
}

static TOKEN_SOURCE: std::sync::OnceLock<TokenSource> = std::sync::OnceLock::new();

/// The layout this process runs in: the app inside a bundle, the repository otherwise.
pub fn layout() -> Layout {
    if crate::identity::in_app_bundle() {
        Layout::App
    } else {
        Layout::Repository
    }
}

impl Settings {
    pub fn load() -> Result<Settings, String> {
        match layout() {
            Layout::App => Settings::load_app(),
            Layout::Repository => Settings::load_repository(),
        }
    }

    fn load_app() -> Result<Settings, String> {
        let env_home = std::env::var_os("BELLWETHER_HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
        let place = app_home(env_home, crate::login_items::bundle_agent_overrides(), bellwether_brain::home::dir)?;
        let config = bellwether_core::config::load(&place.home, std::env::var("BELLWETHER_CONFIG").ok().as_deref()).map_err(|e| e.message)?;
        let json: Value = serde_json::from_str(&bellwether_core::json::stringify(&config.merged)).map_err(|e| format!("config: {e}"))?;
        let port = place.port.unwrap_or_else(|| config.port());
        let _ = TOKEN_SOURCE.set(TokenSource::App(place.home.clone()));
        let log_file = if place.custom { place.home.join("logs") } else { bellwether_brain::home::log_dir() }.join("shell.log");
        let brain_env = if place.custom { vec![("BELLWETHER_HOME".to_string(), place.home.clone())] } else { Vec::new() };
        Ok(Settings {
            layout: Layout::App,
            agent_dir: None,
            data_dir: place.home,
            log_file,
            port,
            token: token_now(),
            config: json,
            brain_env,
            brain_gets_token: false,
            resources: crate::identity::bundle_path().map(|b| b.join("Contents").join("Resources")),
        })
    }

    fn load_repository() -> Result<Settings, String> {
        // BELLWETHER_AGENT_DIR wins; otherwise the agent directory this binary was built
        // from: the shell runs from a checkout, next to its brain and config.
        let agent_dir = env_os("AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
        let agent_dir = agent_dir.canonicalize().map_err(|e| format!("agent directory {}: {e}", agent_dir.display()))?;
        let data_dir = env_os("DATA_DIR").map(PathBuf::from).unwrap_or_else(|| agent_dir.join("data"));

        // BELLWETHER_CONFIG (a test aid, read-only like the rest): another config file,
        // e.g. one with a dummy name in privacy.sharingProcesses (Milestone 7 (b)).
        let real = agent_dir.join("config.json");
        let file = match env_os("CONFIG") {
            Some(p) => PathBuf::from(p),
            None if real.exists() => real,
            None => agent_dir.join("config.example.json"),
        };
        let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let config: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;
        let port = config.get("port").and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).ok_or("config: port must be 1024–65535")? as u16;

        // The token, in the CLI's order (brain/envload.mjs): an exported
        // BELLWETHER_HOOK_TOKEN, then one in the .env (env_file), then the old name,
        // exported, then in the .env.
        let _ = TOKEN_SOURCE.set(TokenSource::Repository(agent_dir.clone()));
        let token = token_now();
        let file = file.canonicalize().unwrap_or(file);
        Ok(Settings {
            layout: Layout::Repository,
            log_file: data_dir.join("shell.log"),
            brain_env: vec![("BELLWETHER_HOME".to_string(), data_dir.clone()), ("BELLWETHER_CONFIG".to_string(), file)],
            brain_gets_token: true,
            agent_dir: Some(agent_dir),
            data_dir,
            port,
            token,
            config,
            resources: None,
        })
    }

    pub fn f64_at(&self, pointer: &str, default: f64) -> f64 {
        self.config.pointer(pointer).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default)
    }
}

/// Where the app's files are, and whether a test build fixes the port.
#[derive(Debug, PartialEq, Eq)]
pub struct AppHome {
    pub home: PathBuf,
    /// Not the default folder: logs go to `<home>/logs`, and a brain the strip starts is
    /// told the home.
    pub custom: bool,
    /// A test build's `--port`.
    pub port: Option<u16>,
}

/// The app's home: `BELLWETHER_HOME`, else the test build's bundled home, else the
/// default. A test build decides: a BELLWETHER_HOME other than its own is refused, as
/// `autostart` refuses it (milestone 1, F10), so its strip and its brain agent can never
/// disagree.
pub fn app_home(env_home: Option<PathBuf>, bundle: (Option<u16>, Option<String>), default: impl Fn() -> PathBuf) -> Result<AppHome, String> {
    let (port, bundled) = bundle;
    let bundled = bundled.map(PathBuf::from);
    match (env_home, bundled) {
        (Some(e), Some(b)) if e != b => Err(format!("this is a test build whose home is {}, but BELLWETHER_HOME is {}: the bundle decides (unset BELLWETHER_HOME, or rebuild with `--test-home {}`)", b.display(), e.display(), e.display())),
        (Some(h), _) | (None, Some(h)) => Ok(AppHome { home: h, custom: true, port }),
        (None, None) => Ok(AppHome { home: default(), custom: false, port }),
    }
}

/// `BELLWETHER_<name>` from the environment, or else the name it had before the
/// rename, `PASTURE_<name>` (read for one release; brain/envpath.mjs does the same).
pub fn env_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("BELLWETHER_{name}")).or_else(|| std::env::var_os(format!("PASTURE_{name}")))
}

/// Which .env holds the token: the same rule as the CLI (brain/envpath.mjs). BELLWETHER_ENV
/// if set; else the repository's own `.env`; else, when the repository sits at
/// `agents/<name>` inside a hub and has no `.env` of its own, the hub's.
pub fn env_file(agent_dir: &Path) -> PathBuf {
    if let Some(p) = env_os("ENV").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let own = agent_dir.join(".env");
    if own.exists() {
        return own;
    }
    if agent_dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "agents") {
        let hub = agent_dir.join("..").join("..").join(".env");
        if hub.exists() {
            return hub;
        }
    }
    own
}

/// The hook token as it is right now, read again on every call. A strip started before
/// there is a token uses it to connect once one appears (brain.rs), without a restart.
/// - App: BELLWETHER_HOOK_TOKEN, else the home's `hook-token`; a token that fails the
///   brain's check (too short, a published example) counts as none.
/// - Repository: the environment, then the `.env`, as the Node CLI reads them.
pub fn token_now() -> Option<String> {
    match TOKEN_SOURCE.get()? {
        TokenSource::App(home) => bellwether_core::token::read(home, std::env::var("BELLWETHER_HOOK_TOKEN").ok()).ok().map(|(t, _)| t),
        TokenSource::Repository(agent_dir) => token_from(Some(agent_dir), |k| std::env::var(k).ok()),
    }
}

/// Why there is no usable token, in words (never the token).
pub fn token_problem() -> String {
    match TOKEN_SOURCE.get() {
        Some(TokenSource::App(home)) => match bellwether_core::token::read(home, std::env::var("BELLWETHER_HOOK_TOKEN").ok()) {
            Ok(_) => "the token is usable now".into(),
            // The token module's message already says what to run.
            Err(e) => e.message,
        },
        _ => "BELLWETHER_HOOK_TOKEN is not set (environment or the .env; `bellwether token init` adds one)".into(),
    }
}

/// The repository's token: the environment first; the `.env` is read only when the
/// environment has neither name.
fn token_from(agent_dir: Option<&Path>, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |v: Option<String>| v.filter(|v| !v.is_empty());
    if let Some(t) = set(var("BELLWETHER_HOOK_TOKEN")) {
        return Some(t);
    }
    let text = agent_dir.and_then(|d| std::fs::read_to_string(env_file(d)).ok()).unwrap_or_default();
    token_in_order(var, &text)
}

/// The hook token: an exported BELLWETHER_HOOK_TOKEN, then the .env's, then the old
/// name exported, then the old name in the .env. Only those keys are read from the
/// file; every other line is ignored and never kept or logged. Empty means unset.
fn token_in_order(var: impl Fn(&str) -> Option<String>, env_text: &str) -> Option<String> {
    let set = |v: Option<String>| v.filter(|v| !v.is_empty());
    set(var("BELLWETHER_HOOK_TOKEN"))
        .or_else(|| set(parse_env_value(env_text, "BELLWETHER_HOOK_TOKEN")))
        .or_else(|| set(var("PASTURE_HOOK_TOKEN")))
        .or_else(|| set(parse_env_value(env_text, "PASTURE_HOOK_TOKEN")))
}

#[cfg(test)]
fn token_from_env_file(path: &Path) -> Option<String> {
    token_in_order(|_| None, &std::fs::read_to_string(path).ok()?)
}

/// Reads one key the way Node's `--env-file` does (Node 22.14, which the brain runs
/// with), so the shell and the brain always see the same token. The rules, pinned by
/// the shared vectors in test/fixtures/env-vectors.json:
/// - `\r\n` counts as `\n`; a line starting with `#` (after leading spaces) is skipped;
/// - the key is what precedes the first `=`, trimmed of spaces, with one `export `
///   prefix removed (`export  KEY`, two spaces, is not KEY);
/// - a value starting with `"`, `'` or `` ` `` is what lies before the matching
///   closing quote (`\n` becomes a newline inside double quotes); with no closing
///   quote the value is the rest of the line, quote included;
/// - an unquoted value stops at `#` and is trimmed of spaces (tabs are kept);
/// - the last assignment wins.
pub fn parse_env_value(text: &str, wanted: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let mut found = None;
    for raw in text.split('\n') {
        let line = raw.trim_start_matches(' ');
        if line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let mut key = line[..eq].trim_matches(' ');
        if let Some(rest) = key.strip_prefix("export ") {
            key = rest;
        }
        if key != wanted {
            continue;
        }
        let v = line[eq + 1..].trim_start_matches(' ');
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'' | '`')) => match v[1..].find(q) {
                Some(end) => {
                    let inner = &v[1..1 + end];
                    if q == '"' { inner.replace("\\n", "\n") } else { inner.to_string() }
                }
                None => v.trim_end_matches(' ').to_string(),
            },
            _ => v[..v.find('#').unwrap_or(v.len())].trim_matches(' ').to_string(),
        };
        found = Some(value);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same vectors test/env-vectors.test.mjs checks against Node itself.
    #[test]
    fn matches_nodes_env_file_parser() {
        let json: serde_json::Value = serde_json::from_str(include_str!("../../../test/fixtures/env-vectors.json")).unwrap();
        let vectors = json["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 20);
        let mut failures = Vec::new();
        for v in vectors {
            let text = v["text"].as_str().unwrap();
            let expected = v["expected"].as_str().map(str::to_string);
            let got = parse_env_value(text, "BELLWETHER_HOOK_TOKEN");
            if got != expected {
                failures.push(format!("{}: expected {:?}, got {:?}", v["name"], expected, got));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn empty_means_unset_and_other_lines_are_ignored() {
        let dir = std::env::temp_dir().join(format!("bellwether-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(".env");
        std::fs::write(&f, "OTHER=secret\n# comment\nexport BELLWETHER_HOOK_TOKEN=\"abc_DEF-123\" # mine\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("abc_DEF-123"));
        std::fs::write(&f, "BELLWETHER_HOOK_TOKEN=\n").unwrap();
        assert_eq!(token_from_env_file(&f), None);
        // the name before the rename is still read, and the new one wins over it
        std::fs::write(&f, "PASTURE_HOOK_TOKEN=old_TOKEN-123\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("old_TOKEN-123"));
        std::fs::write(&f, "PASTURE_HOOK_TOKEN=old_TOKEN-123\nBELLWETHER_HOOK_TOKEN=new_TOKEN-456\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("new_TOKEN-456"));
        // the order with the environment: exported new, file new, exported old, file old
        let file_new_and_old = "PASTURE_HOOK_TOKEN=file_old\nBELLWETHER_HOOK_TOKEN=file_new\n";
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(token_in_order(env(&[("BELLWETHER_HOOK_TOKEN", "env_new"), ("PASTURE_HOOK_TOKEN", "env_old")]), file_new_and_old).as_deref(), Some("env_new"));
        assert_eq!(token_in_order(env(&[("PASTURE_HOOK_TOKEN", "env_old")]), file_new_and_old).as_deref(), Some("file_new"), "a stale exported old name doesn't beat the .env's new one");
        assert_eq!(token_in_order(env(&[("PASTURE_HOOK_TOKEN", "env_old")]), "PASTURE_HOOK_TOKEN=file_old\n").as_deref(), Some("env_old"));
        assert_eq!(token_in_order(env(&[]), "PASTURE_HOOK_TOKEN=file_old\n").as_deref(), Some("file_old"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_app_home_is_the_environment_or_the_test_builds_or_the_default() {
        let default = || PathBuf::from("/Users/x/Library/Application Support/Bellwether");
        assert_eq!(app_home(None, (None, None), default).unwrap(), AppHome { home: default(), custom: false, port: None });
        assert_eq!(app_home(Some("/tmp/a".into()), (None, None), default).unwrap(), AppHome { home: "/tmp/a".into(), custom: true, port: None });
        // A test build: its bundled port and home, whatever starts it (Finder included).
        let test_build = (Some(47418), Some("/tmp/bw-m5".to_string()));
        assert_eq!(app_home(None, test_build.clone(), default).unwrap(), AppHome { home: "/tmp/bw-m5".into(), custom: true, port: Some(47418) });
        assert_eq!(app_home(Some("/tmp/bw-m5".into()), test_build.clone(), default).unwrap().port, Some(47418));
        let e = app_home(Some("/tmp/other".into()), test_build, default).unwrap_err();
        assert!(e.contains("the bundle decides") && e.contains("/tmp/bw-m5"), "{e}");
    }

    /// The `.env` is a FIFO nobody writes to, so opening it blocks: `token_from` must finish
    /// without touching it when the environment has the token. (A test that only checked the
    /// answer passed even with the read moved above the early return.)
    #[test]
    fn the_repository_reads_the_env_file_only_when_the_environment_has_no_token() {
        let dir = std::env::temp_dir().join(format!("bellwether-lazy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = std::ffi::CString::new(dir.join(".env").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "mkfifo");
        let (tx, rx) = std::sync::mpsc::channel();
        let d = dir.clone();
        // If the FIFO is opened, this thread blocks for good; the test fails on the timeout.
        std::thread::spawn(move || {
            let _ = tx.send(token_from(Some(&d), |k| (k == "BELLWETHER_HOOK_TOKEN").then(|| "fromEnv_0123456789abcd".to_string())));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(3)).expect("token_from blocked: it opened the .env although the environment has the token");
        assert_eq!(got.as_deref(), Some("fromEnv_0123456789abcd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_written_later_is_found_on_the_next_look() {
        let dir = std::env::temp_dir().join(format!("bellwether-later-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(token_from(Some(&dir), |_| None), None, "no token yet");
        std::fs::write(dir.join(".env"), "BELLWETHER_HOOK_TOKEN=appearedLater_0123456789\n").unwrap();
        assert_eq!(token_from(Some(&dir), |_| None).as_deref(), Some("appearedLater_0123456789"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_file_is_the_repos_own_then_a_hubs() {
        let base = std::env::temp_dir().join(format!("bellwether-envfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let alone = base.join("bellwether");
        std::fs::create_dir_all(&alone).unwrap();
        assert_eq!(env_file(&alone), alone.join(".env"), "no .env anywhere: the repo's own (where token init writes)");
        let agent = base.join("hub").join("agents").join("bellwether");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(base.join("hub").join(".env"), "BELLWETHER_HOOK_TOKEN=x\n").unwrap();
        assert_eq!(env_file(&agent), agent.join("..").join("..").join(".env"), "inside a hub with no .env of its own");
        std::fs::write(agent.join(".env"), "BELLWETHER_HOOK_TOKEN=y\n").unwrap();
        assert_eq!(env_file(&agent), agent.join(".env"), "its own .env wins");
        let _ = std::fs::remove_dir_all(&base);
    }
}
