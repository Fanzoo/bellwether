//! The locator's Windows calls (plan WD4, milestone W3): the window list, the process list
//! with "is it this user's", a process's marker variables and command line (read from its
//! PEB), and raising a window. Thin FFI only: the decisions are in locator.rs, which the
//! Mac tests with fakes.

use std::collections::HashMap;
use std::ffi::c_void;
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleWindow, GetStdHandle, SetConsoleCtrlHandler, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ};
use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, FlashWindowEx, GetAncestor, GetForegroundWindow, GA_ROOTOWNER, GetWindowThreadProcessId, IsIconic, SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, SW_RESTORE};

use crate::locator::{self, Native, Proc, Win};
use crate::win;

/// The windows the locator considers (`locator::usable_windows`), front to back.
pub struct WinWindows;

impl locator::Windows for WinWindows {
    fn list(&self) -> Vec<Win> {
        let procs = win::processes();
        locator::usable_windows(win::windows(&procs), std::process::id(), |h| unsafe { IsIconic(h as usize as HWND) } != 0)
    }
}

pub struct WinNative;

/// Only these are filtered by user in the locator; the rest are left at 0 unchecked.
const CHECKED: [&str; 3] = ["claude", "node", "herdr"];

impl Native for WinNative {
    fn processes(&self) -> HashMap<i32, Proc> {
        let me = token_user(unsafe { GetCurrentProcess() });
        win::processes()
            .into_iter()
            .map(|p| {
                let stem = p.exe.strip_suffix(".exe").or_else(|| p.exe.strip_suffix(".EXE")).unwrap_or(&p.exe).to_ascii_lowercase();
                let uid = if CHECKED.contains(&stem.as_str()) && !same_user(me.as_deref(), p.pid) { u32::MAX } else { 0 };
                (p.pid as i32, Proc { pid: p.pid as i32, ppid: p.ppid as i32, uid, tty: String::new(), comm: p.exe })
            })
            .collect()
    }

    fn environment(&self, pid: i32, keys: &[&str]) -> Option<Vec<(String, String)>> {
        let block = read_params(pid as u32, Field::Environment)?;
        Some(locator::env_values(&block, keys))
    }

    fn command_line_has(&self, pid: i32, needle: &str) -> bool {
        read_params(pid as u32, Field::CommandLine).map(|w| String::from_utf16_lossy(&w).to_ascii_lowercase().contains(&needle.to_ascii_lowercase())).unwrap_or(false)
    }

    fn raise(&self, hwnd: u64) -> Result<(), String> {
        let h = hwnd as usize as HWND;
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            if SetForegroundWindow(h) != 0 && GetForegroundWindow() == h {
                return Ok(());
            }
            // Refused: only the process that had the last input may change the foreground,
            // and the focus runs a second or more after the click (herdr's CLI), so a mouse
            // move over another window in between loses it (PC test, 2026-10-31). Sharing the
            // foreground window's input state for a moment lifts the rule.
            let fg = GetForegroundWindow();
            let theirs = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
            let mine = GetCurrentThreadId();
            if theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, 1) != 0 {
                BringWindowToTop(h);
                SetForegroundWindow(h);
                AttachThreadInput(mine, theirs, 0);
                if GetForegroundWindow() == h {
                    crate::log::log("locator: the first SetForegroundWindow was refused; the second, sharing the foreground window's input, took");
                    return Ok(());
                }
            }
            let f = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: h, dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG, uCount: 0, dwTimeout: 0 };
            FlashWindowEx(&f);
        }
        Err("SetForegroundWindow".into())
    }

    fn console_host(&self, pid: i32) -> Option<u64> {
        console_host(pid as u32).map(|h| h as usize as u64)
    }
}

/// One console at a time: attaching is process-wide.
static CONSOLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// While attached to another process's console, its Ctrl-C, Ctrl-Break or close would also
/// reach this process, whose default answer is to exit: they are taken and ignored.
unsafe extern "system" fn ignore_console_events(_event: u32) -> i32 {
    1
}

/// The top-level window that owns `pid`'s console window: attach to its console for a
/// moment, read `GetConsoleWindow` (a Windows Terminal tab's hidden pseudo-console window,
/// owned by the Terminal window showing the tab), and detach. The standard handles are
/// put back as they were, so nothing started meanwhile inherits that console.
fn console_host(pid: u32) -> Option<HWND> {
    let _one = CONSOLE.lock().ok()?;
    unsafe {
        // A strip started from a terminal (a development run) has a console of its own.
        if !GetConsoleWindow().is_null() {
            return None;
        }
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|k| (k, GetStdHandle(k)));
        SetConsoleCtrlHandler(Some(ignore_console_events), 1);
        let host = if AttachConsole(pid) != 0 {
            let con = GetConsoleWindow();
            let root = if con.is_null() { std::ptr::null_mut() } else { GetAncestor(con, GA_ROOTOWNER) };
            FreeConsole();
            root
        } else {
            std::ptr::null_mut()
        };
        for (k, h) in saved {
            SetStdHandle(k, h);
        }
        SetConsoleCtrlHandler(Some(ignore_console_events), 0);
        (!host.is_null()).then_some(host)
    }
}

/// A process's user SID, as bytes.
fn token_user(process: HANDLE) -> Option<Vec<u8>> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = len > 0 && GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, len, &mut len) != 0;
        CloseHandle(token);
        ok.then_some(buf)
    }
}

fn same_user(me: Option<&[u8]>, pid: u32) -> bool {
    let Some(me) = me else { return false };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let theirs = token_user(h);
        CloseHandle(h);
        let Some(theirs) = theirs else { return false };
        let (a, b) = (&*(me.as_ptr() as *const TOKEN_USER), &*(theirs.as_ptr() as *const TOKEN_USER));
        EqualSid(a.User.Sid, b.User.Sid) != 0
    }
}

enum Field {
    CommandLine,
    Environment,
}

/// `PROCESS_BASIC_INFORMATION`, as ntdll fills it (declared here so no PEB type is needed).
#[repr(C)]
struct BasicInfo {
    exit_status: isize,
    peb: usize,
    affinity: usize,
    priority: isize,
    pid: usize,
    parent: usize,
}

// 64-bit offsets (x64 and ARM64): PEB.ProcessParameters; in RTL_USER_PROCESS_PARAMETERS
// the CommandLine UNICODE_STRING, the Environment pointer and EnvironmentSize.
const PEB_PARAMS: usize = 0x20;
const PARAMS_COMMAND_LINE: usize = 0x70;
const PARAMS_ENVIRONMENT: usize = 0x80;
const PARAMS_ENVIRONMENT_SIZE: usize = 0x3f0;
/// No environment or command line is read past this (a sane block is far smaller).
const MAX_BYTES: usize = 1 << 20;

/// A same-user process's command line or environment block, as UTF-16. Another user's
/// process can't be opened for reading, so it gives `None`.
fn read_params(pid: u32, field: Field) -> Option<Vec<u16>> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if h.is_null() {
            return None;
        }
        let out = read_params_of(h, field);
        CloseHandle(h);
        out
    }
}

unsafe fn read_params_of(h: HANDLE, field: Field) -> Option<Vec<u16>> {
    let mut info: BasicInfo = std::mem::zeroed();
    let mut len = 0u32;
    if NtQueryInformationProcess(h, ProcessBasicInformation, &mut info as *mut _ as *mut c_void, std::mem::size_of::<BasicInfo>() as u32, &mut len) != 0 || info.peb == 0 {
        return None;
    }
    let read = |addr: usize, buf: *mut c_void, n: usize| ReadProcessMemory(h, addr as *const c_void, buf, n, std::ptr::null_mut()) != 0;
    let mut params = 0usize;
    if !read(info.peb + PEB_PARAMS, &mut params as *mut usize as *mut c_void, 8) || params == 0 {
        return None;
    }
    let (addr, bytes) = match field {
        Field::CommandLine => {
            // UNICODE_STRING { u16 Length; u16 MaximumLength; (padding) PWSTR Buffer }
            let mut s = [0u8; 16];
            if !read(params + PARAMS_COMMAND_LINE, s.as_mut_ptr() as *mut c_void, 16) {
                return None;
            }
            (usize::from_le_bytes(s[8..16].try_into().ok()?), u16::from_le_bytes([s[0], s[1]]) as usize)
        }
        Field::Environment => {
            let (mut env, mut size) = (0usize, 0usize);
            if !read(params + PARAMS_ENVIRONMENT, &mut env as *mut usize as *mut c_void, 8) || !read(params + PARAMS_ENVIRONMENT_SIZE, &mut size as *mut usize as *mut c_void, 8) {
                return None;
            }
            (env, size)
        }
    };
    if addr == 0 || bytes == 0 || bytes > MAX_BYTES {
        return None;
    }
    let mut buf = vec![0u16; bytes / 2];
    read(addr, buf.as_mut_ptr() as *mut c_void, buf.len() * 2).then_some(buf)
}
