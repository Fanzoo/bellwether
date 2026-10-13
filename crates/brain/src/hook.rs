//! POST /hook: the only thing Claude Code ever talks to.
//!
//! A 2xx response with a JSON body is parsed by Claude Code as hook output, and on
//! PermissionRequest that output can approve or deny on the user's behalf. So the
//! handler returns a [`HookAnswer`], which has exactly two values, and the only way to
//! turn one into a response gives it an empty body type: writing a body here does not
//! compile.
//!
//! Guards, in order (each failure is an empty 403):
//!   1. an Origin header (a browser; DNS rebinding)
//!   2. Host not 127.0.0.1:<port> or localhost:<port>
//!   3. Content-Type not application/json (forces a CORS preflight, never answered)
//!   4. X-Bellwether-Token wrong or missing, compared in constant time over sha256
//!
//! Everything past the guards fails open and silent: a malformed or oversize body or an
//! unknown event still gets 204. The answer goes out once the body has arrived and
//! before it is parsed. X-Pasture-* headers are not accepted (maintainer decision 14).

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{now_ms, Brain, Mode};

pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The event name `bellwether doctor` posts to check the token: counted, never shown.
pub const DOCTOR_PING: &str = "BellwetherDoctorPing";

/// The 16 events `hooks install` subscribes to (brain/hooks.mjs EVENTS).
pub const EVENTS: [&str; 16] = [
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "Elicitation",
    "ElicitationResult",
];

/// Everything /hook can answer. Nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAnswer {
    /// 204, no body.
    NoContent,
    /// 403, `Content-Length: 0`, no body.
    Forbidden,
}

impl HookAnswer {
    /// The body type is `Empty`: there is no value it could carry.
    pub fn into_response(self) -> Response<Empty<Bytes>> {
        let builder = match self {
            HookAnswer::NoContent => Response::builder().status(StatusCode::NO_CONTENT),
            HookAnswer::Forbidden => Response::builder().status(StatusCode::FORBIDDEN).header("Content-Length", "0"),
        };
        builder.body(Empty::new()).expect("a status and a static header")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Origin,
    Host,
    ContentType,
    Token,
}

impl Rejection {
    fn name(self) -> &'static str {
        match self {
            Rejection::Origin => "origin",
            Rejection::Host => "host",
            Rejection::ContentType => "contentType",
            Rejection::Token => "token",
        }
    }
}

pub fn digest(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

/// Equal-length digests, compared without an early exit: the time leaks nothing.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The four guards, in order. `None` means the request may proceed.
pub fn rejection(headers: &hyper::HeaderMap, port: u16, expected: &[u8; 32]) -> Option<Rejection> {
    if headers.contains_key("origin") {
        return Some(Rejection::Origin);
    }
    let host = headers.get("host").map(|v| String::from_utf8_lossy(v.as_bytes()).to_ascii_lowercase()).unwrap_or_default();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Some(Rejection::Host);
    }
    let media = headers.get("content-type").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    if media.split(';').next().unwrap_or("").trim().to_ascii_lowercase() != "application/json" {
        return Some(Rejection::ContentType);
    }
    // Exactly one token header. Node joins repeated headers into one value ("a, b"),
    // which never matches, so two tokens are refused here too, even if one is right.
    let mut tokens = headers.get_all("x-bellwether-token").iter();
    match (tokens.next(), tokens.next()) {
        (Some(given), None) if same(&digest(given.as_bytes()), expected) => None,
        _ => Some(Rejection::Token),
    }
}

pub(crate) async fn handle(brain: Arc<Brain>, req: Request<Incoming>) -> HookAnswer {
    let c = &brain.counters;
    if let Some(reason) = rejection(req.headers(), brain.port, &brain.expected) {
        match reason {
            Rejection::Origin => &c.rejected_origin,
            Rejection::Host => &c.rejected_host,
            Rejection::ContentType => &c.rejected_content_type,
            Rejection::Token => &c.rejected_token,
        }
        .fetch_add(1, Ordering::Relaxed);
        *c.last_rejected_at.lock().unwrap() = Some(now_ms());
        brain.log.log(&format!("rejected /hook: {}", reason.name()));
        return HookAnswer::Forbidden;
    }

    let received_at = now_ms();
    let body = if brain.mode == Mode::Hang {
        Some(read_body(req.into_body()).await)
    } else {
        tokio::time::timeout(brain.request_timeout, read_body(req.into_body())).await.ok()
    };
    // A body that hasn't arrived in time: the Node brain's server answers 408 here, but
    // a HookAnswer is 204 or 403, so this is an uncounted, empty 204 (test/DIVERGENCES.md).
    let Some(body) = body else {
        brain.log.log(&format!("dropped /hook: the body didn't arrive within {} s", brain.request_timeout.as_secs_f64()));
        return HookAnswer::NoContent;
    };
    match brain.mode {
        // Debug only: the connection stays open until Claude Code gives up.
        Mode::Hang => std::future::pending::<()>().await,
        Mode::Slow(ms) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
        Mode::Normal => {}
    }
    c.accepted.fetch_add(1, Ordering::Relaxed);
    // Parsed after the answer: on this single-threaded runtime the spawned task runs
    // once the connection task has written the 204.
    let b = brain.clone();
    tokio::task::spawn(async move { consume(&b, body, received_at) });
    HookAnswer::NoContent
}

/// The body, or `None` when it was larger than MAX_BODY_BYTES (it is drained either
/// way; an oversize body is never kept).
async fn read_body(mut body: Incoming) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut oversize = false;
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else { break };
        if let Ok(data) = frame.into_data() {
            if oversize {
                continue;
            }
            if buf.len() + data.len() > MAX_BODY_BYTES {
                oversize = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&data);
            }
        }
    }
    if oversize {
        None
    } else {
        Some(buf)
    }
}

fn consume(brain: &Brain, body: Option<Vec<u8>>, received_at: u64) {
    let c = &brain.counters;
    let Some(body) = body else {
        c.oversize.fetch_add(1, Ordering::Relaxed);
        brain.log.log(&format!("dropped /hook body over {MAX_BODY_BYTES} bytes"));
        return;
    };
    let payload: Option<serde_json::Value> = serde_json::from_str(&String::from_utf8_lossy(&body)).ok();
    let Some(payload) = payload.filter(|p| p.is_object()) else {
        c.malformed.fetch_add(1, Ordering::Relaxed);
        brain.log.log("dropped /hook body: not a JSON object");
        return;
    };
    let event = payload.get("hook_event_name").and_then(|v| v.as_str()).unwrap_or("");
    if !EVENTS.contains(&event) {
        c.unknown_event.fetch_add(1, Ordering::Relaxed);
    }
    if event == DOCTOR_PING {
        return;
    }
    *c.last_event_at.lock().unwrap() = Some(received_at);
    // Milestone 3: the event goes to the state machine here (a try_send; a full
    // channel counts `dropped` and never delays the answer).
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::HeaderMap;

    const TOKEN: &str = "testTokenNotReal_abcdefghijklmnop";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    fn good() -> Vec<(&'static str, &'static str)> {
        vec![("host", "127.0.0.1:4000"), ("content-type", "application/json"), ("x-bellwether-token", TOKEN)]
    }

    #[test]
    fn the_guards_in_order() {
        let want = digest(TOKEN.as_bytes());
        let check = |extra: &[(&'static str, &str)], drop: &[&str]| {
            let mut pairs: Vec<(&'static str, &str)> = good().into_iter().filter(|(k, _)| !drop.contains(k)).collect();
            for (k, v) in extra {
                pairs.retain(|(k2, _)| k2 != k);
                pairs.push((k, v));
            }
            rejection(&headers(&pairs), 4000, &want)
        };
        assert_eq!(check(&[], &[]), None);
        assert_eq!(check(&[("host", "localhost:4000"), ("content-type", "application/json; charset=utf-8")], &[]), None);
        assert_eq!(check(&[("host", "LOCALHOST:4000")], &[]), None, "Host is compared without case, like Node");
        assert_eq!(check(&[("origin", "null")], &[]), Some(Rejection::Origin));
        assert_eq!(check(&[("origin", "https://evil.example"), ("host", "evil.example")], &[]), Some(Rejection::Origin), "Origin is checked first");
        assert_eq!(check(&[("host", "evil.example")], &[]), Some(Rejection::Host));
        assert_eq!(check(&[("host", "127.0.0.1:1")], &[]), Some(Rejection::Host));
        assert_eq!(check(&[], &["host"]), Some(Rejection::Host));
        assert_eq!(check(&[("content-type", "text/plain")], &[]), Some(Rejection::ContentType));
        assert_eq!(check(&[], &["content-type"]), Some(Rejection::ContentType));
        assert_eq!(check(&[], &["x-bellwether-token"]), Some(Rejection::Token));
        assert_eq!(check(&[("x-bellwether-token", "wrong")], &[]), Some(Rejection::Token));
        assert_eq!(check(&[("x-bellwether-token", &TOKEN[..TOKEN.len() - 1])], &[]), Some(Rejection::Token));
        assert_eq!(check(&[("x-pasture-token", TOKEN)], &["x-bellwether-token"]), Some(Rejection::Token), "the pre-rename header is not accepted");
    }

    #[test]
    fn two_token_headers_are_refused_even_if_one_is_right() {
        let want = digest(TOKEN.as_bytes());
        for pair in [[TOKEN, "wrong"], ["wrong", TOKEN], [TOKEN, TOKEN]] {
            let mut h = headers(&[("host", "127.0.0.1:4000"), ("content-type", "application/json")]);
            for t in pair {
                h.append("x-bellwether-token", t.parse().unwrap());
            }
            assert_eq!(rejection(&h, 4000, &want), Some(Rejection::Token), "{pair:?}");
        }
        let joined = headers(&[("host", "127.0.0.1:4000"), ("content-type", "application/json"), ("x-bellwether-token", &format!("{TOKEN}, {TOKEN}"))]);
        assert_eq!(rejection(&joined, 4000, &want), Some(Rejection::Token), "a joined value");
    }

    #[test]
    fn answers_have_no_body() {
        use http_body_util::BodyExt;
        use hyper::body::Body;
        for (a, status) in [(HookAnswer::NoContent, 204), (HookAnswer::Forbidden, 403)] {
            let r = a.into_response();
            assert_eq!(r.status().as_u16(), status);
            assert_eq!(r.body().size_hint().exact(), Some(0));
            assert!(r.headers().get("content-type").is_none());
            let _ = r.into_body().boxed();
        }
        assert_eq!(HookAnswer::Forbidden.into_response().headers()["content-length"], "0");
    }
}
