//! The app's side of the migration (distribution plan D9, milestone 8): `bellwether migrate
//! --to-app` and `--undo`, and the setup window's step 1. The steps themselves are
//! `bellwether_brain::toapp`; this file is the [`Host`] they run on.
//!
//! Which host, decided before anything is read ([`choose`]):
//! - **the real HOME** (the passwd entry's): [`Real`] — `/bin/launchctl`, SMAppService
//!   through `login_items`, signals by exact pid, and this bundle's strip started and quit as
//!   `bellwether shell start` / `stop` do (`open -g -a`, SIGTERM by exact pid). Refused in a
//!   test process (the test preload's `BELLWETHER_TEST_ROOT` is set), and refused when a test
//!   stand-in is named;
//! - **any other HOME** (a test): [`Stubbed`] — launchctl, the login items and the strip only
//!   through the stand-in programs named by `BELLWETHER_LAUNCHCTL`, `BELLWETHER_LOGIN_ITEMS`
//!   and `BELLWETHER_STRIP` (never `/bin/launchctl` or `/usr/bin/open` themselves); without
//!   them, none is called at all, and no new brain is waited for.
//!
//! So nothing a test does can reach the real launchd, the real login items or a real strip,
//! and the real ones are reached only for the real HOME, from Bellwether.app.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bellwether_brain::legacy::{self, Loaded};
use bellwether_brain::toapp::{self, Holder, Host};
use bellwether_brain::{exit, BrainError};

use crate::login_items::{self, Service};

/// Which host a run gets.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    Real,
    Stubbed { launchctl: Option<PathBuf>, login_items: Option<PathBuf>, strip: Option<PathBuf> },
}

/// What this executable is, for the real host: a release Bellwether.app (the permanent id,
/// no test port or home in its bundle), a test build of it, or anything else (a development
/// or repository binary, an ad-hoc bundle under its own id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppBuild {
    Release,
    TestBuild(String),
    NotTheApp(String),
}

/// This executable, by its resolved path and its bundle (read from the bundle, nothing else).
pub fn this_build() -> AppBuild {
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let Some(bundle) = crate::identity::bundle_path() else {
        return AppBuild::NotTheApp(format!("{} isn't inside an app bundle", exe.map(|e| e.display().to_string()).unwrap_or_else(|| "this executable".into())));
    };
    let id = crate::identity::bundle_id().unwrap_or_default();
    classify(&bundle, exe.as_deref(), &id, crate::login_items::bundle_agent_overrides())
}

/// [`this_build`] from its parts (tests spoof them).
pub fn classify(bundle: &Path, exe: Option<&Path>, id: &str, overrides: (Option<u16>, Option<String>)) -> AppBuild {
    let named = bundle.file_name().is_some_and(|n| n == "Bellwether.app") && exe.is_some_and(|e| e == bundle.join("Contents").join("MacOS").join("Bellwether"));
    if !named || id != crate::identity::PERMANENT_ID {
        return AppBuild::NotTheApp(format!("{} (id {:?}) isn't Bellwether.app's own executable", exe.map(|e| e.display().to_string()).unwrap_or_default(), id));
    }
    match overrides {
        (None, None) => AppBuild::Release,
        (port, home) => AppBuild::TestBuild(format!("{} is a test build ({})", bundle.display(), [port.map(|p| format!("port {p}")), home.map(|h| format!("home {h}"))].into_iter().flatten().collect::<Vec<_>>().join(", "))),
    }
}

/// Decides the host from the facts, before anything is read: whether HOME is the real
/// home, whether this is a test process (`BELLWETHER_TEST_ROOT`), the stand-ins named, and
/// (for the real home) what this executable is: only a release Bellwether.app may act on,
/// or even preview, the real home (BR-3K's M8 item: a stale development binary did read it).
pub fn choose(real_home: bool, test_process: bool, launchctl: Option<&str>, login_items: Option<&str>, strip: Option<&str>, build: impl FnOnce() -> AppBuild) -> Result<Kind, String> {
    let named = [launchctl, login_items, strip].iter().any(|v| v.is_some_and(|s| !s.is_empty()));
    if real_home {
        if test_process {
            return Err("refused: this is a test process (BELLWETHER_TEST_ROOT is set) with your real HOME. A migration from a test only ever runs under a temp HOME; nothing was read or changed".into());
        }
        if named {
            return Err("refused: BELLWETHER_LAUNCHCTL, BELLWETHER_LOGIN_ITEMS and BELLWETHER_STRIP are test stand-ins, honoured only under a temp HOME; unset them. Nothing was read or changed".into());
        }
        return match build() {
            AppBuild::Release => Ok(Kind::Real),
            AppBuild::TestBuild(why) | AppBuild::NotTheApp(why) => Err(format!("refused: the move into the app runs only from an installed Bellwether.app (a release build), and {why}. Nothing was read or changed. Run /Applications/Bellwether.app/Contents/MacOS/Bellwether migrate --to-app, or use its setup window")),
        };
    }
    let stand_in = |var: &str, v: Option<&str>| -> Result<Option<PathBuf>, String> {
        let Some(v) = v.filter(|v| !v.is_empty()) else { return Ok(None) };
        let p = PathBuf::from(v);
        if !p.is_absolute() || !p.is_file() {
            return Err(format!("refused: {var} must name an existing program by its absolute path ({v})"));
        }
        let real = std::fs::canonicalize(&p).unwrap_or(p.clone());
        if real == Path::new("/bin/launchctl") || real.file_name().is_some_and(|n| n == "launchctl") {
            return Err(format!("refused: {var} names launchctl itself ({v}); it must be a stand-in"));
        }
        if real == Path::new("/usr/bin/open") || real.file_name().is_some_and(|n| n == "open") {
            return Err(format!("refused: {var} names open itself ({v}); it must be a stand-in"));
        }
        Ok(Some(p))
    };
    Ok(Kind::Stubbed { launchctl: stand_in("BELLWETHER_LAUNCHCTL", launchctl)?, login_items: stand_in("BELLWETHER_LOGIN_ITEMS", login_items)?, strip: stand_in("BELLWETHER_STRIP", strip)? })
}

/// The host for this process and `user_home`.
pub fn host_for(user_home: &Path) -> Result<Box<dyn Host + Send>, String> {
    let env = |k: &str| std::env::var(k).ok();
    let kind = choose(bellwether_brain::migrate::is_real_home(user_home), env("BELLWETHER_TEST_ROOT").is_some_and(|v| !v.is_empty()), env("BELLWETHER_LAUNCHCTL").as_deref(), env("BELLWETHER_LOGIN_ITEMS").as_deref(), env("BELLWETHER_STRIP").as_deref(), this_build)?;
    Ok(match kind {
        Kind::Real => Box::new(Real),
        Kind::Stubbed { launchctl, login_items, strip } => Box::new(Stubbed { launchctl, login_items, strip }),
    })
}

fn uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    0
}

/// The label of this app's own brain agent.
fn app_label() -> String {
    format!("{}.brain", crate::identity::bundle_id().unwrap_or_else(|| crate::identity::PERMANENT_ID.into()))
}

/// SIGTERM to exactly `pid`, then up to 5 s for it to go.
fn terminate(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
    #[cfg(not(unix))]
    false
}

fn doctor_lines(home: &Path, settings: &Path, port: u16) -> Vec<String> {
    let report = crate::cli::doctor_report(home, Some(settings.to_path_buf()), Some(port), false);
    bellwether_brain::doctor::render(&report).lines().map(str::to_string).collect()
}

/// Runs a program for at most 15 s: Ok(stdout) on success, Err(what it said) otherwise.
fn run(program: &Path, args: &[String]) -> Result<String, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    match legacy::run_output(cmd, Duration::from_secs(15)) {
        Ok((true, out, _)) => Ok(out),
        Ok((false, out, err)) => Err(format!("{} {}", out.trim(), err.trim()).trim().to_string()),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// The real machine: only for the real HOME (see [`choose`]).
pub struct Real;

impl Host for Real {
    fn acts(&self) -> bool {
        true
    }
    fn domain(&self) -> String {
        format!("gui/{}", uid())
    }
    fn loaded(&self, label: &str) -> Loaded {
        legacy::launchd_lookup(label)
    }
    fn app_agent(&self) -> Option<i32> {
        match legacy::launchd_lookup(&app_label()) {
            Loaded::Running(p) => Some(p),
            _ => None,
        }
    }
    fn launchctl(&mut self, args: &[String]) -> Result<(), String> {
        #[cfg(test)]
        panic!("a unit test reached the real launchctl ({args:?})");
        #[cfg(not(test))]
        run(Path::new("/bin/launchctl"), args).map(|_| ())
    }
    fn register(&mut self) -> Result<Vec<String>, String> {
        #[cfg(test)]
        panic!("a unit test reached SMAppService");
        #[cfg(not(test))]
        {
            let mut lines = Vec::new();
            for s in [Service::Brain, Service::MainApp] {
                let st = login_items::register(s).map_err(|e| format!("{}: {e}", s.name()))?;
                lines.push(login_items::advice(s, st));
                if st != login_items::Status::Enabled {
                    return Err(login_items::advice(s, st));
                }
            }
            Ok(lines)
        }
    }
    fn unregister(&mut self) -> Result<Vec<String>, String> {
        #[cfg(test)]
        panic!("a unit test reached SMAppService");
        #[cfg(not(test))]
        {
            let mut lines = Vec::new();
            let mut failed = Vec::new();
            for s in [Service::MainApp, Service::Brain] {
                // Already off (an undo that stopped after this step, run again): SMAppService
                // would answer an error for an item it has no registration of (deviation 134).
                // Only NotRegistered: NotFound says SMAppService couldn't find the service (its
                // plist missing from the bundle, say), which doesn't prove there is no stale
                // registration, so it goes to unregister and its error names the cause.
                let now = login_items::status(s);
                if now == login_items::Status::NotRegistered {
                    lines.push(login_items::advice(s, now));
                    continue;
                }
                match login_items::unregister(s) {
                    Ok(st) => lines.push(login_items::advice(s, st)),
                    Err(e) => failed.push(format!("{}: {e}", s.name())),
                }
            }
            if failed.is_empty() {
                Ok(lines)
            } else {
                Err(failed.join("; "))
            }
        }
    }
    fn registered(&self) -> Option<bool> {
        let st = [Service::Brain, Service::MainApp].map(login_items::status);
        if st.contains(&login_items::Status::Unavailable) {
            None
        } else {
            Some(st.iter().all(|x| *x == login_items::Status::Enabled))
        }
    }
    fn port_holder(&self, port: u16) -> Holder {
        toapp::port_holder(port)
    }
    fn process_path(&self, pid: i32) -> Option<PathBuf> {
        crate::identity::pid_path(pid)
    }
    fn terminate(&mut self, pid: i32) -> bool {
        terminate(pid)
    }
    fn doctor(&self, home: &Path, settings: &Path, port: u16) -> Vec<String> {
        doctor_lines(home, settings, port)
    }
    fn strip_acts(&self) -> bool {
        true
    }
    fn start_strip(&mut self) -> Result<String, String> {
        #[cfg(test)]
        panic!("a unit test reached the real strip (open)");
        #[cfg(not(test))]
        {
            let bundle = crate::identity::bundle_path().ok_or("not part of Bellwether.app")?;
            crate::cli::start_strip(&bundle, false).map_err(|(e, _)| e)
        }
    }
    fn stop_strip(&mut self) -> Result<Vec<String>, String> {
        #[cfg(test)]
        panic!("a unit test reached the real strip (SIGTERM)");
        #[cfg(not(test))]
        {
            // What `stop_own_instances` printed after an undo before deviation 131: anything of
            // this bundle still running once the login items went (deviation 134).
            let quit = crate::cli::quit_strips().map(|(said, _)| said);
            let rest = crate::cli::stragglers(true);
            match quit {
                Ok(mut said) => {
                    if said.is_empty() && !rest.is_empty() {
                        said.push("the app's strip wasn't running".into());
                    }
                    said.extend(rest);
                    Ok(said)
                }
                Err(e) => Err(std::iter::once(e).chain(rest).collect::<Vec<_>>().join("\n")),
            }
        }
    }
}

/// What a temp HOME says instead of asking port 47417.
pub const LIVE_PORT_REFUSAL: &str = "whatever it is: a temp HOME never asks the live port (a test never touches 47417; give the test install another port in its config.json)";

/// A temp HOME: launchctl, the login items and the strip only through the stand-ins (tests).
/// The strip's answers `start` and `stop` with what it did, one line each.
pub struct Stubbed {
    launchctl: Option<PathBuf>,
    login_items: Option<PathBuf>,
    strip: Option<PathBuf>,
}

impl Host for Stubbed {
    fn acts(&self) -> bool {
        self.launchctl.is_some() && self.login_items.is_some()
    }
    fn why_not(&self) -> String {
        "HOME is not your real home directory (a test), and no stand-ins for launchctl and the login items are set".into()
    }
    fn domain(&self) -> String {
        format!("gui/{}", uid())
    }
    fn loaded(&self, label: &str) -> Loaded {
        let Some(l) = &self.launchctl else { return Loaded::NotAsked };
        let target = legacy::target(uid(), label);
        let mut cmd = std::process::Command::new(l);
        cmd.arg("print").arg(&target);
        legacy::run_bounded(cmd, legacy::LAUNCHCTL_TIMEOUT, |ok, out, err| legacy::parse_print(&target, ok, out, err))
    }
    fn app_agent(&self) -> Option<i32> {
        match self.loaded(&app_label()) {
            Loaded::Running(p) => Some(p),
            _ => None,
        }
    }
    fn launchctl(&mut self, args: &[String]) -> Result<(), String> {
        match &self.launchctl {
            Some(l) => run(l, args).map(|_| ()),
            None => Err("no launchctl stand-in".into()),
        }
    }
    fn register(&mut self) -> Result<Vec<String>, String> {
        match &self.login_items {
            Some(p) => run(p, &["register".into()]).map(|o| o.lines().map(str::to_string).collect()),
            None => Err("no login-items stand-in".into()),
        }
    }
    fn unregister(&mut self) -> Result<Vec<String>, String> {
        match &self.login_items {
            Some(p) => run(p, &["unregister".into()]).map(|o| o.lines().map(str::to_string).collect()),
            None => Err("no login-items stand-in".into()),
        }
    }
    fn registered(&self) -> Option<bool> {
        // The stand-in answers `status` with "registered" or "off"; anything else is unknown.
        let p = self.login_items.as_ref()?;
        match run(p, &["status".into()]).ok()?.trim() {
            "registered" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }
    fn port_holder(&self, port: u16) -> Holder {
        // A temp HOME never asks the live port, whose brain is the maintainer's own: the
        // run stops there, as for any port held by something else.
        if port == bellwether_brain::DEFAULT_PORT {
            return Holder::Other { pid: None, command: Some(LIVE_PORT_REFUSAL.into()) };
        }
        toapp::port_holder(port)
    }
    fn process_path(&self, pid: i32) -> Option<PathBuf> {
        crate::identity::pid_path(pid)
    }
    fn terminate(&mut self, pid: i32) -> bool {
        terminate(pid)
    }
    fn doctor(&self, home: &Path, settings: &Path, port: u16) -> Vec<String> {
        doctor_lines(home, settings, port)
    }
    fn verify_limit(&self) -> Duration {
        // A test may shorten the wait; the real host always waits D9's 10 s.
        std::env::var("BELLWETHER_TEST_VERIFY_MS").ok().and_then(|v| v.parse().ok()).map(Duration::from_millis).unwrap_or(toapp::VERIFY_LIMIT)
    }
    fn strip_acts(&self) -> bool {
        self.strip.is_some()
    }
    fn strip_why_not(&self) -> String {
        "HOME is not your real home directory (a test), and no stand-in for the strip is set".into()
    }
    fn start_strip(&mut self) -> Result<String, String> {
        match &self.strip {
            Some(p) => run(p, &["start".into()]),
            None => Err("no strip stand-in".into()),
        }
    }
    fn stop_strip(&mut self) -> Result<Vec<String>, String> {
        match &self.strip {
            Some(p) => run(p, &["stop".into()]).map(|o| o.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()),
            None => Err("no strip stand-in".into()),
        }
    }
}

/// Before a real `--write`: what would make registering the login items fail, checked while
/// nothing has changed (otherwise step 4 would stop the old agents only to undo it).
pub fn real_write_refusal() -> Option<String> {
    if !crate::identity::in_app_bundle() {
        return Some("not running from Bellwether.app: the migration registers the app's login items (SMAppService), which a development binary can't. Nothing was changed. Run Bellwether.app's `Contents/MacOS/Bellwether migrate --to-app --write`, or use its setup window".into());
    }
    if crate::identity::session() == crate::identity::Session::Stale {
        return Some(format!("{}. Nothing was changed", login_items::STALE_SESSION));
    }
    if let Err(e) = login_items::install_refusal(&[Service::Brain, Service::MainApp], login_items::bundle_agent_overrides(), std::env::var("BELLWETHER_HOME").ok().filter(|h| !h.is_empty())) {
        return Some(format!("{e}. Nothing was changed"));
    }
    if crate::identity::bundle_id().as_deref() == Some(crate::identity::PERMANENT_ID) && crate::identity::ad_hoc() {
        return Some("refused: this build is signed ad hoc but carries the permanent id; use a Developer ID build. Nothing was changed".into());
    }
    None
}

const USAGE: &str = "usage: bellwether migrate --to-app [--from <dir>] [--env <file>] [--settings <path>] [--force] [--write]\n       bellwether migrate --undo [--write]";

/// `bellwether migrate --to-app …` and `bellwether migrate --undo …`.
pub fn command(args: &[String]) -> Result<i32, (String, i32)> {
    let a = bellwether_core::cliargs::parse(args, &["to-app", "undo", "write", "force"], &["from", "env", "settings"], 0).map_err(|e| (e.message, e.code))?;
    if a.has("to-app") == a.has("undo") {
        return Err((format!("give one of --to-app and --undo\n{USAGE}"), exit::BAD_CONFIG));
    }
    if a.has("undo") && (a.has("from") || a.has("env") || a.has("force")) {
        return Err((format!("--undo takes only --write (and --settings)\n{USAGE}"), exit::BAD_CONFIG));
    }
    let user_home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    // First of all, before any file is read: which host, and whether this run may act.
    let mut host = host_for(&user_home).map_err(|e| (e, exit::BAD_CONFIG))?;
    let real = bellwether_brain::migrate::is_real_home(&user_home);
    if real && a.has("write") {
        if let Some(why) = real_write_refusal() {
            return Err((why, exit::NOT_RUNNING));
        }
    }
    let home = bellwether_brain::home::dir();
    bellwether_brain::cli::remember_token(&home);
    let _ = bellwether_brain::cli::config(&home).map_err(|e: BrainError| (e.message, e.code))?;
    let o = toapp::Options {
        from: a.value("from").map(bellwether_brain::cli::user_path),
        env_from: a.value("env").map(bellwether_brain::cli::user_path),
        settings: a.value("settings").map(bellwether_brain::cli::user_path).unwrap_or_else(bellwether_core::outfile::user_settings),
        write: a.has("write"),
        force: a.has("force"),
        home,
        user_home,
        // The CLI looks after the strip: started at the end of a migration, quit by an undo.
        strip: true,
    };
    let r = if a.has("undo") { toapp::undo(&o, host.as_mut()) } else { toapp::to_app(&o, host.as_mut()) };
    match r {
        Ok(out) => {
            bellwether_brain::cli::out(&(out.lines.join("\n") + "\n"));
            Ok(exit::OK)
        }
        Err(e) => Err((e.message, e.code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn release() -> AppBuild {
        AppBuild::Release
    }

    #[test]
    fn the_real_home_only_from_an_installed_release_app() {
        // Spoofed builds only: nothing here runs as an app or reads the real home.
        let not_app = || AppBuild::NotTheApp("/x/target-dev/release-check/release/bellwether-shell isn't inside an app bundle".into());
        let e = choose(true, false, None, None, None, not_app).unwrap_err();
        assert!(e.contains("only from an installed Bellwether.app") && e.contains("Nothing was read or changed") && e.contains("release-check"), "{e}");
        let e = choose(true, false, None, None, None, || AppBuild::TestBuild("/x/Bellwether.app is a test build (port 47431)".into())).unwrap_err();
        assert!(e.contains("test build"), "{e}");
        // A temp HOME never asks what the build is (stand-ins only).
        assert_eq!(choose(false, false, None, None, None, || panic!("not asked")), Ok(Kind::Stubbed { launchctl: None, login_items: None, strip: None }));
        // What counts: Bellwether.app's own executable (resolved), the permanent id, no test flags.
        let b = Path::new("/Applications/Bellwether.app");
        let exe = b.join("Contents/MacOS/Bellwether");
        assert_eq!(classify(b, Some(&exe), crate::identity::PERMANENT_ID, (None, None)), AppBuild::Release);
        assert!(matches!(classify(b, Some(&exe), crate::identity::PERMANENT_ID, (Some(47431), None)), AppBuild::TestBuild(_)));
        assert!(matches!(classify(b, Some(&exe), crate::identity::PERMANENT_ID, (None, Some("/tmp/bw".into()))), AppBuild::TestBuild(_)));
        assert!(matches!(classify(b, Some(&exe), "com.fanzootechnology.bellwether.adhoc", (None, None)), AppBuild::NotTheApp(_)), "an ad-hoc bundle's own id");
        assert!(matches!(classify(Path::new("/x/Other.app"), Some(Path::new("/x/Other.app/Contents/MacOS/Bellwether")), crate::identity::PERMANENT_ID, (None, None)), AppBuild::NotTheApp(_)));
        assert!(matches!(classify(b, Some(&b.join("Contents/MacOS/bellwether-shell")), crate::identity::PERMANENT_ID, (None, None)), AppBuild::NotTheApp(_)));
        assert!(matches!(classify(b, None, crate::identity::PERMANENT_ID, (None, None)), AppBuild::NotTheApp(_)));
        // This test binary itself is not the app.
        assert!(matches!(this_build(), AppBuild::NotTheApp(_)));
    }

    // Symlinks and shell-script stand-ins: Unix only (Windows plan W1).
    #[cfg(unix)]
    #[test]
    fn the_real_machine_only_for_the_real_home_and_never_from_a_test() {
        assert_eq!(choose(true, false, None, None, None, release), Ok(Kind::Real));
        assert!(choose(true, true, None, None, None, release).unwrap_err().contains("test process"), "a test with the real HOME is refused");
        assert!(choose(true, false, Some("/tmp/fake"), None, None, release).unwrap_err().contains("only under a temp HOME"));
        assert!(choose(true, false, None, Some("/tmp/fake"), None, release).is_err());
        // A temp HOME: stand-ins only, and never launchctl itself.
        assert_eq!(choose(false, true, None, None, None, release), Ok(Kind::Stubbed { launchctl: None, login_items: None, strip: None }));
        assert!(choose(false, false, Some("/bin/launchctl"), None, None, release).unwrap_err().contains("launchctl itself"));
        assert!(choose(false, false, Some("relative/fake"), None, None, release).unwrap_err().contains("absolute path"));
        assert!(choose(false, false, Some("/nonexistent/fake"), None, None, release).is_err());
        let dir = std::env::temp_dir().join(format!("bw-migration-choose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-launchctl");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        let named = dir.join("launchctl");
        std::fs::write(&named, "#!/bin/sh\n").unwrap();
        assert!(choose(false, false, named.to_str(), None, None, release).unwrap_err().contains("launchctl itself"), "not even a copy named launchctl");
        let link = dir.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("/bin/launchctl", &link).unwrap();
        assert!(choose(false, false, link.to_str(), None, None, release).unwrap_err().contains("launchctl itself"), "not through a symlink");
        assert_eq!(choose(false, false, fake.to_str(), fake.to_str(), None, release), Ok(Kind::Stubbed { launchctl: Some(fake.clone()), login_items: Some(fake.clone()), strip: None }));
        // The strip's stand-in: refused with the real HOME, and never open (or launchctl) itself.
        assert!(choose(true, false, None, None, Some("/tmp/fake"), release).unwrap_err().contains("BELLWETHER_STRIP"));
        assert!(choose(false, false, None, None, Some("/usr/bin/open"), release).unwrap_err().contains("open itself"));
        let open_copy = dir.join("open");
        std::fs::write(&open_copy, "#!/bin/sh\n").unwrap();
        assert!(choose(false, false, None, None, open_copy.to_str(), release).unwrap_err().contains("open itself"), "not even a copy named open");
        assert!(choose(false, false, None, None, link.to_str(), release).unwrap_err().contains("launchctl itself"));
        assert_eq!(choose(false, false, fake.to_str(), fake.to_str(), fake.to_str(), release), Ok(Kind::Stubbed { launchctl: Some(fake.clone()), login_items: Some(fake.clone()), strip: Some(fake.clone()) }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_temp_home_without_stand_ins_calls_nothing() {
        let mut h = Stubbed { launchctl: None, login_items: None, strip: None };
        assert!(!h.acts());
        assert_eq!(h.loaded("local.bellwether.brain"), Loaded::NotAsked);
        assert!(h.launchctl(&["bootout".into(), "gui/501/local.bellwether.brain".into()]).is_err());
        assert!(h.register().is_err());
        assert!(h.unregister().is_err());
        assert!(!h.strip_acts());
        assert!(h.start_strip().is_err() && h.stop_strip().is_err());
        // The live port is never asked under a temp HOME.
        assert_eq!(h.port_holder(47417), Holder::Other { pid: None, command: Some(LIVE_PORT_REFUSAL.into()) });
        let e = toapp::port_verdict(47417, &h.port_holder(47417), &[], None).unwrap_err();
        assert!(e.contains("a temp HOME never asks the live port"), "{e}");
    }

    #[test]
    fn the_real_host_never_runs_in_a_unit_test() {
        // Read, not called: every mutating call of the real host starts with a panic in a
        // test build, so no unit test can reach launchctl or SMAppService through it.
        let src = include_str!("migration.rs");
        let real = src.split("impl Host for Real {").nth(1).unwrap().split("\n}\n").next().unwrap();
        for f in ["fn launchctl(", "fn register(", "fn unregister(", "fn start_strip(", "fn stop_strip("] {
            let body = real.split(f).nth(1).unwrap_or_else(|| panic!("{f}"));
            let first = body.split('{').nth(1).unwrap().trim_start();
            assert!(first.starts_with("#[cfg(test)]\n        panic!("), "{f} must panic first in a test build");
        }
        assert_eq!(real.matches("/bin/launchctl").count(), 1, "one call site of the real launchctl");
        // The host is chosen, from HOME and the environment, before anything else in `command`.
        let body = src.split("pub fn command(").nth(1).unwrap();
        assert!(body.find("host_for(&user_home)").unwrap() < body.find("home::dir()").unwrap());
        assert!(body.find("real_write_refusal()").unwrap() < body.find("toapp::to_app").unwrap());
        // The CLI looks after the strip (step 8, and undo); the setup window never does.
        assert!(body.contains("strip: true,"));
        assert!(include_str!("onboarding.rs").split("fn migration_options(").nth(1).unwrap().split("\n    }\n").next().unwrap().contains("strip: false"));
    }
}
