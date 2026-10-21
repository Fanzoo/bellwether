//! Updates (distribution plan D7, milestone 9): `tauri-plugin-updater`, driven from Rust
//! only. No capability grants an `updater:` permission, so no webview can reach it.
//!
//! - **Who updates.** Only the Developer ID app: inside a bundle whose id is the permanent
//!   one, not a test build (a bundled test port or home), not signed ad hoc, and built with
//!   the updater configuration of `tauri.dist.conf.json` (endpoint and public key). A
//!   repository build, a dev build and an ad-hoc or test bundle never update themselves;
//!   the strip logs why at start (`updates: off (…)`).
//! - **When.** 10 s after start and every 24 h (maintainer decision 7: check daily and ask),
//!   unless config.json has `"updates": {"check": "off"}`; and whenever the tray's "Check
//!   for Updates…" is chosen (that one works with `"off"` too, and says when there is
//!   nothing new).
//! - **Asking.** A native alert (NSAlert, on the main thread). Install downloads the
//!   signed `.app.tar.gz`, checks its minisign signature against the bundled public key
//!   (the plugin refuses anything else), swaps the bundle and relaunches. The relaunched
//!   app then restarts this app's own brain agent when its version differs (agent.rs,
//!   plan D5, deviation 135). Nothing here restarts it before the relaunch: the new
//!   version's strip does it, with the new version's code.
//! - **Tests.** A debug build only (never a release build) reads
//!   `BELLWETHER_UPDATE_ENDPOINT` (a `latest.json` on a local server; plan's test plan,
//!   M9), `BELLWETHER_UPDATE_ANSWER` (`install` or `later`, so a scripted test needs no
//!   click) and `BELLWETHER_UPDATE_FIRST_CHECK_SEC`. With the endpoint set, a debug bundle
//!   of any id updates (it is how the update is tested without the Developer ID).

use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Runtime};

use crate::log::log;
use crate::settings::Layout;

/// What `tauri.dist.conf.json` carries until the maintainer pastes the real public key.
pub const PUBKEY_PLACEHOLDER: &str = "SET-ME";

/// The release feed: GitHub's "latest release" asset (drafts and pre-releases are skipped
/// by GitHub, so an rc reaches users only once it is published as the latest release).
/// It lives in tauri.dist.conf.json; a test pins the two together.
#[cfg_attr(not(test), allow(dead_code))]
pub const ENDPOINT: &str = "https://github.com/Fanzoo/bellwether/releases/latest/download/latest.json";

const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const TIMEOUT: Duration = Duration::from_secs(60);

/// `updates.check` in config.json.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The default: check at start and daily, and ask before installing.
    Prompt,
    /// No automatic checks; the tray item still checks.
    Off,
}

/// The policy, and a warning for a value that isn't one of the two.
pub fn policy(config: &Value) -> (Policy, Option<String>) {
    match config.pointer("/updates/check") {
        None | Some(Value::Null) => (Policy::Prompt, None),
        Some(Value::String(s)) if s == "prompt" => (Policy::Prompt, None),
        Some(Value::String(s)) if s == "off" => (Policy::Off, None),
        Some(other) => (Policy::Prompt, Some(format!("config: updates.check is {other}; it takes \"prompt\" or \"off\", so \"prompt\" is used"))),
    }
}

/// What decides whether this build may update itself.
#[derive(Clone, Debug)]
pub struct Build<'a> {
    pub layout: Layout,
    /// `plugins.updater` of the compiled-in config (only `tauri.dist.conf.json` has one).
    pub plugin_config: Option<&'a Value>,
    pub bundle_id: Option<&'a str>,
    /// A test port or home in the bundled agent plist.
    pub test_build: bool,
    pub ad_hoc: bool,
    /// A debug build with `BELLWETHER_UPDATE_ENDPOINT` set.
    pub test_endpoint: bool,
}

/// Why this build doesn't update itself, or None when it does.
pub fn refusal(b: &Build) -> Option<String> {
    if b.layout != Layout::App {
        return Some("a repository build; it is updated with git".into());
    }
    let Some(conf) = b.plugin_config else {
        return Some("this build has no updater configuration (only scripts/build-app.mjs builds with tauri.dist.conf.json have one)".into());
    };
    let key = conf.get("pubkey").and_then(Value::as_str).unwrap_or("").trim();
    if key.is_empty() || key.starts_with(PUBKEY_PLACEHOLDER) {
        return Some("no updater public key in tauri.dist.conf.json yet (docs/RELEASING.md, one-time setup)".into());
    }
    if b.test_endpoint {
        return None;
    }
    if b.bundle_id != Some(crate::identity::PERMANENT_ID) {
        return Some(format!("only the Developer ID app ({}) updates itself; this is {}", crate::identity::PERMANENT_ID, b.bundle_id.unwrap_or("an unknown bundle")));
    }
    if b.test_build {
        return Some("a test build (bundled test port or home) never updates itself".into());
    }
    if b.ad_hoc {
        return Some("signed ad hoc; only the Developer ID app updates itself".into());
    }
    None
}

/// The test endpoint, in a debug build only.
#[cfg(debug_assertions)]
pub fn test_endpoint() -> Option<String> {
    std::env::var("BELLWETHER_UPDATE_ENDPOINT").ok().filter(|s| !s.is_empty())
}
#[cfg(not(debug_assertions))]
pub fn test_endpoint() -> Option<String> {
    None
}

/// The scripted answer to the alert, in a debug build only.
#[cfg(debug_assertions)]
fn test_answer() -> Option<Answer> {
    match std::env::var("BELLWETHER_UPDATE_ANSWER").ok()?.as_str() {
        "install" => Some(Answer::Install),
        "later" => Some(Answer::Later),
        _ => None,
    }
}
#[cfg(not(debug_assertions))]
fn test_answer() -> Option<Answer> {
    None
}

#[cfg(debug_assertions)]
fn first_check() -> Duration {
    std::env::var("BELLWETHER_UPDATE_FIRST_CHECK_SEC").ok().and_then(|s| s.parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0).map(Duration::from_secs_f64).unwrap_or(FIRST_CHECK)
}
#[cfg(not(debug_assertions))]
fn first_check() -> Duration {
    FIRST_CHECK
}

/// Who asked for a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// At start or the daily check: quiet unless there is an update.
    Scheduled,
    /// The tray's "Check for Updates…": always says what it found.
    Tray,
}

// Only macOS asks (NSAlert) until W8 (rfd on Windows).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Install,
    Later,
}

/// The alert's words for a found update. Notes are cut to keep the alert small.
pub fn offer_text(current: &str, new: &str, notes: Option<&str>) -> (String, String) {
    let mut info = format!("You have {current}. Bellwether restarts to finish; your sessions' sheep, settings and hooks stay as they are.");
    if let Some(n) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        let cut: String = n.chars().take(600).collect();
        info = format!("{cut}{}\n\n{info}", if cut.len() < n.len() { "…" } else { "" });
    }
    (format!("Bellwether {new} is available"), info)
}

static BUSY: AtomicBool = AtomicBool::new(false);

/// The automatic checks: after start, then daily (not with `updates.check: "off"`).
pub fn schedule<R: Runtime>(app: AppHandle<R>, policy: Policy) {
    if policy == Policy::Off {
        log("updates: automatic checks are off (config updates.check \"off\"); the tray's Check for Updates… still checks");
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(first_check());
        loop {
            check(&app, Trigger::Scheduled);
            std::thread::sleep(EVERY);
        }
    });
}

/// One check, on its own thread; a second one while the first runs is dropped.
pub fn check_in_background<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) {
    let app = app.clone();
    std::thread::spawn(move || check(&app, trigger));
}

fn check<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) {
    if BUSY.swap(true, Ordering::SeqCst) {
        log("updates: a check is already running");
        return;
    }
    let restart = tauri::async_runtime::block_on(run_check(app, trigger));
    BUSY.store(false, Ordering::SeqCst);
    if restart {
        log("updates: installed; relaunching");
        app.restart();
    }
}

/// Returns true when an update was installed and the app should relaunch.
async fn run_check<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) -> bool {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    log(&format!("updates: checking ({}; this is {current})", if trigger == Trigger::Tray { "tray" } else { "scheduled" }));
    let mut builder = app.updater_builder().timeout(TIMEOUT);
    if let Some(url) = test_endpoint() {
        match url.parse() {
            Ok(u) => match builder.endpoints(vec![u]) {
                Ok(b) => builder = b,
                Err(e) => {
                    log(&format!("warning: updates: test endpoint refused: {e}"));
                    return false;
                }
            },
            Err(e) => {
                log(&format!("warning: updates: BELLWETHER_UPDATE_ENDPOINT isn't a URL: {e}"));
                return false;
            }
        }
    }
    let found = match builder.build() {
        Ok(u) => u.check().await,
        Err(e) => Err(e),
    };
    let update = match found {
        Ok(Some(u)) => u,
        Ok(None) => {
            log(&format!("updates: {current} is the newest version"));
            if trigger == Trigger::Tray {
                tell(app, "Bellwether is up to date", &format!("{current} is the newest version."));
            }
            return false;
        }
        Err(e) => {
            log(&format!("warning: updates: the check failed: {e}"));
            if trigger == Trigger::Tray {
                tell(app, "Couldn't check for updates", &format!("{e}\n\nTry again later, or download the newest version from github.com/Fanzoo/bellwether/releases."));
            }
            return false;
        }
    };
    log(&format!("updates: {} is available (this is {current})", update.version));
    let (title, info) = offer_text(&current, &update.version, update.body.as_deref());
    let answer = match test_answer() {
        Some(a) => {
            log(&format!("updates: scripted answer {a:?} (debug build, BELLWETHER_UPDATE_ANSWER)"));
            a
        }
        None => ask(app, &title, &info),
    };
    if answer == Answer::Later {
        log("updates: later");
        return false;
    }
    log(&format!("updates: downloading {}", update.version));
    let started = std::time::Instant::now();
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => {
            log(&format!("updates: {} downloaded, signature checked and installed in {} ms", update.version, started.elapsed().as_millis()));
            true
        }
        Err(e) => {
            log(&format!("error: updates: installing {} failed: {e}", update.version));
            tell(app, "The update wasn't installed", &format!("{e}\n\nBellwether {current} keeps running. The newest version is also at github.com/Fanzoo/bellwether/releases."));
            false
        }
    }
}

/// The offer: "Install and Relaunch" or "Later".
#[cfg(target_os = "macos")]
fn ask<R: Runtime>(app: &AppHandle<R>, title: &str, info: &str) -> Answer {
    match alert(app, title, info, &["Install and Relaunch", "Later"]) {
        Some(0) => Answer::Install,
        _ => Answer::Later,
    }
}
#[cfg(not(target_os = "macos"))]
fn ask<R: Runtime>(_app: &AppHandle<R>, _title: &str, _info: &str) -> Answer {
    Answer::Later
}

/// An alert with one OK button (none while a debug build's test answer is scripted, so a
/// scripted run never waits for a click).
#[cfg(target_os = "macos")]
fn tell<R: Runtime>(app: &AppHandle<R>, title: &str, info: &str) {
    if test_answer().is_some() {
        log(&format!("updates: (scripted run, no alert) {title}"));
        return;
    }
    let _ = alert(app, title, info, &["OK"]);
}
#[cfg(not(target_os = "macos"))]
fn tell<R: Runtime>(_app: &AppHandle<R>, _title: &str, _info: &str) {}

/// An NSAlert on the main thread; the index of the button chosen.
#[cfg(target_os = "macos")]
fn alert<R: Runtime>(app: &AppHandle<R>, title: &str, info: &str, buttons: &[&str]) -> Option<usize> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (title, info, buttons) = (title.to_string(), info.to_string(), buttons.iter().map(|b| b.to_string()).collect::<Vec<_>>());
    app.run_on_main_thread(move || {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};
        use objc2_foundation::NSString;
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(None);
            return;
        };
        // An Accessory app has no Dock icon; bring it forward so the alert isn't behind.
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        let a = NSAlert::new(mtm);
        a.setMessageText(&NSString::from_str(&title));
        a.setInformativeText(&NSString::from_str(&info));
        for b in &buttons {
            a.addButtonWithTitle(&NSString::from_str(b));
        }
        let answer = a.runModal() - NSAlertFirstButtonReturn;
        let _ = tx.send(usize::try_from(answer).ok());
    })
    .ok()?;
    rx.recv().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build<'a>(conf: Option<&'a Value>) -> Build<'a> {
        Build { layout: Layout::App, plugin_config: conf, bundle_id: Some(crate::identity::PERMANENT_ID), test_build: false, ad_hoc: false, test_endpoint: false }
    }

    #[test]
    fn only_the_developer_id_app_with_a_real_key_updates_itself() {
        let real = json!({"pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk", "endpoints": [ENDPOINT]});
        let placeholder = json!({"pubkey": "SET-ME: …", "endpoints": [ENDPOINT]});
        assert_eq!(refusal(&build(Some(&real))), None);
        assert!(refusal(&Build { layout: Layout::Repository, ..build(Some(&real)) }).unwrap().contains("repository"));
        assert!(refusal(&build(None)).unwrap().contains("no updater configuration"));
        assert!(refusal(&build(Some(&placeholder))).unwrap().contains("public key"));
        assert!(refusal(&build(Some(&json!({"endpoints": [ENDPOINT]})))).unwrap().contains("public key"));
        assert!(refusal(&Build { bundle_id: Some("com.fanzootechnology.bellwether.adhoc"), ..build(Some(&real)) }).unwrap().contains("this is com.fanzootechnology.bellwether.adhoc"));
        assert!(refusal(&Build { bundle_id: Some("org.example.other"), ..build(Some(&real)) }).is_some(), "a dev build's id or any other");
        assert!(refusal(&Build { test_build: true, ..build(Some(&real)) }).unwrap().contains("test build"));
        assert!(refusal(&Build { ad_hoc: true, ..build(Some(&real)) }).unwrap().contains("ad hoc"));
        // The debug-only test endpoint lets any bundle update, but never without a key or
        // outside a bundle.
        assert_eq!(refusal(&Build { bundle_id: Some("com.fanzootechnology.bellwether.adhoc"), test_build: true, ad_hoc: true, test_endpoint: true, ..build(Some(&real)) }), None);
        assert!(refusal(&Build { test_endpoint: true, ..build(Some(&placeholder)) }).is_some());
        assert!(refusal(&Build { layout: Layout::Repository, test_endpoint: true, ..build(Some(&real)) }).is_some());
    }

    #[test]
    fn updates_check_is_prompt_or_off() {
        assert_eq!(policy(&json!({})), (Policy::Prompt, None));
        assert_eq!(policy(&json!({"updates": {"check": "prompt"}})), (Policy::Prompt, None));
        assert_eq!(policy(&json!({"updates": {"check": "off"}})), (Policy::Off, None));
        let (p, w) = policy(&json!({"updates": {"check": false}}));
        assert_eq!(p, Policy::Prompt);
        assert!(w.unwrap().contains("\"prompt\" or \"off\""));
    }

    #[test]
    fn the_offer_names_both_versions_and_cuts_long_notes() {
        let (t, i) = offer_text("0.2.0-rc.1", "0.2.0-rc.2", None);
        assert_eq!(t, "Bellwether 0.2.0-rc.2 is available");
        assert!(i.starts_with("You have 0.2.0-rc.1."));
        let (_, i) = offer_text("1", "2", Some(&"n".repeat(2000)));
        assert!(i.starts_with(&"n".repeat(600)) && i[600..].starts_with('…'));
    }

    /// The feed and key are in the distribution overlay only, so dev builds (tauri.conf.json)
    /// have no updater configuration at all.
    #[test]
    fn the_feed_is_in_the_distribution_overlay_only() {
        let dist: Value = serde_json::from_str(include_str!("../tauri.dist.conf.json")).unwrap();
        assert_eq!(dist["plugins"]["updater"]["endpoints"], json!([ENDPOINT]));
        let mut keys: Vec<&String> = dist["plugins"]["updater"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["endpoints", "pubkey", "requireSignedVersion"], "no dangerous* switch, no windows block");
        // The signature must name the version latest.json announces (scripts/release.mjs signs
        // with --app-version), so a tampered feed can't pair a new number with an old build.
        assert_eq!(dist["plugins"]["updater"]["requireSignedVersion"], json!(true));
        assert!(dist["bundle"].get("createUpdaterArtifacts").is_none(), "the updater archive is made after notarization (scripts/release.mjs)");
        let dev: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(dev.get("plugins").is_none());
    }

    /// The test hooks exist in debug builds only: a release build reads none of the
    /// three variables (each has a `not(debug_assertions)` twin that ignores them).
    #[test]
    fn the_test_variables_are_debug_only() {
        let src = include_str!("updater.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for var in ["BELLWETHER_UPDATE_ENDPOINT\")", "BELLWETHER_UPDATE_ANSWER\")", "BELLWETHER_UPDATE_FIRST_CHECK_SEC\")"] {
            let at = code.find(&format!("std::env::var(\"{var}")).unwrap_or_else(|| panic!("{var} not read"));
            let before = &code[..at];
            let gate = before.rfind("#[cfg(").unwrap();
            assert!(before[gate..].starts_with("#[cfg(debug_assertions)]"), "{var} is read outside a debug-only function");
            assert_eq!(code.matches(&format!("std::env::var(\"{var}")).count(), 1);
        }
        assert_eq!(code.matches("#[cfg(not(debug_assertions))]").count(), 3);
        assert!(!code.contains("dangerous"), "no insecure transport switch");
    }
}
