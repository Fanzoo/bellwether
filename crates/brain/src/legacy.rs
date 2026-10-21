//! The repository versions' LaunchAgents (distribution plan D5): `local.bellwether.brain`
//! and `.shell` (the renamed repository), `local.pasture.brain` and `.shell` (before the
//! rename). The app never writes, loads or removes them (that is the migration's, D9); it
//! only finds them, so doctor and the tray can name them: a legacy brain and the app's
//! brain would fight over one port.
//!
//! An agent counts when either is true:
//! - its plist is in ~/Library/LaunchAgents (launchd loads it at the next login, loaded
//!   now or not);
//! - launchd has the label loaded in **this user's** GUI domain (`gui/<uid>`), even with its
//!   plist deleted (it runs until logout).
//!
//! launchd is asked with `launchctl print gui/<uid>/<label>` only (read-only), and the
//! answer counts only when its first line names exactly that domain and label, so a label
//! in another user's domain never matters. It is asked only when HOME is this user's real
//! home: a test's temp HOME has its own LaunchAgents folder, and the real launchd state
//! would be the wrong answer for it.

use std::path::{Path, PathBuf};

/// The four LaunchAgent labels of the repository versions (plan D5).
pub const LABELS: [&str; 4] = ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.brain", "local.pasture.shell"];

/// What launchd says about a label in this user's GUI domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// Loaded and running, with its pid.
    Running(i32),
    /// Loaded, not running at the moment.
    Idle,
    /// launchd has no such service in this user's GUI domain.
    No,
    /// launchd wasn't asked (HOME isn't the real home, or not macOS).
    NotAsked,
    /// launchd didn't give a usable answer (for example from a terminal that predates the
    /// last login, F9); the reason, in one line.
    Unknown(String),
}

impl Loaded {
    pub fn is_loaded(&self) -> bool {
        matches!(self, Loaded::Running(_) | Loaded::Idle)
    }
}

/// One legacy agent that is still around.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub label: &'static str,
    /// `~/Library/LaunchAgents/<label>.plist`, when it is there.
    pub plist: Option<PathBuf>,
    pub loaded: Loaded,
}

impl Agent {
    /// "local.bellwether.brain (loaded, pid 25449)" and the like.
    pub fn describe(&self) -> String {
        let state = match (&self.loaded, self.plist.is_some()) {
            (Loaded::Running(pid), true) => format!("loaded, pid {pid}"),
            (Loaded::Running(pid), false) => format!("loaded, pid {pid}; its plist is gone, so it stops at logout"),
            (Loaded::Idle, true) => "loaded, not running".into(),
            (Loaded::Idle, false) => "loaded, not running; its plist is gone".into(),
            (Loaded::No, _) => "plist only: launchd loads it at your next login".into(),
            (Loaded::NotAsked, _) => "plist present".into(),
            (Loaded::Unknown(why), _) => format!("plist present; launchd didn't say whether it is loaded ({why})"),
        };
        format!("{} ({state})", self.label)
    }
}

impl Agent {
    /// The tray's short form: "local.bellwether.brain (running)".
    pub fn short(&self) -> String {
        let state = match &self.loaded {
            Loaded::Running(_) => "running",
            Loaded::Idle => "loaded",
            _ => "starts at login",
        };
        format!("{} ({state})", self.label)
    }
}

/// The legacy agents still around, in [`LABELS`] order. `lookup` answers for one label
/// (see [`launchd_lookup`]); a label with neither a plist nor a loaded service is left out.
pub fn scan(user_home: &Path, lookup: &dyn Fn(&str) -> Loaded) -> Vec<Agent> {
    let dir = user_home.join("Library").join("LaunchAgents");
    LABELS
        .iter()
        .filter_map(|label| {
            let file = dir.join(format!("{label}.plist"));
            // symlink_metadata: a plist that is a symlink still counts.
            let plist = std::fs::symlink_metadata(&file).is_ok().then_some(file);
            let loaded = lookup(label);
            (plist.is_some() || loaded.is_loaded()).then_some(Agent { label, plist, loaded })
        })
        .collect()
}

/// [`scan`] with launchd asked only when `user_home` is this user's real home.
pub fn scan_here(user_home: &Path) -> Vec<Agent> {
    if crate::migrate::is_real_home(user_home) {
        scan(user_home, &launchd_lookup)
    } else {
        scan(user_home, &|_| Loaded::NotAsked)
    }
}

/// [`scan`], plus the labels launchd gave no usable answer for (a timeout, a terminal from
/// before the last login) that have no plist either: [`scan`] leaves those out, and doctor
/// says it couldn't tell.
pub fn scan_reporting_unknown(user_home: &Path, lookup: &dyn Fn(&str) -> Loaded) -> (Vec<Agent>, Vec<(&'static str, String)>) {
    let unknown = std::cell::RefCell::new(Vec::new());
    let agents = scan(user_home, &|label| {
        let l = lookup(label);
        if let Loaded::Unknown(why) = &l {
            unknown.borrow_mut().push((label.to_string(), why.clone()));
        }
        l
    });
    let unknown = unknown.into_inner().into_iter().filter_map(|(l, why)| {
        let label = *LABELS.iter().find(|x| **x == l)?;
        (!agents.iter().any(|a| a.label == label)).then_some((label, why))
    });
    let unknown = unknown.collect();
    (agents, unknown)
}

/// [`scan_reporting_unknown`] with launchd asked only when `user_home` is the real home.
pub fn scan_here_reporting_unknown(user_home: &Path) -> (Vec<Agent>, Vec<(&'static str, String)>) {
    if crate::migrate::is_real_home(user_home) {
        scan_reporting_unknown(user_home, &launchd_lookup)
    } else {
        scan_reporting_unknown(user_home, &|_| Loaded::NotAsked)
    }
}

/// One line for doctor and the tray, naming each agent and its state.
pub fn summary(agents: &[Agent]) -> String {
    agents.iter().map(Agent::describe).collect::<Vec<_>>().join(", ")
}

/// The service target launchd is asked about: this user's GUI domain only.
pub fn target(uid: u32, label: &str) -> String {
    format!("gui/{uid}/{label}")
}

/// Reads `launchctl print <target>`'s answer. Only an answer whose first line is exactly
/// `<target> = {` counts as loaded, so a service of another domain or another label never
/// does; its top-level `pid = N` line (one tab deep) is the running pid.
pub fn parse_print(target: &str, success: bool, stdout: &str, stderr: &str) -> Loaded {
    if success {
        if stdout.lines().next().map(str::trim_end) != Some(&format!("{target} = {{")) {
            return Loaded::Unknown(format!("launchctl print answered for something other than {target}"));
        }
        let pid = stdout.lines().find_map(|l| l.strip_prefix("\tpid = ")).and_then(|p| p.trim().parse::<i32>().ok());
        return match pid {
            Some(p) if p > 0 => Loaded::Running(p),
            _ => Loaded::Idle,
        };
    }
    let text = format!("{stdout}\n{stderr}");
    if text.contains("Could not find service") {
        return Loaded::No;
    }
    let why = text.lines().map(str::trim).find(|l| !l.is_empty() && *l != "Bad request.").unwrap_or("no answer").to_string();
    Loaded::Unknown(why)
}

/// How long one `launchctl print` may take before its answer counts as unknown.
pub const LAUNCHCTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Asks launchd (read-only) about one label in this user's GUI domain, waiting at most
/// [`LAUNCHCTL_TIMEOUT`].
#[cfg(target_os = "macos")]
pub fn launchd_lookup(label: &str) -> Loaded {
    let target = target(unsafe { libc::getuid() }, label);
    let mut cmd = std::process::Command::new("/bin/launchctl");
    cmd.arg("print").arg(&target);
    run_bounded(cmd, LAUNCHCTL_TIMEOUT, |ok, out, err| parse_print(&target, ok, out, err))
}

/// Runs `cmd` (stdin closed, output captured) for at most `limit`; a command that takes
/// longer is stopped, by its own handle only, and the answer is unknown.
pub fn run_bounded(cmd: std::process::Command, limit: std::time::Duration, read: impl Fn(bool, &str, &str) -> Loaded) -> Loaded {
    match run_output(cmd, limit) {
        Ok((ok, out, err)) => read(ok, &out, &err),
        Err(Bounded::Spawn(e)) => Loaded::Unknown(format!("couldn't run launchctl: {e}")),
        Err(Bounded::Timeout) => Loaded::Unknown(format!("launchctl print didn't answer within {} s", limit.as_secs_f64())),
        Err(Bounded::Wait(e)) => Loaded::Unknown(format!("launchctl: {e}")),
    }
}

/// Why a bounded command gave no answer.
#[derive(Debug)]
pub enum Bounded {
    Spawn(std::io::Error),
    Timeout,
    Wait(std::io::Error),
}

/// Runs `cmd` (stdin closed) for at most `limit`: (success, stdout, stderr). One that takes
/// longer is stopped, by its own handle only.
pub fn run_output(mut cmd: std::process::Command, limit: std::time::Duration) -> Result<(bool, String, String), Bounded> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(Bounded::Spawn)?;
    // The answer is small; read both pipes on threads so a full pipe can't stall the wait.
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Bounded::Timeout);
            }
            Err(e) => return Err(Bounded::Wait(e)),
        }
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    Ok((status.success(), String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned()))
}

/// Why ~/Library/LaunchAgents can't be looked into, when it exists but can't be listed or
/// searched (a plist in it would then be missed); None when it can, or doesn't exist.
pub fn unreadable(user_home: &Path) -> Option<String> {
    let dir = user_home.join("Library").join("LaunchAgents");
    if let Err(e) = std::fs::read_dir(&dir) {
        return (e.kind() != std::io::ErrorKind::NotFound).then(|| format!("can't read {} ({e})", dir.display()));
    }
    // Listable but not searchable (no x bit): stat of a name inside fails.
    match std::fs::symlink_metadata(dir.join(format!("{}.plist", LABELS[0]))) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Some(format!("can't read {} ({e})", dir.display())),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn launchd_lookup(_label: &str) -> Loaded {
    Loaded::NotAsked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp HOME per call. (The name used to be made of the plists alone, so two
    /// tests running at once with the same list, `home_with(&[])`, shared one folder and one
    /// removed it while the other wrote into it: the EINVAL seen once in M7.)
    fn home_with(plists: &[&str]) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("bw-legacy-{}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst), plists.join("+")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Library").join("LaunchAgents")).unwrap();
        for p in plists {
            std::fs::write(dir.join("Library").join("LaunchAgents").join(format!("{p}.plist")), "<plist/>").unwrap();
        }
        dir
    }

    const PRINT: &str = "gui/501/local.bellwether.brain = {\n\tactive count = 1\n\tpath = /Users/u/Library/LaunchAgents/local.bellwether.brain.plist\n\tstate = running\n\tdomain = gui/501 [114788]\n\tpid = 25449\n\tendpoints = {\n\t\t\"x\" = {\n\t\t\tpid = 7\n\t\t}\n\t}\n}\n";

    #[test]
    fn launchds_answer_counts_only_for_this_domain_and_label() {
        let t = target(501, "local.bellwether.brain");
        assert_eq!(t, "gui/501/local.bellwether.brain");
        assert_eq!(parse_print(&t, true, PRINT, ""), Loaded::Running(25449));
        assert_eq!(parse_print(&t, true, &PRINT.replace("\tpid = 25449\n", ""), ""), Loaded::Idle, "a nested pid is not the service's");
        // Another user's domain, or another label, in the answer: never "loaded".
        let other = PRINT.replace("gui/501/", "gui/502/");
        assert!(matches!(parse_print(&t, true, &other, ""), Loaded::Unknown(_)));
        assert!(matches!(parse_print(&t, true, &PRINT.replace("brain = {", "brainx = {"), ""), Loaded::Unknown(_)));
        // What launchctl prints for a label it doesn't have (exit 113), measured.
        assert_eq!(parse_print(&t, false, "", "Bad request.\nCould not find service \"local.bellwether.brain\" in domain for user gui: 501\n"), Loaded::No);
        // A terminal from before the last login (F9).
        assert_eq!(parse_print(&t, false, "", "Could not print domain: 141: Reentrancy avoided\n"), Loaded::Unknown("Could not print domain: 141: Reentrancy avoided".into()));
    }

    #[test]
    fn plists_and_launchd_each_count_alone() {
        // A plist present but not loaded; both names.
        let home = home_with(&["local.bellwether.brain", "local.pasture.shell"]);
        let found = scan(&home, &|_| Loaded::No);
        assert_eq!(found.iter().map(|a| a.label).collect::<Vec<_>>(), ["local.bellwether.brain", "local.pasture.shell"]);
        assert!(found[0].describe().contains("plist only: launchd loads it at your next login"), "{}", found[0].describe());
        // Loaded, with its plist deleted.
        let empty = home_with(&[]);
        let found = scan(&empty, &|l| if l == "local.pasture.brain" { Loaded::Running(4242) } else { Loaded::No });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].describe(), "local.pasture.brain (loaded, pid 4242; its plist is gone, so it stops at logout)");
        assert_eq!(scan(&empty, &|_| Loaded::Idle).len(), 4, "every label, loaded without a plist");
        // Neither: nothing. launchd unsure and no plist: nothing either.
        assert!(scan(&empty, &|_| Loaded::No).is_empty());
        assert!(scan(&empty, &|_| Loaded::Unknown("x".into())).is_empty());
        assert!(scan(&empty, &|_| Loaded::NotAsked).is_empty());
        // Both loaded and present; the summary names each.
        let home2 = home_with(&["local.bellwether.brain", "local.bellwether.shell"]);
        let found = scan(&home2, &|l| if l.starts_with("local.bellwether.") { Loaded::Running(if l.ends_with("brain") { 25449 } else { 32594 }) } else { Loaded::No });
        assert_eq!(summary(&found), "local.bellwether.brain (loaded, pid 25449), local.bellwether.shell (loaded, pid 32594)");
        assert_eq!(found[0].short(), "local.bellwether.brain (running)");
        // Labels that only look alike are not ours.
        std::fs::write(home2.join("Library").join("LaunchAgents").join("local.bellwether.brain.plist.bak"), "").unwrap();
        std::fs::write(home2.join("Library").join("LaunchAgents").join("com.example.bellwether.brain.plist"), "").unwrap();
        assert_eq!(scan(&home2, &|_| Loaded::No).len(), 2);
        for d in [home, empty, home2] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn launchd_unsure_without_a_plist_is_reported_not_dropped() {
        let home = home_with(&["local.pasture.brain"]);
        let (agents, unknown) = scan_reporting_unknown(&home, &|_| Loaded::Unknown("launchctl print didn't answer within 2 s".into()));
        assert_eq!(agents.len(), 1, "the plist still counts");
        assert!(agents[0].describe().contains("launchd didn't say whether it is loaded (launchctl print didn't answer within 2 s)"), "{}", agents[0].describe());
        assert_eq!(unknown.iter().map(|(l, _)| *l).collect::<Vec<_>>(), ["local.bellwether.brain", "local.bellwether.shell", "local.pasture.shell"]);
        let (_, none) = scan_reporting_unknown(&home, &|_| Loaded::No);
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_temp_home_never_asks_launchd() {
        let home = home_with(&["local.bellwether.shell"]);
        assert!(!crate::migrate::is_real_home(&home));
        let found = scan_here(&home);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].loaded, Loaded::NotAsked);
        assert_eq!(found[0].describe(), "local.bellwether.shell (plist present)");
        let _ = std::fs::remove_dir_all(home);
    }

    // /bin/sleep and /bin/echo: Unix only (Windows plan W1).
    #[cfg(unix)]
    #[test]
    fn a_slow_launchctl_is_unknown_and_stopped() {
        let mut cmd = std::process::Command::new("/bin/sleep");
        cmd.arg("30");
        let t = std::time::Instant::now();
        let r = run_bounded(cmd, std::time::Duration::from_millis(300), |_, _, _| Loaded::Idle);
        assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
        assert_eq!(r, Loaded::Unknown("launchctl print didn't answer within 0.3 s".into()));
        let mut cmd = std::process::Command::new("/bin/echo");
        cmd.arg("gui/501/local.bellwether.brain = {");
        assert_eq!(run_bounded(cmd, LAUNCHCTL_TIMEOUT, |ok, out, err| parse_print("gui/501/local.bellwether.brain", ok, out, err)), Loaded::Idle);
        assert_eq!(LAUNCHCTL_TIMEOUT, std::time::Duration::from_secs(2));
    }

    #[test]
    #[cfg(unix)]
    fn an_unsearchable_folder_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let home = home_with(&["local.bellwether.brain"]);
        let dir = home.join("Library").join("LaunchAgents");
        assert_eq!(unreadable(&home), None);
        for mode in [0o000, 0o300, 0o600] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
            let why = unreadable(&home);
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(why.as_deref().is_some_and(|w| w.starts_with("can't read ") && w.contains("LaunchAgents")), "mode {mode:o}: {why:?}");
        }
        let empty = home_with(&[]);
        assert_eq!(unreadable(&empty.join("nothing")), None, "no folder at all is not a problem");
        for d in [home, empty] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn scanning_only_reads() {
        // Nothing here may create, write, move or delete a file: scan and the lookup read.
        let src = include_str!("legacy.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for w in ["fs::write", "File::create", "fs::rename", "fs::remove", "create_dir", "fs::copy", "OpenOptions", "\"bootout\"", "\"bootstrap\"", "\"load\"", "\"unload\"", "\"kickstart\"", "\"remove\""] {
            assert!(!code.contains(w), "legacy.rs uses {w}");
        }
        assert_eq!(code.matches(".arg(\"print\")").count(), 1, "launchctl is only ever asked to print");
    }
}
