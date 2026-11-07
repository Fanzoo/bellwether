//! Start at login, the brain's life and the strip's single instance on Windows (Windows
//! plan WD6, W6).
//!
//! - **Start at login:** two `REG_SZ` values under `HKCU\Software\Microsoft\Windows\
//!   CurrentVersion\Run`, `Bellwether` (the strip) and `Bellwether Brain` (`… serve`). The
//!   test folder's build writes `Bellwether (test)` and `Bellwether Brain (test)` instead,
//!   so it never touches an installed app's values (deviation W6-1). Whether the user
//!   turned one off in Task Manager is read from `…\Explorer\StartupApproved\Run`: first
//!   byte `02` on, `03` off, anything else unknown, never taken as off.
//! - **The install:** `<dir>\Bellwether.exe` with `<dir>\bin\bellwether.exe` beside it (the
//!   test folder now, the installer from W8). A development build in `target-dev\` has no
//!   `bin\`, so it never writes a Run value.
//! - **The test folder finds its own home** ([`adopt_test_folder`]): a Run value gets no
//!   environment, so the test build, started outside its `.cmd` files, sets the folder's
//!   home, config and agent directory itself, as `cli.cmd` does.
//! - **The running strip** is found by the single-instance plugin's hidden window
//!   (`<id>-sic`, `<id>-siw`; tauri-plugin-single-instance 2.5.2), and asked to quit through
//!   it: the same `WM_COPYDATA` a second launch sends, with `--quit` in its arguments.
//!
//! The pure parts build on every OS (the Mac tests them); the registry, process and window
//! calls are Windows only.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::login_items::{Service, Status};

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Settings > Apps > Startup.
pub const SETTINGS_URI: &str = "ms-settings:startupapps";
/// The argument that asks a running strip to quit (`shell stop`, `autostart uninstall`).
pub const QUIT_ARG: &str = "--quit";
/// tauri-plugin-single-instance 2.5.2's `WM_COPYDATA` tag (platform_impl/windows.rs).
pub const COPYDATA_TAG: usize = 1542;

/// Whether this is the test folder's build (`--features test-build`).
pub fn test_build() -> bool {
    cfg!(feature = "test-build")
}

/// The Run value's name for a service.
pub fn value_name(s: Service, test: bool) -> &'static str {
    match (s, test) {
        (Service::MainApp, false) => "Bellwether",
        (Service::Brain, false) => "Bellwether Brain",
        (Service::MainApp, true) => "Bellwether (test)",
        (Service::Brain, true) => "Bellwether Brain (test)",
    }
}

/// The command a Run value holds: the quoted exe, then ` serve` for the brain.
pub fn command(s: Service, exe: &Path) -> String {
    let exe = plain(exe);
    match s {
        Service::MainApp => format!("\"{}\"", exe.display()),
        Service::Brain => format!("\"{}\" serve", exe.display()),
    }
}

/// Without the `\\?\` prefix `canonicalize` adds on Windows.
pub fn plain(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// Paths compared as NTFS does: case-insensitive, either slash, no `\\?\`, no trailing slash.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| plain(p).to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// Two Run commands are the same when they differ only in case and outer spaces.
pub fn same_command(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// What `StartupApproved\Run` says about a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approved {
    /// No entry (never switched in Task Manager), or `02`.
    On,
    /// `03`: turned off in Task Manager or Settings > Apps > Startup.
    Off,
    /// Some other first byte: not a layout we know, so never taken as off.
    Unknown(u8),
}

pub fn approved(bytes: Option<&[u8]>) -> Approved {
    match bytes.and_then(|b| b.first()) {
        None | Some(2) => Approved::On,
        Some(3) => Approved::Off,
        Some(&b) => Approved::Unknown(b),
    }
}

/// A Run value's status: absent, pointing at another exe, turned off, or on. Turned off in
/// Task Manager is the Windows analogue of macOS's `requiresApproval` (WD6): it is there,
/// but Windows won't start it until the user turns it on.
pub fn status_of(value: Option<&str>, wanted: &str, approved: Approved) -> Status {
    match value {
        None => Status::NotRegistered,
        Some(v) if !same_command(v, wanted) => Status::Elsewhere,
        Some(_) if approved == Approved::Off => Status::RequiresApproval,
        Some(_) => Status::Enabled,
    }
}

/// The install this exe belongs to: `<dir>\Bellwether.exe`, when `<dir>\bin\bellwether.exe`
/// is there too. `current` is either of the two. Anything else (a development build in
/// `target-dev\debug`, whose `bellwether.exe` and `bellwether-shell.exe` have no `bin\`) is
/// no install.
pub fn install_exe_of(current: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let parent = current.parent()?;
    let dir = if parent.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("bin")) { parent.parent()? } else { parent };
    let gui = dir.join("Bellwether.exe");
    (exists(&gui) && exists(&dir.join("bin").join("bellwether.exe"))).then(|| plain(&gui))
}

/// This process's install (see [`install_exe_of`]).
pub fn install_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    install_exe_of(&me, &|p| p.is_file())
}

/// Whether `exe` is an exe of the install whose GUI exe is `ours`: it sits in the same
/// directory (an update replaces the file in place; a renamed old exe stays beside it).
pub fn same_install(exe: &Path, ours: &Path) -> bool {
    match (exe.parent(), ours.parent()) {
        (Some(a), Some(b)) => same_path(a, b),
        _ => false,
    }
}

/// The test folder an exe is in: `<dir>` holding `run-test.cmd` (the exe in it, or in its
/// `bin\`). Its (home, config, agent dir) are what `cli.cmd` and `run-test.cmd` set.
pub fn test_folder_of(current: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let parent = current.parent()?;
    let dir = if parent.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("bin")) { parent.parent()? } else { parent };
    let dir = plain(dir);
    exists(&dir.join("run-test.cmd")).then(|| (dir.join("home"), dir.join("config.json"), dir))
}

/// The test build, started without BELLWETHER_HOME (by a Run value at login, or from
/// Explorer), sets its test folder's home, config and agent directory, as `cli.cmd` does.
/// Called first thing in `main`, before any thread exists. A release never does this, and
/// a BELLWETHER_HOME already set is left as it is.
pub fn adopt_test_folder() {
    if !test_build() || std::env::var_os("BELLWETHER_HOME").is_some_and(|h| !h.is_empty()) {
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    if let Some((home, config, agent)) = test_folder_of(&me, &|p| p.is_file()) {
        let _ = std::fs::create_dir_all(&home);
        std::env::set_var("BELLWETHER_HOME", &home);
        std::env::set_var("BELLWETHER_CONFIG", &config);
        std::env::set_var("BELLWETHER_AGENT_DIR", &agent);
        std::env::remove_var("BELLWETHER_DATA_DIR");
    }
}

/// The home a process started by a Run value will use: the test folder's (test build), or
/// `%APPDATA%\Bellwether` (it gets no BELLWETHER_HOME).
pub fn run_home(test: bool, test_folder: Option<&(PathBuf, PathBuf, PathBuf)>, default: PathBuf) -> Result<PathBuf, String> {
    if !test {
        return Ok(default);
    }
    test_folder.map(|(h, _, _)| h.clone()).ok_or_else(|| "refused: this test build isn't in its test folder (no run-test.cmd beside it), so at login it wouldn't find its home or its test port. Start at login from the unzipped folder".into())
}

/// The checks before a Run value is written (as `login_items::preflight` on macOS): the
/// processes Windows starts at login get no environment, so a BELLWETHER_HOME that differs
/// from the home they will use is refused (M1 finding F10). The brain needs a usable token
/// in that home, or it exits at once (deviation 60).
pub fn preflight(services: &[Service], registering: bool, home: Result<PathBuf, String>, env_home: Option<PathBuf>, token: &dyn Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    if !registering {
        return Ok(());
    }
    let home = home?;
    if let Some(e) = env_home.filter(|e| !same_path(e, &home)) {
        return Err(format!("refused: BELLWETHER_HOME is set ({}), but what Windows starts at login gets no environment, so it would use {}. Unset BELLWETHER_HOME (or, in the test folder, use cli.cmd, which sets the folder's own)", plain(&e).display(), plain(&home).display()));
    }
    if services.contains(&Service::Brain) {
        token(&home)?;
    }
    Ok(())
}

/// The single-instance plugin's hidden window for a lock id: (class, window name).
pub fn strip_window_names(lock_id: &str) -> (String, String) {
    (format!("{lock_id}-sic"), format!("{lock_id}-siw"))
}

/// The lock ids a strip on `port` may hold: the release's and the development/test
/// build's identifiers (lib.rs `single_instance_id`).
pub fn lock_ids(port: u16) -> Vec<String> {
    [crate::identity::PERMANENT_ID.to_string(), dev_identifier()].iter().map(|id| crate::single_instance_id(id, port)).collect()
}

/// The development and test builds' identifier, from tauri.conf.json (its one place).
pub fn dev_identifier() -> String {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap_or_default();
    conf["identifier"].as_str().unwrap_or_default().to_string()
}

/// What a second launch sends the running strip (`cwd|argv…` with `|` between, NUL
/// ended; the plugin splits it back), here with [`QUIT_ARG`].
pub fn quit_payload(cwd: &str, exe: &str) -> String {
    format!("{cwd}|{exe}|{QUIT_ARG}\0")
}

/// What `autostart` says when the exe isn't an install.
pub const NOT_AN_INSTALL: &str = "not an installed Bellwether: start at login needs Bellwether.exe with bin\\bellwether.exe beside it (the test folder, or the installer's folder), so a development build has none to change, and nothing was written";

/// The advice line for a status, in Windows' words.
pub fn advice(s: Service, st: Status) -> String {
    let name = value_name(s, test_build());
    match st {
        Status::Enabled => format!("{}: enabled (the Run value \"{name}\")", s.name()),
        Status::NotRegistered => format!("{}: off (`bellwether autostart install --write` turns it on)", s.name()),
        Status::RequiresApproval => format!("{}: turned off in Task Manager (or Settings > Apps > Startup), so Windows won't start it at login. Turn \"{name}\" on there (`bellwether autostart settings` opens it)", s.name()),
        Status::Elsewhere => format!("{}: the Run value \"{name}\" starts another copy of Bellwether, not this one (`bellwether autostart install --write` points it here)", s.name()),
        Status::NotFound | Status::Unavailable => format!("{}: unavailable here ({NOT_AN_INSTALL})", s.name()),
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, WM_COPYDATA};

    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const STILL_ACTIVE: u32 = 259;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A value under HKCU: its bytes as `RegGetValueW` gives them, None when absent.
    fn read(key: &str, value: &str, kind: u32) -> Option<Vec<u8>> {
        let (k, v) = (wide(key), wide(value));
        let mut len = 0u32;
        if unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), kind, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) } != 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize + 2];
        len = buf.len() as u32;
        if unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), kind, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut len) } != 0 {
            return None;
        }
        buf.truncate(len as usize);
        Some(buf)
    }

    fn read_string(key: &str, value: &str) -> Option<String> {
        let b = read(key, value, RRF_RT_REG_SZ)?;
        let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(crate::win::from_wide(&w))
    }

    /// The Run value's command, if there is one.
    pub fn run_value(s: Service) -> Option<String> {
        read_string(RUN_KEY, value_name(s, test_build()))
    }

    pub fn approval(s: Service) -> Approved {
        approved(read(APPROVED_KEY, value_name(s, test_build()), RRF_RT_REG_BINARY).as_deref())
    }

    pub fn status(s: Service) -> Status {
        let Some(exe) = install_exe() else { return Status::Unavailable };
        status_of(run_value(s).as_deref(), &command(s, &exe), approval(s))
    }

    /// Whether the brain starts at login: its Run value is this install's and on.
    pub fn brain_at_login() -> bool {
        status(Service::Brain) == Status::Enabled
    }

    fn home_for_run() -> Result<PathBuf, String> {
        let folder = std::env::current_exe().ok().and_then(|me| test_folder_of(&me, &|p| p.is_file()));
        run_home(test_build(), folder.as_ref(), bellwether_brain::home::platform_dir())
    }

    pub fn preflight_here(services: &[Service], registering: bool) -> Result<(), String> {
        if install_exe().is_none() {
            return Err(NOT_AN_INSTALL.into());
        }
        let env_home = std::env::var_os("BELLWETHER_HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
        preflight(services, registering, home_for_run(), env_home, &|h| crate::login_items::agent_token_check(h))
    }

    pub fn register(s: Service) -> Result<Status, String> {
        preflight_here(&[s], true)?;
        let exe = install_exe().ok_or(NOT_AN_INSTALL)?;
        let (k, v) = (wide(RUN_KEY), wide(value_name(s, test_build())));
        let data = wide(&command(s, &exe));
        let mut key: HKEY = std::ptr::null_mut();
        let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut()) };
        if r != 0 {
            return Err(format!("couldn't open HKCU\\{RUN_KEY} (error {r})"));
        }
        let r = unsafe { RegSetValueExW(key, v.as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) };
        unsafe { RegCloseKey(key) };
        if r != 0 {
            return Err(format!("couldn't write the Run value (error {r})"));
        }
        Ok(status(s))
    }

    /// Removes the Run value, only this name. Its StartupApproved entry is Windows' own
    /// record of the user's choice, left as it is.
    pub fn unregister(s: Service) -> Result<Status, String> {
        let (k, v) = (wide(RUN_KEY), wide(value_name(s, test_build())));
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr()) };
        if r != 0 && r != ERROR_FILE_NOT_FOUND {
            return Err(format!("couldn't remove the Run value (error {r})"));
        }
        Ok(if install_exe().is_some() { status(s) } else { Status::NotRegistered })
    }

    pub fn open_settings() {
        let (op, uri) = (wide("open"), wide(SETTINGS_URI));
        unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), uri.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    }

    /// The exe a process runs, by pid.
    pub fn process_path(pid: u32) -> Option<PathBuf> {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
        unsafe { CloseHandle(h) };
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }

    pub fn alive(pid: u32) -> bool {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
        unsafe { CloseHandle(h) };
        ok && code == STILL_ACTIVE
    }

    /// Waits up to `limit` for a process to end: true when it has (or was already gone).
    pub fn wait_exit(pid: u32, limit: Duration) -> bool {
        let h: HANDLE = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if h.is_null() {
            return !alive(pid);
        }
        let r = unsafe { WaitForSingleObject(h, limit.as_millis().min(u32::MAX as u128 - 1) as u32) };
        unsafe { CloseHandle(h) };
        r == WAIT_OBJECT_0
    }

    /// The running strip on `port`: its plugin window and pid.
    pub fn strip(port: u16) -> Option<(isize, u32)> {
        for id in lock_ids(port) {
            let (class, name) = strip_window_names(&id);
            let (c, n) = (wide(&class), wide(&name));
            let hwnd = unsafe { FindWindowW(c.as_ptr(), n.as_ptr()) };
            if !hwnd.is_null() {
                let mut pid = 0u32;
                unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
                return Some((hwnd as isize, pid));
            }
        }
        None
    }

    /// Asks the strip on `port` to quit, as a second launch would hand it arguments, and
    /// waits up to 5 s. None: no strip ran. Some((pid, quit)).
    pub fn quit_strip(port: u16) -> Option<(u32, bool)> {
        let (hwnd, pid) = strip(port)?;
        let cwd = std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned();
        let exe = install_exe().unwrap_or_default().to_string_lossy().into_owned();
        let data = quit_payload(&cwd, &exe);
        let cds = COPYDATASTRUCT { dwData: COPYDATA_TAG, cbData: data.len() as u32, lpData: data.as_ptr() as *mut c_void };
        let mut result = 0usize;
        unsafe { SendMessageTimeoutW(hwnd as _, WM_COPYDATA, 0, &cds as *const _ as isize, SMTO_ABORTIFHUNG, 3000, &mut result) };
        Some((pid, wait_exit(pid, Duration::from_secs(5))))
    }

    /// Starts the install's strip, detached, unless one runs on `port`; waits up to 10 s
    /// for its window and 1.5 s more to see it stays (a strip that finds another holding
    /// the lock hands over and quits at once).
    pub fn start_strip(port: u16) -> Result<(u32, bool), String> {
        if let Some((_, pid)) = strip(port) {
            return Ok((pid, false));
        }
        let exe = install_exe().ok_or(NOT_AN_INSTALL)?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, DETACHED);
        cmd.spawn().map_err(|e| format!("couldn't start {}: {e}", exe.display()))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some((_, pid)) = strip(port) {
                std::thread::sleep(Duration::from_millis(1500));
                if alive(pid) && strip(port).is_some_and(|(_, p)| p == pid) {
                    return Ok((pid, true));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!("the strip didn't start within 10 s; its log (shell.log in {}) says why", bellwether_brain::home::dir().display()))
    }

    /// `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`: no console, and outlives whoever
    /// started it (WD6).
    pub const DETACHED: u32 = 0x0000_0008 | 0x0000_0200;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_values_name_the_install_and_the_test_folder_has_its_own() {
        let exe = Path::new(r"C:\Users\me\AppData\Local\Bellwether\Bellwether.exe");
        assert_eq!(command(Service::MainApp, exe), r#""C:\Users\me\AppData\Local\Bellwether\Bellwether.exe""#);
        assert_eq!(command(Service::Brain, exe), r#""C:\Users\me\AppData\Local\Bellwether\Bellwether.exe" serve"#);
        assert_eq!(command(Service::Brain, Path::new(r"\\?\C:\x\Bellwether.exe")), r#""C:\x\Bellwether.exe" serve"#, "never the \\\\?\\ prefix");
        assert_eq!((value_name(Service::MainApp, false), value_name(Service::Brain, false)), ("Bellwether", "Bellwether Brain"));
        for s in [Service::MainApp, Service::Brain] {
            assert_ne!(value_name(s, true), value_name(s, false), "the test build never writes the installed app's values");
            assert!(value_name(s, true).ends_with("(test)"));
        }
    }

    #[test]
    fn startup_approved_is_on_off_or_unknown_never_guessed_off() {
        assert_eq!(approved(None), Approved::On, "never switched in Task Manager");
        assert_eq!(approved(Some(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])), Approved::On);
        assert_eq!(approved(Some(&[3, 0, 0, 0, 0x10, 0x20, 0, 0, 0, 0, 0, 0])), Approved::Off);
        assert_eq!(approved(Some(&[6, 0])), Approved::Unknown(6));
        assert_eq!(approved(Some(&[])), Approved::On);
        let want = r#""C:\B\Bellwether.exe" serve"#;
        assert_eq!(status_of(None, want, Approved::Off), Status::NotRegistered);
        assert_eq!(status_of(Some(want), want, Approved::On), Status::Enabled);
        assert_eq!(status_of(Some(&want.to_uppercase()), want, Approved::On), Status::Enabled, "NTFS ignores case");
        assert_eq!(status_of(Some(want), want, Approved::Off), Status::RequiresApproval, "turned off in Task Manager");
        assert_eq!(status_of(Some(want), want, Approved::Unknown(9)), Status::Enabled);
        assert_eq!(status_of(Some(r#""D:\Old\Bellwether.exe" serve"#), want, Approved::On), Status::Elsewhere, "the install moved");
    }

    // Forward slashes: Windows reads them too, and the Mac splits paths only on them.
    fn same(a: Option<PathBuf>, b: &str) -> bool {
        a.is_some_and(|a| same_path(&a, Path::new(b)))
    }

    #[test]
    fn only_a_folder_with_bin_is_an_install() {
        let files = ["C:/T/Bellwether.exe", "C:/T/bin/bellwether.exe", "C:/R/target-dev/debug/bellwether.exe", "C:/R/target-dev/debug/bellwether-shell.exe"];
        // NTFS ignores case, and so does this stand-in.
        let exists = |p: &Path| files.iter().any(|f| same_path(Path::new(f), p));
        assert!(same(install_exe_of(Path::new("C:/T/Bellwether.exe"), &exists), "C:/T/Bellwether.exe"));
        assert!(same(install_exe_of(Path::new("C:/T/bin/bellwether.exe"), &exists), "C:/T/Bellwether.exe"), "the console CLI finds its GUI exe");
        assert_eq!(install_exe_of(Path::new("C:/R/target-dev/debug/bellwether.exe"), &exists), None, "a development build has no bin\\");
        assert_eq!(install_exe_of(Path::new("C:/R/target-dev/debug/bellwether-shell.exe"), &exists), None);
        assert!(same_install(Path::new("c:/t/bellwether.old.exe"), Path::new("C:/T/Bellwether.exe")), "an old exe renamed beside the new one");
        assert!(!same_install(Path::new("C:/Other/Bellwether.exe"), Path::new("C:/T/Bellwether.exe")));
        assert!(!same_install(Path::new("C:/T/bin/bellwether.exe"), Path::new("C:/T/Bellwether.exe")), "a brain started by hand from a terminal is left alone");
        assert!(same_path(Path::new(r"C:\T\Home\"), Path::new("c:/t/home")));
    }

    #[test]
    fn the_test_folder_finds_its_own_home_and_a_run_value_never_needs_the_environment() {
        let exists = |p: &Path| same_path(p, Path::new("C:/T/run-test.cmd"));
        for exe in ["C:/T/Bellwether.exe", "C:/T/bin/bellwether.exe"] {
            let (home, config, agent) = test_folder_of(Path::new(exe), &exists).unwrap();
            assert!(same_path(&home, Path::new("C:/T/home")) && same_path(&config, Path::new("C:/T/config.json")) && same_path(&agent, Path::new("C:/T")), "{exe}");
        }
        assert_eq!(test_folder_of(Path::new("C:/Elsewhere/Bellwether.exe"), &exists), None);
        let src = include_str!("winstart.rs");
        let adopt = src.split("pub fn adopt_test_folder()").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert!(adopt.contains("if !test_build() ||"), "a release never adopts a folder");

        let folder = test_folder_of(Path::new("C:/T/Bellwether.exe"), &exists);
        let default = PathBuf::from("C:/Users/me/AppData/Roaming/Bellwether");
        assert_eq!(run_home(false, folder.as_ref(), default.clone()).unwrap(), default);
        assert!(same_path(&run_home(true, folder.as_ref(), default.clone()).unwrap(), Path::new("C:/T/home")));
        assert!(run_home(true, None, default.clone()).unwrap_err().contains("isn't in its test folder"));
    }

    #[test]
    fn writing_a_run_value_is_refused_when_login_would_use_another_home_or_no_token() {
        let ok = |_: &Path| Ok(());
        let no_token = |_: &Path| Err("refused: no token".to_string());
        let home = || Ok(PathBuf::from("C:/T/home"));
        assert!(preflight(&[Service::Brain, Service::MainApp], true, home(), None, &ok).is_ok());
        assert!(preflight(&[Service::Brain], true, home(), Some(PathBuf::from("c:/t/HOME/")), &ok).is_ok(), "cli.cmd's own home, as NTFS compares it");
        let e = preflight(&[Service::MainApp], true, home(), Some(PathBuf::from(r"C:\Other")), &ok).unwrap_err();
        assert!(e.contains("gets no environment") && e.contains("C:/T/home"), "{e}");
        assert_eq!(preflight(&[Service::Brain], true, home(), None, &no_token).unwrap_err(), "refused: no token");
        assert!(preflight(&[Service::MainApp], true, home(), None, &no_token).is_ok(), "only the brain needs the token");
        assert!(preflight(&[Service::Brain], false, Err("x".into()), Some(PathBuf::from(r"C:\Other")), &no_token).is_ok(), "removing is never refused");
    }

    #[test]
    fn the_running_strip_is_found_and_asked_through_the_plugins_window() {
        let dev = dev_identifier();
        assert!(!dev.is_empty() && dev != crate::identity::PERMANENT_ID);
        assert_eq!(strip_window_names(&format!("{dev}.p47491")), (format!("{dev}.p47491-sic"), format!("{dev}.p47491-siw")));
        assert_eq!(lock_ids(47491), vec!["com.fanzootechnology.bellwether.p47491".to_string(), format!("{dev}.p47491")]);
        assert_eq!(lock_ids(47417), vec!["com.fanzootechnology.bellwether".to_string(), dev]);
        assert_eq!(quit_payload(r"C:\T", r"C:\T\Bellwether.exe"), "C:\\T|C:\\T\\Bellwether.exe|--quit\0");
        // The plugin's own names and tag, in the version the lockfile pins.
        let lock = include_str!("../Cargo.lock");
        assert!(lock.contains("name = \"tauri-plugin-single-instance\"\nversion = \"2.5.2\""), "re-check the window names and COPYDATA tag against the plugin's platform_impl/windows.rs");
        assert_eq!(COPYDATA_TAG, 1542);
    }

    #[test]
    fn the_advice_says_task_manager_and_how_to_fix_it() {
        let off = advice(Service::Brain, Status::RequiresApproval);
        assert!(off.contains("Task Manager") && off.contains("bellwether autostart settings"), "{off}");
        assert!(advice(Service::MainApp, Status::Elsewhere).contains("another copy"));
        assert!(advice(Service::Brain, Status::NotRegistered).contains("autostart install --write"));
    }
}
