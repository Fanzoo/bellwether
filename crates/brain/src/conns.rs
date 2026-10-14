//! Bounded connections (milestone 3, from the M1 review): the M1 brain accepted any
//! number of connections, and with about 250 idle ones open (launchd gives an agent 256
//! file descriptors) a new hook waited for the 10 s header timeout. Now:
//!
//! - at start the brain raises its soft file-descriptor limit to the hard limit (up to
//!   [`FD_TARGET`]), as Node does;
//! - a connection with no request in flight that has sent nothing for [`IDLE_TIMEOUT`] is
//!   closed;
//! - at most `max` connections are open. A new one past the cap closes the connection
//!   that has been idle longest; if none is idle, the oldest long-poll on `/state` (whose
//!   client simply polls again). A hook in flight is never closed.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;

/// How long a connection may sit with nothing in flight and nothing sent.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// The default cap on open connections.
pub const MAX_CONNECTIONS: usize = 1024;
/// The soft file-descriptor limit the brain asks for.
pub const FD_TARGET: u64 = 8192;

/// One open connection, as the registry sees it.
pub struct Conn {
    started: Instant,
    /// ms since `started` when a byte last arrived, or a request finished.
    last_active: AtomicU64,
    /// Requests being handled now.
    pub busy: AtomicUsize,
    /// A `/state` long-poll is waiting on this connection.
    pub waiting: AtomicBool,
    pub kill: Notify,
}

impl Conn {
    fn new() -> Conn {
        Conn { started: Instant::now(), last_active: AtomicU64::new(0), busy: AtomicUsize::new(0), waiting: AtomicBool::new(false), kill: Notify::new() }
    }

    pub fn touch(&self) {
        self.last_active.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle_for(&self) -> Option<Duration> {
        if self.busy.load(Ordering::Relaxed) > 0 {
            return None;
        }
        let last = Duration::from_millis(self.last_active.load(Ordering::Relaxed));
        Some(self.started.elapsed().saturating_sub(last))
    }

    /// Resolves when this connection has been idle for `timeout`.
    pub async fn idle(&self, timeout: Duration) {
        loop {
            let wait = match self.idle_for() {
                Some(d) if d >= timeout => return,
                Some(d) => timeout - d,
                None => timeout,
            };
            tokio::time::sleep(wait.max(Duration::from_millis(50))).await;
        }
    }
}

/// Every open connection, and the cap.
pub struct Registry {
    max: usize,
    next: AtomicU64,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    /// Connections closed to make room (for /health and tests).
    pub evicted: AtomicU64,
}

impl Registry {
    pub fn new(max: usize) -> Registry {
        Registry { max: max.max(1), next: AtomicU64::new(1), conns: Mutex::new(HashMap::new()), evicted: AtomicU64::new(0) }
    }

    pub fn open(&self) -> usize {
        self.conns.lock().unwrap().len()
    }

    /// Registers a new connection, closing another first when at the cap.
    pub fn admit(&self) -> (u64, Arc<Conn>) {
        let mut conns = self.conns.lock().unwrap();
        if conns.len() >= self.max {
            let idle = conns.iter().filter_map(|(id, c)| c.idle_for().map(|d| (*id, d))).max_by_key(|(_, d)| *d).map(|(id, _)| id);
            let victim = idle.or_else(|| conns.iter().filter(|(_, c)| c.waiting.load(Ordering::Relaxed)).min_by_key(|(_, c)| c.started).map(|(id, _)| *id));
            if let Some(id) = victim {
                if let Some(c) = conns.remove(&id) {
                    c.kill.notify_one();
                    self.evicted.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let c = Arc::new(Conn::new());
        conns.insert(id, c.clone());
        (id, c)
    }

    pub fn close(&self, id: u64) {
        self.conns.lock().unwrap().remove(&id);
    }
}

/// A stream that tells its connection when bytes arrive.
pub struct Tracked<S> {
    inner: S,
    conn: Arc<Conn>,
}

impl<S> Tracked<S> {
    pub fn new(inner: S, conn: Arc<Conn>) -> Tracked<S> {
        Tracked { inner, conn }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Tracked<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.conn.touch();
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tracked<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Raises the soft limit on open files towards the hard limit (at most FD_TARGET), and
/// returns the soft limit now in force (None where there is no such limit).
pub fn raise_fd_limit() -> Option<u64> {
    #[cfg(unix)]
    unsafe {
        let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return None;
        }
        let want = (FD_TARGET as libc::rlim_t).min(lim.rlim_max);
        if lim.rlim_cur < want {
            let raised = libc::rlimit { rlim_cur: want, rlim_max: lim.rlim_max };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0 {
                lim.rlim_cur = want;
            }
        }
        Some(lim.rlim_cur as u64)
    }
    #[cfg(not(unix))]
    None
}

/// The cap to use: `want`, or fewer when the descriptor limit can't hold it.
pub fn cap_for(want: usize, fd_limit: Option<u64>) -> usize {
    match fd_limit {
        Some(n) => want.min((n as usize).saturating_sub(64).max(16)),
        None => want,
    }
}
