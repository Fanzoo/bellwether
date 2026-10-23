//! Start at login through SMAppService (distribution plan D5): the brain as a login
//! agent whose program is this app's own executable (`Bellwether serve`), and the app
//! itself as the main-app login item. Nothing is ever written to ~/Library/LaunchAgents;
//! the agent's plist lives inside the signed bundle, in Contents/Library/LaunchAgents.
//!
//! SMAppService works only from inside an app bundle; a bare development binary gets
//! a plain error, never a half-registered item.

use crate::identity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// `<bundle id>.brain.plist`: `Bellwether serve`, kept alive by launchd.
    Brain,
    /// The app (the strip) at login.
    MainApp,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::Brain => "brain agent",
            Service::MainApp => "app at login",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    /// Not inside an app bundle, or not macOS 13+.
    Unavailable,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::NotRegistered => "notRegistered",
            Status::Enabled => "enabled",
            Status::RequiresApproval => "requiresApproval",
            Status::NotFound => "notFound",
            Status::Unavailable => "unavailable",
        }
    }
}

pub const STALE_SESSION: &str = "can't check or change login items from this terminal session: it predates your last login (launchd won't answer it, which happens in a terminal whose server outlived a logout, such as herdr or tmux). Open a new terminal window and run the command there";

/// What the bundled brain agent will run with, from its plist: (`--port`, BELLWETHER_HOME).
/// A test build (`build-app.mjs --test-port/--test-home`) carries them; a release build
/// carries neither.
pub fn agent_overrides(plist: &str) -> (Option<u16>, Option<String>) {
    let strings: Vec<&str> = plist.split("<string>").skip(1).filter_map(|s| s.split("</string>").next()).collect();
    let port = strings.iter().position(|s| *s == "--port").and_then(|i| strings.get(i + 1)).and_then(|p| p.parse().ok());
    let home = plist.split("<key>BELLWETHER_HOME</key>").nth(1).and_then(|r| r.split("<string>").nth(1)).and_then(|r| r.split("</string>").next()).map(|h| h.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"));
    (port, home)
}

/// `autostart install` refuses what would start at login with the wrong settings
/// (M1 finding F10). The app login item gets no environment and no arguments, so its
/// strip always uses the default port (47417) and the default home: from a test build,
/// or with BELLWETHER_HOME set, it would show a live install's flock. The brain agent
/// runs with what its bundled plist says, so a BELLWETHER_HOME that differs from that
/// would be silently ignored.
pub fn install_refusal(services: &[Service], overrides: (Option<u16>, Option<String>), env_home: Option<String>) -> Result<(), String> {
    let (port, home) = overrides;
    if services.contains(&Service::MainApp) && (port.is_some() || home.is_some() || env_home.is_some()) {
        let why = match (port, &home, &env_home) {
            (Some(p), _, _) => format!("this is a test build (its brain agent runs on port {p})"),
            (None, Some(h), _) => format!("this is a test build (its brain agent uses BELLWETHER_HOME {h})"),
            (None, None, Some(h)) => format!("BELLWETHER_HOME is set ({h})"),
            _ => unreachable!(),
        };
        return Err(format!("refused: {why}, but the app started at login gets no environment, so its strip would use the defaults: port {} and ~/Library/Application Support/Bellwether. Use --brain-only with test builds", bellwether_brain::DEFAULT_PORT));
    }
    if services.contains(&Service::Brain) {
        if let Some(e) = env_home.filter(|e| Some(e) != home.as_ref()) {
            return Err(format!("refused: BELLWETHER_HOME is set ({e}), but the brain agent will use {}: the bundle decides (build with `--test-home` for a test home)", home.unwrap_or_else(|| "~/Library/Application Support/Bellwether".into())));
        }
    }
    Ok(())
}

/// The bundled agent's (`--port`, BELLWETHER_HOME), read from the bundle.
pub fn bundle_agent_overrides() -> (Option<u16>, Option<String>) {
    let (Some(bundle), Some(name)) = (crate::identity::bundle_path(), brain_plist_name()) else { return (None, None) };
    agent_overrides(&std::fs::read_to_string(bundle.join("Contents").join("Library").join("LaunchAgents").join(name)).unwrap_or_default())
}

/// The checks every change of a login item passes first, whichever path asks for it
/// (the CLI, or the app's `--action` hand-off): not from an old login session (F9), and
/// for a registration, nothing that would start with the wrong port or home (F10).
///
/// `agent_token` is the brain agent's own token, read from its home as the agent will read
/// it (no environment): a registered agent without a usable one exits 3 at once and
/// launchd starts it again every 10 s, so registering the brain is refused until `token
/// init` has run (deviation 60).
pub fn preflight(services: &[Service], registering: bool, session: crate::identity::Session, overrides: (Option<u16>, Option<String>), env_home: Option<String>, agent_token: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
    if session == crate::identity::Session::Stale {
        return Err(STALE_SESSION.to_string());
    }
    if registering {
        install_refusal(services, overrides, env_home)?;
        if services.contains(&Service::Brain) {
            agent_token()?;
        }
    }
    Ok(())
}

/// The home the bundled brain agent uses: its plist's BELLWETHER_HOME (a test build), else
/// ~/Library/Application Support/Bellwether under this process's HOME.
pub fn agent_home(overrides: &(Option<u16>, Option<String>)) -> std::path::PathBuf {
    match &overrides.1 {
        Some(h) => std::path::PathBuf::from(h),
        None => bellwether_core::paths::user_home().unwrap_or_default().join("Library").join("Application Support").join("Bellwether"),
    }
}

/// Whether `home` holds a hook token the brain agent can use; the refusal if not.
pub fn agent_token_check(home: &std::path::Path) -> Result<(), String> {
    bellwether_core::token::read(home, None).map(|_| ()).map_err(|e| {
        format!(
            "refused: the brain agent would start without a usable hook token ({}). It reads only {} (it gets no environment), and without a token it exits at once and launchd restarts it every 10 s. Run `bellwether token init` first{}, then install again",
            e.message,
            bellwether_core::token::file(home).display(),
            if home.ends_with("Library/Application Support/Bellwether") { String::new() } else { format!(" (with BELLWETHER_HOME={})", home.display()) }
        )
    })
}

/// `preflight` with this process's session, bundle and environment.
pub fn preflight_here(services: &[Service], registering: bool) -> Result<(), String> {
    let overrides = bundle_agent_overrides();
    let home = agent_home(&overrides);
    preflight(services, registering, crate::identity::session(), overrides, std::env::var("BELLWETHER_HOME").ok().filter(|h| !h.is_empty()), &|| agent_token_check(&home))
}

/// The agent plist's file name: `<bundle id>.brain.plist`.
pub fn brain_plist_name() -> Option<String> {
    identity::bundle_id().map(|id| format!("{id}.brain.plist"))
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{Service, Status};
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    fn service(s: Service) -> Result<Retained<SMAppService>, String> {
        if !crate::identity::in_app_bundle() {
            return Err("not running from Bellwether.app: SMAppService needs the app bundle (build it with `node scripts/build-app.mjs`)".into());
        }
        Ok(match s {
            Service::Brain => {
                let name = super::brain_plist_name().ok_or("the bundle has no CFBundleIdentifier")?;
                unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(&name)) }
            }
            Service::MainApp => unsafe { SMAppService::mainAppService() },
        })
    }

    fn map(st: SMAppServiceStatus) -> Status {
        match st {
            SMAppServiceStatus::NotRegistered => Status::NotRegistered,
            SMAppServiceStatus::Enabled => Status::Enabled,
            SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
            SMAppServiceStatus::NotFound => Status::NotFound,
            _ => Status::Unavailable,
        }
    }

    pub fn status(s: Service) -> Status {
        match service(s) {
            Ok(svc) => map(unsafe { svc.status() }),
            Err(_) => Status::Unavailable,
        }
    }

    pub fn register(s: Service) -> Result<Status, String> {
        super::preflight_here(&[s], true)?;
        if crate::identity::bundle_id().as_deref() == Some(crate::identity::PERMANENT_ID) && crate::identity::ad_hoc() {
            return Err(format!("refused: this build is signed ad hoc but carries the permanent id {}. Registering it would leave a login-item record whose launch constraint the Developer ID build then fails. Use a Developer ID build, or `node scripts/build-app.mjs --adhoc`, which uses its own id", crate::identity::PERMANENT_ID));
        }
        let svc = service(s)?;
        unsafe { svc.registerAndReturnError() }.map_err(|e| format!("{} (error {})", e.localizedDescription(), e.code()))?;
        Ok(map(unsafe { svc.status() }))
    }

    pub fn unregister(s: Service) -> Result<Status, String> {
        super::preflight_here(&[s], false)?;
        let svc = service(s)?;
        unsafe { svc.unregisterAndReturnError() }.map_err(|e| format!("{} (error {})", e.localizedDescription(), e.code()))?;
        Ok(map(unsafe { svc.status() }))
    }

    pub fn open_settings() {
        unsafe { SMAppService::openSystemSettingsLoginItems() }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{open_settings, register, status, unregister};

#[cfg(not(target_os = "macos"))]
pub fn status(_s: Service) -> Status {
    Status::Unavailable
}
#[cfg(not(target_os = "macos"))]
pub fn register(_s: Service) -> Result<Status, String> {
    Err("start at login is macOS-only for now".into())
}
#[cfg(not(target_os = "macos"))]
pub fn unregister(_s: Service) -> Result<Status, String> {
    Err("start at login is macOS-only for now".into())
}
#[cfg(not(target_os = "macos"))]
pub fn open_settings() {}

/// What to do about `requiresApproval`: the item is registered, but macOS won't start it
/// until the user turns it on.
pub const APPROVAL: &str = "registered, but waiting for your approval: open System Settings > General > Login Items & Extensions (Login Items before macOS 15) and turn Bellwether on under \"Allow in the Background\" (`bellwether autostart settings` opens that pane). It won't start until you do";

/// The exit code of `autostart install --write`, from the statuses it ended with: 2 (not
/// running) when any item waits for approval, as macOS won't start it yet.
pub fn install_exit(ended: &[Status]) -> i32 {
    if ended.contains(&Status::RequiresApproval) {
        bellwether_brain::exit::NOT_RUNNING
    } else {
        bellwether_brain::exit::OK
    }
}

/// What to tell the user about a status, in one line.
pub fn advice(s: Service, st: Status) -> String {
    match st {
        Status::Enabled => format!("{}: enabled", s.name()),
        Status::NotRegistered => format!("{}: off (`bellwether autostart install --write` turns it on)", s.name()),
        Status::RequiresApproval => format!("{}: {APPROVAL}", s.name()),
        // macOS reports notFound for a service it has never had registered, as well as
        // for a plist missing from the bundle (measured on macOS 26.6).
        Status::NotFound => format!("{}: off; macOS has never had it registered (`bellwether autostart install --write`). If it still says notFound after that, the bundle lacks the item: rebuild with `node scripts/build-app.mjs`", s.name()),
        Status::Unavailable => format!("{}: unavailable here (needs Bellwether.app on macOS 13 or newer)", s.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Session;

    #[test]
    fn the_brain_agent_needs_a_token_in_its_own_home() {
        let home = std::env::temp_dir().join(format!("bw-agent-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let e = agent_token_check(&home).unwrap_err();
        assert!(e.contains("Run `bellwether token init` first (with BELLWETHER_HOME=") && e.contains("restarts it every 10 s"), "{e}");
        std::fs::write(home.join("hook-token"), "short\n").unwrap();
        assert!(agent_token_check(&home).is_err(), "an unusable token is refused too");
        std::fs::write(home.join("hook-token"), "testTokenNotReal_abcdefghijklmnopq\n").unwrap();
        assert!(agent_token_check(&home).is_ok());
        assert_eq!(agent_home(&(Some(47431), Some("/tmp/x".into()))), std::path::PathBuf::from("/tmp/x"));
        assert!(agent_home(&(None, None)).ends_with("Library/Application Support/Bellwether"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn test_builds_never_start_the_app_at_login() {
        let release = include_str!("../macos/com.fanzootechnology.bellwether.brain.plist");
        assert_eq!(agent_overrides(release), (None, None));
        let test = release.replace("<string>serve</string>", "<string>serve</string>\n    <string>--port</string>\n    <string>47418</string>").replace("  <key>RunAtLoad</key>", "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>BELLWETHER_HOME</key>\n    <string>/tmp/bw-m1</string>\n  </dict>\n  <key>RunAtLoad</key>");
        let o = agent_overrides(&test);
        assert_eq!(o, (Some(47418), Some("/tmp/bw-m1".into())));
        let both = [Service::Brain, Service::MainApp];
        let e = install_refusal(&both, o.clone(), None).unwrap_err();
        assert!(e.contains("port 47418") && e.contains("--brain-only"), "{e}");
        assert!(install_refusal(&[Service::Brain], o.clone(), None).is_ok(), "a test build's brain agent carries its own port and home");
        assert!(install_refusal(&[Service::Brain], o.clone(), Some("/tmp/bw-m1".into())).is_ok());
        assert!(install_refusal(&[Service::Brain], o, Some("/tmp/other".into())).unwrap_err().contains("the bundle decides"));
        assert!(install_refusal(&both, (None, None), None).is_ok(), "a release build may start both");
        assert!(install_refusal(&both, (None, None), Some("/tmp/x".into())).unwrap_err().contains("BELLWETHER_HOME is set"));
        assert!(install_refusal(&[Service::Brain], (None, None), Some("/tmp/x".into())).is_err());
    }

    #[test]
    fn every_path_gets_the_same_checks() {
        // The app's --action hand-off and the CLI both end in register/unregister, which
        // call preflight first: a test build's app item and an old session are refused.
        let test_build = (Some(47418), Some("/tmp/bw-m1".to_string()));
        let ok = || Ok(());
        let no_token = || Err("refused: no token".to_string());
        let e = preflight(&[Service::MainApp], true, Session::Current, test_build.clone(), None, &ok).unwrap_err();
        assert!(e.contains("--brain-only"), "{e}");
        assert!(preflight(&[Service::Brain], true, Session::Current, test_build.clone(), None, &ok).is_ok());
        for registering in [true, false] {
            let e = preflight(&[Service::Brain], registering, Session::Stale, (None, None), None, &ok).unwrap_err();
            assert!(e.contains("predates your last login"), "{e}");
        }
        assert!(preflight(&[Service::MainApp], false, Session::Current, test_build.clone(), None, &no_token).is_ok(), "unregistering is never refused in a current session");
        // No usable token in the agent's home: registering the brain is refused, the app
        // item alone and unregistering are not.
        assert_eq!(preflight(&[Service::Brain], true, Session::Current, test_build.clone(), None, &no_token).unwrap_err(), "refused: no token");
        assert!(preflight(&[Service::Brain], false, Session::Current, test_build.clone(), None, &no_token).is_ok());
        assert!(preflight(&[Service::MainApp], true, Session::Current, (None, None), None, &no_token).is_ok());
        let src = include_str!("login_items.rs");
        let body = |name: &str| src.split(&format!("pub fn {name}(s: Service)")).nth(1).unwrap().split("pub fn").next().unwrap().to_string();
        assert!(body("register").contains("super::preflight_here(&[s], true)?"), "register runs the checks");
        assert!(body("unregister").contains("super::preflight_here(&[s], false)?"), "unregister runs the checks");
    }
}
