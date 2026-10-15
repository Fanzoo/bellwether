//! The hook token never appears in anything a person (or a model reading the output) sees
//! (a port of maskToken in brain/util.mjs): the current token wherever it occurs, and any
//! value written under an X-Bellwether-Token header, become their first 4 characters and
//! "…". Only files bellwether writes on purpose keep the value.

use std::sync::{Mutex, OnceLock};

use regex::Regex;

use crate::jsstr;

fn header_value() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let ws = format!("[{}]", jsstr::JS_SPACE_CLASS);
        Regex::new(&format!(r#"["']?[xX]-(?:[bB][eE][lL][lL][wW][eE][tT][hH][eE][rR]|[pP][aA][sS][tT][uU][rR][eE])-[tT][oO][kK][eE][nN]["']?{ws}*[:=]{ws}*["']?([^"'{},}}\\]{{5,}})"#, jsstr::JS_SPACE_CLASS)).unwrap()
    })
}

/// Token values seen under a token header in this process's output so far, in order.
static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `maskToken(text, token, seen)` over unit strings; `seen` keeps its insertion order.
pub fn mask_with(text: &str, token: Option<&str>, seen: &mut Vec<String>) -> String {
    for c in header_value().captures_iter(text) {
        let v = c[1].to_string();
        if !v.ends_with('…') && !seen.contains(&v) {
            seen.push(v);
        }
    }
    if let Some(t) = token.filter(|t| jsstr::len(t) >= 8) {
        if !seen.iter().any(|s| s == t) {
            seen.push(t.to_string());
        }
    }
    let mut order: Vec<&String> = seen.iter().collect();
    order.sort_by_key(|v| std::cmp::Reverse(jsstr::len(v)));
    let mut out = text.to_string();
    for v in order {
        out = out.replace(v.as_str(), &format!("{}…", jsstr::prefix(v, 4)));
    }
    out
}

/// Masks real text with this process's memory of token values (the CLI's output).
pub fn mask(text: &str, token: Option<&str>) -> String {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    jsstr::to_string_lossy(&mask_with(&jsstr::from_str(text), token.map(jsstr::from_str).as_deref(), &mut seen))
}

/// Masks `text` knowing only `token` and the header values in `text` itself (no memory of
/// earlier output): one command's output, for a caller that isn't the CLI process (the
/// onboarding window's hook preview).
pub fn mask_alone(text: &str, token: Option<&str>) -> String {
    let mut seen = Vec::new();
    jsstr::to_string_lossy(&mask_with(&jsstr::from_str(text), token.map(jsstr::from_str).as_deref(), &mut seen))
}
