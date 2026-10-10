//! SIGINT, SIGTERM and SIGHUP: the handler only records which signal arrived (all a
//! signal handler may safely do); a thread in lib.rs notices and quits normally.

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicI32, Ordering};

    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    extern "C" fn on_signal(sig: libc::c_int) {
        RECEIVED.store(sig, Ordering::SeqCst);
    }

    pub fn install() {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: on_signal only performs an atomic store, which is async-signal-safe.
            unsafe {
                libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
            }
        }
    }

    pub fn received() -> Option<i32> {
        match RECEIVED.load(Ordering::SeqCst) {
            0 => None,
            s => Some(s),
        }
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn install() {}
    pub fn received() -> Option<i32> {
        None
    }
}

pub use imp::{install, received};
