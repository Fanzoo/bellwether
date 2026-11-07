//! The few process calls the shell makes through libc on Unix, behind one name each, so
//! the crate compiles for Windows (Windows plan W1). On Unix they are exactly the libc
//! calls they replace. On Windows: `alive` asks the process itself (W6); the rest are
//! stand-ins, as Windows does those another way: the strip is stopped through its
//! single-instance window (winstart.rs), the locator compares SIDs (W3).

/// The real user id (`getuid`). Windows: 0, unused there until W3.
pub fn uid() -> u32 {
    #[cfg(unix)]
    return unsafe { libc::getuid() };
    #[cfg(not(unix))]
    0
}

/// The parent's pid (`getppid`). Windows: 0 (no parent excluded) until W2.
pub fn parent_pid() -> i32 {
    #[cfg(unix)]
    return unsafe { libc::getppid() };
    #[cfg(not(unix))]
    0
}

/// Whether a pid is alive (`kill(pid, 0) == 0`; Windows: its exit code is still
/// STILL_ACTIVE).
#[cfg_attr(windows, allow(dead_code))]
pub fn alive(pid: i32) -> bool {
    #[cfg(unix)]
    return unsafe { libc::kill(pid, 0) } == 0;
    #[cfg(windows)]
    return crate::winstart::alive(pid as u32);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// SIGTERM to a pid. Windows: nothing (no SIGTERM for a windowless process; the strip is
/// asked through its single-instance window, the brain through the tokened stop, WD6).
#[cfg_attr(windows, allow(dead_code))]
pub fn terminate(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}
