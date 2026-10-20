//! bellwether's brain in Rust: the loopback listener Claude Code's HTTP hooks post to,
//! and the state the strip reads (distribution plan, milestones 1 and 3).
//!
//!   POST /hook     always an empty 204, or an empty 403 when a guard fails
//!   GET  /health   {ok, name, version, pid, port, lastEventAt, mode, counters}; no token
//!   GET  /state    the snapshot, long-polled (state.rs); token required
//!   POST /control  mute, unmute, dismiss, placed, reload-token (state.rs); token required
//!   OPTIONS *      an empty 405, with no CORS headers
//!   anything else  an empty 404
//!
//! The contract is the Node brain's (brain/server.mjs, hook-route.mjs, state-routes.mjs,
//! brain.mjs), pinned by the black-box suites that run against both. The state machine is
//! crates/core's, owned by one task (actor.rs). Platform-neutral: no Tauri, no AppKit.

mod actor;
pub mod cli;
pub mod client;
pub mod conns;
pub mod doctor;
mod hook;
pub mod home;
pub mod legacy;
pub mod migrate;
pub mod pets;
pub mod saver;
pub mod toapp;
mod state;

pub use actor::Timings;
pub use hook::{HookAnswer, DOCTOR_PING, EVENTS, MAX_BODY_BYTES};

use bellwether_core::config::Config;
use bellwether_core::publisher::Published;
use bellwether_core::redact::Cleaner;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The bind address is a constant, never configuration.
pub const BIND_HOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
pub const DEFAULT_PORT: u16 = 47417;
/// Claude Code gives up after `timeout: 1`; nothing legitimate is slower than these.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// After `reload-token`, how long the previous token is still accepted.
pub const TOKEN_GRACE: Duration = Duration::from_secs(60);

/// Exit codes, the same as the Node CLI's: 0 ok, 1 unexpected, 2 not running / port
/// held, 3 bad config or token.
pub mod exit {
    pub const OK: i32 = 0;
    pub const UNEXPECTED: i32 = 1;
    pub const NOT_RUNNING: i32 = 2;
    pub const BAD_CONFIG: i32 = 3;
}

/// A failure a command reports in one line, with its exit code.
#[derive(Debug)]
pub struct BrainError {
    pub message: String,
    pub code: i32,
}

impl BrainError {
    pub fn new(message: impl Into<String>, code: i32) -> BrainError {
        BrainError { message: message.into(), code }
    }
}

impl std::fmt::Display for BrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<bellwether_core::config::ConfigError> for BrainError {
    fn from(e: bellwether_core::config::ConfigError) -> BrainError {
        BrainError::new(e.message, e.code)
    }
}

/// Debug modes for measuring hook timeouts (`serve --hang`, `serve --slow=<ms>`). Both
/// still never send anything but an empty 204.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Hang,
    Slow(u64),
}

impl Mode {
    fn label(self) -> String {
        match self {
            Mode::Normal => "normal".into(),
            Mode::Hang => "hang".into(),
            Mode::Slow(ms) => format!("slow={ms}"),
        }
    }
}

/// The counters `/health` reports. They live as long as the process.
#[derive(Default)]
pub struct Counters {
    pub accepted: AtomicU64,
    pub rejected_origin: AtomicU64,
    pub rejected_host: AtomicU64,
    pub rejected_content_type: AtomicU64,
    pub rejected_token: AtomicU64,
    pub malformed: AtomicU64,
    pub oversize: AtomicU64,
    pub unknown_event: AtomicU64,
    pub consumer_errors: AtomicU64,
    /// Events the state machine couldn't take in time (its queue was full). New compared
    /// with the Node brain (test/DIVERGENCES.md).
    pub dropped: AtomicU64,
    pub last_rejected_at: Mutex<Option<u64>>,
    pub last_event_at: Mutex<Option<u64>>,
}

impl Counters {
    pub fn json(&self, log_dropped: u64) -> serde_json::Value {
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        serde_json::json!({
            "accepted": n(&self.accepted),
            "rejected": {
                "origin": n(&self.rejected_origin),
                "host": n(&self.rejected_host),
                "contentType": n(&self.rejected_content_type),
                "token": n(&self.rejected_token),
            },
            "lastRejectedAt": *self.last_rejected_at.lock().unwrap(),
            "malformed": n(&self.malformed),
            "oversize": n(&self.oversize),
            "unknownEvent": n(&self.unknown_event),
            "consumerErrors": n(&self.consumer_errors),
            "dropped": n(&self.dropped),
            "logDropped": log_dropped,
        })
    }
}

/// What `serve` needs.
pub struct Options {
    pub token: String,
    /// The token file `reload-token` reads again; None when the token came from the
    /// environment (it can't be reloaded).
    pub token_file: Option<PathBuf>,
    pub mode: Mode,
    pub log: Logger,
    /// How long a /hook body may take to arrive: REQUEST_TIMEOUT, shorter only in tests.
    pub request_timeout: Duration,
    /// The merged config (behaviour, bubble, palette).
    pub config: Config,
    /// state.json's directory (the app's home); None keeps nothing between runs.
    pub home: Option<PathBuf>,
    /// `record`: the recording to append event shapes to.
    pub record_to: Option<PathBuf>,
    pub timings: Timings,
    /// The connection cap (conns.rs); lowered to fit the file-descriptor limit.
    pub max_connections: usize,
    pub idle_timeout: Duration,
    /// Raise the soft file-descriptor limit at start (always, but for one test that shows
    /// what happens without it).
    pub raise_fd_limit: bool,
    /// Lines to log once state.json is read and the brain is about to serve (the
    /// "listening on" line comes after any warning about state.json, as in Node).
    pub announce: Vec<String>,
    /// What writes state.json on its own thread (tests put in a slow one).
    pub state_writer: saver::Writer,
}

impl Options {
    pub fn new(token: String, mode: Mode, log: Logger) -> Options {
        Options {
            token,
            token_file: None,
            mode,
            log,
            request_timeout: REQUEST_TIMEOUT,
            config: Config::defaults(),
            home: None,
            record_to: None,
            timings: Timings::default(),
            max_connections: conns::MAX_CONNECTIONS,
            idle_timeout: conns::IDLE_TIMEOUT,
            raise_fd_limit: true,
            announce: Vec::new(),
            state_writer: saver::default_writer(),
        }
    }
}

/// Content-free log lines, `<ms> bellwether: <line>`, to stderr and (if set) a file.
///
/// Never on the serving thread: `log` puts the line in a bounded queue for a writer thread
/// per sink, and drops it (counting it in `dropped`, shown as `/health` counters.logDropped)
/// when that queue is full. So a stderr pipe nobody reads, which blocks a write, stalls
/// only the writer thread: `/hook` never waits for a log line (plan D1).
#[derive(Clone, Default)]
pub struct Logger {
    sinks: Vec<std::sync::mpsc::SyncSender<LogMsg>>,
    dropped: Arc<AtomicU64>,
}

pub enum LogMsg {
    Line(Arc<String>),
    Flush(std::sync::mpsc::Sender<()>),
}

/// How many lines may wait for a sink before new ones are dropped.
pub const LOG_QUEUE: usize = 1024;

impl Logger {
    fn sink(mut out: impl Write + Send + 'static) -> std::sync::mpsc::SyncSender<LogMsg> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<LogMsg>(LOG_QUEUE);
        std::thread::spawn(move || {
            for msg in rx {
                match msg {
                    LogMsg::Line(text) => {
                        let _ = out.write_all(text.as_bytes());
                        let _ = out.flush();
                    }
                    LogMsg::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        tx
    }

    pub fn to_file(path: PathBuf) -> Logger {
        if let Some(dir) = path.parent() {
            let _ = home::create_private_dir(dir);
        }
        let mut sinks = vec![Logger::sink(std::io::stderr())];
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            sinks.push(Logger::sink(f));
        }
        Logger { sinks, dropped: Arc::new(AtomicU64::new(0)) }
    }

    pub fn stderr_only() -> Logger {
        Logger { sinks: vec![Logger::sink(std::io::stderr())], dropped: Arc::new(AtomicU64::new(0)) }
    }

    pub fn log(&self, line: &str) {
        let text = Arc::new(format!("{} bellwether: {line}\n", now_ms()));
        for s in &self.sinks {
            if s.try_send(LogMsg::Line(text.clone())).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Lines dropped because a sink's queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Waits (at most `limit` per sink) for the queued lines to be written: at exit.
    pub fn flush(&self, limit: Duration) {
        for s in &self.sinks {
            let (tx, rx) = std::sync::mpsc::channel();
            if s.try_send(LogMsg::Flush(tx)).is_ok() {
                let _ = rx.recv_timeout(limit);
            }
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The accepted token digests: the current one, and for a minute after `reload-token`
/// the previous one.
struct Tokens {
    current: [u8; 32],
    previous: Option<([u8; 32], Instant)>,
}

/// Everything a request handler can see.
pub(crate) struct Brain {
    pub port: u16,
    tokens: Mutex<Tokens>,
    token_file: Option<PathBuf>,
    pub mode: Mode,
    pub counters: Arc<Counters>,
    pub log: Logger,
    pub request_timeout: Duration,
    pub inbox: tokio::sync::mpsc::Sender<actor::Msg>,
    pub snapshots: tokio::sync::watch::Receiver<Arc<Published>>,
}

impl Brain {
    /// The guards, in order: Origin, Host, (for /hook) Content-Type, then exactly one
    /// X-Bellwether-Token that matches, compared in constant time over sha256 digests.
    pub(crate) fn rejection(&self, headers: &hyper::HeaderMap, content_type: bool) -> Option<hook::Rejection> {
        let t = self.tokens.lock().unwrap();
        let previous = t.previous.filter(|(_, until)| Instant::now() < *until).map(|(d, _)| d);
        hook::rejection_with(headers, self.port, &t.current, previous.as_ref(), content_type)
    }

    /// `/control {"action": "reload-token"}`: the token file again, the old token kept
    /// for TOKEN_GRACE.
    pub(crate) fn reload_token(&self) -> (bool, Option<String>) {
        let Some(file) = &self.token_file else {
            return (false, Some("this brain's token comes from BELLWETHER_HOOK_TOKEN in its environment; restart it to change the token".into()));
        };
        let home = file.parent().map(PathBuf::from).unwrap_or_default();
        match bellwether_core::token::read(&home, None) {
            Ok((token, _)) => {
                let mut t = self.tokens.lock().unwrap();
                let new = hook::digest(token.as_bytes());
                if new != t.current {
                    t.previous = Some((t.current, Instant::now() + TOKEN_GRACE));
                    t.current = new;
                }
                self.log.log("reloaded the hook token; the previous one is accepted for another minute");
                (true, None)
            }
            Err(e) => (false, Some(e.message)),
        }
    }
}

/// A bound listener, not yet serving. Binding is separate so `serve` can report a held
/// port before it says it is listening, and tests can learn the port.
pub struct Bound {
    listener: std::net::TcpListener,
    pub port: u16,
}

/// Binds 127.0.0.1:<port>, and only that. Port 0 asks the OS for a free one (tests).
pub fn bind(port: u16) -> Result<Bound, BrainError> {
    let listener = std::net::TcpListener::bind(SocketAddr::from((BIND_HOST, port))).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            BrainError::new(format!("port {port} on {BIND_HOST} is already in use. Run `bellwether doctor` to see what holds it."), exit::NOT_RUNNING)
        } else {
            BrainError::new(format!("cannot listen on {BIND_HOST}:{port}: {e}"), exit::UNEXPECTED)
        }
    })?;
    listener.set_nonblocking(true).map_err(|e| BrainError::new(format!("cannot listen: {e}"), exit::UNEXPECTED))?;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    Ok(Bound { listener, port })
}

/// Serves on a current-thread tokio runtime until SIGINT or SIGTERM (or `stop`
/// resolves, for tests). Returns the number of events accepted.
pub fn serve(bound: Bound, opts: Options, stop: Option<tokio::sync::oneshot::Receiver<()>>) -> Result<u64, BrainError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| BrainError::new(format!("cannot start the runtime: {e}"), exit::UNEXPECTED))?;
    rt.block_on(run(bound, opts, stop))
}

async fn run(bound: Bound, opts: Options, stop: Option<tokio::sync::oneshot::Receiver<()>>) -> Result<u64, BrainError> {
    let listener = tokio::net::TcpListener::from_std(bound.listener).map_err(|e| BrainError::new(format!("cannot listen: {e}"), exit::UNEXPECTED))?;
    let fd_limit = if opts.raise_fd_limit { conns::raise_fd_limit() } else { None };
    let registry = Arc::new(conns::Registry::new(conns::cap_for(opts.max_connections, fd_limit)));
    let counters = Arc::new(Counters::default());
    let cleaner = Cleaner::new(opts.config.max_chars(), &opts.config.redact_patterns()).map_err(|e| BrainError::new(e.to_string(), exit::BAD_CONFIG))?;
    let record_to = match &opts.record_to {
        Some(f) => {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Some(std::fs::OpenOptions::new().create(true).append(true).open(f).map_err(|e| BrainError::new(format!("cannot open {}: {}", f.display(), bellwether_core::config::io_code(&e)), exit::BAD_CONFIG))?)
        }
        None => None,
    };
    let state_file = match &opts.home {
        Some(h) => h.join("state.json"),
        None => std::env::temp_dir().join(format!("bellwether-nostate-{}", std::process::id())).join("state.json"),
    };
    let (actor_stop_tx, actor_stop) = tokio::sync::oneshot::channel();
    let setup = actor::Setup { merged: opts.config.merged.clone(), cleaner, state_file, record_to, timings: opts.timings, log: opts.log.clone(), counters: counters.clone(), writer: opts.state_writer.clone() };
    let (inbox, snapshots, actor_task) = actor::start(setup, actor_stop);
    let brain = Arc::new(Brain {
        port: bound.port,
        tokens: Mutex::new(Tokens { current: hook::digest(opts.token.as_bytes()), previous: None }),
        token_file: opts.token_file.clone(),
        mode: opts.mode,
        counters: counters.clone(),
        log: opts.log.clone(),
        request_timeout: opts.request_timeout,
        inbox,
        snapshots,
    });
    let log = brain.log.clone();
    for line in &opts.announce {
        log.log(line);
    }
    let idle_timeout = opts.idle_timeout;
    let accepting = tokio::task::spawn(async move {
        loop {
            let stream = match listener.accept().await {
                Ok((s, _)) => s,
                Err(_) => {
                    // Out of descriptors, say: let idle connections go before trying again.
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            let (id, conn) = registry.admit();
            conn.touch();
            let brain = brain.clone();
            let registry = registry.clone();
            tokio::task::spawn(async move {
                let c = conn.clone();
                let service = hyper::service::service_fn(move |req| {
                    let brain = brain.clone();
                    let c = c.clone();
                    async move {
                        c.busy.fetch_add(1, Ordering::Relaxed);
                        let r = route(brain, &c, req).await;
                        c.busy.fetch_sub(1, Ordering::Relaxed);
                        c.touch();
                        r
                    }
                });
                let served = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(HEADER_TIMEOUT)
                    .serve_connection(TokioIo::new(conns::Tracked::new(stream, conn.clone())), service);
                tokio::select! {
                    _ = served => {}
                    _ = conn.kill.notified() => {}
                    _ = conn.idle(idle_timeout) => {}
                }
                registry.close(id);
            });
        }
    });
    tokio::select! {
        _ = stop_signal(stop, log.clone()) => {}
        _ = parent_watchdog(log.clone()) => {}
    }
    accepting.abort();
    let _ = actor_stop_tx.send(());
    let _ = actor_task.await;
    Ok(counters.accepted.load(Ordering::Relaxed))
}

/// A brain the app started carries BELLWETHER_PARENT_PID: when that process is gone (even
/// by SIGKILL), this brain stops within a second (it looks every 250 ms). Never set for a
/// brain started by launchd or by hand.
async fn parent_watchdog(log: Logger) {
    #[cfg(unix)]
    {
        let parent = std::env::var("BELLWETHER_PARENT_PID").ok().and_then(|p| p.parse::<i32>().ok()).filter(|p| *p > 1);
        if let Some(parent) = parent {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if unsafe { libc::getppid() } != parent {
                    log.log(&format!("the app that started this brain (pid {parent}) is gone; stopping"));
                    return;
                }
            }
        }
    }
    let _ = log;
    std::future::pending::<()>().await
}

/// Resolves on SIGINT or SIGTERM, or when a test's `stop` fires.
async fn stop_signal(stop: Option<tokio::sync::oneshot::Receiver<()>>, log: Logger) {
    use std::future::Future;
    use std::task::Poll;
    let by_test = async {
        match stop {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let mut by_test = std::pin::pin!(by_test);
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut int), Ok(mut term)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else {
            return by_test.await;
        };
        let why = std::future::poll_fn(|cx| {
            if int.poll_recv(cx).is_ready() {
                return Poll::Ready(Some("SIGINT"));
            }
            if term.poll_recv(cx).is_ready() {
                return Poll::Ready(Some("SIGTERM"));
            }
            by_test.as_mut().poll(cx).map(|_| None)
        })
        .await;
        if let Some(sig) = why {
            log.log(&format!("{sig}, stopping"));
        }
    }
    #[cfg(not(unix))]
    {
        let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
        let why = std::future::poll_fn(|cx| {
            if ctrl_c.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Some("Ctrl-C"));
            }
            by_test.as_mut().poll(cx).map(|_| None)
        })
        .await;
        if let Some(sig) = why {
            log.log(&format!("{sig}, stopping"));
        }
    }
}

pub(crate) type Body = BoxBody<Bytes, Infallible>;

async fn route(brain: Arc<Brain>, conn: &Arc<conns::Conn>, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();
    if path == "/hook" && req.method() == Method::POST {
        // The only conversion from a HookAnswer: a status and an empty body.
        return Ok(hook::handle(brain, req).await.into_response().map(|b| b.boxed()));
    }
    let is_get = req.method() == Method::GET;
    let is_options = req.method() == Method::OPTIONS;
    if let Some(r) = state::route(&brain, conn, req, &path).await {
        return Ok(r);
    }
    if path == "/health" && is_get {
        return Ok(health(&brain));
    }
    let status = if is_options { StatusCode::METHOD_NOT_ALLOWED } else { StatusCode::NOT_FOUND };
    Ok(Response::builder().status(status).header("Content-Length", "0").body(Empty::new().boxed()).unwrap())
}

/// No token and no content: names, numbers and the mode only.
fn health(brain: &Brain) -> Response<Body> {
    let body = serde_json::json!({
        "ok": true,
        "name": "bellwether",
        "version": VERSION,
        "pid": std::process::id(),
        "port": brain.port,
        "lastEventAt": *brain.counters.last_event_at.lock().unwrap(),
        "mode": brain.mode.label(),
        "counters": brain.counters.json(brain.log.dropped()),
    })
    .to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len().to_string())
        .header("Cache-Control", "no-store")
        .body(Full::new(Bytes::from(body)).boxed())
        .unwrap()
}

/// The hook token's format (bellwether_core::token::check), as a BrainError.
pub fn check_token(token: &str) -> Result<(), BrainError> {
    bellwether_core::token::check(token).map_err(BrainError::from)
}

pub use bellwether_core::token::KNOWN_EXAMPLE_TOKENS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        assert!(check_token("testTokenNotReal_abcdefghijklmnop").is_ok());
        assert!(check_token("short").is_err());
        assert!(check_token("has spaces in it but is long").is_err());
        assert!(check_token(KNOWN_EXAMPLE_TOKENS[0]).is_err());
    }
}
