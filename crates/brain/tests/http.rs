//! The brain over real HTTP: every /hook answer is status-only, the guards refuse with
//! an empty 403, and /health carries only names and numbers. Raw sockets, so every
//! header (Host, Origin) is under the test's control.

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
    s.write_all(head.as_bytes()).unwrap();
    let _ = s.write_all(body);
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
    let r = request(p, "GET", "/state", &hook_headers(p), b"").unwrap();
    assert_eq!(r.status, 404, "/state arrives in milestone 3");
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
    let counters = ["\"accepted\"", "\"consumerErrors\"", "\"dropped\"", "\"lastRejectedAt\"", "\"malformed\"", "\"oversize\"", "\"rejected\"", "\"unknownEvent\""];
    let at: Vec<usize> = counters.iter().map(|k| text.find(k).unwrap_or_else(|| panic!("{k} missing: {text}"))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "counter keys not in sorted order: {text}");
}
