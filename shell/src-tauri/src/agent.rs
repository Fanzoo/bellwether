//! This app's own brain agent (`<bundle id>.brain`, registered with SMAppService): whether
//! the brain answering on the port is it, and restarting it when it runs another version
//! than the app (distribution plan D5; deviation 135).
//!
//! An update swaps the bundle and relaunches the app (the strip), but launchd keeps the
//! agent's old process running: its executable was moved out of the bundle and deleted, so
//! it is still the version from before the update. At start the strip compares the
//! versions and, when the brain on the port is this app's agent, restarts it with
//! `launchctl kickstart -k` ([`reconcile`]). A brain is this app's agent when launchd's pid
//! for the agent's label is the pid `/health` reports AND that pid runs this bundle's
//! executable, or an executable since deleted ([`owner`]). Before deviation 135 a deleted
//! one counted as foreign (`proc_pidpath` fails with ENOENT after the update's swap), so
//! nothing was ever restarted. Any other brain (one started by hand, a repository install's
//! `local.bellwether.*` agent, the agent of another copy of the app with the same id) is
//! left alone.
//!
//! Which launchd, decided before anything is asked ([`host`]):
//! - **no stand-in named**: [`Real`] — `/bin/launchctl print` (read only) and `kickstart -k`
//!   for this bundle's own label. Outside an app bundle there is no label, so nothing is
//!   asked at all. In a unit test build both calls panic first, so no test can reach them;
//! - **`BELLWETHER_LAUNCHCTL`** names a stand-in (tests): [`StandIn`], only under a temp
//!   HOME and never launchctl itself, the same rules as the migration's (migration.rs);
//! - **a stand-in with the real HOME**: [`Off`], which asks nothing and says why.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bellwether_brain::legacy::{self, Loaded};

pub use crate::identity::Exe;
use serde_json::Value;

/// How many restarts the strip tries before it gives up and says so (menu and log).
pub const ATTEMPTS: u32 = 2;
/// How long each restart may take to answer with the app's version.
pub const WAIT: Duration = Duration::from_secs(10);
/// How often `/health` is asked while waiting.
const POLL: Duration = Duration::from_millis(50);

/// What the version check needs from launchd.
pub trait Launchd {
    /// This app's brain agent label, if this process has one.
    fn label(&self) -> Option<String>;
    /// launchd's pid for `label` in this user's GUI domain (read only), if it runs.
    fn pid(&self, label: &str) -> Option<i32>;
    /// `launchctl kickstart -k gui/<uid>/<label>`.
    fn kickstart(&mut self, label: &str) -> Result<(), String>;
    /// Why nothing is asked, when that is so.
    fn off(&self) -> Option<String> {
        None
    }
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

/// The real launchd, for this bundle's own label only.
pub struct Real;

impl Launchd for Real {
    fn label(&self) -> Option<String> {
        crate::login_items::brain_plist_name().and_then(|n| n.strip_suffix(".plist").map(str::to_string))
    }
    fn pid(&self, label: &str) -> Option<i32> {
        #[cfg(test)]
        panic!("a unit test reached the real launchctl (print {label})");
        #[cfg(not(test))]
        match legacy::launchd_lookup(label) {
            Loaded::Running(p) => Some(p),
            _ => None,
        }
    }
    fn kickstart(&mut self, label: &str) -> Result<(), String> {
        #[cfg(test)]
        panic!("a unit test reached the real launchctl (kickstart {label})");
        #[cfg(not(test))]
        run(Path::new("/bin/launchctl"), label)
    }
}

/// A stand-in launchctl (tests): `print` and `kickstart -k` go to it, nothing else.
pub struct StandIn {
    pub launchctl: PathBuf,
    pub label: String,
}

impl Launchd for StandIn {
    fn label(&self) -> Option<String> {
        Some(self.label.clone())
    }
    fn pid(&self, label: &str) -> Option<i32> {
        let target = legacy::target(uid(), label);
        let mut cmd = std::process::Command::new(&self.launchctl);
        cmd.arg("print").arg(&target);
        match legacy::run_bounded(cmd, legacy::LAUNCHCTL_TIMEOUT, |ok, out, err| legacy::parse_print(&target, ok, out, err)) {
            Loaded::Running(p) => Some(p),
            _ => None,
        }
    }
    fn kickstart(&mut self, label: &str) -> Result<(), String> {
        run(&self.launchctl, label)
    }
}

/// Asks nothing (a stand-in named with the real HOME).
pub struct Off(pub String);

impl Launchd for Off {
    fn label(&self) -> Option<String> {
        None
    }
    fn pid(&self, _label: &str) -> Option<i32> {
        None
    }
    fn kickstart(&mut self, _label: &str) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn off(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// `<program> kickstart -k gui/<uid>/<label>`, for at most 10 s.
fn run(program: &Path, label: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(["kickstart", "-k", &legacy::target(uid(), label)]);
    match legacy::run_output(cmd, Duration::from_secs(10)) {
        Ok((true, _, _)) => Ok(()),
        Ok((false, out, err)) => Err(format!("{} {}", out.trim(), err.trim()).trim().to_string()),
        Err(legacy::Bounded::Timeout) => Err("no answer within 10 s".into()),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// Which launchd, from the facts: whether HOME is the real home and the stand-in named.
/// Ok(None) is the real one; Err is a refusal (nothing is asked).
pub fn choose(real_home: bool, launchctl: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(v) = launchctl.filter(|v| !v.is_empty()) else { return Ok(None) };
    if real_home {
        return Err("BELLWETHER_LAUNCHCTL is a test stand-in, honoured only under a temp HOME; launchd isn't asked about the brain agent".into());
    }
    let p = PathBuf::from(v);
    if !p.is_absolute() || !p.is_file() {
        return Err(format!("BELLWETHER_LAUNCHCTL must name an existing program by its absolute path ({v}); launchd isn't asked about the brain agent"));
    }
    let real = std::fs::canonicalize(&p).unwrap_or(p.clone());
    if real == Path::new("/bin/launchctl") || real.file_name().is_some_and(|n| n == "launchctl") {
        return Err(format!("BELLWETHER_LAUNCHCTL names launchctl itself ({v}); it must be a stand-in, so launchd isn't asked about the brain agent"));
    }
    Ok(Some(p))
}

/// The launchd for this process.
pub fn host() -> Box<dyn Launchd + Send> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match choose(bellwether_brain::migrate::is_real_home(&home), std::env::var("BELLWETHER_LAUNCHCTL").ok().as_deref()) {
        Ok(None) => Box::new(Real),
        Ok(Some(launchctl)) => {
            let label = format!("{}.brain", crate::identity::bundle_id().unwrap_or_else(|| crate::identity::PERMANENT_ID.into()));
            Box::new(StandIn { launchctl, label })
        }
        Err(why) => Box::new(Off(why)),
    }
}

/// What the version check found, and did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing that says it is a bellwether brain answered `/health`.
    NoBrain,
    Same { pid: i64 },
    /// Another version that isn't this app's agent: left alone.
    Foreign { theirs: String, pid: i64, why: String },
    Restarted { theirs: String, old_pid: i64, new_pid: i64, ms: u128, attempts: u32 },
    /// This app's agent, still at another version after every attempt.
    GaveUp { theirs: String, now: Option<String>, attempts: u32, errors: Vec<String> },
}

impl Outcome {
    /// The strip's log line.
    pub fn log_line(&self, port: u16, ours: &str) -> String {
        match self {
            Outcome::NoBrain => format!("brain on port {port}: none answered at start"),
            Outcome::Same { pid } => format!("brain on port {port}: version {ours}, pid {pid}"),
            Outcome::Foreign { theirs, pid, why } => format!("warning: the brain on port {port} is version {theirs} (pid {pid}), not this app's {ours}, and it isn't this app's login agent ({why}), so it is left alone. `bellwether doctor` shows what it is"),
            Outcome::Restarted { theirs, old_pid, new_pid, ms, attempts } => format!("the brain on port {port} was this app's login agent at version {theirs} (pid {old_pid}); restarted it (launchctl kickstart -k{}): now version {ours} (pid {new_pid}), {ms} ms after the restart", if *attempts > 1 { format!(", attempt {attempts}") } else { String::new() }),
            Outcome::GaveUp { theirs, now, attempts, errors } => format!(
                "warning: the brain on port {port} is this app's login agent at version {theirs}; this app is {ours}. {attempts} restart(s) (launchctl kickstart -k) didn't bring up version {ours} within {} s each{}{}; not trying again until the next start. `bellwether doctor` says what to do",
                WAIT.as_secs(),
                now.as_ref().map(|v| format!(" (it now answers {v})")).unwrap_or_else(|| " (nothing answers now)".into()),
                if errors.is_empty() { String::new() } else { format!("; launchctl said: {}", errors.join("; ")) }
            ),
        }
    }

    /// A line for the tray menu, when the user should know.
    pub fn notice(&self, ours: &str) -> Option<String> {
        match self {
            Outcome::GaveUp { theirs, .. } => Some(format!("Brain is still {theirs}, not {ours} (see bellwether doctor)")),
            _ => None,
        }
    }
}

fn version_of(h: &Value) -> Option<&str> {
    h.get("version").and_then(Value::as_str)
}

/// Everything the version check uses: launchd, which executable is this bundle's, how to
/// tell what a pid runs, and its bounds. [`Check::here`] is the real one; tests build
/// their own (stand-in launchd, made-up executables, short waits).
pub struct Check {
    pub host: Box<dyn Launchd + Send>,
    /// This bundle's executable, resolved; None outside an app bundle.
    pub this_exe: Option<PathBuf>,
    pub exe_of: Box<dyn Fn(i64) -> Exe + Send>,
    pub wait: Duration,
    pub attempts: u32,
}

impl Check {
    pub fn here() -> Check {
        Check { host: host(), this_exe: crate::identity::bundle_executable(), exe_of: Box::new(|pid| crate::identity::pid_exe(pid as i32)), wait: WAIT, attempts: ATTEMPTS }
    }
}

/// Whose the brain on the port is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// launchd's agent for this label, running this bundle's executable.
    Ours,
    /// launchd's agent for this label, whose executable has been deleted: the bundle was
    /// swapped under it (an update). Restarted, and the restart is checked (below).
    Presumed,
    /// launchd's agent for this label, but running another copy's executable (a second
    /// Bellwether.app with the same id): never this copy's to restart.
    OtherCopy(PathBuf),
    /// Not launchd's agent for this label at all.
    NotAgent(String),
}

/// Whose the brain with `pid` is. A copy of the app elsewhere (Downloads, an opened DMG)
/// has the same label, so launchd's pid alone can't tell; the executable that pid runs can,
/// except after an update deleted it, which only happens to this install's own agent's file
/// while it runs ([`Owner::Presumed`]). launchd can't tell either: for an SMAppService agent
/// `launchctl print` has `path = (submitted by smd.N)` and a bundle-relative `program
/// identifier`, no bundle path (deviation 135).
pub fn owner(c: &Check, label: Option<&str>, pid: i64) -> Owner {
    let Some(label) = label else {
        return Owner::NotAgent(c.host.off().unwrap_or_else(|| "this binary isn't inside an app bundle, so it has no login agent".into()));
    };
    match c.host.pid(label) {
        Some(p) if i64::from(p) == pid && pid > 1 => {}
        Some(p) => return Owner::NotAgent(format!("launchd runs {label} as pid {p}")),
        None => return Owner::NotAgent(format!("launchd isn't running {label}")),
    }
    let Some(this) = &c.this_exe else { return Owner::NotAgent("this binary isn't inside an app bundle".into()) };
    match (c.exe_of)(pid) {
        Exe::At(p) if p.canonicalize().ok().as_ref() == Some(this) || &p == this => Owner::Ours,
        Exe::At(p) => Owner::OtherCopy(p),
        Exe::Deleted => Owner::Presumed,
        Exe::Unknown(e) => Owner::NotAgent(format!("what pid {pid} runs can't be read: {e}")),
    }
}

fn other_copy(label: &str, p: &Path, this: Option<&PathBuf>) -> String {
    format!("{label} runs {}, another copy of the app, not this one ({})", p.display(), this.map(|t| t.display().to_string()).unwrap_or_default())
}

/// Compares the brain on the port with `ours` and, when it is this copy's own agent at
/// another version, restarts it: up to `c.attempts` times, each waiting up to `c.wait` for
/// `/health` to answer with `ours`. Nothing else is ever restarted, and a restart that
/// shows the agent belongs to another copy isn't repeated.
pub fn reconcile(c: &mut Check, health: &mut dyn FnMut() -> Option<Value>, ours: &str) -> Outcome {
    let Some(h) = health().filter(|h| h.get("name").and_then(Value::as_str) == Some("bellwether")) else { return Outcome::NoBrain };
    let pid = h.get("pid").and_then(Value::as_i64).unwrap_or(0);
    let theirs = version_of(&h).unwrap_or("?").to_string();
    if theirs == ours {
        return Outcome::Same { pid };
    }
    let label = c.host.label();
    match owner(c, label.as_deref(), pid) {
        Owner::Ours | Owner::Presumed => {}
        Owner::OtherCopy(p) => return Outcome::Foreign { theirs, pid, why: other_copy(label.as_deref().unwrap_or("?"), &p, c.this_exe.as_ref()) },
        Owner::NotAgent(why) => return Outcome::Foreign { theirs, pid, why },
    }
    let label = label.unwrap_or_default();
    let attempts = c.attempts.max(1);
    let mut errors = Vec::new();
    let mut now = None;
    for attempt in 1..=attempts {
        let started = Instant::now();
        if let Err(e) = c.host.kickstart(&label) {
            errors.push(e);
            now = health().and_then(|h| version_of(&h).map(str::to_string));
            continue;
        }
        let deadline = started + c.wait;
        let last = loop {
            let h = health();
            if let Some(h) = &h {
                if version_of(h) == Some(ours) {
                    let new_pid = h.get("pid").and_then(Value::as_i64).unwrap_or(0);
                    return Outcome::Restarted { theirs, old_pid: pid, new_pid, ms: started.elapsed().as_millis(), attempts: attempt };
                }
            }
            if Instant::now() >= deadline {
                break h;
            }
            std::thread::sleep(POLL);
        };
        now = last.as_ref().and_then(|h| version_of(h).map(str::to_string));
        // The restarted agent runs another copy's executable: the agent is that copy's.
        if let Some(new_pid) = last.as_ref().and_then(|h| h.get("pid")).and_then(Value::as_i64) {
            if let Exe::At(p) = (c.exe_of)(new_pid) {
                if c.this_exe.as_ref().is_some_and(|t| p.canonicalize().ok().as_ref() != Some(t) && &p != t) {
                    return Outcome::Foreign { theirs, pid: new_pid, why: format!("after one restart, {}; not restarting it again", other_copy(&label, &p, c.this_exe.as_ref())) };
                }
            }
        }
    }
    Outcome::GaveUp { theirs, now, attempts, errors }
}

/// Doctor's word on a brain of another version than the app's: the brain line becomes a
/// WARN, and this is appended to it.
pub fn doctor_suffix(theirs: &str, ours: &str, pid: i64, label: Option<&str>, owner: &Owner) -> String {
    match (owner, label) {
        (Owner::Ours | Owner::Presumed, Some(l)) => format!(
            "; this app is {ours}: this brain is this app's login agent, still running {theirs} (after an update, the app restarts it when it starts). Quit Bellwether from its menu-bar sheep and open it again, or run `launchctl kickstart -k gui/$(id -u)/{l}`"
        ),
        (Owner::OtherCopy(p), _) => format!(
            "; this app is {ours}, and this brain is the login agent of another copy of Bellwether ({}), so this copy leaves it alone. Use one copy: quit this one and open the other, or move this one to /Applications in place of the other",
            p.display()
        ),
        (Owner::NotAgent(whose), _) => format!(
            "; this app is {ours}, and this brain isn't this app's login agent ({whose}), so Bellwether leaves it alone: a brain started by hand, a repository install's (see `legacy agents`), or another copy of the app's. Stop it (pid {pid}), then quit and reopen Bellwether so its own brain takes the port"
        ),
        (_, None) => doctor_suffix(theirs, ours, pid, None, &Owner::NotAgent("no login agent label".into())),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in launchctl: `print` answers launchd's pid for the label from `$dir/agent-pid`
    /// (none: not running); `kickstart` is logged and "restarts" the agent as pid 5002: with
    /// `$dir/new-exe`, one running that file's path (another copy) at the old version; else,
    /// unless `$dir/stuck` exists, one at `$dir/new-version`. The "brain" is the health
    /// closure below (`$dir/version`, `$dir/pid`); what a pid runs is `$dir/exe-<pid>`
    /// ("deleted", or a path), this bundle's executable when there is no such file.
    const FAKE: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/launchctl.log"
case "$1" in
  print)
    if [ -f "$D/agent-pid" ]; then printf '%s = {\n\tpath = (submitted by smd.528)\n\tstate = running\n\tprogram identifier = Contents/MacOS/Bellwether (mode: 2)\n\tpid = %s\n}\n' "$2" "$(cat "$D/agent-pid")"; exit 0; fi
    echo "Could not find service \"x\" in domain for user gui: 501" >&2; exit 113 ;;
  kickstart)
    [ -f "$D/refuse" ] && { echo "Could not kickstart service: 1: Operation not permitted" >&2; exit 1; }
    if [ -f "$D/new-exe" ]; then cp "$D/new-exe" "$D/exe-5002"; echo 5002 > "$D/pid"; echo 5002 > "$D/agent-pid"; exit 0; fi
    if [ ! -f "$D/stuck" ]; then cp "$D/new-version" "$D/version"; echo 5002 > "$D/pid"; echo 5002 > "$D/agent-pid"; fi
    exit 0 ;;
esac
exit 64
"#;

    pub const THIS: &str = "/x/Applications/Bellwether.app/Contents/MacOS/Bellwether";
    const COPY: &str = "/x/Downloads/Bellwether.app/Contents/MacOS/Bellwether";

    pub struct World {
        pub dir: PathBuf,
    }

    impl World {
        pub fn new(name: &str, version: &str, pid: i32, agent_pid: Option<i32>) -> World {
            let dir = std::env::temp_dir().join(format!("bw-agent-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let fake = dir.join("fake-launchctl");
            std::fs::write(&fake, FAKE).unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join("version"), version).unwrap();
            std::fs::write(dir.join("new-version"), OURS).unwrap();
            std::fs::write(dir.join("pid"), pid.to_string()).unwrap();
            if let Some(p) = agent_pid {
                std::fs::write(dir.join("agent-pid"), p.to_string()).unwrap();
            }
            World { dir }
        }
        pub fn check(&self) -> Check {
            let dir = self.dir.clone();
            Check {
                host: Box::new(StandIn { launchctl: self.dir.join("fake-launchctl"), label: "com.fanzootechnology.bellwether.brain".into() }),
                this_exe: Some(PathBuf::from(THIS)),
                exe_of: Box::new(move |pid| match std::fs::read_to_string(dir.join(format!("exe-{pid}"))) {
                    Ok(t) if t.trim() == "deleted" => Exe::Deleted,
                    Ok(t) => Exe::At(PathBuf::from(t.trim())),
                    Err(_) => Exe::At(PathBuf::from(THIS)),
                }),
                wait: SHORT,
                attempts: ATTEMPTS,
            }
        }
        pub fn health(&self) -> impl FnMut() -> Option<Value> + Send + 'static {
            let dir = self.dir.clone();
            move || {
                let v = std::fs::read_to_string(dir.join("version")).ok()?;
                let pid: i64 = std::fs::read_to_string(dir.join("pid")).ok()?.trim().parse().ok()?;
                Some(serde_json::json!({"ok": true, "name": "bellwether", "version": v.trim(), "pid": pid}))
            }
        }
        pub fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("launchctl.log")).unwrap_or_default().lines().map(str::to_string).collect()
        }
        fn kickstarts(&self) -> usize {
            self.calls().iter().filter(|c| c.starts_with("kickstart -k")).count()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub const OURS: &str = "0.2.0-rc.3";
    const SHORT: Duration = Duration::from_millis(400);

    #[test]
    fn an_older_agent_brain_is_restarted_and_answers_with_the_apps_version() {
        let w = World::new("older", "0.2.0-rc.2", 4001, Some(4001));
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        let Outcome::Restarted { theirs, old_pid, new_pid, attempts, .. } = &o else { panic!("{o:?}") };
        assert_eq!((theirs.as_str(), *old_pid, *new_pid, *attempts), ("0.2.0-rc.2", 4001, 5002, 1));
        let uid = uid();
        assert_eq!(w.calls(), [format!("print gui/{uid}/com.fanzootechnology.bellwether.brain"), format!("kickstart -k gui/{uid}/com.fanzootechnology.bellwether.brain")]);
        assert!(o.log_line(47999, OURS).contains("was this app's login agent at version 0.2.0-rc.2 (pid 4001); restarted it"));
        assert_eq!(o.notice(OURS), None);
        // A newer brain than the app (a downgrade) is "another version" too.
        let w = World::new("newer", "0.3.0", 4001, Some(4001));
        assert!(matches!(reconcile(&mut w.check(), &mut w.health(), OURS), Outcome::Restarted { .. }));
    }

    #[test]
    fn after_an_update_the_agents_deleted_executable_still_counts_as_this_installs() {
        // Jeff's case: the update deleted the old bundle under the running agent.
        let w = World::new("deleted", "0.2.0-rc.2", 4001, Some(4001));
        std::fs::write(w.dir.join("exe-4001"), "deleted").unwrap();
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        assert!(matches!(o, Outcome::Restarted { new_pid: 5002, .. }), "{o:?}");
        assert_eq!(w.kickstarts(), 1);
    }

    #[test]
    fn another_copy_of_the_app_never_restarts_the_installed_agent() {
        // This strip is a copy elsewhere (Downloads, an opened DMG) with the same id; the
        // agent on the port runs the installed copy's executable, which still exists.
        let w = World::new("copy", "0.2.0-rc.2", 4001, Some(4001));
        std::fs::write(w.dir.join("exe-4001"), COPY).unwrap();
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        let Outcome::Foreign { why, .. } = &o else { panic!("{o:?}") };
        assert_eq!(why, &format!("com.fanzootechnology.bellwether.brain runs {COPY}, another copy of the app, not this one ({THIS})"));
        assert_eq!(w.kickstarts(), 0, "{:?}", w.calls());
        let line = o.log_line(47999, OURS);
        assert!(line.starts_with("warning: ") && line.contains("left alone") && line.contains("another copy of the app"), "{line}");
        assert_eq!(o.notice(OURS), None, "no \"Brain is still\" line for a copy");
        // A deleted executable whose restart comes back as another copy's agent: one
        // restart only, then left alone (never the second kickstart).
        let w = World::new("copy-after", "0.2.0-rc.2", 4001, Some(4001));
        std::fs::write(w.dir.join("exe-4001"), "deleted").unwrap();
        std::fs::write(w.dir.join("new-exe"), COPY).unwrap();
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        let Outcome::Foreign { why, pid, .. } = &o else { panic!("{o:?}") };
        assert!(why.starts_with("after one restart, com.fanzootechnology.bellwether.brain runs") && why.ends_with("not restarting it again") && *pid == 5002, "{why}");
        assert_eq!(w.kickstarts(), 1);
        assert_eq!(o.notice(OURS), None);
    }

    #[test]
    fn the_same_version_is_left_alone_and_launchd_is_not_even_asked() {
        let w = World::new("same", OURS, 4001, Some(4001));
        assert_eq!(reconcile(&mut w.check(), &mut w.health(), OURS), Outcome::Same { pid: 4001 });
        assert!(w.calls().is_empty(), "{:?}", w.calls());
    }

    #[test]
    fn a_brain_that_isnt_the_apps_agent_is_never_restarted() {
        // launchd runs the agent as another pid (a dev brain or a legacy agent's holds the port).
        let w = World::new("other", "0.2.0-rc.2", 4001, Some(4777));
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        assert_eq!(o, Outcome::Foreign { theirs: "0.2.0-rc.2".into(), pid: 4001, why: "launchd runs com.fanzootechnology.bellwether.brain as pid 4777".into() });
        assert_eq!(w.kickstarts(), 0, "{:?}", w.calls());
        assert!(o.log_line(47999, OURS).contains("left alone"));
        // The agent isn't running at all.
        let w = World::new("none", "0.2.0-rc.2", 4001, None);
        assert!(matches!(reconcile(&mut w.check(), &mut w.health(), OURS), Outcome::Foreign { why, .. } if why.contains("isn't running")));
        assert_eq!(w.kickstarts(), 0);
        // What the pid runs can't be read.
        let w = World::new("unknown", "0.2.0-rc.2", 4001, Some(4001));
        let mut c = w.check();
        c.exe_of = Box::new(|_| Exe::Unknown("Operation not permitted".into()));
        assert!(matches!(reconcile(&mut c, &mut w.health(), OURS), Outcome::Foreign { why, .. } if why.contains("can't be read")));
        assert_eq!(w.kickstarts(), 0);
        // Not in a bundle (no executable of its own), no label, or a refused stand-in.
        let w = World::new("off", "0.2.0-rc.2", 4001, Some(4001));
        let mut c = w.check();
        c.this_exe = None;
        assert!(matches!(reconcile(&mut c, &mut w.health(), OURS), Outcome::Foreign { why, .. } if why.contains("isn't inside an app bundle")));
        let mut c = w.check();
        c.host = Box::new(Off("refused".into()));
        assert_eq!(reconcile(&mut c, &mut w.health(), OURS), Outcome::Foreign { theirs: "0.2.0-rc.2".into(), pid: 4001, why: "refused".into() });
        assert_eq!(w.kickstarts(), 0);
        // Not a bellwether brain, or nothing: nothing to compare.
        let mut other = || Some(serde_json::json!({"name": "other", "version": "9", "pid": 4001}));
        assert_eq!(reconcile(&mut w.check(), &mut other, OURS), Outcome::NoBrain);
        assert_eq!(reconcile(&mut w.check(), &mut || None, OURS), Outcome::NoBrain);
        assert_eq!(w.kickstarts(), 0);
    }

    #[test]
    fn a_restart_that_doesnt_take_is_tried_a_bounded_number_of_times_then_said() {
        let w = World::new("stuck", "0.2.0-rc.2", 4001, Some(4001));
        std::fs::write(w.dir.join("stuck"), "").unwrap();
        let started = Instant::now();
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        assert_eq!(o, Outcome::GaveUp { theirs: "0.2.0-rc.2".into(), now: Some("0.2.0-rc.2".into()), attempts: 2, errors: vec![] });
        assert_eq!(w.kickstarts(), ATTEMPTS as usize, "exactly {ATTEMPTS} restarts, no loop");
        assert!(started.elapsed() < SHORT * 2 + Duration::from_secs(3));
        assert_eq!(o.notice(OURS).as_deref(), Some("Brain is still 0.2.0-rc.2, not 0.2.0-rc.3 (see bellwether doctor)"));
        let line = o.log_line(47999, OURS);
        assert!(line.starts_with("warning: ") && line.contains("2 restart(s)") && line.contains("not trying again until the next start") && line.contains("bellwether doctor"), "{line}");
        // launchctl refuses: each refusal is one attempt, and its words are kept.
        let w = World::new("refuse", "0.2.0-rc.2", 4001, Some(4001));
        std::fs::write(w.dir.join("refuse"), "").unwrap();
        let o = reconcile(&mut w.check(), &mut w.health(), OURS);
        let Outcome::GaveUp { errors, attempts, .. } = &o else { panic!("{o:?}") };
        assert_eq!((*attempts, errors.len()), (2, 2));
        assert!(errors[0].contains("Operation not permitted"), "{errors:?}");
        assert!(o.log_line(47999, OURS).contains("launchctl said: Could not kickstart"));
    }

    #[test]
    fn the_stand_in_only_under_a_temp_home_and_never_launchctl_itself() {
        assert_eq!(choose(true, None), Ok(None));
        assert_eq!(choose(false, None), Ok(None));
        assert!(choose(true, Some("/tmp/fake")).unwrap_err().contains("only under a temp HOME"));
        assert!(choose(false, Some("/bin/launchctl")).unwrap_err().contains("launchctl itself"));
        assert!(choose(false, Some("relative")).unwrap_err().contains("absolute path"));
        let w = World::new("choose", OURS, 1, None);
        let fake = w.dir.join("fake-launchctl");
        assert_eq!(choose(false, fake.to_str()), Ok(Some(fake.clone())));
        let link = w.dir.join("launchctl");
        std::os::unix::fs::symlink("/bin/launchctl", &link).unwrap();
        assert!(choose(false, link.to_str()).unwrap_err().contains("launchctl itself"));
    }

    #[test]
    fn doctor_names_the_fix_for_the_apps_agent_and_leaves_others_alone() {
        let l = Some("com.fanzootechnology.bellwether.brain");
        for o in [Owner::Ours, Owner::Presumed] {
            let s = doctor_suffix("0.2.0-rc.2", OURS, 4001, l, &o);
            assert!(s.contains("this app is 0.2.0-rc.3: this brain is this app's login agent, still running 0.2.0-rc.2"), "{s}");
            assert!(s.contains("Quit Bellwether from its menu-bar sheep and open it again") && s.contains("launchctl kickstart -k gui/$(id -u)/com.fanzootechnology.bellwether.brain"), "{s}");
        }
        let s = doctor_suffix("0.2.0-rc.2", OURS, 4001, l, &Owner::NotAgent("launchd runs com.fanzootechnology.bellwether.brain as pid 4777".into()));
        assert!(s.contains("isn't this app's login agent (launchd runs com.fanzootechnology.bellwether.brain as pid 4777)") && s.contains("Stop it (pid 4001)") && !s.contains("kickstart"), "{s}");
        let s = doctor_suffix("0.2.0-rc.2", OURS, 4001, l, &Owner::OtherCopy(COPY.into()));
        assert!(s.contains(&format!("the login agent of another copy of Bellwether ({COPY})")) && !s.contains("kickstart"), "{s}");
        assert!(doctor_suffix("0.2.0-rc.2", OURS, 4001, None, &Owner::Ours).contains("isn't this app's login agent"));
    }

    #[test]
    fn the_real_launchd_never_runs_in_a_unit_test() {
        // Read, not called: both real calls start with a panic in a test build.
        let src = include_str!("agent.rs");
        let real = src.split("impl Launchd for Real {").nth(1).unwrap().split("\n}\n").next().unwrap();
        for f in ["fn pid(", "fn kickstart("] {
            let body = real.split(f).nth(1).unwrap_or_else(|| panic!("{f}"));
            let first = body.split('{').nth(1).unwrap().trim_start();
            assert!(first.starts_with("#[cfg(test)]\n        panic!("), "{f} must panic first in a test build");
        }
        let code = src.split("#[cfg(test)]\npub(crate) mod tests").next().unwrap();
        assert_eq!(code.matches("run(Path::new(\"/bin/launchctl\")").count(), 1, "one call site of the real launchctl, in Real");
        // The agent is launchd's pid for the label AND this bundle's (or a deleted) executable.
        assert!(code.contains("Some(p) if i64::from(p) == pid && pid > 1 => {}"));
        assert!(code.contains("Exe::At(p) => Owner::OtherCopy(p),") && code.contains("Exe::Deleted => Owner::Presumed,"));
    }
}
