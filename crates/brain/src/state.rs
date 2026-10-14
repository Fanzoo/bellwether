//! GET /state and POST /control, for the shell and `bellwether status` (a port of
//! brain/state-routes.mjs). The same guards as /hook, before anything else: no Origin
//! header, Host 127.0.0.1:<port> or localhost:<port>, and the X-Bellwether-Token. No CORS
//! headers ever, so a browser can't read them even by accident.
//!
//!   GET  /state?after=<rev>[&wait=<ms>]  the snapshot as soon as rev > after, or the
//!        current one after `wait` (default and maximum 25 s; 0 answers at once)
//!   POST /control  {action: "mute" | "unmute" | "dismiss" | "placed" | "reload-token", ...}
//!
//! `reload-token` is new with the Rust brain (distribution plan D3, rotation): the brain
//! reads the token file again and accepts the old token for one more minute.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bellwether_core::json::{self, Js, Obj};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use tokio::sync::oneshot;

use crate::actor::Msg;
use crate::conns::Conn;
use crate::{Body, Brain};

const LONG_POLL_MS: f64 = 25_000.0;
const MAX_CONTROL_BYTES: usize = 16 * 1024;

fn empty(status: StatusCode) -> Response<Body> {
    Response::builder().status(status).header("Content-Length", "0").body(Empty::new().boxed()).unwrap()
}

fn json_response(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len().to_string())
        .header("Cache-Control", "no-store")
        .body(Full::new(Bytes::from(body)).boxed())
        .unwrap()
}

fn control_answer(status: StatusCode, ok: bool, error: Option<&str>) -> Response<Body> {
    let mut o = Obj::new().with("ok", ok);
    if let Some(e) = error {
        o.set("error", e);
    }
    json_response(status, json::stringify(&Js::Obj(o)))
}

/// `URLSearchParams.get(name)`: the first value, `+` as space, percent-decoded (a bad
/// escape kept as it is), invalid UTF-8 as U+FFFD.
pub fn query_get(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if decode(k) == name {
            return Some(decode(v));
        }
    }
    None
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if b.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit) => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `Number.parseInt(s, 10)`: leading whitespace, a sign, then digits up to the first
/// non-digit; NaN when there are none.
pub fn parse_int(s: &str) -> f64 {
    let t = s.trim_start_matches(bellwether_core::jsstr::is_js_space);
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    let n: f64 = digits.parse().unwrap_or(f64::INFINITY);
    if neg {
        -n
    } else {
        n
    }
}

/// Routes /state and /control; None for any other path.
pub(crate) async fn route(brain: &Arc<Brain>, conn: &Arc<Conn>, req: Request<Incoming>, path: &str) -> Option<Response<Body>> {
    if path != "/state" && path != "/control" {
        return None;
    }
    if let Some(reason) = brain.rejection(req.headers(), false) {
        brain.log.log(&format!("rejected {path}: {}", reason.name()));
        return Some(empty(StatusCode::FORBIDDEN));
    }
    if path == "/state" && req.method() == Method::GET {
        let query = req.uri().query().unwrap_or("");
        let after = parse_int(&query_get(query, "after").unwrap_or_else(|| "-1".into()));
        let after = if after.is_finite() { after } else { -1.0 };
        let w = parse_int(&query_get(query, "wait").unwrap_or_else(|| LONG_POLL_MS.to_string()));
        let wait = LONG_POLL_MS.min(if w.is_nan() || w == 0.0 { 0.0 } else { w.max(0.0) });
        let mut rx = brain.snapshots.clone();
        // As publisher.waitFor: answer now when rev > after (or wait is 0), else at the
        // next publish, whatever its rev, or after `wait`.
        if !((rx.borrow_and_update().rev as f64) > after || wait <= 0.0) {
            conn.waiting.store(true, Ordering::Relaxed);
            let _ = tokio::time::timeout(Duration::from_millis(wait as u64), rx.changed()).await;
            conn.waiting.store(false, Ordering::Relaxed);
        }
        let body = rx.borrow().body.clone();
        return Some(json_response(StatusCode::OK, (*body).clone()));
    }
    if path == "/control" && req.method() == Method::POST {
        let media = req.headers().get("content-type").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
        let media = media.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let mut body = req.into_body();
        let mut buf = Vec::new();
        let mut size = 0usize;
        while let Some(frame) = body.frame().await {
            let Ok(frame) = frame else { break };
            if let Ok(data) = frame.into_data() {
                size += data.len();
                if size <= MAX_CONTROL_BYTES {
                    buf.extend_from_slice(&data);
                }
            }
        }
        if media != "application/json" {
            return Some(control_answer(StatusCode::UNSUPPORTED_MEDIA_TYPE, false, Some("Content-Type must be application/json")));
        }
        if size > MAX_CONTROL_BYTES {
            return Some(control_answer(StatusCode::PAYLOAD_TOO_LARGE, false, Some("body too large")));
        }
        let Ok(msg) = json::parse(&String::from_utf8_lossy(&buf)) else {
            return Some(control_answer(StatusCode::BAD_REQUEST, false, Some("not JSON")));
        };
        if msg.get("action").as_str() == Some("reload-token") {
            let (ok, error) = brain.reload_token();
            return Some(control_answer(if ok { StatusCode::OK } else { StatusCode::BAD_REQUEST }, ok, error.as_deref()));
        }
        let (reply, answer) = oneshot::channel();
        if brain.inbox.send(Msg::Control { msg, reply }).await.is_err() {
            return Some(control_answer(StatusCode::SERVICE_UNAVAILABLE, false, Some("the state machine has stopped")));
        }
        let r = answer.await.unwrap_or(bellwether_core::flock::ControlResult { ok: false, error: Some("no answer".into()) });
        return Some(json_response(if r.ok { StatusCode::OK } else { StatusCode::BAD_REQUEST }, json::stringify(&r.to_js())));
    }
    Some(empty(StatusCode::METHOD_NOT_ALLOWED))
}
