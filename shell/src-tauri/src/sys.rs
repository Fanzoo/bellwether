//! The few process calls the shell makes through libc on Unix, behind one name each, so
//! the crate compiles for Windows (Windows plan W1). On Unix they are exactly the libc
//! calls they replace. On Windows they are stand-ins until the milestone that needs them:
//! the parent pid and the strip stop are W2's and W6's (a Toolhelp snapshot, the tokened
//! stop), the uid is W3's (the locator's same-user rule compares SIDs there).

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

/// Whether a pid is alive (`kill(pid, 0) == 0`). Windows: false until W2; nothing lists
/// processes there yet (`identity::bundle_processes` is empty).
pub fn alive(pid: i32) -> bool {
    #[cfg(unix)]
    return unsafe { libc::kill(pid, 0) } == 0;
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// SIGTERM to a pid. Windows: nothing until W6 (the tokened stop; Windows has no
/// SIGTERM for a windowless process).
pub fn terminate(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}
