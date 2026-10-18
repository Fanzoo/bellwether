//! Which app this process is: its bundle, bundle id and version. A dev binary run from
//! target-dev/ has no bundle (or Tauri's embedded dev Info.plist); the signed app has
//! the permanent id from tauri.dist.conf.json.

use std::path::{Path, PathBuf};

/// The signed app's permanent bundle id (maintainer decision 1). Only a Developer ID
/// build may carry it into SMAppService or TCC: an ad-hoc build registered under it
/// leaves a Background Task Management record whose launch constraint a later
/// Developer ID build fails ("Launch Constraint Violation", M1 finding F7).
pub const PERMANENT_ID: &str = "com.fanzootechnology.bellwether";

/// Whether the app bundle is signed ad hoc (or not at all), from `codesign -dv`.
pub fn ad_hoc() -> bool {
    let Some(bundle) = bundle_path() else { return true };
    match std::process::Command::new("/usr/bin/codesign").arg("-dv").arg(&bundle).output() {
        Ok(o) => !o.status.success() || String::from_utf8_lossy(&o.stderr).contains("Signature=adhoc"),
        Err(_) => true,
    }
}

/// The bundle's CFBundleIdentifier, if this executable is inside an app bundle. Read
/// from the bundle's path, not `NSBundle.mainBundle`: called through the CLI symlink on
/// PATH, mainBundle is the symlink's directory, not the app (measured).
#[cfg(target_os = "macos")]
pub fn bundle_id() -> Option<String> {
    let path = bundle_path()?;
    let b = objc2_foundation::NSBundle::bundleWithPath(&objc2_foundation::NSString::from_str(&path.to_string_lossy()))?;
    b.bundleIdentifier().map(|s| s.to_string())
}

/// Whether this process was started by the executable's own path (not a symlink to it).
/// SMAppService finds the app through `NSBundle.mainBundle`, which only works then.
pub fn started_from_bundle() -> bool {
    let Some(arg0) = std::env::args_os().next() else { return false };
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else { return false };
    // argv[0] may be a bare name found on PATH; resolve it the way the shell did.
    let given = std::path::PathBuf::from(&arg0);
    let given = if given.components().count() > 1 { given } else { return false };
    std::fs::symlink_metadata(&given).map(|m| !m.file_type().is_symlink()).unwrap_or(false) && given.canonicalize().map(|g| g == exe).unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
pub fn bundle_id() -> Option<String> {
    None
}

/// `…/Bellwether.app`, when the executable sits at `<bundle>/Contents/MacOS/<name>`.
/// Symlinks are resolved first, so the CLI on PATH finds the app it belongs to.
pub fn bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

pub fn in_app_bundle() -> bool {
    bundle_path().is_some()
}

/// Whether this process can see the user's current login session. A terminal whose
/// server outlived a logout (herdr, tmux, screen) keeps the old session: launchd answers
/// it with "Reentrancy avoided", SMAppService reports every item as notFound, and
/// codesign can't read certificate names (M1 finding F9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Current,
    /// From before the user's last login.
    Stale,
    Unknown,
}

pub fn session() -> Session {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("/bin/launchctl").arg("managername").stdin(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => Session::Current,
            Ok(o) if String::from_utf8_lossy(&o.stderr).contains("Could not get manager name") => Session::Stale,
            _ => Session::Unknown,
        }
    }
    #[cfg(not(target_os = "macos"))]
    Session::Current
}

/// The executable a process runs, by pid (`proc_pidpath`, read only).
#[cfg(target_os = "macos")]
pub fn pid_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(not(target_os = "macos"))]
pub fn pid_path(_pid: i32) -> Option<PathBuf> {
    None
}

/// This bundle's main executable, resolved.
pub fn bundle_executable() -> Option<PathBuf> {
    let exe = bundle_path()?.join("Contents").join("MacOS").join("Bellwether");
    exe.canonicalize().ok()
}

/// What a process runs, by pid, as `proc_pidpath` sees it (read only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exe {
    At(PathBuf),
    /// Its executable file no longer exists: an update swapped the bundle and deleted the
    /// old one while the process kept running (ENOENT; deviation 135).
    Deleted,
    /// Anything else (no such process, no permission).
    Unknown(String),
}

#[cfg(target_os = "macos")]
pub fn pid_exe(pid: i32) -> Exe {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if n <= 0 {
        let e = std::io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENOENT) { Exe::Deleted } else { Exe::Unknown(e.to_string()) };
    }
    buf.truncate(n as usize);
    Exe::At(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(not(target_os = "macos"))]
pub fn pid_exe(_pid: i32) -> Exe {
    Exe::Unknown("not macOS".into())
}

/// Whether a process runs `exe` (resolved): its executable is that file, or (after an
/// update deleted it) its argv[0] is that very path. LaunchServices starts the strip by its
/// full path, so a strip that outlived a bundle swap is still found; a process whose file
/// exists elsewhere (another copy) never is.
pub fn runs(found: &Exe, args: &str, exe: &Path) -> bool {
    match found {
        Exe::At(p) => p.canonicalize().ok().as_deref() == Some(exe),
        Exe::Deleted => {
            let e = exe.to_string_lossy();
            args == e || args.strip_prefix(e.as_ref()).is_some_and(|rest| rest.starts_with(' '))
        }
        Exe::Unknown(_) => false,
    }
}

/// Processes running `exe` (see [`runs`]): (pid, ppid, arguments).
pub fn processes_of(exe: &Path) -> Vec<(i32, i32, String)> {
    let Ok(out) = std::process::Command::new("/bin/ps").args(["-axo", "pid=,ppid=,args="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, rest) = l.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (ppid, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let (pid, ppid): (i32, i32) = (pid.parse().ok()?, ppid.parse().ok()?);
            let args = args.trim().to_string();
            runs(&pid_exe(pid), &args, exe).then_some((pid, ppid, args))
        })
        .collect()
}

/// Processes running this exact bundle executable (never a different copy, and never
/// the repository's bellwether-shell), including one whose file an update has since
/// replaced: (pid, ppid, arguments).
pub fn bundle_processes() -> Vec<(i32, i32, String)> {
    let Some(exe) = bundle_executable() else { return Vec::new() };
    processes_of(&exe)
}
