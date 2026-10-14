//! Redaction and truncation for the only free text the brain keeps: tool summaries (a
//! Bash command, say) and notification or elicitation messages. A port of
//! brain/redact.mjs, pinned to it by test/fixtures/redact-vectors.json and by the
//! differential (`npm run differential`, 10⁵ generated cases).
//!
//! Order matters, as in Node: normalise (NFKC, invisible characters out, whitespace
//! collapsed), then redact, then truncate, so a secret cut at the truncation point can't
//! leave a recognisable prefix.
//!
//! Every string here is a unit string (see [`crate::jsstr`]), so the patterns count
//! UTF-16 code units the way JS regular expressions without the `u` flag do. Where the
//! JS patterns and Rust's `regex` differ (distribution plan D2, "the three risky areas"):
//!
//! - **Look-around.** `regex` has none. A look-behind or look-ahead becomes a plain match
//!   plus an explicit check of the neighbouring characters; a match that fails the check
//!   is skipped and the search goes on where JS would find its next candidate. Where a
//!   look-ahead only refuses one character (`\s{1,20}(?!-)\S+`), it is folded into a class
//!   (`\s{1,20}[^\s-]\S*`), which matches exactly the same strings.
//! - **Classes.** JS `\d`, `\w` and `\b` are ASCII without the `u` flag; `\s` is JS's own
//!   whitespace set (U+FEFF in, U+0085 out). The patterns spell those out: `[0-9]`, an
//!   explicit space class ([`jsstr::JS_SPACE_CLASS`]) and `(?-u:\b)`. Case-insensitive
//!   words are spelled `[bB][eE]…`, so no Unicode case folding (the Kelvin sign, long s)
//!   can match where JS doesn't.
//! - **Replace order.** Each rule replaces left to right over the whole text, as a JS
//!   global replace does, and the rules run in the Node order.
//! - **NFKC.** unicode-normalization's tables (Unicode 16) against Node's ICU (76.1,
//!   Unicode 16); the full-width and compatibility vectors and the differential pin it.
//! - **Truncation** is by code point (`[...s]` in JS), a pair counting once.
//!
//! User `bubble.redactPatterns` are written in the JS dialect. [`compile_pattern`]
//! translates them, and refuses (a config error, exit 3, naming the pattern) what `regex`
//! can't do: look-around and backreferences (test/DIVERGENCES.md).

use std::sync::OnceLock;

use regex::{Captures, Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;

use crate::json::quote;
use crate::jsstr::{self, is_js_space, CodePoint, JS_SPACE_CLASS as WS};

pub const REDACTED: &str = "•••";

// ------------------------------------------------------------------ normalising

fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{AD}')
}

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}')
}

/// NFKC over a unit string. A lone surrogate has no decomposition and composes with
/// nothing, so each stretch between lone surrogates is normalised on its own, as ICU
/// does it.
fn nfkc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    for cp in jsstr::code_points(s) {
        match cp {
            CodePoint::Char(c) => run.push(c),
            CodePoint::Lone(u) => {
                for c in run.nfkc() {
                    jsstr::push_real(&mut out, c);
                }
                run.clear();
                out.push(jsstr::unit_char(u));
            }
        }
    }
    for c in run.nfkc() {
        jsstr::push_real(&mut out, c);
    }
    out
}

/// Text as a bubble may show it: NFKC (so ＰＡＳＳＷＯＲＤ folds to PASSWORD before any
/// pattern looks at it), invisible characters removed, control characters and runs of
/// whitespace collapsed to single spaces, trimmed.
pub fn normalise_text(text: &str) -> String {
    let folded = nfkc(text);
    let mut out = String::with_capacity(folded.len());
    let mut in_space = false;
    // INVISIBLE removed; a run of CONTROL becomes one space; then every run of
    // whitespace (the spaces just made included) becomes one space.
    for c in folded.chars().filter(|c| !is_invisible(*c)) {
        if is_control(c) || is_js_space(c) {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    jsstr::trim(&out).to_string()
}

/// Truncates to at most `max` code points, ending with an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    let cps = jsstr::code_points(s);
    if cps.len() <= max {
        return s.to_string();
    }
    let mut out = jsstr::from_code_points(&cps[..max.saturating_sub(1)]);
    out.push('…');
    out
}

/// A sheep's label (a folder name): cleaned like bubble text, no redaction, at most `max`.
pub fn clean_label(text: &str, max: usize) -> Option<String> {
    let s = truncate(&normalise_text(text), max);
    (!s.is_empty()).then_some(s)
}

// ------------------------------------------------------------------ secret names

const EXACT_WORDS: [&str; 6] = ["key", "pass", "sig", "auth", "pwd", "pin"];
const SUFFIX_WORDS: [&str; 14] = ["token", "secret", "password", "passwd", "passphrase", "apikey", "credential", "credentials", "signature", "privatekey", "secretkey", "accesskey", "sessionkey", "authkey"];

/// Whether a name is secret: one of its components (split on `_ - . / :`, whitespace and
/// camelCase boundaries) is one of the words above, or ends in one of the longer ones.
pub fn is_secret_name(name: &str) -> bool {
    if !could_be_secret(name) {
        return false;
    }
    // name.replace(/([a-z0-9])([A-Z])/g, '$1 $2'), left to right, without overlap.
    let chars: Vec<char> = name.chars().collect();
    let mut spaced = String::with_capacity(name.len() + 4);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c.is_ascii_lowercase() || c.is_ascii_digit()) && i + 1 < chars.len() && chars[i + 1].is_ascii_uppercase() {
            spaced.push(c);
            spaced.push(' ');
            spaced.push(chars[i + 1]);
            i += 2;
        } else {
            spaced.push(c);
            i += 1;
        }
    }
    spaced
        .split(|c: char| is_js_space(c) || matches!(c, '_' | '.' | '-' | '/' | ':'))
        .filter(|p| !p.is_empty())
        .map(|p| p.to_lowercase())
        .any(|p| EXACT_WORDS.contains(&p.as_str()) || SUFFIX_WORDS.iter().any(|w| p == *w || p.ends_with(w)))
}

/// A necessary condition for is_secret_name, without allocating: every secret word
/// contains one of these, and every component is a stretch of the name. The scans call it
/// for every candidate name in a 5 MB text.
fn could_be_secret(name: &str) -> bool {
    const STEMS: [&[u8]; 9] = [b"key", b"pass", b"sig", b"auth", b"pwd", b"pin", b"token", b"secret", b"credential"];
    let b = name.as_bytes();
    b.len() >= 3 && STEMS.iter().any(|w| b.windows(w.len()).any(|x| x.eq_ignore_ascii_case(w)))
}

/// `\\?["']?\s{0,20}[=:]` right at `at`: the separator a name=value needs, checked by
/// hand so a secret-looking word with no value after it costs no regex run.
fn separator_follows(text: &str, at: usize) -> bool {
    let mut it = text[at..].chars().peekable();
    if it.peek() == Some(&'\\') {
        it.next();
    }
    if matches!(it.peek(), Some('"' | '\'')) {
        it.next();
    }
    for _ in 0..=20 {
        match it.next() {
            Some('=' | ':') => return true,
            Some(c) if is_js_space(c) => continue,
            _ => return false,
        }
    }
    false
}

/// Values too plain to be a secret: short numbers, booleans, empty. Left readable.
fn is_plain_value(v: &str) -> bool {
    // v.replace(/^\\?["']|\\?["']$/g, '')
    let quote = |c: char| c == '"' || c == '\'';
    let mut start = 0;
    if v.starts_with('\\') && v[1..].starts_with(quote) {
        start = 2;
    } else if v.starts_with(quote) {
        start = 1;
    }
    let mut end = v.len();
    let rest = &v[start..];
    if rest.len() >= 2 && rest.ends_with(quote) && rest[..rest.len() - 1].ends_with('\\') {
        end -= 2;
    } else if !rest.is_empty() && rest.ends_with(quote) {
        end -= 1;
    }
    let bare = &v[start..end.max(start)];
    if bare.is_empty() || bare == REDACTED || bare == "-" {
        return true;
    }
    if (1..=6).contains(&bare.len()) && bare.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    ["true", "false", "yes", "no", "on", "off", "null", "none", "nil"].iter().any(|w| bare.eq_ignore_ascii_case(w))
}

// ------------------------------------------------------------------ the built-in rules

/// `[bB][eE][aA][rR]…`: an ASCII word, either case, and nothing else.
fn ci(word: &str) -> String {
    word.chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else if c.is_ascii_alphanumeric() {
                c.to_string()
            } else {
                format!("\\x{{{:X}}}", c as u32)
            }
        })
        .collect()
}

fn build(pattern: &str) -> Regex {
    RegexBuilder::new(pattern).size_limit(256 << 20).dfa_size_limit(64 << 20).build().unwrap_or_else(|e| panic!("built-in pattern {pattern}: {e}"))
}

/// The JS VALUE: an escaped-quoted string (\"…\" inside a shell string), a quoted string
/// (spaces included), or a run up to whitespace, a quote or a JSON/query/shell delimiter.
fn value_pattern() -> String {
    format!(r#"(?:\\"[^"\\\n]{{0,200}}\\"|"[^"\n]{{0,200}}"|'[^'\n]{{0,200}}'|[^{WS}"'`,;&|}}<>)\\]{{1,200}})"#)
}

/// VALUE_REST: what an unquoted value cut at its bound still runs on with.
fn is_value_char(c: char) -> bool {
    !(is_js_space(c) || matches!(c, '"' | '\'' | '`' | ',' | ';' | '&' | '|' | '}' | '<' | '>' | ')' | '\\'))
}

struct Builtins {
    authorization: Regex,
    bearer: Regex,
    cookie: Regex,
    key_value: Regex,
    flag_value: Regex,
    curl_user: Regex,
    dash_p_glued: Regex,
    login_p: Regex,
    mysql_p: Regex,
    openssl_pass: Regex,
    aws_set: Regex,
    vault_login: Regex,
    tokens: Vec<Regex>,
}

fn builtins() -> &'static Builtins {
    static B: OnceLock<Builtins> = OnceLock::new();
    B.get_or_init(|| {
        let s = format!("[{WS}]");
        let ns = format!("[^{WS}]");
        let b = r"(?-u:\b)";
        let schemes = ["basic", "bearer", "digest", "token", "negotiate", "ntlm", "aws4-hmac-sha256"].iter().map(|w| ci(w)).collect::<Vec<_>>().join("|");
        Builtins {
            // Authorization: Basic|Bearer|… <credentials> (or = for env vars); the scheme stays.
            authorization: build(&format!(r#"({}\\?["']?{s}{{0,20}}[:=]{s}{{0,20}}(?:(?:{schemes}){s}{{1,20}})?)[^{WS}"',;\\]+"#, ci("authorization"))),
            bearer: build(&format!(r"({b}(?:{}|{}){s}{{1,20}})[A-Za-z0-9._~+/=\-]{{6,}}", ci("bearer"), ci("basic"))),
            // Cookie: / Set-Cookie: — the whole value list.
            cookie: build(&format!(r#"({b}(?:{})?{}\\?["']?{s}{{0,20}}[:=]{s}{{0,20}})[^"'\n\\]{{1,2000}}"#, ci("set-"), ci("cookie"))),
            // name=value, name: value, "name":"value", \"name\":\"value\", ?name=value&
            // (look-behind (?<![A-Za-z0-9_.-]) checked by hand)
            key_value: build(&format!(r#"^([A-Za-z0-9_.\-]{{1,80}})(\\?["']?{s}{{0,20}}[=:]{s}{{0,20}})({})"#, value_pattern())),
            // --name value, --name=value, -name value (look-behind (?<=^|\s) by hand)
            flag_value: build(&format!(r"^(--?[A-Za-z][A-Za-z0-9_\-]{{0,60}})({s}{{1,20}}|=)({})", value_pattern())),
            // curl -u user:pass, -uuser:pass, --user=user:pass
            curl_user: build(&format!(r#"((?:^|{s})(?:-u|--user)(?:{s}{{1,20}}|=)?["']?[^{WS}:"']{{1,200}}:)[^{WS}"']+"#)),
            // -pSECRET glued (mysql): 6+ characters that aren't a port mapping
            // (look-ahead (?![\d:.,/-]+(?:\s|$)) by hand)
            dash_p_glued: build(&format!(r#"((?:^|{s})-p)[^{WS}"']{{6,}}"#)),
            // -p SECRET with a space, only for tools where -p means password;
            // \s{{1,20}}(?!-)[^\s"']+ is \s{{1,20}}[^\s"'-][^\s"']*
            login_p: build(&format!(r#"({b}(?:docker|podman|nerdctl|buildah|skopeo|helm|oras){s}+(?:registry{s}+)?login{b}[^|;&\n]{{0,300}}?{s}-p{s}{{1,20}})[^{WS}"'\-][^{WS}"']*"#)),
            mysql_p: build(&format!(r#"({b}(?:mysql|mysqldump|mysqladmin|mysqlimport|mysqlshow|mariadb|mariadb-dump|sshpass){b}[^|;&\n]{{0,300}}?{s}-p{s}{{1,20}})[^{WS}"'\-][^{WS}"']*"#)),
            // openssl -pass pass:… / -passin / -passout
            openssl_pass: build(&format!(r#"((?:^|{s})-pass(?:in|out|word)?{s}{{1,20}}(?:pass:)?)[^{WS}"']+"#)),
            // aws configure set <secret-name> <value>
            aws_set: build(&format!(r"({b}aws{s}+configure{s}+set{s}+)([A-Za-z0-9_.\-]{{1,80}})({s}{{1,20}})({ns}+)")),
            // vault login <token>; \s{{1,20}}(?!-)\S+ is \s{{1,20}}[^\s-]\S*
            vault_login: build(&format!(r"({b}vault{s}+login{s}{{1,20}})[^{WS}\-]{ns}*")),
            // Token formats recognisable on their own.
            tokens: [
                format!(r"{b}(?:sk|pk|rk)[\-_](?:live_|test_)?[A-Za-z0-9_\-]{{16,}}"),
                format!(r"{b}xox[abposr]-[A-Za-z0-9\-]+"),
                format!(r"{b}gh[pousr]_[A-Za-z0-9]{{20,}}"),
                format!(r"{b}glpat-[A-Za-z0-9_\-]{{16,}}"),
                format!(r"{b}AIza[0-9A-Za-z_\-]{{30,}}"),
                format!(r"{b}AKIA[0-9A-Z]{{16}}{b}"),
                format!(r"{b}eyJ[A-Za-z0-9_\-]{{10,}}\.[A-Za-z0-9_\-]{{10,}}\.[A-Za-z0-9_\-]+"),
            ]
            .iter()
            .map(|p| build(p))
            .collect(),
        }
    })
}

/// The next character boundary after byte `i`.
fn next_char(text: &str, i: usize) -> usize {
    i + text[i..].chars().next().map_or(1, char::len_utf8)
}

/// A JS global `replace`: every match left to right, an empty match moving one code unit on.
fn replace_all(text: &str, re: &Regex, mut rep: impl FnMut(&Captures) -> String) -> String {
    let mut out = String::new();
    let (mut last, mut pos) = (0, 0);
    while pos <= text.len() {
        let Some(c) = re.captures_at(text, pos) else { break };
        let m = c.get(0).unwrap();
        out.push_str(&text[last..m.start()]);
        out.push_str(&rep(&c));
        last = m.end();
        pos = if m.is_empty() {
            if m.end() >= text.len() {
                break;
            }
            next_char(text, m.end())
        } else {
            m.end()
        };
    }
    out.push_str(&text[last..]);
    out
}

/// `'$1•••'`
fn keep_first(c: &Captures) -> String {
    format!("{}{REDACTED}", &c[1])
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// The end of the run of `is_name_char` characters that starts at byte `q`.
fn name_run_end(text: &str, q: usize) -> usize {
    text[q..].char_indices().find(|(_, c)| !is_name_char(*c)).map_or(text.len(), |(i, _)| q + i)
}

/// name=value. JS: `(?<![A-Za-z0-9_.-])(name)(sep)(VALUE)` with scanReplace: a name that
/// isn't secret (or a plain value) doesn't consume the match, and scanning resumes one
/// character later.
///
/// The candidates are the starts of runs of name characters (any later position in a run
/// fails the look-behind). A separator can't start with a name character, so the name is
/// the whole run, and a run longer than 80 can't match at all. So the name is known
/// before any regular expression runs, and only a secret name gets the anchored match
/// for its separator and value, over a window longer than any match can be.
fn key_values(text: &str) -> String {
    let re = &builtins().key_value;
    let mut out = String::new();
    let (mut last, mut q) = (0, 0);
    let mut prev_name = false;
    while q < text.len() {
        let c = text[q..].chars().next().unwrap();
        if !is_name_char(c) || prev_name {
            prev_name = is_name_char(c);
            q += c.len_utf8();
            continue;
        }
        let end = name_run_end(text, q);
        prev_name = true;
        let candidate = end - q <= 80 && is_secret_name(&text[q..end]) && separator_follows(text, end);
        let caps = if candidate { re.captures(window(text, q)) } else { None };
        let Some(c) = caps.filter(|c| !is_plain_value(&c[3])) else {
            q = end;
            continue;
        };
        out.push_str(&text[last..q]);
        out.push_str(&c[1]);
        out.push_str(&c[2]);
        out.push_str(REDACTED);
        last = rest_of_value(text, q + c.get(0).unwrap().end());
        q = last;
        prev_name = text[..q].chars().next_back().is_some_and(is_name_char);
    }
    out.push_str(&text[last..]);
    out
}

/// `text[q..]`, cut a little past the longest match an anchored rule can make (a name or
/// flag, a separator and a VALUE: about 330 code units), so a failing match never reads
/// the rest of a 5 MB text.
fn window(text: &str, q: usize) -> &str {
    match text[q..].char_indices().nth(400) {
        Some((i, _)) => &text[q..q + i],
        None => &text[q..],
    }
}

/// An unquoted value cut at its bound is redacted to its end (VALUE_REST).
fn rest_of_value(text: &str, from: usize) -> usize {
    text[from..].char_indices().find(|(_, c)| !is_value_char(*c)).map_or(text.len(), |(i, _)| from + i)
}

/// --name value. JS: `(?<=^|\s)(flag)(sep)(VALUE)`, scanned like key_values: a candidate
/// is a `-` at the start or after whitespace; the separator can't start with a flag
/// character, so the flag is its whole run of `[A-Za-z0-9_-]`, and only a secret flag
/// gets the anchored match.
fn flag_values(text: &str) -> String {
    let re = &builtins().flag_value;
    let flag_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::new();
    let (mut last, mut q) = (0, 0);
    while q < text.len() {
        let after_space = q == 0 || text[..q].chars().next_back().is_some_and(is_js_space);
        let c = text[q..].chars().next().unwrap();
        if c != '-' || !after_space {
            q += c.len_utf8();
            continue;
        }
        let end = text[q..].char_indices().find(|(_, c)| !flag_char(*c)).map_or(text.len(), |(i, _)| q + i);
        let caps = if is_secret_name(&text[q..end]) { re.captures(window(text, q)) } else { None };
        let Some(c) = caps.filter(|c| !is_plain_value(&c[3]) && !c[3].starts_with('-')) else {
            q = end;
            continue;
        };
        out.push_str(&text[last..q]);
        out.push_str(&c[1]);
        out.push_str(&c[2]);
        out.push_str(REDACTED);
        last = rest_of_value(text, q + c.get(0).unwrap().end());
        q = last;
    }
    out.push_str(&text[last..]);
    out
}

/// -pSECRET. JS: `((?:^|\s)-p)(?![\d:.,/-]+(?:\s|$))[^\s"']{6,}`. The look-ahead sits at a
/// fixed place for each start, so a match whose look-ahead holds is refused and the
/// search goes on one character later.
fn dash_p_glued(text: &str) -> String {
    let re = &builtins().dash_p_glued;
    let port_like = |at: usize| {
        let mut n = 0;
        for c in text[at..].chars() {
            if c.is_ascii_digit() || matches!(c, ':' | '.' | ',' | '/' | '-') {
                n += 1;
                continue;
            }
            return n > 0 && is_js_space(c);
        }
        n > 0
    };
    let mut out = String::new();
    let (mut last, mut pos) = (0, 0);
    while pos < text.len() {
        let Some(c) = re.captures_at(text, pos) else { break };
        let (m, g1) = (c.get(0).unwrap(), c.get(1).unwrap());
        if port_like(g1.end()) {
            pos = next_char(text, m.start());
            continue;
        }
        out.push_str(&text[last..m.start()]);
        out.push_str(g1.as_str());
        out.push_str(REDACTED);
        last = m.end();
        pos = last;
    }
    out.push_str(&text[last..]);
    out
}

/// user:password@ in a URL. The userinfo runs from `scheme://` to the LAST `@` before
/// the host; the authority ends at the first `/` or whitespace, so an `@` in the path or
/// the query is never taken for it. Everything from the first `:` of the userinfo to that
/// `@` is the password, `@` and `:` in it included; the user name stays readable. A
/// userinfo without `:` (or with an empty password) is left alone, as before. A plain
/// scan: each `://` scans only up to the next `/`, so the whole is linear. The Node brain
/// does the same (brain/redact.mjs `urlCredentials`).
fn url_credentials(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    let mut from = 0;
    while let Some(off) = text[from..].find("://") {
        let i = from + off;
        let start = i + 3;
        let (mut colon, mut at) = (None, None);
        for (j, c) in text[start..].char_indices() {
            if c == '/' || is_js_space(c) {
                break;
            }
            if c == '@' {
                at = Some(start + j);
            } else if c == ':' && colon.is_none() {
                colon = Some(start + j);
            }
        }
        if let (Some(colon), Some(at)) = (colon, at) {
            if colon + 1 < at {
                out.push_str(&text[last..=colon]);
                out.push_str(REDACTED);
                last = at;
            }
        }
        from = i + 1;
    }
    out.push_str(&text[last..]);
    out
}

/// Bare base64-encoded bytes (best effort): `(?<![A-Za-z0-9+/=_.-])[A-Za-z0-9+/]{16,}={0,2}
/// (?![A-Za-z0-9+/=_.-])`, scanned by hand. A greedy run that fails the look-ahead fails
/// for every shorter choice too (the next character is then `=` or base64), and every
/// later start inside the run fails the look-behind, so the scan moves past the run.
fn base64_runs(text: &str) -> String {
    let b64 = |c: char| c.is_ascii_alphanumeric() || c == '+' || c == '/';
    let near = |c: char| b64(c) || matches!(c, '=' | '_' | '.' | '-');
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut last = 0;
    let mut q = 0;
    while q < bytes.len() {
        if !b64(bytes[q] as char) || (q > 0 && near(bytes[q - 1] as char)) {
            q += 1;
            continue;
        }
        let mut end = q;
        while end < bytes.len() && b64(bytes[end] as char) {
            end += 1;
        }
        if end - q < 16 {
            q = end;
            continue;
        }
        let mut e2 = end;
        while e2 < bytes.len() && bytes[e2] == b'=' && e2 - end < 2 {
            e2 += 1;
        }
        // ASCII only on either side, so byte positions are character positions here;
        // a following multi-byte character is never in `near`.
        if e2 < bytes.len() && near(bytes[e2] as char) {
            q = end;
            continue;
        }
        if looks_encoded(&text[q..e2]) {
            out.push_str(&text[last..q]);
            out.push_str(REDACTED);
            last = e2;
        }
        q = e2;
    }
    out.push_str(&text[last..]);
    out
}

/// A standalone run of base64 characters is redacted only when it looks like encoded
/// bytes: 16+ characters, upper and lower case and digits, not hex, padded with = or 32+
/// long, at most one `/`, and at least 3.5 bits of entropy per character.
fn looks_encoded(tok: &str) -> bool {
    let unpadded = tok.trim_end_matches('=');
    if tok.len() > 4096 || (!unpadded.is_empty() && unpadded.bytes().all(|b| b.is_ascii_hexdigit())) {
        return false;
    }
    if !(tok.bytes().any(|b| b.is_ascii_uppercase()) && tok.bytes().any(|b| b.is_ascii_lowercase()) && tok.bytes().any(|b| b.is_ascii_digit())) {
        return false;
    }
    if !(tok.ends_with('=') || tok.len() >= 32) {
        return false;
    }
    if tok.bytes().filter(|b| *b == b'/').count() > 1 {
        return false;
    }
    // Object.values(counts) in JS order: the digit keys first, ascending, then the rest
    // in the order first seen. The sum is taken in that order, as Node takes it.
    let mut counts: Vec<(u8, usize)> = Vec::new();
    for b in tok.bytes() {
        match counts.iter_mut().find(|(c, _)| *c == b) {
            Some((_, n)) => *n += 1,
            None => counts.push((b, 1)),
        }
    }
    let mut ordered: Vec<(u8, usize)> = counts.iter().filter(|(c, _)| c.is_ascii_digit()).copied().collect();
    ordered.sort_by_key(|(c, _)| *c);
    ordered.extend(counts.iter().filter(|(c, _)| !c.is_ascii_digit()));
    let len = tok.len() as f64;
    let mut h = 0.0f64;
    for (_, n) in ordered {
        let p = n as f64 / len;
        h -= p * p.log2();
    }
    h >= 3.5
}

/// The patterns always applied, in the Node order, before `bubble.redactPatterns`.
fn apply_builtins(text: &str) -> String {
    let b = builtins();
    let mut s = replace_all(text, &b.authorization, keep_first);
    s = replace_all(&s, &b.bearer, keep_first);
    s = replace_all(&s, &b.cookie, keep_first);
    s = key_values(&s);
    s = flag_values(&s);
    s = replace_all(&s, &b.curl_user, keep_first);
    s = dash_p_glued(&s);
    s = replace_all(&s, &b.login_p, keep_first);
    s = replace_all(&s, &b.mysql_p, keep_first);
    s = replace_all(&s, &b.openssl_pass, keep_first);
    s = replace_all(&s, &b.aws_set, |c| if is_secret_name(&c[2]) { format!("{}{}{}{REDACTED}", &c[1], &c[2], &c[3]) } else { c[0].to_string() });
    s = replace_all(&s, &b.vault_login, keep_first);
    s = url_credentials(&s);
    for re in &b.tokens {
        s = replace_all(&s, re, |_| REDACTED.to_string());
    }
    base64_runs(&s)
}

// ------------------------------------------------------------------ user patterns

/// Why a `bubble.redactPatterns` entry can't be used. Every one is a config error (exit 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    NotAString,
    Invalid { pattern: String, reason: String },
    LookAround { pattern: String },
    Backreference { pattern: String },
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatternError::NotAString => write!(f, "bubble.redactPatterns: every entry must be a non-empty string"),
            PatternError::Invalid { pattern, reason } => write!(f, "bubble.redactPatterns: {} is not a valid regular expression ({reason})", quote(pattern)),
            PatternError::LookAround { pattern } => write!(f, "bubble.redactPatterns: {} uses look-ahead or look-behind ((?=, (?!, (?<= or (?<!), which bellwether's regular expressions don't support; rewrite it without", quote(pattern)),
            PatternError::Backreference { pattern } => write!(f, "bubble.redactPatterns: {} uses a backreference (\\1, \\k<name>), which bellwether's regular expressions don't support; rewrite it without", quote(pattern)),
        }
    }
}

/// A literal character in Rust regex syntax, safe inside and outside a class.
fn lit(c: char) -> String {
    if c.is_ascii_alphanumeric() || c == '_' || c == ' ' {
        c.to_string()
    } else {
        format!("\\x{{{:X}}}", c as u32)
    }
}

const DOT: &str = r"[^\n\r\x{2028}\x{2029}]";

fn class_escape(e: char) -> Option<String> {
    Some(match e {
        'd' => "0-9".into(),
        'w' => "A-Za-z0-9_".into(),
        's' => WS.into(),
        _ => return None,
    })
}

/// One escape's character (`\t`, `\x41`, `\u00e9`, `\cA`, an identity escape), and how
/// many pattern characters after the backslash it used.
fn escape_char(chars: &[char], i: usize, in_class: bool) -> (char, usize) {
    let e = chars[i];
    let hex = |from: usize, n: usize| -> Option<u32> {
        let s: String = chars.get(from..from + n)?.iter().collect();
        (s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(&s, 16).ok()).flatten()
    };
    match e {
        't' => ('\t', 1),
        'n' => ('\n', 1),
        'r' => ('\r', 1),
        'v' => ('\u{B}', 1),
        'f' => ('\u{C}', 1),
        'b' if in_class => ('\u{8}', 1),
        '0' if !chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => ('\0', 1),
        'x' => match hex(i + 1, 2) {
            Some(v) => (char::from_u32(v).unwrap(), 3),
            None => ('x', 1),
        },
        'u' => match hex(i + 1, 4) {
            Some(v) => (jsstr::unit_char(v as u16), 5),
            None => ('u', 1),
        },
        'c' => match chars.get(i + 1) {
            Some(l) if l.is_ascii_alphabetic() || (in_class && (l.is_ascii_digit() || *l == '_')) => (char::from_u32(*l as u32 % 32).unwrap(), 2),
            // `\c` not followed by a letter: the backslash itself is literal (Annex B).
            _ => ('\\', 0),
        },
        // Annex B legacy octal (`\01`, `\101`) inside a class, where it can't be a backreference.
        '0'..='7' if in_class => {
            let mut v = 0u32;
            let mut n = 0;
            while n < 3 && chars.get(i + n).is_some_and(|c| ('0'..='7').contains(c)) && v * 8 + (chars[i + n] as u32 - '0' as u32) <= 0o377 {
                v = v * 8 + (chars[i + n] as u32 - '0' as u32);
                n += 1;
            }
            (char::from_u32(v).unwrap(), n)
        }
        other => (other, 1),
    }
}

/// Translates a pattern in the config's JS dialect (no `u` flag) into Rust `regex` syntax
/// over unit strings: `\d \w \s \b` as JS has them, `.` without line terminators, `{`
/// literal unless it starts a quantifier, every literal escaped. Look-around and
/// backreferences are refused.
pub fn translate(pattern: &str) -> Result<String, PatternError> {
    let invalid = |reason: &str| PatternError::Invalid { pattern: pattern.to_string(), reason: reason.to_string() };
    let (body, insensitive) = match pattern.strip_prefix("(?i)") {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let chars: Vec<char> = jsstr::from_str(body).chars().collect();
    let mut out = String::from(if insensitive { "(?i)" } else { "" });
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let Some(&e) = chars.get(i + 1) else { return Err(invalid("\\ at end of pattern")) };
                match e {
                    'd' => out.push_str("[0-9]"),
                    'D' => out.push_str("[^0-9]"),
                    'w' => out.push_str("[A-Za-z0-9_]"),
                    'W' => out.push_str("[^A-Za-z0-9_]"),
                    's' => out.push_str(&format!("[{WS}]")),
                    'S' => out.push_str(&format!("[^{WS}]")),
                    'b' => out.push_str(r"(?-u:\b)"),
                    'B' => out.push_str(r"(?-u:\B)"),
                    '1'..='9' => return Err(PatternError::Backreference { pattern: pattern.to_string() }),
                    'k' if chars.get(i + 2) == Some(&'<') => return Err(PatternError::Backreference { pattern: pattern.to_string() }),
                    _ => {
                        let (ch, used) = escape_char(&chars, i + 1, false);
                        out.push_str(&lit(ch));
                        i += used;
                    }
                }
                i += if matches!(e, 'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'b' | 'B') { 2 } else { 1 };
                continue;
            }
            '[' => {
                let (class, next) = translate_class(&chars, i).map_err(|r| invalid(&r))?;
                out.push_str(&class);
                i = next;
                continue;
            }
            '(' if chars.get(i + 1) == Some(&'?') => match chars.get(i + 2) {
                Some(':') => {
                    out.push_str("(?:");
                    i += 3;
                    continue;
                }
                Some('=' | '!') => return Err(PatternError::LookAround { pattern: pattern.to_string() }),
                Some('<') if matches!(chars.get(i + 3), Some('=' | '!')) => return Err(PatternError::LookAround { pattern: pattern.to_string() }),
                Some('<') => {
                    let close = chars[i + 3..].iter().position(|c| *c == '>').ok_or_else(|| invalid("invalid capture group name"))?;
                    let name: String = chars[i + 3..i + 3 + close].iter().collect();
                    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') || name.starts_with(|c: char| c.is_ascii_digit()) {
                        return Err(invalid("invalid capture group name"));
                    }
                    out.push_str(&format!("(?<{}>", name.replace('$', "_")));
                    i += 4 + close;
                    continue;
                }
                _ => return Err(invalid("invalid group")),
            },
            '.' => out.push_str(DOT),
            '{' => {
                // A quantifier: {n}, {n,} or {n,m}. Anything else is a literal brace.
                let rest: String = chars[i..chars.len().min(i + 24)].iter().collect();
                let q = Regex::new(r"^\{[0-9]+(?:,[0-9]*)?\}").unwrap();
                match q.find(&rest) {
                    Some(m) => {
                        out.push_str(m.as_str());
                        i += m.as_str().chars().count();
                        continue;
                    }
                    None => out.push_str(r"\{"),
                }
            }
            '}' => out.push_str(r"\}"),
            ']' => out.push_str(r"\]"),
            '(' | ')' | '|' | '*' | '+' | '?' | '^' | '$' => out.push(c),
            other => out.push_str(&lit(other)),
        }
        i += 1;
    }
    Ok(out)
}

enum ClassItem {
    Char(char),
    Set(String),
}

/// A JS class starting at `chars[at] == '['`: its Rust form and the index after it.
fn translate_class(chars: &[char], at: usize) -> Result<(String, usize), String> {
    let mut i = at + 1;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let item = |i: &mut usize| -> Result<ClassItem, String> {
        let c = chars[*i];
        if c != '\\' {
            *i += 1;
            return Ok(ClassItem::Char(c));
        }
        let e = *chars.get(*i + 1).ok_or("\\ at end of pattern")?;
        if let Some(set) = class_escape(e) {
            *i += 2;
            return Ok(ClassItem::Set(set));
        }
        if let Some(set) = class_escape(e.to_ascii_lowercase()).filter(|_| matches!(e, 'D' | 'W' | 'S')) {
            *i += 2;
            return Ok(ClassItem::Set(format!("[^{set}]")));
        }
        if e == 'k' && chars.get(*i + 2) == Some(&'<') {
            // In a class `\k` is only ever the letter.
            *i += 2;
            return Ok(ClassItem::Char('k'));
        }
        let (ch, used) = escape_char(chars, *i + 1, true);
        *i += 1 + used;
        Ok(ClassItem::Char(ch))
    };
    let mut parts: Vec<String> = Vec::new();
    loop {
        match chars.get(i) {
            None => return Err("missing /".into()),
            Some(']') => {
                i += 1;
                break;
            }
            Some(_) => {}
        }
        let left = item(&mut i)?;
        let is_range = chars.get(i) == Some(&'-') && chars.get(i + 1).is_some_and(|c| *c != ']');
        if !is_range {
            parts.push(match left {
                ClassItem::Char(c) => lit(c),
                ClassItem::Set(s) if s.starts_with('[') => s,
                ClassItem::Set(s) => format!("[{s}]"),
            });
            continue;
        }
        i += 1;
        let right = item(&mut i)?;
        match (left, right) {
            (ClassItem::Char(a), ClassItem::Char(b)) => {
                if a > b {
                    return Err("range out of order in character class".into());
                }
                parts.push(format!("{}-{}", lit(a), lit(b)));
            }
            // Annex B: a range with a class escape at either end is the two ends and `-`.
            (l, r) => {
                for x in [l, ClassItem::Char('-'), r] {
                    parts.push(match x {
                        ClassItem::Char(c) => lit(c),
                        ClassItem::Set(s) if s.starts_with('[') => s,
                        ClassItem::Set(s) => format!("[{s}]"),
                    });
                }
            }
        }
    }
    let body = parts.concat();
    let class = match (negated, body.is_empty()) {
        (false, true) => r"[^\x{0}-\x{10FFFF}]".to_string(),
        (true, true) => r"[\x{0}-\x{10FFFF}]".to_string(),
        (n, false) => format!("[{}{body}]", if n { "^" } else { "" }),
    };
    Ok((class, i))
}

/// One `bubble.redactPatterns` entry, compiled. JS has no inline flags, so the config's
/// leading `(?i)` is the `i` flag; anywhere else it is an error, as in Node.
pub fn compile_pattern(pattern: &str) -> Result<Regex, PatternError> {
    if pattern.is_empty() {
        return Err(PatternError::NotAString);
    }
    let rust = translate(pattern)?;
    RegexBuilder::new(&rust).size_limit(64 << 20).dfa_size_limit(64 << 20).build().map_err(|e| {
        let reason = match &e {
            regex::Error::Syntax(s) => s.lines().rev().find(|l| l.starts_with("error:")).map(|l| l.trim_start_matches("error: ").to_string()).unwrap_or_else(|| s.clone()),
            regex::Error::CompiledTooBig(_) => "too large to compile".to_string(),
            other => other.to_string(),
        };
        PatternError::Invalid { pattern: pattern.to_string(), reason }
    })
}

// ------------------------------------------------------------------ the cleaner

/// `makeTextCleaner({maxChars, redactPatterns})`: normalise, redact (built-in rules, then
/// the user's), truncate. Strings in and out are unit strings.
pub struct Cleaner {
    pub max_chars: usize,
    user: Vec<Regex>,
}

impl Cleaner {
    pub fn new(max_chars: usize, patterns: &[String]) -> Result<Cleaner, PatternError> {
        // Compile the built-in rules now, not on the first hook the brain cleans.
        builtins();
        let user = patterns.iter().map(|p| compile_pattern(p)).collect::<Result<_, _>>()?;
        Ok(Cleaner { max_chars, user })
    }

    pub fn clean(&self, text: &str) -> String {
        let mut s = apply_builtins(&normalise_text(text));
        for re in &self.user {
            s = replace_all(&s, re, |_| REDACTED.to_string());
        }
        truncate(&s, self.max_chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(text: &str) -> String {
        Cleaner::new(500, &[]).unwrap().clean(text)
    }

    #[test]
    fn the_rules_redact_and_keep_the_name() {
        assert_eq!(clean("export GITHUB_TOKEN=abcd1234efgh"), "export GITHUB_TOKEN=•••");
        assert_eq!(clean("curl -u admin:S3cr3t x"), "curl -u admin:••• x");
        assert_eq!(clean("mysql -uroot -pS3cr3tPw"), "mysql -uroot -p•••");
        assert_eq!(clean("docker run -p8080:80 img"), "docker run -p8080:80 img");
        assert_eq!(clean("git commit -m \"fix auth flow\""), "git commit -m \"fix auth flow\"");
    }

    #[test]
    fn url_userinfo_runs_to_the_last_at() {
        assert_eq!(clean("curl https://u%40x:p@ss:w0rd@h.test/"), "curl https://u%40x:•••@h.test/");
        assert_eq!(clean("curl https://h.test/users/bob@example.com"), "curl https://h.test/users/bob@example.com");
        assert_eq!(clean("git clone https://bob:pw@h.test/bob@example.com/r"), "git clone https://bob:•••@h.test/bob@example.com/r");
        assert_eq!(clean("ssh://git@github.com/x"), "ssh://git@github.com/x");
    }

    #[test]
    fn translation_keeps_js_meaning_and_refuses_what_regex_cannot_do() {
        let c = |p: &str, t: &str| Cleaner::new(200, &[p.to_string()]).unwrap().clean(t);
        assert_eq!(c("(?i)hunter\\d", "HUNTER2 hunter3 hunter١ hunterx"), "••• ••• hunter١ hunterx");
        assert_eq!(c("a{", "xa{y"), "x•••y");
        assert_eq!(c("[\\d-z]+", "ab-9z"), "ab•••");
        assert!(matches!(compile_pattern("a(?=b)"), Err(PatternError::LookAround { .. })));
        assert!(matches!(compile_pattern("(?<!x)a"), Err(PatternError::LookAround { .. })));
        assert!(matches!(compile_pattern("(a)\\1"), Err(PatternError::Backreference { .. })));
        assert!(matches!(compile_pattern("(unclosed"), Err(PatternError::Invalid { .. })));
        assert!(matches!(compile_pattern("x(?i)y"), Err(PatternError::Invalid { .. })));
        assert_eq!(compile_pattern("").unwrap_err(), PatternError::NotAString);
    }

    #[test]
    fn normalising_and_truncating() {
        assert_eq!(normalise_text("  ＴＯＫＥＮ\u{200B}\u{7}\u{1b}  x\u{A0}y  "), "TOKEN x y");
        let s = jsstr::from_str(&"😀".repeat(30));
        let t = truncate(&s, 20);
        assert_eq!(jsstr::code_points(&t).len(), 20);
        assert!(jsstr::to_string_lossy(&t).ends_with("😀…"));
    }
}
