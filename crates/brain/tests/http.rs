//! The brain over real HTTP: every /hook answer is status-only, the guards refuse with
//! an empty 403, /health carries only names and numbers, /state and /control answer the
//! token holder only, and connections are bounded. Raw sockets, so every header (Host,
//! Origin) is under the test's control.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";

struct Running {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn start(mode: bellwether_brain::Mode) -> Running {
    start_with(bellwether_brain::Options::new(TOKEN.into(), mode, bellwether_brain::Logger::stderr_only()))
}

fn start_with(opts: bellwether_brain::Options) -> Running {
    let bound = bellwether_brain::bind(0).unwrap();
    let port = bound.port;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::spawn(move || {
        bellwether_brain::serve(bound, opts, Some(rx)).unwrap();
    });
    Running { port, stop: Some(tx), thread: Some(thread) }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// One request on a fresh connection (`Connection: close`), read to the end.
fn request(port: u16, method: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> Option<Reply> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    // Head and body in one write, as an HTTP client sends them. Two writes can reach the
    // brain as two segments: a guard answers 403 before reading the body, and Windows
    // closes a socket with unread data with a reset, which can lose the 403 here (the
    // VM run's one flake, W2).
    let mut all = head.into_bytes();
    all.extend_from_slice(body);
    s.write_all(&all).unwrap();
    let mut raw = Vec::new();
    if s.read_to_end(&mut raw).is_err() && raw.is_empty() {
        return None;
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let text = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = text.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines.filter_map(|l| l.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string()))).collect();
    Some(Reply { status, headers, body: raw[split + 4..].to_vec() })
}

fn hook_headers(port: u16) -> Vec<(&'static str, String)> {
    vec![("Host", format!("127.0.0.1:{port}")), ("Content-Type", "application/json".into()), ("X-Bellwether-Token", TOKEN.into())]
}

fn with(mut h: Vec<(&'static str, String)>, name: &'static str, value: Option<&str>) -> Vec<(&'static str, String)> {
    h.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    if let Some(v) = value {
        h.push((name, v.to_string()));
    }
    h
}

fn health(port: u16) -> serde_json::Value {
    let r = request(port, "GET", "/health", &[("Host", format!("127.0.0.1:{port}"))], b"").unwrap();
    assert_eq!(r.status, 200);
    serde_json::from_slice(&r.body).unwrap()
}

fn assert_empty(r: &Reply, status: u16) {
    assert_eq!(r.status, status);
    assert!(r.body.is_empty(), "body must be empty");
    assert!(r.header("content-type").is_none(), "no Content-Type");
    assert!(r.header("transfer-encoding").is_none(), "no chunked body");
}

fn settle() {
    std::thread::sleep(Duration::from_millis(30));
}

#[test]
fn hook_answers_are_status_only_and_the_guards_refuse() {
    let b = start(bellwether_brain::Mode::Normal);
    let p = b.port;
    let ok = request(p, "POST", "/hook", &hook_headers(p), br#"{"hook_event_name":"Stop","session_id":"s1"}"#).unwrap();
    assert_empty(&ok, 204);
    let ok = request(p, "POST", "/hook", &with(with(hook_headers(p), "Host", Some(&format!("localhost:{p}"))), "Content-Type", Some("application/json; charset=utf-8")), b"{}").unwrap();
    assert_empty(&ok, 204);

    let cases: Vec<(&str, Vec<(&'static str, String)>, &str)> = vec![
        ("an Origin", with(hook_headers(p), "Origin", Some("https://evil.example")), "origin"),
        ("a null Origin", with(hook_headers(p), "Origin", Some("null")), "origin"),
        ("Host evil.example", with(hook_headers(p), "Host", Some("evil.example")), "host"),
        ("Host on another port", with(hook_headers(p), "Host", Some("127.0.0.1:1")), "host"),
        ("no token", with(hook_headers(p), "X-Bellwether-Token", None), "token"),
        ("a wrong token", with(hook_headers(p), "X-Bellwether-Token", Some("wrong")), "token"),
        ("a prefix of the token", with(hook_headers(p), "X-Bellwether-Token", Some(&TOKEN[..TOKEN.len() - 1])), "token"),
        ("the pre-rename header", with(with(hook_headers(p), "X-Bellwether-Token", None), "X-Pasture-Token", Some(TOKEN)), "token"),
        ("text/plain", with(hook_headers(p), "Content-Type", Some("text/plain")), "contentType"),
        ("no Content-Type", with(hook_headers(p), "Content-Type", None), "contentType"),
    ];
    for (what, headers, counter) in cases {
        let before = health(p)["counters"]["rejected"][counter].as_u64().unwrap();
        let r = request(p, "POST", "/hook", &headers, br#"{"hook_event_name":"Stop","session_id":"spoof"}"#).unwrap();
        assert_empty(&r, 403);
        assert_eq!(r.header("content-length"), Some("0"), "{what}");
        assert_eq!(health(p)["counters"]["rejected"][counter].as_u64().unwrap(), before + 1, "{what}");
    }

    for (body, counter) in [(&br#"{"hook_event_name":"#[..], "malformed"), (b"[1,2,3]", "malformed"), (b"", "malformed"), (br#"{"hook_event_name":"SomethingNew"}"#, "unknownEvent")] {
        let before = health(p)["counters"][counter].as_u64().unwrap();
        assert_empty(&request(p, "POST", "/hook", &hook_headers(p), body).unwrap(), 204);
        settle();
        assert_eq!(health(p)["counters"][counter].as_u64().unwrap(), before + 1, "{counter}");
    }

    let big = vec![b' '; 9 * 1024 * 1024];
    assert_empty(&request(p, "POST", "/hook", &hook_headers(p), &big).unwrap(), 204);
    settle();
    assert_eq!(health(p)["counters"]["oversize"], 1);

    let r = request(p, "OPTIONS", "/hook", &[("Host", format!("127.0.0.1:{p}")), ("Origin", "https://evil.example".into())], b"").unwrap();
    assert_eq!(r.status, 405);
    assert!(r.header("access-control-allow-origin").is_none());
    assert!(r.body.is_empty());
    let r = request(p, "GET", "/hook", &hook_headers(p), b"").unwrap();
    assert_eq!(r.status, 404);
    assert!(r.body.is_empty());
    let r = request(p, "GET", "/state", &[("Host", format!("127.0.0.1:{p}"))], b"").unwrap();
    assert_eq!(r.status, 403, "/state needs the token");
    assert!(r.body.is_empty());
}

fn state_headers(port: u16, token: &str) -> Vec<(&'static str, String)> {
    vec![("Host", format!("127.0.0.1:{port}")), ("X-Bellwether-Token", token.to_string())]
}

fn json_body(r: &Reply) -> serde_json::Value {
    serde_json::from_slice(&r.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&r.body)))
}

/// /state and /control (brain/state-routes.mjs): the guards first, JSON with no CORS
/// headers, the long poll answering at the next publish, /control's 4xx answers.
#[test]
fn state_and_control() {
    let b = start(bellwether_brain::Mode::Normal);
    let p = b.port;
    for (what, h) in [
        ("no token", vec![("Host", format!("127.0.0.1:{p}"))]),
        ("an Origin", with(state_headers(p, TOKEN), "Origin", Some("https://evil.example"))),
        ("a foreign Host", with(state_headers(p, TOKEN), "Host", Some("evil.example"))),
    ] {
        let r = request(p, "GET", "/state?after=-1&wait=0", &h, b"").unwrap();
        assert_eq!(r.status, 403, "{what}");
        assert!(r.body.is_empty(), "{what}");
        let c = request(p, "POST", "/control", &with(h, "Content-Type", Some("application/json")), br#"{"action":"mute"}"#).unwrap();
        assert_eq!(c.status, 403, "{what}");
    }
    let r = request(p, "GET", "/state?after=-1&wait=0", &state_headers(p, TOKEN), b"").unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(r.header("access-control-allow-origin").is_none());
    let rev = json_body(&r)["rev"].as_u64().unwrap();

    // Nothing changes: the long poll answers after `wait`.
    let t0 = std::time::Instant::now();
    let idle = request(p, "GET", &format!("/state?after={rev}&wait=300"), &state_headers(p, TOKEN), b"").unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(280));
    assert_eq!(json_body(&idle)["rev"].as_u64(), Some(rev));

    // A hook changes the flock: the waiting poll answers at once.
    let waiter = std::thread::spawn(move || request(p, "GET", &format!("/state?after={rev}&wait=10000"), &state_headers(p, TOKEN), b"").unwrap());
    std::thread::sleep(Duration::from_millis(100));
    let t1 = std::time::Instant::now();
    let mut h = hook_headers(p);
    h.push(("X-Bellwether-Herdr-Pane", "w5:p5".into()));
    assert_empty(&request(p, "POST", "/hook", &h, br#"{"hook_event_name":"UserPromptSubmit","session_id":"lp","cwd":"/p/longpoll"}"#).unwrap(), 204);
    let changed = json_body(&waiter.join().unwrap());
    assert!(t1.elapsed() < Duration::from_secs(1), "answered promptly");
    assert!(changed["rev"].as_u64().unwrap() > rev);
    assert_eq!(changed["flock"][0]["label"], "longpoll");
    assert_eq!(changed["flock"][0]["identityKey"], "herdr:#w5:p5");

    let ctl = |body: &[u8], ct: &str| {
        let h = with(state_headers(p, TOKEN), "Content-Type", Some(ct));
        request(p, "POST", "/control", &h, body).unwrap()
    };
    let m = ctl(br#"{"action":"mute"}"#, "application/json");
    assert_eq!((m.status, json_body(&m)), (200, serde_json::json!({ "ok": true })));
    std::thread::sleep(Duration::from_millis(300));
    let s = request(p, "GET", "/state?after=-1&wait=0", &state_headers(p, TOKEN), b"").unwrap();
    assert_eq!(json_body(&s)["muted"], true);
    assert_eq!(ctl(br#"{"action":"explode"}"#, "application/json").status, 400);
    assert_eq!(ctl(b"{nope", "application/json").status, 400);
    assert_eq!(ctl(br#"{"action":"mute"}"#, "text/plain").status, 415);
    let big = format!(r#"{{"action":"mute","pad":"{}"}}"#, "x".repeat(20_000));
    assert_eq!(ctl(big.as_bytes(), "application/json").status, 413);
    assert_eq!(request(p, "PUT", "/state", &state_headers(p, TOKEN), b"").unwrap().status, 405);
}

/// `/control {"action":"reload-token"}` (plan D3, rotation): the brain reads its token file
/// again, and the old token keeps working for a minute; any other token is refused.
#[test]
fn reload_token_keeps_the_old_token_for_a_minute() {
    let home = std::env::temp_dir().join(format!("bw-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let new_token = "rotatedToken_ABCDEFGHIJKLMNOP";
    bellwether_core::token::write(&home, TOKEN).unwrap();
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.token_file = Some(bellwether_core::token::file(&home));
    let b = start_with(opts);
    let p = b.port;
    let post = |token: &str| request(p, "POST", "/hook", &with(hook_headers(p), "X-Bellwether-Token", Some(token)), br#"{"hook_event_name":"Stop"}"#).unwrap().status;
    assert_eq!(post(new_token), 403, "not before the reload");
    bellwether_core::token::write(&home, new_token).unwrap();
    let r = request(p, "POST", "/control", &with(state_headers(p, TOKEN), "Content-Type", Some("application/json")), br#"{"action":"reload-token"}"#).unwrap();
    assert_eq!((r.status, json_body(&r)), (200, serde_json::json!({ "ok": true })));
    assert_eq!(post(new_token), 204, "the new token");
    assert_eq!(post(TOKEN), 204, "the old one, within the minute");
    assert_eq!(post("someOtherToken_0123456789"), 403);
    let s = request(p, "GET", "/state?after=-1&wait=0", &state_headers(p, new_token), b"").unwrap();
    assert_eq!(s.status, 200);
    let _ = std::fs::remove_dir_all(&home);

    // A brain whose token came from the environment can't reload it.
    let env_brain = start(bellwether_brain::Mode::Normal);
    let q = env_brain.port;
    let r = request(q, "POST", "/control", &with(state_headers(q, TOKEN), "Content-Type", Some("application/json")), br#"{"action":"reload-token"}"#).unwrap();
    assert_eq!(r.status, 400);
    assert!(json_body(&r)["error"].as_str().unwrap().contains("BELLWETHER_HOOK_TOKEN"));
}

/// `/control {"action":"stop"}` (Windows plan WD6, W2): the tokened stop the app uses on
/// Windows. Refused without the token; with it, the brain answers 200, saves state.json and
/// its serve returns, as on SIGTERM.
#[test]
fn stop_answers_then_saves_and_returns() {
    let home = std::env::temp_dir().join(format!("bw-stop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.home = Some(home.clone());
    opts.timings.persist = Duration::from_secs(3600);
    let bound = bellwether_brain::bind(0).unwrap();
    let p = bound.port;
    let (done_tx, done) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let r = bellwether_brain::serve(bound, opts, None);
        let _ = done_tx.send(r.is_ok());
    });
    mute(p, "mute");
    let ctl = |token: &str| request(p, "POST", "/control", &with(state_headers(p, token), "Content-Type", Some("application/json")), br#"{"action":"stop"}"#).unwrap();
    assert_eq!(ctl("wrongTokenNotReal_0123456789").status, 403, "the token is required");
    assert_eq!(health(p)["ok"], true, "still serving after a refused stop");
    let r = ctl(TOKEN);
    assert_eq!((r.status, json_body(&r)), (200, serde_json::json!({ "ok": true })));
    assert_eq!(done.recv_timeout(Duration::from_secs(5)), Ok(true), "serve returned after stop");
    thread.join().unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).expect("state.json saved on the way out")).unwrap();
    assert_eq!(saved["muted"], true);
    assert!(TcpStream::connect(("127.0.0.1", p)).is_err(), "the port is free again");
    let _ = std::fs::remove_dir_all(&home);
}

/// The connection cap (milestone 3, from the M1 review): past `max_connections` the brain
/// closes the connection idle longest, so with 300 idle sockets open a hook on a new
/// connection still gets its 204 at once.
#[test]
fn the_cap_closes_idle_connections_first() {
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.max_connections = 50;
    let b = start_with(opts);
    let mut idle = Vec::new();
    for _ in 0..300 {
        let s = TcpStream::connect(("127.0.0.1", b.port)).unwrap();
        idle.push(s);
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(200));
    let t0 = std::time::Instant::now();
    assert_empty(&request(b.port, "POST", "/hook", &hook_headers(b.port), br#"{"hook_event_name":"Stop"}"#).unwrap(), 204);
    assert!(t0.elapsed() < Duration::from_millis(500), "{:?}", t0.elapsed());
    // The oldest idle sockets were closed by the brain: a read sees the end.
    let mut closed = 0;
    for s in &mut idle {
        s.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        let mut buf = [0u8; 1];
        closed += matches!(s.read(&mut buf), Ok(0)) as usize;
    }
    assert!(closed >= 200, "only {closed} of 300 idle connections were closed with a cap of 50");
}

/// A connection with nothing in flight that sends nothing is closed after the idle timeout.
#[test]
fn idle_connections_time_out() {
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.idle_timeout = Duration::from_millis(300);
    let b = start_with(opts);
    let mut s = TcpStream::connect(("127.0.0.1", b.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let t0 = std::time::Instant::now();
    let mut buf = [0u8; 16];
    assert_eq!(s.read(&mut buf).unwrap(), 0, "closed by the brain");
    assert!(t0.elapsed() >= Duration::from_millis(250) && t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
}

#[test]
fn health_carries_no_content() {
    let b = start(bellwether_brain::Mode::Normal);
    let h = health(b.port);
    let mut keys: Vec<&str> = h.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["counters", "lastEventAt", "mode", "name", "ok", "pid", "port", "version"]);
    assert_eq!(h["port"], b.port);
    assert_eq!(h["name"], "bellwether");
    assert_eq!(h["mode"], "normal");
    assert_eq!(h["counters"]["dropped"], 0);
    assert_eq!(h["counters"]["logDropped"], 0);
    assert!(h["lastEventAt"].is_null());
    request(b.port, "POST", "/hook", &hook_headers(b.port), br#"{"hook_event_name":"BellwetherDoctorPing"}"#).unwrap();
    settle();
    assert!(health(b.port)["lastEventAt"].is_null(), "doctor's ping is not an event");
    request(b.port, "POST", "/hook", &hook_headers(b.port), br#"{"hook_event_name":"Stop"}"#).unwrap();
    settle();
    assert!(health(b.port)["lastEventAt"].is_u64());
}

#[test]
fn slow_answers_late_and_hang_never_answers() {
    let b = start(bellwether_brain::Mode::Slow(150));
    let t0 = std::time::Instant::now();
    assert_empty(&request(b.port, "POST", "/hook", &hook_headers(b.port), b"{}").unwrap(), 204);
    assert!(t0.elapsed() >= Duration::from_millis(140));
    assert_eq!(health(b.port)["mode"], "slow=150");

    let h = start(bellwether_brain::Mode::Hang);
    let mut s = TcpStream::connect(("127.0.0.1", h.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    s.write_all(format!("POST /hook HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nX-Bellwether-Token: {TOKEN}\r\nContent-Length: 2\r\n\r\n{{}}", h.port).as_bytes()).unwrap();
    let mut buf = [0u8; 64];
    let e = s.read(&mut buf).unwrap_err();
    assert!(matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "hung: {e:?}");
}

#[test]
fn a_held_port_is_exit_2() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let e = bellwether_brain::bind(held.local_addr().unwrap().port()).err().unwrap();
    assert_eq!(e.code, 2);
    assert!(e.message.contains("already in use"));
}

/// test/DIVERGENCES.md, "a /hook body that stalls": the Node brain's server answers 408
/// when a body hasn't arrived within its request timeout. A HookAnswer is only 204 or
/// 403, so the Rust brain answers an empty 204 and counts nothing.
#[test]
fn a_stalled_body_gets_an_uncounted_empty_204() {
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.request_timeout = Duration::from_millis(300);
    let b = start_with(opts);
    let before = health(b.port)["counters"].clone();
    let mut s = TcpStream::connect(("127.0.0.1", b.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    // Content-Length promises 100 bytes; 10 arrive, then nothing.
    s.write_all(format!("POST /hook HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nX-Bellwether-Token: {TOKEN}\r\nContent-Length: 100\r\n\r\n{{\"hook_eve", b.port).as_bytes()).unwrap();
    let t0 = std::time::Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 512];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut buf).expect("an answer before the read timeout");
        assert!(n > 0, "closed without an answer");
        raw.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    assert!(text.starts_with("HTTP/1.1 204 "), "{text}");
    assert!(!text.to_ascii_lowercase().contains("content-type"), "{text}");
    assert!(t0.elapsed() >= Duration::from_millis(250), "answered only after the timeout");
    drop(s);
    settle();
    let after = health(b.port)["counters"].clone();
    for k in ["accepted", "malformed", "oversize", "unknownEvent", "consumerErrors", "dropped"] {
        assert_eq!(after[k], before[k], "{k} must not count a stalled body");
    }
    assert_eq!(after["rejected"], before["rejected"]);
}

/// test/DIVERGENCES.md, "/health key order": serde_json writes object keys sorted; the
/// Node brain writes them in source order. Consumers compare key sets, never order.
#[test]
fn health_keys_are_written_sorted() {
    let b = start(bellwether_brain::Mode::Normal);
    let r = request(b.port, "GET", "/health", &[("Host", format!("127.0.0.1:{}", b.port))], b"").unwrap();
    let text = String::from_utf8(r.body).unwrap();
    let top = ["\"counters\"", "\"lastEventAt\"", "\"mode\"", "\"name\"", "\"ok\"", "\"pid\"", "\"port\"", "\"version\""];
    let at: Vec<usize> = top.iter().map(|k| text.find(k).unwrap_or_else(|| panic!("{k} missing: {text}"))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "top-level keys not in sorted order: {text}");
    let counters = ["\"accepted\"", "\"consumerErrors\"", "\"dropped\"", "\"lastRejectedAt\"", "\"logDropped\"", "\"malformed\"", "\"oversize\"", "\"rejected\"", "\"unknownEvent\""];
    let at: Vec<usize> = counters.iter().map(|k| text.find(k).unwrap_or_else(|| panic!("{k} missing: {text}"))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "counter keys not in sorted order: {text}");
}

/// A brain whose state.json writes are held by the test (M13): `started` counts writes
/// begun; each write waits for `go` (or 20 s) and then saves for real.
fn held_writer() -> (bellwether_brain::saver::Writer, std::sync::mpsc::Sender<()>, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = std::sync::Mutex::new(go_rx);
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let s = started.clone();
    let writer: bellwether_brain::saver::Writer = std::sync::Arc::new(move |file: &std::path::Path, v: &bellwether_core::json::Js| {
        s.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
        bellwether_core::persist::save_state(file, v)
    });
    (writer, go_tx, started)
}

fn mute(port: u16, action: &str) {
    let body = format!(r#"{{"action":"{action}"}}"#);
    let r = request(port, "POST", "/control", &with(state_headers(port, TOKEN), "Content-Type", Some("application/json")), body.as_bytes()).unwrap();
    assert_eq!(r.status, 200);
}

/// Deviation 130's follow-up (M13): state.json is written off the serving thread, so with
/// the disk stalled mid-write, hooks and /state are still answered at once; the stalled
/// write's newer state waits (only the latest), and a clean stop writes it.
#[test]
fn a_stalled_state_json_write_never_holds_up_a_hook() {
    let home = std::env::temp_dir().join(format!("bw-stalled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let (writer, go, started) = held_writer();
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.home = Some(home.clone());
    opts.timings.persist = Duration::from_millis(50);
    opts.state_writer = writer;
    let mut b = start_with(opts);
    let p = b.port;
    mute(p, "mute");
    let t0 = std::time::Instant::now();
    while started.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(t0.elapsed() < Duration::from_secs(5), "the brain never started a save");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The write is now stuck on the "disk". Every hook still gets its 204 straight away.
    let mut slowest = Duration::ZERO;
    for i in 0..60 {
        let body = format!(r#"{{"hook_event_name":"SessionStart","session_id":"stall-{i}","cwd":"/tmp/p{i}"}}"#);
        let t = std::time::Instant::now();
        assert_empty(&request(p, "POST", "/hook", &hook_headers(p), body.as_bytes()).expect("an answer"), 204);
        slowest = slowest.max(t.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    let t = std::time::Instant::now();
    assert_eq!(request(p, "GET", "/state?after=-1&wait=0", &state_headers(p, TOKEN), b"").unwrap().status, 200);
    slowest = slowest.max(t.elapsed());
    assert!(slowest < Duration::from_millis(500), "a hook waited {slowest:?} for a stalled write");
    assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1, "the newer states wait behind the stalled write, not alongside it");
    mute(p, "unmute");
    std::thread::sleep(Duration::from_millis(150));
    // The disk comes back; the stop writes the latest state (unmuted) before serve returns.
    drop(go);
    b.stop.take().unwrap().send(()).unwrap();
    b.thread.take().unwrap().join().unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["muted"], false, "the latest state, not the stalled one");
    assert!(started.load(std::sync::atomic::Ordering::SeqCst) <= 3, "coalesced: {} writes", started.load(std::sync::atomic::Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&home);
}

/// A clean stop saves the last state and waits for the write (M13): with the save timer
/// never due, the mute sent just before the stop is in state.json once serve returns, even
/// though the write takes 300 ms.
#[test]
fn a_clean_stop_writes_the_last_state_before_serve_returns() {
    let home = std::env::temp_dir().join(format!("bw-flush-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut opts = bellwether_brain::Options::new(TOKEN.into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.home = Some(home.clone());
    opts.timings.persist = Duration::from_secs(3600);
    opts.state_writer = std::sync::Arc::new(|file: &std::path::Path, v: &bellwether_core::json::Js| {
        std::thread::sleep(Duration::from_millis(300));
        bellwether_core::persist::save_state(file, v)
    });
    let mut b = start_with(opts);
    mute(b.port, "mute");
    assert!(!home.join("state.json").exists(), "nothing saved before the stop");
    let t0 = std::time::Instant::now();
    b.stop.take().unwrap().send(()).unwrap();
    b.thread.take().unwrap().join().unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(250), "serve returned before the write ({:?})", t0.elapsed());
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["muted"], true);
    let _ = std::fs::remove_dir_all(&home);
}
