//! Where the strip's settings come from. Read-only: the shell never writes config.json,
//! the token or a `.env`. Two layouts (distribution plan M5, deviation 45):
//!
//! - **App**: the executable is inside an app bundle (`Bellwether.app/Contents/MacOS`).
//!   Its home is `BELLWETHER_HOME`, else a test build's bundled home, else
//!   `~/Library/Application Support/Bellwether`: `config.json` there (every key optional,
//!   checked as the Rust brain checks it), the token from `BELLWETHER_HOOK_TOKEN` or the
//!   home's `hook-token`, `shell-state.json`, imported pets. It logs to
//!   `~/Library/Logs/Bellwether/shell.log` (`<home>/logs/` for any other home).
//! - **Repository**: any binary outside an app bundle, a development build in
//!   `target-dev/` (docs/DEVELOPMENT.md). It reads the same files as the development CLI
//!   (milestone 11, M11-b): its home is `BELLWETHER_HOME`, else `BELLWETHER_DATA_DIR`, else
//!   the checkout's `data/`; the token is an exported `BELLWETHER_HOOK_TOKEN`, else that
//!   home's `hook-token`, read as the app reads it; the config is `BELLWETHER_CONFIG`, else
//!   the checkout's `config.json`, else `config.example.json`; it logs to
//!   `<home>/shell.log`. The checkout's `.env` and the `PASTURE_*` names are not read.
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
    /// (an exported token, or the home's `hook-token`, which the brain would read too).
    pub brain_gets_token: bool,
    /// App only: `Contents/Resources`, where bundled pets live.
    pub resources: Option<PathBuf>,
}

/// Where the token comes from, for `token_now`: the home whose `hook-token` is read when
/// BELLWETHER_HOOK_TOKEN isn't set (the app's, or a checkout's).
enum TokenSource {
    App(PathBuf),
    /// A checkout's home (M11-b): read the same way as the app's.
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
        // BELLWETHER_AGENT_DIR wins; otherwise the checkout this binary was built from.
        let var = |name: &str| std::env::var_os(format!("BELLWETHER_{name}")).filter(|v| !v.is_empty());
        let agent_dir = var("AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
        let agent_dir = agent_dir.canonicalize().map_err(|e| format!("agent directory {}: {e}", agent_dir.display()))?;
        let data_dir = repository_home(var, &agent_dir);

        // BELLWETHER_CONFIG (a test aid, read-only like the rest): another config file,
        // e.g. one with a dummy name in privacy.sharingProcesses (Milestone 7 (b)).
        let real = agent_dir.join("config.json");
        let file = match var("CONFIG") {
            Some(p) => PathBuf::from(p),
            None if real.exists() => real,
            None => agent_dir.join("config.example.json"),
        };
        let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let config: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;
        let port = config.get("port").and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).ok_or("config: port must be 1024–65535")? as u16;

        // The token, as the development CLI reads it with BELLWETHER_HOME set to the same
        // home: an exported BELLWETHER_HOOK_TOKEN, else the home's `hook-token`.
        let _ = TOKEN_SOURCE.set(TokenSource::Repository(data_dir.clone()));
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

/// `BELLWETHER_<name>` from the environment, or else the name it had before the rename,
/// `PASTURE_<name>`. Since milestone 11 only `SHELL_TRACE` is read through it (the strip's
/// debug trace, in both layouts); the repository layout's names are read without the old
/// one (`load_repository`).
pub fn env_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("BELLWETHER_{name}")).or_else(|| std::env::var_os(format!("PASTURE_{name}")))
}

/// A checkout's home (M11-b): `BELLWETHER_HOME`, else `BELLWETHER_DATA_DIR`, else the
/// checkout's `data/`. `var` answers `BELLWETHER_<name>` (empty counts as unset).
fn repository_home(var: impl Fn(&str) -> Option<std::ffi::OsString>, agent_dir: &Path) -> PathBuf {
    var("HOME").or_else(|| var("DATA_DIR")).map(PathBuf::from).unwrap_or_else(|| agent_dir.join("data"))
}

/// The hook token as it is right now, read again on every call. A strip started before
/// there is a token uses it to connect once one appears (brain.rs), without a restart.
/// In both layouts: BELLWETHER_HOOK_TOKEN, else the home's `hook-token`; a token that fails
/// the brain's check (too short, a published example) counts as none.
pub fn token_now() -> Option<String> {
    let (TokenSource::App(home) | TokenSource::Repository(home)) = TOKEN_SOURCE.get()?;
    token_at(home, std::env::var("BELLWETHER_HOOK_TOKEN").ok())
}

/// The token for a home: `env` (an exported BELLWETHER_HOOK_TOKEN) if set, and only
/// otherwise the home's `hook-token`, by the app's rules (`bellwether_core::token::read`).
fn token_at(home: &Path, env: Option<String>) -> Option<String> {
    bellwether_core::token::read(home, env).ok().map(|(t, _)| t)
}

/// Why there is no usable token, in words (never the token).
pub fn token_problem() -> String {
    match TOKEN_SOURCE.get() {
        Some(TokenSource::App(home) | TokenSource::Repository(home)) => match bellwether_core::token::read(home, std::env::var("BELLWETHER_HOOK_TOKEN").ok()) {
            Ok(_) => "the token is usable now".into(),
            // The token module's message already says what to run.
            Err(e) => e.message,
        },
        None => "no hook token: BELLWETHER_HOOK_TOKEN is not set (`bellwether token init` makes one)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A checkout's home: BELLWETHER_HOME beats BELLWETHER_DATA_DIR, which beats `data/`;
    /// an empty value counts as unset (M11-b).
    #[test]
    fn a_checkouts_home_is_bellwether_home_then_the_data_dir_then_data() {
        let agent = Path::new("/x/checkout");
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| std::ffi::OsString::from(v)).filter(|v| !v.is_empty());
        assert_eq!(repository_home(env(&[("HOME", "/h"), ("DATA_DIR", "/d")]), agent), PathBuf::from("/h"));
        assert_eq!(repository_home(env(&[("DATA_DIR", "/d")]), agent), PathBuf::from("/d"));
        assert_eq!(repository_home(env(&[("HOME", ""), ("DATA_DIR", "/d")]), agent), PathBuf::from("/d"));
        assert_eq!(repository_home(env(&[]), agent), agent.join("data"));
        // The old name is not read.
        assert_eq!(repository_home(env(&[("PASTURE_DATA_DIR", "/p")]), agent), agent.join("data"));
    }

    /// The token: an exported one, then the home's `hook-token`, then nothing; a `.env`
    /// beside it (with a different token, under either name) is never read.
    #[test]
    fn a_checkouts_token_is_the_environment_then_hook_token_never_a_dotenv() {
        let dir = std::env::temp_dir().join(format!("bellwether-token-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env"), "BELLWETHER_HOOK_TOKEN=fromDotEnv_0123456789ab\nPASTURE_HOOK_TOKEN=oldDotEnv_0123456789ab\n").unwrap();
        assert_eq!(token_at(&dir, None), None, "no hook-token: nothing, whatever the .env holds");
        assert_eq!(token_at(&dir, Some(String::new())), None, "an empty export counts as none");
        std::fs::write(dir.join("hook-token"), "fromFile_0123456789abcd\n").unwrap();
        assert_eq!(token_at(&dir, None).as_deref(), Some("fromFile_0123456789abcd"));
        assert_eq!(token_at(&dir, Some("fromEnv_0123456789abcd".into())).as_deref(), Some("fromEnv_0123456789abcd"), "an exported token beats hook-token");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `hook-token` is a FIFO nobody writes to, so opening it blocks: the token file must be
    /// read only when the environment has no token. (A test that only checked the answer
    /// passed even with the read moved above the early return.)
    // A FIFO: Unix only (Windows plan W1).
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_read_only_when_the_environment_has_no_token() {
        let dir = std::env::temp_dir().join(format!("bellwether-lazy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = std::ffi::CString::new(dir.join("hook-token").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "mkfifo");
        let (tx, rx) = std::sync::mpsc::channel();
        let d = dir.clone();
        // If the FIFO is opened, this thread blocks for good; the test fails on the timeout.
        std::thread::spawn(move || {
            let _ = tx.send(token_at(&d, Some("fromEnv_0123456789abcd".to_string())));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(3)).expect("token_at blocked: it opened hook-token although the environment has the token");
        assert_eq!(got.as_deref(), Some("fromEnv_0123456789abcd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_written_later_is_found_on_the_next_look() {
        let dir = std::env::temp_dir().join(format!("bellwether-later-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(token_at(&dir, None), None, "no token yet");
        std::fs::write(dir.join("hook-token"), "appearedLater_0123456789\n").unwrap();
        assert_eq!(token_at(&dir, None).as_deref(), Some("appearedLater_0123456789"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Source check: the repository layout reads no `.env` and no `PASTURE_*` name.
    #[test]
    fn the_repository_layout_reads_no_dotenv_and_no_old_names() {
        let src = include_str!("settings.rs");
        let repo = src.split("fn load_repository()").nth(1).unwrap().split("\n    pub fn f64_at").next().unwrap();
        assert!(!repo.contains("env_os(") && !repo.contains(".env\"") && !repo.contains("PASTURE"), "load_repository reads an old name or a .env");
    }
}
