//! A small blocking HTTP client for the brain's own API (`status`, `mute`, `replay`), a
//! port of brain/client.mjs. Every request uses its own connection (`Connection: close`,
//! as Node's `agent: false`). A GET whose connection is reset is retried once on a fresh
//! one; a POST is not, because /hook events must not be delivered twice.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::BIND_HOST;

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A failed request, by Node's error code where there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Refused,
    Code(String),
}

impl Failure {
    pub fn code(&self) -> String {
        match self {
            Failure::Refused => "ECONNREFUSED".into(),
            Failure::Code(c) => c.clone(),
        }
    }
}

fn code_of(e: &std::io::Error) -> Failure {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => Failure::Refused,
        ConnectionReset => Failure::Code("ECONNRESET".into()),
        BrokenPipe => Failure::Code("EPIPE".into()),
        TimedOut | WouldBlock => Failure::Code("ETIMEDOUT".into()),
        UnexpectedEof => Failure::Code("ECONNRESET".into()),
        _ => Failure::Code(e.to_string()),
    }
}

/// One request; `headers` are added to Host, Connection and Content-Length.
pub fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>, timeout: Duration) -> Result<Reply, Failure> {
    let r = once(port, method, path, headers, body, timeout);
    match &r {
        Err(Failure::Code(c)) if method == "GET" && (c == "ECONNRESET" || c == "EPIPE") => once(port, method, path, headers, body, timeout),
        _ => r,
    }
}

fn once(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>, timeout: Duration) -> Result<Reply, Failure> {
    let addr = SocketAddr::from((BIND_HOST, port));
    let mut s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10))).map_err(|e| code_of(&e))?;
    let _ = s.set_nodelay(true);
    s.set_read_timeout(Some(timeout)).map_err(|e| code_of(&e))?;
    s.set_write_timeout(Some(timeout)).map_err(|e| code_of(&e))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {BIND_HOST}:{port}\r\nConnection: close\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(b) = body {
        out.extend_from_slice(b);
    }
    s.write_all(&out).map_err(|e| code_of(&e))?;
    let mut raw = Vec::new();
    if let Err(e) = s.read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(code_of(&e));
        }
    }
    parse(&raw).ok_or_else(|| Failure::Code(if raw.is_empty() { "ECONNRESET".into() } else { "a malformed HTTP answer".into() }))
}

fn parse(raw: &[u8]) -> Option<Reply> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let mut chunked = false;
    let mut length = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse::<usize>().ok();
            }
        }
    }
    let rest = &raw[split + 4..];
    let body = if chunked {
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let eol = rest[at..].windows(2).position(|w| w == b"\r\n")?;
            let size = usize::from_str_radix(String::from_utf8_lossy(&rest[at..at + eol]).split(';').next()?.trim(), 16).ok()?;
            at += eol + 2;
            if size == 0 {
                break;
            }
            out.extend_from_slice(rest.get(at..at + size)?);
            at += size + 2;
        }
        out
    } else {
        match length {
            Some(n) => rest.get(..n).unwrap_or(rest).to_vec(),
            None => rest.to_vec(),
        }
    };
    Some(Reply { status, body })
}
