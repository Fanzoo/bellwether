//! Windows' own calls for the strip (Windows plan W1): the tool-window style (WD2), keeping
//! it on top (deviation W1-15) and the desktop dump's collection (WD12). Thin FFI only; the
//! decisions are in desktop.rs and dump.rs, which the Mac tests.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_STYLECHANGING, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    EVENT_SYSTEM_FOREGROUND, HWND_TOPMOST, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::desktop::OnTop;
use crate::dump;

/// The subclass id of the tool-window keeper (any number unique to this window).
const KEEP_TOOL: usize = 0xB311;

/// Every extended-style change on a strip passes through here first, and keeps
/// `WS_EX_TOOLWINDOW` (and never `WS_EX_APPWINDOW`) in it. tao rewrites the whole extended
/// style whenever a window flag changes (click-through on and off at 30 Hz, show, hide), so
/// a style set once would be lost at the first toggle (deviation W1-2).
unsafe extern "system" fn keep_tool(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_STYLECHANGING && wparam as i32 == GWL_EXSTYLE && lparam != 0 {
        let s = lparam as *mut STYLESTRUCT;
        (*s).styleNew = ((*s).styleNew | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Makes a strip a tool window, for good: in neither the taskbar nor Alt-Tab nor Task
/// View (WD2 step 1). Main thread (the window's own). Returns the log line.
pub fn make_tool_window(hwnd: HWND) -> String {
    unsafe {
        let kept = SetWindowSubclass(hwnd, Some(keep_tool), KEEP_TOOL, 0) != 0;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ((ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW) as isize);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        let now = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
        match (now, kept) {
            (true, true) => "a tool window (WS_EX_TOOLWINDOW, kept on every style change): not in the taskbar, Alt-Tab or Task View".into(),
            (true, false) => "warning: a tool window now, but the style keeper didn't install; it may show in Alt-Tab after the first click-through toggle".into(),
            (false, _) => "warning: WS_EX_TOOLWINDOW didn't take; the strip may show in Alt-Tab".into(),
        }
    }
}

fn topmost(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 }
}

/// Puts a strip back on top of the z-order's topmost band, and says whether it had to be
/// (deviation W1-15; W2-17). `SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE`: the frame stays, and
/// neither activation nor focus moves; the extended style's other bits (tool window,
/// click-through, no-activate) are not touched. Main thread.
pub fn keep_topmost(hwnd: HWND) -> OnTop {
    let before = topmost(hwnd);
    unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    crate::desktop::on_top(before, topmost(hwnd))
}

/// What the foreground-change hook calls (set once).
static ON_FOREGROUND: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

unsafe extern "system" fn foreground_changed(_hook: HWINEVENTHOOK, _event: u32, _hwnd: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    if let Some(f) = ON_FOREGROUND.get() {
        // A panic must not cross into Windows.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    }
}

/// Calls `f` whenever another process's window comes to the foreground
/// (`EVENT_SYSTEM_FOREGROUND`, out of context, so `f` runs on this thread while it pumps
/// messages: call from the main thread). Once per process; false if it couldn't be set.
pub fn on_foreground_change(f: Box<dyn Fn() + Send + Sync>) -> bool {
    if ON_FOREGROUND.set(f).is_err() {
        return false;
    }
    let hook = unsafe {
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, std::ptr::null_mut(), Some(foreground_changed), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
    };
    !hook.is_null()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let (k, v) = (wide(key), wide(value));
    let mut out: u32 = 0;
    let mut len = 4u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, k.as_ptr(), v.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut out as *mut u32 as *mut c_void, &mut len) };
    (r == 0).then_some(out)
}

fn reg_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut len) };
    (r == 0).then(|| from_wide(&buf))
}

fn os() -> dump::Os {
    const KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let n = |v: &str| reg_dword(KEY, v).map(|x| x.to_string()).unwrap_or_else(|| "?".into());
    let build = reg_string(KEY, "CurrentBuildNumber").unwrap_or_else(|| "?".into());
    dump::Os {
        version: format!("{}.{}.{}.{}", n("CurrentMajorVersionNumber"), n("CurrentMinorVersionNumber"), build, n("UBR")),
        display_version: reg_string(KEY, "DisplayVersion").unwrap_or_default(),
    }
}

fn rect4(r: RECT) -> [i32; 4] {
    [r.left, r.top, r.right, r.bottom]
}

unsafe extern "system" fn each_monitor(m: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> i32 {
    let list = &mut *(data as *mut Vec<HMONITOR>);
    list.push(m);
    1
}

fn monitors(strip_height_px: f64) -> Vec<dump::MonitorInfo> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each_monitor), &mut handles as *mut _ as LPARAM) };
    handles
        .into_iter()
        .filter_map(|m| unsafe {
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
                return None;
            }
            let (mut dx, mut dy) = (96u32, 96u32);
            let scale = if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) == 0 { dx as f64 / 96.0 } else { 1.0 };
            let mi = info.monitorInfo;
            Some(dump::monitor(from_wide(&info.szDevice), mi.dwFlags & 1 != 0, scale, rect4(mi.rcMonitor), rect4(mi.rcWork), strip_height_px))
        })
        .collect()
}

fn processes() -> Vec<dump::Process> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            out.push(dump::Process { pid: e.th32ProcessID, ppid: e.th32ParentProcessID, exe: from_wide(&e.szExeFile) });
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

unsafe extern "system" fn each_window(hwnd: HWND, data: LPARAM) -> i32 {
    if IsWindowVisible(hwnd) != 0 {
        let list = &mut *(data as *mut Vec<HWND>);
        list.push(hwnd);
    }
    1
}

fn windows(procs: &[dump::Process]) -> Vec<dump::Window> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(each_window), &mut handles as *mut _ as LPARAM) };
    handles
        .into_iter()
        .map(|hwnd| unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let mut class = [0u16; 256];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let mut r: RECT = std::mem::zeroed();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32) != 0 {
                GetWindowRect(hwnd, &mut r);
            }
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut c_void, 4);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            dump::Window {
                hwnd: hwnd as usize as u64,
                pid,
                exe: procs.iter().find(|p| p.pid == pid).map(|p| p.exe.clone()).unwrap_or_default(),
                class: String::from_utf16_lossy(&class[..n]),
                bounds: rect4(r),
                cloaked: cloaked != 0,
                tool: ex & WS_EX_TOOLWINDOW != 0,
                topmost: ex & WS_EX_TOPMOST != 0,
            }
        })
        .collect()
}

/// The desktop as this process sees it with per-monitor DPI v2 (as the strip runs), so
/// every rectangle is in physical pixels. Called before anything else of the app starts.
pub fn desktop(strip_height_px: f64) -> dump::Desktop {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let procs = processes();
    dump::Desktop {
        version: 1,
        os: os(),
        monitors: monitors(strip_height_px),
        foreground: unsafe { GetForegroundWindow() } as usize as u64,
        windows: windows(&procs),
        processes: procs,
    }
}
