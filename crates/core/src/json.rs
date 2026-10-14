//! JSON the way `JSON.parse` and `JSON.stringify` do it, over JS values.
//!
//! The Node brain's contracts are byte-level: the snapshot `/state` sends, the lines of a
//! recording, `state.json` and the transition lines of the golden fixtures are all
//! `JSON.stringify` output. So the port has its own small JSON, and keeps serde_json for
//! the files only Rust reads. What it reproduces:
//!
//! - strings are unit strings (see [`crate::jsstr`]): `"\ud800"` parses to a lone
//!   surrogate, and is written back the same way;
//! - numbers are f64, written as `Number.prototype.toString` writes them (`1e21`,
//!   `1.5e-7`, `-0` as `0`); a number that isn't finite is written as `null`;
//! - object keys keep JS property order: integer-like keys (`"1"`, `"10"`) first, in
//!   ascending order, then the others in the order they were first set. A duplicate key
//!   keeps its first position and its last value. `__proto__` is an ordinary key;
//! - `Undefined` is a property that is there but undefined: `stringify` leaves it out of
//!   objects and writes `null` for it in arrays, as JS does.
//!
//! Parsing is strict JSON (whitespace is space, tab, CR and LF only; no comments, no
//! trailing commas, no BOM). It refuses nesting deeper than [`MAX_DEPTH`] instead of
//! running out of stack.

use std::collections::HashMap;

use crate::jsstr;

/// How deep `parse` nests before refusing a document.
pub const MAX_DEPTH: usize = 1000;

#[derive(Clone, Debug, PartialEq)]
pub enum Js {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    /// A unit string.
    Str(String),
    Arr(Vec<Js>),
    Obj(Obj),
}

static UNDEFINED: Js = Js::Undefined;

impl Js {
    pub fn str(s: &str) -> Js {
        Js::Str(s.to_string())
    }

    /// The value of a property, or Undefined (for a missing key, or when this isn't an
    /// object), like `value[key]` on parsed JSON.
    pub fn get(&self, key: &str) -> &Js {
        match self {
            Js::Obj(o) => o.get(key).unwrap_or(&UNDEFINED),
            _ => &UNDEFINED,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Js::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Js::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Js::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Js::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&Vec<Js>> {
        match self {
            Js::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// `v && typeof v === 'object' && !Array.isArray(v)`
    pub fn is_plain_object(&self) -> bool {
        matches!(self, Js::Obj(_))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Js::Undefined)
    }

    /// `Number.isInteger(v)`
    pub fn is_integer(&self) -> bool {
        matches!(self, Js::Num(n) if n.is_finite() && n.trunc() == *n)
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Js::Undefined | Js::Null => false,
            Js::Bool(b) => *b,
            Js::Num(n) => *n != 0.0 && !n.is_nan(),
            Js::Str(s) => !s.is_empty(),
            Js::Arr(_) | Js::Obj(_) => true,
        }
    }
}

impl From<&str> for Js {
    fn from(s: &str) -> Js {
        Js::Str(s.to_string())
    }
}

impl From<String> for Js {
    fn from(s: String) -> Js {
        Js::Str(s)
    }
}

impl From<f64> for Js {
    fn from(n: f64) -> Js {
        Js::Num(n)
    }
}

impl From<bool> for Js {
    fn from(b: bool) -> Js {
        Js::Bool(b)
    }
}

/// A JS object: properties in JS order (see the module comment).
#[derive(Clone, Debug, Default)]
pub struct Obj {
    entries: Vec<(String, Js)>,
    index: HashMap<String, usize>,
}

impl PartialEq for Obj {
    fn eq(&self, other: &Obj) -> bool {
        self.iter().eq(other.iter())
    }
}

/// Whether a key is an array index (`"0"`, `"42"`, but not `"01"` or `"4294967295"`):
/// JS lists such keys first, in numeric order.
pub fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || !key.bytes().all(|b| b.is_ascii_digit()) || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n < 4_294_967_295).then_some(n as u32)
}

impl Obj {
    pub fn new() -> Obj {
        Obj::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Js> {
        self.index.get(key).map(|i| &self.entries[*i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Js> {
        let i = *self.index.get(key)?;
        Some(&mut self.entries[i].1)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// Sets a property: a new key goes last (in its class), an existing one keeps its place.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Js>) {
        let key = key.into();
        let value = value.into();
        if let Some(i) = self.index.get(&key) {
            self.entries[*i].1 = value;
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push((key, value));
        }
    }

    /// Builder form of [`Obj::set`].
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Js>) -> Obj {
        self.set(key, value);
        self
    }

    pub fn remove(&mut self, key: &str) -> Option<Js> {
        let i = self.index.remove(key)?;
        let (_, v) = self.entries.remove(i);
        for (_, at) in self.index.iter_mut() {
            if *at > i {
                *at -= 1;
            }
        }
        Some(v)
    }

    /// The properties in JS order: `Object.entries(o)`.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Js)> {
        let mut ints: Vec<(u32, usize)> = Vec::new();
        let mut rest: Vec<usize> = Vec::with_capacity(self.entries.len());
        for (i, (k, _)) in self.entries.iter().enumerate() {
            match array_index(k) {
                Some(n) => ints.push((n, i)),
                None => rest.push(i),
            }
        }
        ints.sort_unstable();
        ints.into_iter().map(|(_, i)| i).chain(rest).map(move |i| (&self.entries[i].0, &self.entries[i].1))
    }

    /// `Object.keys(o)`
    pub fn keys(&self) -> Vec<&String> {
        self.iter().map(|(k, _)| k).collect()
    }
}

impl From<Obj> for Js {
    fn from(o: Obj) -> Js {
        Js::Obj(o)
    }
}

impl From<Vec<Js>> for Js {
    fn from(a: Vec<Js>) -> Js {
        Js::Arr(a)
    }
}

impl<T: Into<Js>> From<Option<T>> for Js {
    fn from(v: Option<T>) -> Js {
        v.map(Into::into).unwrap_or(Js::Undefined)
    }
}

// ------------------------------------------------------------------ parse

/// `JSON.parse(text)`. The error says what was wrong and where, in Rust's words.
pub fn parse(text: &str) -> Result<Js, String> {
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

    fn value(&mut self, depth: usize) -> Result<Js, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end of JSON input")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Js::Str(self.string()?)),
            Some(b't') => self.literal("true", Js::Bool(true)),
            Some(b'f') => self.literal("false", Js::Bool(false)),
            Some(b'n') => self.literal("null", Js::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected token")),
        }
    }

    fn literal(&mut self, word: &str, v: Js) -> Result<Js, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected token"))
        }
    }

    fn number(&mut self) -> Result<Js, String> {
        let start = self.i;
        let digits = |p: &mut Parser| {
            let s = p.i;
            while p.i < p.s.len() && p.s[p.i].is_ascii_digit() {
                p.i += 1;
            }
            p.i - s
        };
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.err("no number after the minus sign")),
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if digits(self) == 0 {
                return Err(self.err("unterminated fractional number"));
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return Err(self.err("exponent part is missing a number"));
            }
        }
        let lit = &self.text[start..self.i];
        lit.parse::<f64>().map(Js::Num).map_err(|_| self.err("bad number"))
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("bad Unicode escape"))?;
        let h = std::str::from_utf8(h).map_err(|_| self.err("bad Unicode escape"))?;
        let v = u16::from_str_radix(h, 16).map_err(|_| self.err("bad Unicode escape"))?;
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.err("bad Unicode escape"));
        }
        self.i += 4;
        Ok(v)
    }

    /// A string, as a unit string.
    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            // Copy a run of plain characters at once.
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' && self.s[self.i] >= 0x20 {
                self.i += 1;
            }
            if self.i > start {
                let run = &self.text[start..self.i];
                if run.is_ascii() {
                    out.push_str(run);
                } else {
                    for c in run.chars() {
                        jsstr::push_real(&mut out, c);
                    }
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

    fn array(&mut self, depth: usize) -> Result<Js, String> {
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Js::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value(depth + 1)?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Js::Arr(out));
                }
                _ => return Err(self.err("expected ',' or ']' after an array element")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Js, String> {
        self.i += 1;
        let mut out = Obj::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Js::Obj(out));
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
                    return Ok(Js::Obj(out));
                }
                _ => return Err(self.err("expected ',' or '}' after a property value")),
            }
        }
    }
}

// ------------------------------------------------------------------ stringify

/// `JSON.stringify(v)`, as real UTF-8 text.
pub fn stringify(v: &Js) -> String {
    let mut out = String::new();
    write(&mut out, v, None, 0);
    out
}

/// `JSON.stringify(v, null, indent)`.
pub fn stringify_pretty(v: &Js, indent: &str) -> String {
    let mut out = String::new();
    write(&mut out, v, Some(indent), 0);
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

fn write(out: &mut String, v: &Js, indent: Option<&str>, depth: usize) {
    match v {
        Js::Undefined | Js::Null => out.push_str("null"),
        Js::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Js::Num(n) => {
            if n.is_finite() {
                out.push_str(&number_to_string(*n));
            } else {
                out.push_str("null");
            }
        }
        Js::Str(s) => quote_into(out, s),
        Js::Arr(a) => {
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
        Js::Obj(o) => {
            let mut first = true;
            for (k, x) in o.iter() {
                if x.is_undefined() {
                    continue;
                }
                out.push(if first { '{' } else { ',' });
                first = false;
                newline(out, indent, depth + 1);
                quote_into(out, k);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write(out, x, indent, depth + 1);
            }
            if first {
                out.push_str("{}");
            } else {
                newline(out, indent, depth);
                out.push('}');
            }
        }
    }
}

/// `JSON.stringify(s)` for a unit string: a pair is written as its character, a lone
/// surrogate as a lower-case `\udxxx` escape.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    quote_into(&mut out, s);
    out
}

fn quote_into(out: &mut String, s: &str) {
    out.push('"');
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if jsstr::is_high(c) && i + 1 < chars.len() && jsstr::is_low(chars[i + 1]) => {
                let hi = jsstr::char_unit(c).unwrap() as u32;
                let lo = jsstr::char_unit(chars[i + 1]).unwrap() as u32;
                out.push(char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)).unwrap());
                i += 1;
            }
            c if jsstr::is_high(c) || jsstr::is_low(c) => out.push_str(&format!("\\u{:04x}", jsstr::char_unit(c).unwrap())),
            c => out.push(c),
        }
        i += 1;
    }
    out.push('"');
}

/// `Number.prototype.toString()` (radix 10) for a finite number: the shortest digits that
/// read back as the same double, laid out by the ECMAScript rules (plain up to 21 digits
/// before the point, `0.000001` down to 6 zeros after it, otherwise `d.ddde±n`).
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // Rust's `{:e}` writes the shortest round-tripping digits: "1.2345e-7", "5e0".
    let e = format!("{:e}", x.abs());
    let (mant, exp) = e.split_once('e').expect("{:e} has an exponent");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().expect("an integer exponent") + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let es = if e >= 0 { format!("+{e}") } else { e.to_string() };
        if k == 1 {
            format!("{digits}e{es}")
        } else {
            format!("{}.{}e{es}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        for (x, want) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (12345678901234567890.0, "12345678901234567000"),
            (0.1 + 0.2, "0.30000000000000004"),
            (-2.5e30, "-2.5e+30"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(number_to_string(x), want, "{x}");
        }
    }

    #[test]
    fn parse_keeps_js_semantics() {
        let v = parse(r#"{"b":1,"2":"two","1":"one","b":3,"__proto__":{"x":1},"s":"\ud800😀"}"#).unwrap();
        let o = v.as_obj().unwrap();
        assert_eq!(o.keys(), ["1", "2", "b", "__proto__", "s"]);
        assert_eq!(v.get("b"), &Js::Num(3.0));
        assert_eq!(stringify(&v), r#"{"1":"one","2":"two","b":3,"__proto__":{"x":1},"s":"\ud800😀"}"#);
        for bad in ["", "{", "[1,]", "{\"a\":1,}", "\u{feff}{}", "01", "1.", "-", "\"\t\"", "nul", "[] x", "{'a':1}"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(MAX_DEPTH + 5) + &"]".repeat(MAX_DEPTH + 5);
        assert!(parse(&deep).is_err());
        assert_eq!(parse(" [ 1e400 , -0 ] ").map(|v| stringify(&v)).unwrap(), "[null,0]");
    }

    #[test]
    fn pretty_matches_json_stringify_with_two_spaces() {
        let v = parse(r#"{"a":[],"b":{},"c":[1,{"d":null}],"e":"x"}"#).unwrap();
        assert_eq!(stringify_pretty(&v, "  "), "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": null\n    }\n  ],\n  \"e\": \"x\"\n}");
        let mut o = Obj::new();
        o.set("u", Js::Undefined);
        o.set("n", 1.0);
        assert_eq!(stringify(&Js::Obj(o)), r#"{"n":1}"#);
        assert_eq!(quote("\u{1}\u{1f}\"\\\u{8}"), r#""\u0001\u001f\"\\\b""#);
    }
}
