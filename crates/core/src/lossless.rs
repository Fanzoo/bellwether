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

    #[test]
    fn bad_json_is_refused() {
        for bad in ["", "{", "[1,]", "{\"a\":1,}", "\u{feff}{}", "01", "1.", "-", "\"\t\"", "nul", "[] x", "{'a':1}", "\"\\x\""] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
