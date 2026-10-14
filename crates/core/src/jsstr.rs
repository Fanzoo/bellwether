//! JavaScript strings, held in a Rust `String`.
//!
//! A JS string is a sequence of UTF-16 code units, and it may hold a lone surrogate (a
//! hook payload with `"\ud800"` in it, or a cut through a surrogate pair). The Node brain
//! counts, slices, bounds and sorts by code unit. To give the same answers, the port keeps
//! every JS string as a *unit string*: one Rust `char` per UTF-16 code unit.
//!
//! - a code unit that is not a surrogate is that character (all of the BMP but
//!   U+D800–U+DFFF);
//! - a surrogate code unit U+D800 + k becomes the private-use character U+F0000 + k
//!   (U+F0000–U+F07FF), so a pair is two characters and a lone surrogate is one.
//!
//! A unit string therefore never holds a character above U+FFFF except these 2048, and
//! `chars().count()` is the JS `length`. A regular expression run over a unit string sees
//! what a JS regular expression without the `u` flag sees: `[^\s]{1,200}` counts code
//! units, and an emoji is two of them. Real text goes in through [`from_str`] and comes
//! out through [`to_string_lossy`] (a lone surrogate becomes U+FFFD, as it does whenever
//! Node writes UTF-8) or [`crate::json::stringify`] (a lone surrogate becomes `\udxxx`,
//! as `JSON.stringify` writes it).

use std::cmp::Ordering;

/// The first character that stands for a surrogate code unit.
pub const SURROGATE_BASE: u32 = 0xF0000;
const SURROGATE_LAST: u32 = SURROGATE_BASE + 0x7FF;

/// The character that stands for one UTF-16 code unit.
pub fn unit_char(u: u16) -> char {
    let u = u as u32;
    if (0xD800..=0xDFFF).contains(&u) {
        char::from_u32(SURROGATE_BASE + (u - 0xD800)).expect("private use plane 15")
    } else {
        char::from_u32(u).expect("a BMP scalar value")
    }
}

/// The code unit a unit-string character stands for. A character outside the unit
/// domain (a real astral character, which a unit string never holds) gives None.
pub fn char_unit(c: char) -> Option<u16> {
    let c = c as u32;
    if c < 0x10000 {
        Some(c as u16)
    } else if (SURROGATE_BASE..=SURROGATE_LAST).contains(&c) {
        Some((0xD800 + (c - SURROGATE_BASE)) as u16)
    } else {
        None
    }
}

/// Whether a unit-string character stands for a high (leading) surrogate.
pub fn is_high(c: char) -> bool {
    (SURROGATE_BASE..SURROGATE_BASE + 0x400).contains(&(c as u32))
}

/// Whether a unit-string character stands for a low (trailing) surrogate.
pub fn is_low(c: char) -> bool {
    (SURROGATE_BASE + 0x400..=SURROGATE_LAST).contains(&(c as u32))
}

/// Real text as a unit string: each astral character becomes its two surrogates.
pub fn from_str(s: &str) -> String {
    if s.chars().all(|c| (c as u32) < 0x10000) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        push_real(&mut out, c);
    }
    out
}

/// Appends one real character to a unit string.
pub fn push_real(out: &mut String, c: char) {
    if (c as u32) < 0x10000 {
        out.push(c);
    } else {
        let mut buf = [0u16; 2];
        for u in c.encode_utf16(&mut buf) {
            out.push(unit_char(*u));
        }
    }
}

/// A unit string from UTF-16 code units.
pub fn from_units(units: &[u16]) -> String {
    units.iter().map(|u| unit_char(*u)).collect()
}

/// The UTF-16 code units of a unit string.
pub fn to_units(s: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match char_unit(c) {
            Some(u) => out.push(u),
            None => {
                let mut buf = [0u16; 2];
                out.extend_from_slice(c.encode_utf16(&mut buf));
            }
        }
    }
    out
}

/// One code point of a unit string, as `[...s]` iterates it: a pair is one, and a lone
/// surrogate is one of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePoint {
    Char(char),
    Lone(u16),
}

/// The code points of a unit string, in order.
pub fn code_points(s: &str) -> Vec<CodePoint> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_high(c) && i + 1 < chars.len() && is_low(chars[i + 1]) {
            let hi = char_unit(c).unwrap() as u32;
            let lo = char_unit(chars[i + 1]).unwrap() as u32;
            out.push(CodePoint::Char(char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)).unwrap()));
            i += 2;
        } else if is_high(c) || is_low(c) {
            out.push(CodePoint::Lone(char_unit(c).unwrap()));
            i += 1;
        } else {
            out.push(CodePoint::Char(c));
            i += 1;
        }
    }
    out
}

/// Code points back into a unit string.
pub fn from_code_points(cps: &[CodePoint]) -> String {
    let mut out = String::new();
    for cp in cps {
        match cp {
            CodePoint::Char(c) => push_real(&mut out, *c),
            CodePoint::Lone(u) => out.push(unit_char(*u)),
        }
    }
    out
}

/// A unit string as real text: pairs become their character, a lone surrogate U+FFFD.
pub fn to_string_lossy(s: &str) -> String {
    if !s.chars().any(|c| (c as u32) >= 0x10000) {
        return s.to_string();
    }
    code_points(s)
        .into_iter()
        .map(|cp| match cp {
            CodePoint::Char(c) => c,
            CodePoint::Lone(_) => '\u{FFFD}',
        })
        .collect()
}

/// The JS `length`: code units.
pub fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s.slice(0, n)` in code units.
pub fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// JS string comparison (`<`, and `Array.prototype.sort` without a comparator): by
/// code unit. A mapped surrogate sorts below U+E000–U+FFFF, as the unit it stands for.
pub fn cmp(a: &str, b: &str) -> Ordering {
    let mut x = a.chars().map(|c| char_unit(c).unwrap_or(0xFFFF));
    let mut y = b.chars().map(|c| char_unit(c).unwrap_or(0xFFFF));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p != q => return p.cmp(&q),
            _ => {}
        }
    }
}

/// `Buffer.byteLength(s)`: UTF-8 bytes, with a lone surrogate written as U+FFFD (3).
pub fn utf8_len(s: &str) -> usize {
    code_points(s)
        .into_iter()
        .map(|cp| match cp {
            CodePoint::Char(c) => c.len_utf8(),
            CodePoint::Lone(_) => 3,
        })
        .sum()
}

/// The UTF-8 bytes Node writes for a JS string (`Buffer.from(s)`, a hash's `update(s)`).
pub fn utf8_bytes(s: &str) -> Vec<u8> {
    to_string_lossy(s).into_bytes()
}

/// JavaScript's `\s` (and what `trim` removes): WhiteSpace and LineTerminator. U+0085 is
/// not in it, and U+FEFF is.
pub fn is_js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

/// The same set, for the inside of a regular-expression class.
pub const JS_SPACE_CLASS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// `s.trim()`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_characters_are_two_units_and_lone_surrogates_survive() {
        let s = from_str("a😀b");
        assert_eq!(len(&s), 4);
        assert_eq!(to_units(&s), vec![0x61, 0xD83D, 0xDE00, 0x62]);
        assert_eq!(to_string_lossy(&s), "a😀b");
        let lone = from_units(&[0x61, 0xD800, 0x62]);
        assert_eq!(len(&lone), 3);
        assert_eq!(to_string_lossy(&lone), "a\u{FFFD}b");
        assert_eq!(utf8_len(&lone), 5);
        assert_eq!(code_points(&s).len(), 3);
        assert_eq!(prefix(&s, 2), from_units(&[0x61, 0xD83D]));
    }

    #[test]
    fn comparison_is_by_code_unit() {
        // U+FF01 sorts above a surrogate pair by code unit (0xD83D), below it by scalar value.
        assert_eq!(cmp(&from_str("\u{FF01}"), &from_str("😀")), Ordering::Greater);
        assert_eq!(cmp("10", "9"), Ordering::Less);
        assert_eq!(cmp("a", "ab"), Ordering::Less);
    }

    #[test]
    fn js_whitespace() {
        assert!(is_js_space('\u{FEFF}') && is_js_space('\u{A0}') && !is_js_space('\u{85}') && !is_js_space('\u{180E}'));
        assert_eq!(trim("\u{FEFF} a \u{2028}"), "a");
    }
}
