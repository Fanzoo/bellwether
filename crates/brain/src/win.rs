//! The brain's Windows calls (Windows plan WD6, W2): the exclusive port bind and the
//! parent watchdog by process handle. Thin FFI only.

use std::net::SocketAddr;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Networking::WinSock::{setsockopt, SOL_SOCKET, SO_EXCLUSIVEADDRUSE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE};

/// Binds 127.0.0.1:<port> with `SO_EXCLUSIVEADDRUSE`: Windows otherwise lets a second
/// socket with `SO_REUSEADDR` bind a port this one holds. With it, a second brain fails to
/// bind (AddrInUse) and exits, as on macOS.
pub fn bind_exclusive(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket;
    let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    let on: i32 = 1;
    let r = unsafe { setsockopt(s.as_raw_socket() as usize, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, &on as *const i32 as *const u8, 4) };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    s.bind(&addr.into())?;
    s.listen(1024)?;
    Ok(s.into())
}

/// This process's parent pid, from a Toolhelp snapshot.
fn parent_pid() -> Option<u32> {
    let me = std::process::id();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        let mut found = None;
        while ok {
            if e.th32ProcessID == me {
                found = Some(e.th32ParentProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

fn created(h: HANDLE) -> Option<u64> {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    (unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| (c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64)
}

/// The parent the brain was started by (`BELLWETHER_PARENT_PID`), opened to wait on.
/// None when it is already gone: it is not this process's parent any more, or the pid now
/// belongs to a process younger than this one (the pid was reused).
pub struct Parent(HANDLE);

unsafe impl Send for Parent {}

impl Parent {
    pub fn open(pid: u32) -> Option<Parent> {
        if parent_pid() != Some(pid) {
            return None;
        }
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        match (created(h), created(unsafe { GetCurrentProcess() })) {
            (Some(theirs), Some(mine)) if theirs <= mine => Some(Parent(h)),
            _ => {
                unsafe { CloseHandle(h) };
                None
            }
        }
    }

    /// Blocks until the parent process has exited (however it ended): true. False if the
    /// wait itself failed.
    pub fn wait(self) -> bool {
        unsafe {
            let r = WaitForSingleObject(self.0, INFINITE);
            CloseHandle(self.0);
            r == WAIT_OBJECT_0
        }
    }
}

// ------------------------------------------------------------------ doctor (read only)

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The executable of a process of this user (or any process the user may query), by
/// `QueryFullProcessImageNameW`: what doctor says answers the port.
pub fn process_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The pid listening on 127.0.0.1:<port> (or on every address), from the TCP table
/// (`GetExtendedTcpTable`, read only), and its executable when it can be read.
pub fn port_owner(port: u16) -> Option<(u32, Option<String>)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    let mut size: u32 = 0;
    unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) };
    if size == 0 {
        return None;
    }
    // u32s, so the table (a u32 count, then rows of six u32s) is aligned.
    let mut buf = vec![0u32; (size as usize).div_ceil(4) + 6];
    let mut size = (buf.len() * 4) as u32;
    if unsafe { GetExtendedTcpTable(buf.as_mut_ptr() as *mut _, &mut size, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) } != 0 {
        return None;
    }
    let n = buf[0] as usize;
    let rows = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(1) as *const MIB_TCPROW_OWNER_PID, n.min((buf.len() - 1) / 6)) };
    let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
    rows.iter()
        .find(|r| u16::from_be(r.dwLocalPort as u16) == port && (r.dwLocalAddr == loopback || r.dwLocalAddr == 0))
        .map(|r| (r.dwOwningPid, process_path(r.dwOwningPid)))
}

/// `HH:MM:SS` in the local time zone for a time in ms since 1970.
pub fn local_hms(ms: f64) -> Option<String> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    // FILETIME: 100 ns intervals since 1601.
    let t = (ms as u64) * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        (FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0).then(|| format!("{:02}:{:02}:{:02}", local.wHour, local.wMinute, local.wSecond))
    }
}

/// Claude Code's policy settings in the registry (`SOFTWARE\Policies\ClaudeCode`, value
/// `Settings`, a JSON string), machine-wide and the user's: (where, the text).
pub fn policy_settings() -> Vec<(String, String)> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
    let read = |root: HKEY| -> Option<String> {
        let (key, value) = (wide("SOFTWARE\\Policies\\ClaudeCode"), wide("Settings"));
        let mut len: u32 = 0;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        if unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) } != 0 || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize / 2 + 1];
        let mut len = (buf.len() * 2) as u32;
        if unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut len) } != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    };
    [("HKLM", HKEY_LOCAL_MACHINE), ("HKCU", HKEY_CURRENT_USER)]
        .into_iter()
        .filter_map(|(name, root)| read(root).map(|t| (format!("{name}\\SOFTWARE\\Policies\\ClaudeCode\\Settings"), t)))
        .collect()
}
