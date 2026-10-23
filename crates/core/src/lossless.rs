//! The lossless JSON of the hook installer (distribution plan D2, R2): a Claude Code
//! settings file read and written back so that "install, then uninstall" gives back the
//! same bytes, and so that the `hooks install` preview is the Node brain's preview
//! (brain/hooks.mjs `parseLossless`, `cloneJson`, `serialise`, `deepEqual`).
//!
//! The rules:
//!
//! - every number literal is kept verbatim (`1.0`, `1e2`, `-0`, `12345678901234567890`),
//!   as Node's `JSON.rawJSON` keeps it; two numbers are equal when their literals are;
//! - object keys keep file order, and a duplicate key keeps its first position with its
//!   last value. (`JSON.parse` would move integer-like keys such as `"1"` first; that is
//!   the one difference from Node, test/DIVERGENCES.md);
//! - `__proto__` is an ordinary key;
//! - strings are unit strings ([`crate::jsstr`]), parsed like `JSON.parse` and written like
//!   `JSON.stringify`: `\"`, `\\`, `\b \f \n \r \t`, other controls as lower-case
//!   `\u00xx`, a lone surrogate as `\udxxx`, and `/` and everything else literally (so
//!   `\/` and `\u00e9` in a file are rewritten, as Node rewrites them);
//! - [`serialise`] writes in the file's own layout: the indent of its first indented line,
//!   its line ending, and whether it ends with a newline, as `serialise()` does.

use crate::jsstr;

/// How deep `parse` nests before refusing a document.
pub const MAX_DEPTH: usize = 1000;

/// A JSON value with its number literals kept.
#[derive(Clone, Debug)]
pub enum Lj {
    Null,
    Bool(bool),
    /// The literal as written.
    Num(String),
    /// A unit string.
    Str(String),
    Arr(Vec<Lj>),
    Obj(LObj),
}

/// An object: properties in file order (new keys go last).
#[derive(Clone, Debug, Default)]
pub struct LObj {
    entries: Vec<(String, Lj)>,
}

impl LObj {
    pub fn new() -> LObj {
        LObj::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k == key)
    }

    pub fn get(&self, key: &str) -> Option<&Lj> {
        self.position(key).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Lj> {
        let i = self.position(key)?;
        Some(&mut self.entries[i].1)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// `setOwn`: an existing key keeps its place, a new one goes last.
    pub fn set(&mut self, key: impl Into<String>, value: Lj) {
        let key = key.into();
        match self.position(&key) {
            Some(i) => self.entries[i].1 = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn with(mut self, key: impl Into<String>, value: Lj) -> LObj {
        self.set(key, value);
        self
    }

    pub fn remove(&mut self, key: &str) -> Option<Lj> {
        let i = self.position(key)?;
        Some(self.entries.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Lj)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> Vec<&String> {
        self.entries.iter().map(|(k, _)| k).collect()
    }
}

impl Lj {
    pub fn str(s: &str) -> Lj {
        Lj::Str(s.to_string())
    }

    pub fn int(n: i64) -> Lj {
        Lj::Num(n.to_string())
    }

    /// `value[key]` on a parsed value: None for a missing key or a non-object.
    pub fn get(&self, key: &str) -> Option<&Lj> {
        match self {
            Lj::Obj(o) => o.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Lj::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&LObj> {
        match self {
            Lj::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_obj_mut(&mut self) -> Option<&mut LObj> {
        match self {
            Lj::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&Vec<Lj>> {
        match self {
            Lj::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// The value of a number, as `Number(literal)` reads it.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Lj::Num(n) => n.parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, Lj::Obj(_))
    }

    /// The JS value, for code that works on [`crate::json::Js`] (doctor's checks). A
    /// number becomes its double, as `JSON.parse` reads it; keys keep file order on the
    /// way in, then follow JS order there.
    pub fn to_js(&self) -> crate::json::Js {
        use crate::json::{Js, Obj};
        match self {
            Lj::Null => Js::Null,
            Lj::Bool(b) => Js::Bool(*b),
            Lj::Num(n) => Js::Num(n.parse::<f64>().unwrap_or(f64::NAN)),
            Lj::Str(s) => Js::Str(s.clone()),
            Lj::Arr(a) => Js::Arr(a.iter().map(Lj::to_js).collect()),
            Lj::Obj(o) => {
                let mut out = Obj::new();
                for (k, v) in o.iter() {
                    out.set(k.clone(), v.to_js());
                }
                Js::Obj(out)
            }
        }
    }

    /// From a JS value, in its iteration order: a number becomes the literal
    /// `Number.prototype.toString` writes (what `JSON.stringify` would write); `undefined`
    /// properties are left out, as `JSON.stringify` leaves them out.
    pub fn from_js(v: &crate::json::Js) -> Lj {
        use crate::json::Js;
        match v {
            Js::Undefined | Js::Null => Lj::Null,
            Js::Bool(b) => Lj::Bool(*b),
            Js::Num(n) if n.is_finite() => Lj::Num(crate::json::number_to_string(*n)),
            Js::Num(_) => Lj::Null,
            Js::Str(s) => Lj::Str(s.clone()),
            Js::Arr(a) => Lj::Arr(a.iter().map(Lj::from_js).collect()),
            Js::Obj(o) => {
                let mut out = LObj::new();
                for (k, x) in o.iter() {
                    if !x.is_undefined() {
                        out.set(k.clone(), Lj::from_js(x));
                    }
                }
                Lj::Obj(out)
            }
        }
    }
}

/// `deepEqual` of brain/hooks.mjs: arrays in order, objects by key set (order ignored),
/// numbers by literal (a raw `1.0` is not the number `1`, as in Node).
pub fn deep_equal(a: &Lj, b: &Lj) -> bool {
    match (a, b) {
        (Lj::Null, Lj::Null) => true,
        (Lj::Bool(x), Lj::Bool(y)) => x == y,
        (Lj::Num(x), Lj::Num(y)) => x == y,
        (Lj::Str(x), Lj::Str(y)) => x == y,
        (Lj::Arr(x), Lj::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| deep_equal(p, q)),
        (Lj::Obj(x), Lj::Obj(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| deep_equal(v, w))),
        _ => false,
    }
}

// ------------------------------------------------------------------ parse

/// `parseLossless(text)`. The error says what was wrong and where.
/// `text` and whether it starts with a UTF-8 byte order mark (U+FEFF), which the parser
/// refuses: a settings.json saved by Notepad as "UTF-8 with BOM" has one. The installer and
/// doctor read the text after it, and the installer writes it back (Windows plan W2).
pub fn split_bom(text: &str) -> (&'static str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

pub fn parse(text: &str) -> Result<Lj, String> {
    let mut p = Parser { s: text.as_bytes(), text, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("unexpected text after the JSON value"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at position {}", self.text[..self.i.min(self.text.len())].chars().count())
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Lj, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end of JSON input")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Lj::Str(self.string()?)),
            Some(b't') => self.literal("true", Lj::Bool(true)),
            Some(b'f') => self.literal("false", Lj::Bool(false)),
            Some(b'n') => self.literal("null", Lj::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected token")),
        }
    }

    fn literal(&mut self, word: &str, v: Lj) -> Result<Lj, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected token"))
        }
    }

    fn digits(&mut self) -> usize {
        let s = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        self.i - s
    }

    fn number(&mut self) -> Result<Lj, String> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("no number after the minus sign")),
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err("unterminated fractional number"));
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("exponent part is missing a number"));
            }
        }
        Ok(Lj::Num(self.text[start..self.i].to_string()))
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("bad Unicode escape"))?;
        if !h.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.err("bad Unicode escape"));
        }
        let v = u16::from_str_radix(std::str::from_utf8(h).unwrap(), 16).map_err(|_| self.err("bad Unicode escape"))?;
        self.i += 4;
        Ok(v)
    }

    /// A string, as a unit string.
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' && self.s[self.i] >= 0x20 {
                self.i += 1;
            }
            if self.i > start {
                for c in self.text[start..self.i].chars() {
                    jsstr::push_real(&mut out, c);
                }
            }
            match self.s.get(self.i) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = *self.s.get(self.i).ok_or_else(|| self.err("unterminated string"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let u = self.hex4()?;
                            out.push(jsstr::unit_char(u));
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("bad escaped character"));
                        }
                    }
                }
                Some(_) => return Err(self.err("bad control character in string")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Lj, String> {
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Lj::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value(depth + 1)?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Lj::Arr(out));
                }
                _ => return Err(self.err("expected ',' or ']' after an array element")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Lj, String> {
        self.i += 1;
        let mut out = LObj::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Lj::Obj(out));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(self.err("expected a property name"));
            }
            let k = self.string()?;
            self.ws();
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.err("expected ':' after a property name"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            out.set(k, v);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Lj::Obj(out));
                }
                _ => return Err(self.err("expected ',' or '}' after a property value")),
            }
        }
    }
}

// ------------------------------------------------------------------ write

/// `JSON.stringify(value, null, indent)`: `indent` is cut to 10 characters, as JS cuts
/// the gap; an empty indent writes everything on one line.
pub fn stringify(v: &Lj, indent: &str) -> String {
    let gap: String = indent.chars().take(10).collect();
    let mut out = String::new();
    write(&mut out, v, if gap.is_empty() { None } else { Some(&gap) }, 0);
    out
}

fn newline(out: &mut String, indent: Option<&str>, depth: usize) {
    if let Some(ind) = indent {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(ind);
        }
    }
}

fn write(out: &mut String, v: &Lj, indent: Option<&str>, depth: usize) {
    match v {
        Lj::Null => out.push_str("null"),
        Lj::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Lj::Num(n) => out.push_str(n),
        Lj::Str(s) => out.push_str(&crate::json::quote(s)),
        Lj::Arr(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                write(out, x, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push(']');
        }
        Lj::Obj(o) => {
            if o.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                out.push_str(&crate::json::quote(k));
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write(out, x, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push('}');
        }
    }
}

/// The indent `serialise()` takes from a file: `oldText.match(/^[\r\n]*[{[][ \t]*\r?\n([ \t]+)\S/)`,
/// else two spaces.
pub fn detect_indent(old: &str) -> String {
    let b = old.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b'\r' | b'\n') {
        i += 1;
    }
    if !matches!(b.get(i), Some(b'{' | b'[')) {
        return "  ".into();
    }
    i += 1;
    while i < b.len() && matches!(b[i], b' ' | b'\t') {
        i += 1;
    }
    if b.get(i) == Some(&b'\r') {
        i += 1;
    }
    if b.get(i) != Some(&b'\n') {
        return "  ".into();
    }
    i += 1;
    let start = i;
    while i < b.len() && matches!(b[i], b' ' | b'\t') {
        i += 1;
    }
    if i == start {
        return "  ".into();
    }
    // `\S`: the next character must not be JS whitespace (a greedy run that backtracks
    // only ever finds a space or a tab next, which isn't `\S` either).
    match old[i..].chars().next() {
        Some(c) if !jsstr::is_js_space(c) => old[start..i].to_string(),
        _ => "  ".into(),
    }
}

/// `serialise(value, oldText)`: JSON in the original file's layout (its indent, its line
/// ending, its trailing-newline choice). A new file gets two spaces and LF.
pub fn serialise(v: &Lj, old: &str) -> String {
    let indent = detect_indent(old);
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let mut text = stringify(v, &indent);
    if eol != "\n" {
        text = text.replace('\n', eol);
    }
    if old.is_empty() || old.ends_with('\n') {
        text.push_str(eol);
    }
    text
}

// ------------------------------------------------------------------ edit

/// config.json's writer (M13): `new` written into `old`'s own text, changing only what
/// changed, so an edit to one key leaves every other line byte for byte as it was.
/// ([`serialise`] lays the whole file out again, as the hook installer's Node original did;
/// the installer keeps it, and its round trip is pinned by the frozen sample.)
///
/// - A value equal to the old one is copied verbatim: its lines, spacing, escapes and
///   trailing whitespace. So is the text before and after the root value.
/// - A changed scalar is replaced where it stands.
/// - A changed object or array is edited member by member (an object's by key, an array's
///   by index): unchanged members stay as they are, removed ones go, and new ones go after
///   the last, with the last member's spacing. So a one-line array or object stays on one
///   line, whatever changed inside it, and a multi-line one stays multi-line.
/// - A value written fresh (a new key, an empty container that gets members, or a value
///   whose type changed) is written on one line inside a one-line container, in that
///   container's spacing (`", "` or `","`, `": "` or `":"`); elsewhere in the file's indent
///   ([`detect_indent`]) and line ending, from the indent of the line it starts on.
///
/// An `old` that isn't valid JSON, or that has a duplicate key (which [`parse`] folds into
/// one), is laid out again by [`serialise`].
pub fn edit(old: &str, new: &Lj) -> String {
    let mut p = Parser { s: old.as_bytes(), text: old, i: 0 };
    p.ws();
    let root = match p.node(0) {
        Ok(n) => n,
        Err(_) => return serialise(new, old),
    };
    p.ws();
    if p.i < p.s.len() || root.has_duplicate_keys() {
        return serialise(new, old);
    }
    let ed = Editor { text: old, indent: detect_indent(old), eol: if old.contains("\r\n") { "\r\n" } else { "\n" } };
    let mut out = String::with_capacity(old.len() + 64);
    out.push_str(&old[..root.start]);
    let line = line_indent(old, root.start);
    ed.render(&mut out, &root, new, false, &line, &Style::default());
    out.push_str(&old[root.end..]);
    out
}

/// Whether a valid JSON text repeats a key in one of its objects ([`parse`] keeps the last
/// value at the first place; [`edit`] lays such a file out again). False for invalid JSON.
pub fn has_duplicate_keys(text: &str) -> bool {
    let mut p = Parser { s: text.as_bytes(), text, i: 0 };
    p.ws();
    p.node(0).is_ok_and(|n| n.has_duplicate_keys())
}

/// A value's place in the text.
struct Node {
    start: usize,
    end: usize,
    shape: Shape,
}

enum Shape {
    Scalar(Lj),
    Obj(Vec<Member>),
    Arr(Vec<Member>),
}

/// A member: `lead` is the whitespace before it, `key` the key's text and value (objects),
/// `mid` the text from the key to the value (`": "`), `post` the whitespace after the value.
struct Member {
    lead: (usize, usize),
    key: Option<(usize, usize, String)>,
    mid: (usize, usize),
    value: Node,
    post: (usize, usize),
}

impl Node {
    fn has_duplicate_keys(&self) -> bool {
        match &self.shape {
            Shape::Scalar(_) => false,
            Shape::Arr(ms) => ms.iter().any(|m| m.value.has_duplicate_keys()),
            Shape::Obj(ms) => {
                let mut seen = std::collections::HashSet::new();
                ms.iter().any(|m| !seen.insert(m.key.as_ref().map(|k| k.2.as_str()).unwrap_or("")) || m.value.has_duplicate_keys())
            }
        }
    }
}

impl Parser<'_> {
    /// A value with the place of every member, for [`edit`].
    fn node(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        let start = self.i;
        let (open, close) = match self.s.get(self.i) {
            Some(b'{') => (b'{', b'}'),
            Some(b'[') => (b'[', b']'),
            _ => {
                let v = self.value(depth)?;
                return Ok(Node { start, end: self.i, shape: Shape::Scalar(v) });
            }
        };
        self.i += 1;
        let mut members = Vec::new();
        loop {
            let ls = self.i;
            self.ws();
            let lead = (ls, self.i);
            if members.is_empty() && self.s.get(self.i) == Some(&close) {
                self.i += 1;
                break;
            }
            let key = if open == b'{' {
                if self.s.get(self.i) != Some(&b'"') {
                    return Err(self.err("expected a property name"));
                }
                let ks = self.i;
                let k = self.string()?;
                Some((ks, self.i, k))
            } else {
                None
            };
            let ms = self.i;
            if key.is_some() {
                self.ws();
                if self.s.get(self.i) != Some(&b':') {
                    return Err(self.err("expected ':' after a property name"));
                }
                self.i += 1;
                self.ws();
            }
            let mid = (ms, self.i);
            let value = self.node(depth + 1)?;
            let ps = self.i;
            self.ws();
            members.push(Member { lead, key, mid, value, post: (ps, self.i) });
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(c) if *c == close => {
                    self.i += 1;
                    break;
                }
                _ => return Err(self.err("expected ',' or the end of the object or array")),
            }
        }
        let shape = if open == b'{' { Shape::Obj(members) } else { Shape::Arr(members) };
        Ok(Node { start, end: self.i, shape })
    }
}

/// The separators a fresh one-line value is written with.
#[derive(Clone)]
struct Style {
    comma: String,
    colon: String,
}

impl Default for Style {
    fn default() -> Style {
        Style { comma: ", ".into(), colon: ": ".into() }
    }
}

/// The whitespace that starts the line `at` is on.
fn line_indent(text: &str, at: usize) -> String {
    let line = text[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
    text[line..at].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// A member's line indent: what follows the last newline of its lead, else `fallback`.
fn indent_of(lead: &str, fallback: &str) -> String {
    match lead.rfind('\n') {
        Some(n) => lead[n + 1..].to_string(),
        None => fallback.to_string(),
    }
}

fn write_inline(out: &mut String, v: &Lj, style: &Style) {
    match v {
        Lj::Arr(a) if !a.is_empty() => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(&style.comma);
                }
                write_inline(out, x, style);
            }
            out.push(']');
        }
        Lj::Obj(o) if !o.is_empty() => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(&style.comma);
                }
                out.push_str(&crate::json::quote(k));
                out.push_str(&style.colon);
                write_inline(out, x, style);
            }
            out.push('}');
        }
        _ => write(out, v, None, 0),
    }
}

struct Editor<'a> {
    text: &'a str,
    indent: String,
    eol: &'static str,
}

impl Editor<'_> {
    fn at(&self, span: (usize, usize)) -> &str {
        &self.text[span.0..span.1]
    }

    /// `new` in place of `node`. `inline`: the container holding it is on one line.
    fn render(&self, out: &mut String, node: &Node, new: &Lj, inline: bool, line: &str, style: &Style) {
        match (&node.shape, new) {
            (Shape::Scalar(old), _) if deep_equal(old, new) => out.push_str(&self.text[node.start..node.end]),
            (Shape::Obj(ms), Lj::Obj(o)) => {
                let entries: Vec<(Option<&str>, &Lj)> = o.iter().map(|(k, v)| (Some(k.as_str()), v)).collect();
                self.container(out, node, ms, &entries, new, inline, line, style)
            }
            (Shape::Arr(ms), Lj::Arr(a)) => {
                let entries: Vec<(Option<&str>, &Lj)> = a.iter().map(|v| (None, v)).collect();
                self.container(out, node, ms, &entries, new, inline, line, style)
            }
            _ => self.fresh(out, new, inline, line, style),
        }
    }

    fn fresh(&self, out: &mut String, v: &Lj, inline: bool, line: &str, style: &Style) {
        if inline {
            write_inline(out, v, style);
        } else {
            out.push_str(&stringify(v, &self.indent).replace('\n', &format!("{}{line}", self.eol)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn container(&self, out: &mut String, node: &Node, ms: &[Member], new: &[(Option<&str>, &Lj)], whole: &Lj, inline: bool, line: &str, style: &Style) {
        let span = &self.text[node.start..node.end];
        let (open, close) = if matches!(node.shape, Shape::Obj(_)) { ('{', '}') } else { ('[', ']') };
        if new.is_empty() {
            if ms.is_empty() {
                out.push_str(span);
            } else {
                out.push(open);
                out.push(close);
            }
            return;
        }
        if ms.is_empty() {
            return self.fresh(out, whole, inline, line, style);
        }
        let multiline = span.contains('\n');
        let last = ms.len() - 1;
        let colon = match &ms[last].key {
            Some(_) => self.at(ms[last].mid).to_string(),
            None => style.colon.clone(),
        };
        // The lead of a member added after the last: the last one's, unless the last is
        // the first of a one-line container (its lead follows the bracket).
        let added_lead = if ms.len() >= 2 || multiline || !self.at(ms[0].lead).is_empty() {
            // From the line break on: the line above's trailing spaces stay with it.
            let l = self.at(ms[last].lead);
            l[l.find(['\r', '\n']).unwrap_or(0)..].to_string()
        } else if (open == '{' && colon.ends_with(' ')) || (open == '[' && style.comma.ends_with(' ')) {
            " ".to_string()
        } else {
            String::new()
        };
        // What a fresh one-line value inside is written with: this container's spacing
        // when it is on one line, else what came from outside.
        // An array has no colon of its own: a tight one (`[1,2]`) asks for a tight one.
        let colon = if open == '[' && !multiline && added_lead.is_empty() { ":".to_string() } else { colon };
        let here = Style { comma: if multiline { style.comma.clone() } else { format!(",{added_lead}") }, colon: colon.clone() };
        out.push(open);
        for (i, (k, v)) in new.iter().enumerate() {
            let is_last = i == new.len() - 1;
            let old = match k {
                Some(k) => ms.iter().position(|m| m.key.as_ref().is_some_and(|x| x.2 == *k)),
                None => (i < ms.len()).then_some(i),
            };
            let post = match old {
                Some(j) => {
                    let m = &ms[j];
                    let lead = self.at(m.lead);
                    out.push_str(lead);
                    if let Some((ks, ke, _)) = &m.key {
                        out.push_str(&self.text[*ks..*ke]);
                    }
                    out.push_str(self.at(m.mid));
                    self.render(out, &m.value, v, !multiline, &indent_of(lead, line), &here);
                    if is_last {
                        self.at(ms[last].post)
                    } else if j == last {
                        ""
                    } else {
                        self.at(m.post)
                    }
                }
                None => {
                    out.push_str(&added_lead);
                    if let Some(k) = k {
                        out.push_str(&crate::json::quote(k));
                        out.push_str(&colon);
                    }
                    self.fresh(out, v, !multiline, &indent_of(&added_lead, line), &here);
                    if is_last {
                        self.at(ms[last].post)
                    } else {
                        ""
                    }
                }
            };
            out.push_str(post);
            if !is_last {
                out.push(',');
            }
        }
        out.push(close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_order_duplicates_and_proto_are_kept() {
        let text = "{\n  \"b\": 1,\n  \"2\": \"two\",\n  \"1\": 1.0,\n  \"b\": 1e2,\n  \"__proto__\": {\"x\": -0},\n  \"big\": 12345678901234567890\n}\n";
        let v = parse(text).unwrap();
        assert_eq!(v.as_obj().unwrap().keys(), ["b", "2", "1", "__proto__", "big"]);
        assert_eq!(serialise(&v, text), "{\n  \"b\": 1e2,\n  \"2\": \"two\",\n  \"1\": 1.0,\n  \"__proto__\": {\n    \"x\": -0\n  },\n  \"big\": 12345678901234567890\n}\n");
        assert!(!deep_equal(&Lj::Num("1.0".into()), &Lj::int(1)));
        assert!(deep_equal(&parse(r#"{"a":1,"b":[2]}"#).unwrap(), &parse(r#"{"b":[2],"a":1}"#).unwrap()));
        assert!(!deep_equal(&parse("[1,2]").unwrap(), &parse("[2,1]").unwrap()));
    }

    #[test]
    fn strings_are_written_as_json_stringify_writes_them() {
        let v = parse(r#""tab\there \u00e9 \ud83d\ude00 \/ \ud800 \u001F \u007f""#).unwrap();
        assert_eq!(stringify(&v, "  "), "\"tab\\there é 😀 / \\ud800 \\u001f \u{7f}\"");
    }

    #[test]
    fn layout_is_detected_like_serialise() {
        for (old, indent) in [("{\n\t\"a\": 1\n}", "\t"), ("{\r\n    \"a\": 1\r\n}\r\n", "    "), ("\n\n[  \n   1]", "   "), ("{ \"a\": 1 }", "  "), ("{\n\n  \"a\": 1}", "  "), ("", "  "), ("{\n  \u{a0}x", "  ")] {
            assert_eq!(detect_indent(old), indent, "{old:?}");
        }
        let v = parse("{\"a\":[1]}").unwrap();
        assert_eq!(serialise(&v, "{\r\n  \"a\": 2\r\n}"), "{\r\n  \"a\": [\r\n    1\r\n  ]\r\n}");
        assert_eq!(serialise(&v, ""), "{\n  \"a\": [\n    1\n  ]\n}\n");
        // JSON.stringify cuts a gap to 10 characters.
        assert_eq!(stringify(&v, &" ".repeat(12)), format!("{{\n{0}\"a\": [\n{0}{0}1\n{0}]\n}}", " ".repeat(10)));
    }

    /// `edit` with `f` applied to the parsed text.
    fn edited(old: &str, f: impl FnOnce(&mut LObj)) -> String {
        let mut v = parse(old).unwrap();
        f(v.as_obj_mut().unwrap());
        let out = edit(old, &v);
        assert!(deep_equal(&parse(&out).unwrap(), &v), "{out}");
        out
    }

    fn pets(o: &mut LObj) -> &mut LObj {
        o.get_mut("pets").and_then(Lj::as_obj_mut).unwrap()
    }

    #[test]
    fn edit_changes_one_line_and_keeps_every_other_byte() {
        // config.example.json, with one-line arrays (palette, sleep, redactPatterns) and
        // an empty inline object: only the pets.default line changes.
        let old = include_str!("../../../config.example.json");
        let out = edited(old, |o| pets(o).set("default", Lj::str("brah")));
        assert_eq!(out, old.replacen("\"default\": \"bellwether\"", "\"default\": \"brah\"", 1));
        assert_ne!(out, old);
        // serialise, by contrast, spreads the one-line arrays over several lines.
        assert!(serialise(&parse(&out).unwrap(), old).contains("\"palette\": [\n"));
        // Nothing changed: the same bytes, for any valid text without duplicate keys.
        for t in [old, "{}", " [ ] ", "{\"a\" :1 ,\"b\":[ 1,2 ] }\n\n", "{\r\n  \"a\": \"\\u00e9\\/\"   \r\n}", "\"x\"", "{\n\t\"a\": 1.0e0,\n\t\"b\": [\n\t\t{}, [], {\"c\": -0}\n\t]\n}"] {
            assert_eq!(edit(t, &parse(t).unwrap()), t, "{t:?}");
        }
    }

    #[test]
    fn an_edited_inline_value_stays_inline_and_a_multi_line_one_multi_line() {
        let old = "{\n  \"palette\": [\"#a\", \"#b\"],\n  \"tight\": [1,2],\n  \"tall\": [\n    1,\n    2\n  ],\n  \"x\": 0\n}\n";
        // The edited value is the inline array itself: changed and added elements are
        // written on its line, in its spacing; removed ones go.
        let out = edited(old, |o| o.set("palette", parse(r##"["#a", "#c", "#d"]"##).unwrap()));
        assert_eq!(out, old.replace(r##"["#a", "#b"]"##, r##"["#a", "#c", "#d"]"##));
        let out = edited(old, |o| o.set("tight", parse("[1,2,[3,{\"k\":4}]]").unwrap()));
        assert_eq!(out, old.replace("[1,2]", "[1,2,[3,{\"k\":4}]]"));
        let out = edited(old, |o| o.set("palette", parse(r##"["#z"]"##).unwrap()));
        assert_eq!(out, old.replace(r##"["#a", "#b"]"##, r##"["#z"]"##));
        let out = edited(old, |o| o.set("palette", Lj::Arr(vec![])));
        assert_eq!(out, old.replace(r##"["#a", "#b"]"##, "[]"));
        // A multi-line array stays multi-line, an added element on a line of its own.
        let out = edited(old, |o| o.set("tall", parse("[1, 2, 3]").unwrap()));
        assert_eq!(out, old.replace("    2\n  ],", "    2,\n    3\n  ],"));
        // A one-element inline array grows in the default ", ".
        assert_eq!(edited("{\"a\": [1]}", |o| o.set("a", parse("[1,2]").unwrap())), "{\"a\": [1, 2]}");
        // A value whose type changes, in a multi-line object: laid out in the file's indent.
        let out = edited(old, |o| o.set("x", parse("{\"y\": [1]}").unwrap()));
        assert_eq!(out, old.replace("\"x\": 0", "\"x\": {\n    \"y\": [\n      1\n    ]\n  }"));
    }

    #[test]
    fn edit_keeps_nested_inline_objects_and_adds_keys_in_the_containers_layout() {
        let old = "{\n    \"pets\": { \"default\": \"esheep64\", \"byAgentType\": {} },\n    \"strip\": {\"heightPx\":140},\n    \"keep\": {\"a\": {\"b\": [1, 2]}}\n}";
        let out = edited(old, |o| pets(o).set("default", Lj::str("brah")));
        assert_eq!(out, old.replace("esheep64", "brah"));
        // A new key in a one-line object goes on its line, in its spacing.
        let out = edited(old, |o| pets(o).set("animationMap", parse("{\"brah\": {\"walk\": \"w\"}}").unwrap()));
        assert_eq!(out, old.replace("\"byAgentType\": {} }", "\"byAgentType\": {}, \"animationMap\": {\"brah\": {\"walk\": \"w\"}} }"));
        let out = edited(old, |o| o.get_mut("strip").and_then(Lj::as_obj_mut).unwrap().set("scale", Lj::Num("1.5".into())));
        assert_eq!(out, old.replace("{\"heightPx\":140}", "{\"heightPx\":140,\"scale\":1.5}"));
        // A new top-level key, in a multi-line object: the last member's lead and indent.
        let out = edited(old, |o| o.set("z", parse("{\"q\": 1}").unwrap()));
        assert_eq!(out, old.replace("[1, 2]}}\n}", "[1, 2]}},\n    \"z\": {\n        \"q\": 1\n    }\n}"));
        // A removed key takes its line with it; the closing line stays.
        let out = edited(old, |o| {
            o.remove("keep");
        });
        assert_eq!(out, "{\n    \"pets\": { \"default\": \"esheep64\", \"byAgentType\": {} },\n    \"strip\": {\"heightPx\":140}\n}");
        // An empty object that gets a key, in a one-line parent: on one line.
        let out = edited(old, |o| pets(o).set("byAgentType", parse("{\"Explore\": \"tiny\"}").unwrap()));
        assert_eq!(out, old.replace("\"byAgentType\": {} }", "\"byAgentType\": {\"Explore\": \"tiny\"} }"));
    }

    #[test]
    fn edit_keeps_trailing_whitespace_line_endings_and_escapes_on_lines_it_doesnt_touch() {
        let old = "\n{  \r\n  \"a\": \"caf\\u00e9\",   \r\n  \"b\": [1, 2], \r\n  \"c\": 1\r\n}\r\n\r\n";
        let out = edited(old, |o| o.set("c", Lj::int(2)));
        assert_eq!(out, old.replace("\"c\": 1", "\"c\": 2"));
        let out = edited(old, |o| o.set("d", parse("[3]").unwrap()));
        assert_eq!(out, old.replace("\"c\": 1\r\n}", "\"c\": 1,\r\n  \"d\": [\r\n    3\r\n  ]\r\n}"));
        // A new file's value in an empty one: the file's layout from serialise's rules.
        assert_eq!(edited("{}\n", |o| o.set("pets", parse("{\"default\":\"x\"}").unwrap())), "{\n  \"pets\": {\n    \"default\": \"x\"\n  }\n}\n");
    }

    #[test]
    fn edit_falls_back_to_serialise_for_duplicate_keys_and_bad_json() {
        let dup = "{\"a\": 1, \"b\": [1], \"a\": 2}";
        let v = parse(dup).unwrap();
        assert_eq!(edit(dup, &v), serialise(&v, dup));
        let nested = "{\"o\": {\"k\": 1, \"k\": 2}}";
        assert_eq!(edit(nested, &parse(nested).unwrap()), serialise(&parse(nested).unwrap(), nested));
        assert_eq!(edit("{nope", &Lj::Obj(LObj::new().with("a", Lj::int(1)))), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn bad_json_is_refused() {
        for bad in ["", "{", "[1,]", "{\"a\":1,}", "\u{feff}{}", "01", "1.", "-", "\"\t\"", "nul", "[] x", "{'a':1}", "\"\\x\""] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
