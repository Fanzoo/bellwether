//! bellwether's brain in Rust (distribution plan, milestone 1): the loopback listener
//! Claude Code's HTTP hooks post to.
//!
//!   POST /hook    always an empty 204, or an empty 403 when a guard fails
//!   GET  /health  {ok, name, version, pid, port, lastEventAt, mode, counters}; no token
//!   OPTIONS *     an empty 405, with no CORS headers
//!   anything else an empty 404
//!
//! `/state` and `/control` come with the state machine in milestone 3; until then the
//! shell keeps talking to the Node brain. The contract is the Node brain's
//! (brain/server.mjs, brain/hook-route.mjs), pinned by test/hook-route-blackbox.test.mjs,
//! which runs against both.
//!
//! Platform-neutral: std, tokio, hyper, sha2 and serde_json only.

mod hook;
pub mod home;

pub use hook::{HookAnswer, MAX_BODY_BYTES};

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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The bind address is a constant, never configuration.
pub const BIND_HOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
pub const DEFAULT_PORT: u16 = 47417;
/// Claude Code gives up after `timeout: 1`; nothing legitimate is slower than these.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Exit codes, the same as the Node CLI's: 0 ok, 1 unexpected, 2 not running / port
/// held, 3 bad config or token.
pub mod exit {
    pub const OK: i32 = 0;
    pub const UNEXPECTED: i32 = 1;
    pub const NOT_RUNNING: i32 = 2;
    pub const BAD_CONFIG: i32 = 3;
}

/// A failure `serve` reports in one line, with its exit code.
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
    /// Events the state machine couldn't take in time (its channel was full). Always 0
    /// until milestone 3 brings the state machine; new compared with the Node brain.
    pub dropped: AtomicU64,
    pub last_rejected_at: Mutex<Option<u64>>,
    pub last_event_at: Mutex<Option<u64>>,
}

impl Counters {
    pub fn json(&self) -> serde_json::Value {
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
        })
    }
}

/// What `serve` needs.
pub struct Options {
    pub token: String,
    pub mode: Mode,
    pub log: Logger,
    /// How long a /hook body may take to arrive: REQUEST_TIMEOUT, shorter only in tests.
    pub request_timeout: Duration,
}

impl Options {
    pub fn new(token: String, mode: Mode, log: Logger) -> Options {
        Options { token, mode, log, request_timeout: REQUEST_TIMEOUT }
    }
}

/// Content-free log lines, `<ms> bellwether: <line>`, to stderr and (if set) a file.
#[derive(Clone, Default)]
pub struct Logger {
    file: Option<PathBuf>,
}

impl Logger {
    pub fn to_file(path: PathBuf) -> Logger {
        if let Some(dir) = path.parent() {
            let _ = home::create_private_dir(dir);
        }
        Logger { file: Some(path) }
    }

    pub fn stderr_only() -> Logger {
        Logger { file: None }
    }

    pub fn log(&self, line: &str) {
        let text = format!("{} bellwether: {line}\n", now_ms());
        let _ = std::io::stderr().write_all(text.as_bytes());
        if let Some(path) = &self.file {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(text.as_bytes());
            }
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Everything a request handler can see.
pub(crate) struct Brain {
    pub port: u16,
    pub expected: [u8; 32],
    pub mode: Mode,
    pub counters: Counters,
    pub log: Logger,
    pub request_timeout: Duration,
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
    let brain = Arc::new(Brain { port: bound.port, expected: hook::digest(opts.token.as_bytes()), mode: opts.mode, counters: Counters::default(), log: opts.log, request_timeout: opts.request_timeout });
    let counters_of = brain.clone();
    let log = brain.log.clone();
    let accepting = tokio::task::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let brain = brain.clone();
            tokio::task::spawn(async move {
                let service = hyper::service::service_fn(move |req| route(brain.clone(), req));
                let conn = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(HEADER_TIMEOUT)
                    .serve_connection(TokioIo::new(stream), service);
                let _ = conn.await;
            });
        }
    });
    stop_signal(stop, log).await;
    accepting.abort();
    Ok(counters_of.counters.accepted.load(Ordering::Relaxed))
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

type Body = BoxBody<Bytes, Infallible>;

async fn route(brain: Arc<Brain>, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path();
    if path == "/hook" && req.method() == Method::POST {
        // The only conversion from a HookAnswer: a status and an empty body.
        return Ok(hook::handle(brain, req).await.into_response().map(|b| b.boxed()));
    }
    if path == "/health" && req.method() == Method::GET {
        return Ok(health(&brain));
    }
    let status = if req.method() == Method::OPTIONS { StatusCode::METHOD_NOT_ALLOWED } else { StatusCode::NOT_FOUND };
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
        "counters": brain.counters.json(),
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

/// The hook token's format: at least 16 characters of [A-Za-z0-9_-] (base64url), and
/// never a value that has been published as an example.
pub fn check_token(token: &str) -> Result<(), BrainError> {
    if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(BrainError::new("the hook token must be at least 16 characters of [A-Za-z0-9_-] (base64url)", exit::BAD_CONFIG));
    }
    if KNOWN_EXAMPLE_TOKENS.contains(&token) {
        return Err(BrainError::new("the hook token is a published example value, so anyone can read it; generate your own", exit::BAD_CONFIG));
    }
    Ok(())
}

/// Token values that have been committed as examples somewhere (brain/config.mjs).
pub const KNOWN_EXAMPLE_TOKENS: &[&str] = &["q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh"];

/// `serve [--port <n>] [--hang | --slow=<ms>]`: parses, finds the token, binds, logs
/// and serves. Returns the exit code.
pub fn cli_serve(args: &[String]) -> i32 {
    match serve_from_args(args) {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("bellwether: {e}");
            e.code
        }
    }
}

fn serve_from_args(args: &[String]) -> Result<(), BrainError> {
    let parsed = ServeArgs::parse(args)?;
    let home = home::dir();
    let port = match parsed.port {
        Some(p) => p,
        None => home::configured_port(&home)?.unwrap_or(DEFAULT_PORT),
    };
    let token = home::token(&home)?;
    check_token(&token)?;
    let log = Logger::to_file(home::log_dir().join("brain.log"));
    let bound = bind(port)?;
    let mode = match parsed.mode {
        Mode::Hang => " (debug: --hang, /hook never answers)".to_string(),
        Mode::Slow(ms) => format!(" (debug: --slow={ms})"),
        Mode::Normal => String::new(),
    };
    log.log(&format!("listening on http://{BIND_HOST}:{}{mode}, pid {}, version {VERSION} (rust)", bound.port, std::process::id()));
    let accepted = serve(bound, Options::new(token, parsed.mode, log.clone()), None)?;
    log.log(&format!("stopped ({accepted} events accepted)"));
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct ServeArgs {
    pub port: Option<u16>,
    pub mode: Mode,
}

impl ServeArgs {
    /// Strict, like the Node CLI: an unknown option, a stray argument or a missing
    /// value is refused (exit 3).
    pub fn parse(args: &[String]) -> Result<ServeArgs, BrainError> {
        let bad = |m: String| BrainError::new(m, exit::BAD_CONFIG);
        let (mut port, mut hang, mut slow) = (None, false, None);
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
                _ => (a.as_str(), None),
            };
            let mut value = |what: &str| -> Result<String, BrainError> {
                if let Some(v) = inline.clone() {
                    return Ok(v);
                }
                i += 1;
                args.get(i).filter(|v| !v.starts_with("--")).cloned().ok_or_else(|| bad(format!("{what} needs a value")))
            };
            match name {
                "--port" => {
                    let v = value("--port")?;
                    let p: u16 = v.parse().ok().filter(|p| *p >= 1024).ok_or_else(|| bad("--port takes an integer from 1024 to 65535".into()))?;
                    port = Some(p);
                }
                "--hang" if inline.is_none() => hang = true,
                "--slow" => {
                    let v = value("--slow")?;
                    slow = Some(v.parse::<u64>().map_err(|_| bad("--slow takes a delay in milliseconds, e.g. --slow=1500".into()))?);
                }
                _ if a.starts_with('-') => return Err(bad(format!("unknown option {a:?} for serve"))),
                _ => return Err(bad(format!("unexpected argument {a:?} for serve"))),
            }
            i += 1;
        }
        let mode = match (hang, slow) {
            (true, Some(ms)) if ms > 0 => return Err(bad("--hang and --slow are exclusive".into())),
            (true, _) => Mode::Hang,
            (false, Some(ms)) if ms > 0 => Mode::Slow(ms),
            _ => Mode::Normal,
        };
        Ok(ServeArgs { port, mode })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn serve_args_are_strict() {
        assert_eq!(ServeArgs::parse(&args(&[])).unwrap(), ServeArgs { port: None, mode: Mode::Normal });
        assert_eq!(ServeArgs::parse(&args(&["--port", "47418"])).unwrap().port, Some(47418));
        assert_eq!(ServeArgs::parse(&args(&["--port=47418"])).unwrap().port, Some(47418));
        assert_eq!(ServeArgs::parse(&args(&["--slow=150"])).unwrap().mode, Mode::Slow(150));
        assert_eq!(ServeArgs::parse(&args(&["--hang"])).unwrap().mode, Mode::Hang);
        for bad in [&["--port"][..], &["--port", "80"], &["--port", "x"], &["--hang", "--slow=5"], &["--nope"], &["stray"], &["--slow=abc"], &["--__proto__"]] {
            let e = ServeArgs::parse(&args(bad)).unwrap_err();
            assert_eq!(e.code, exit::BAD_CONFIG, "{bad:?}");
        }
    }

    #[test]
    fn token_format() {
        assert!(check_token("testTokenNotReal_abcdefghijklmnop").is_ok());
        assert!(check_token("short").is_err());
        assert!(check_token("has spaces in it but is long").is_err());
        assert!(check_token(KNOWN_EXAMPLE_TOKENS[0]).is_err());
    }
}
