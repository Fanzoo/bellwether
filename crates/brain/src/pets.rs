//! `bellwether pets import <animations.xml>` (distribution plan D8): a desktopPet pet file
//! of the user's own, checked, copied to `<home>/pets/<name>/animations.xml` and pinned by
//! its sha256 in `<home>/pets.user.lock.json`. The strip loads an imported pet only while
//! the file still matches its pin, so a file changed after import is refused.
//!
//! The checks, before anything is written: at most 16 MiB; UTF-8; no DOCTYPE or ENTITY
//! (pet files never need one, and refusing them rules out entity expansion); well-formed
//! XML whose root is `<animations>`; an `<image>` with `tilesx` and `tilesy` from 1 to 64
//! (the player's limits) and a base64 PNG; at least one `<animation>` with an integer id.
//! It also proposes `pets.animationMap.<name>` by matching animation names to bellwether's
//! behaviours. It never edits config.json.

use std::path::{Path, PathBuf};

use bellwether_core::hooks::{sha256_hex, write_replace};
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::persist::write_atomic;

use crate::{exit, BrainError};

pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// bellwether's behaviours, and the animation names that stand for each, best first
/// (the first are the behaviour's own name; then esheep64's names for it).
pub const BEHAVIOURS: [(&str, &[&str]); 14] = [
    ("walk", &["walk"]),
    ("run", &["run"]),
    ("graze", &["graze", "eat"]),
    ("sleep", &["sleep", "sleep1a", "sleep1b"]),
    ("alert", &["alert", "boing"]),
    ("turn", &["turn", "rotate1a"]),
    ("trip", &["trip", "fall soft"]),
    ("sit", &["sit", "sync"]),
    ("dizzy", &["dizzy", "sleep2a"]),
    ("chew", &["chew", "eat"]),
    ("arrive", &["arrive", "walk"]),
    ("leave", &["leave", "run"]),
    ("hop", &["hop", "jump"]),
    ("lie", &["lie", "sleep3a"]),
];

pub fn lock_file(home: &Path) -> PathBuf {
    home.join("pets.user.lock.json")
}

pub fn pet_file(home: &Path, name: &str) -> PathBuf {
    home.join("pets").join(name).join("animations.xml")
}

fn bad(m: impl Into<String>) -> BrainError {
    BrainError::new(m, exit::BAD_CONFIG)
}

/// A pet's name is a directory name: 1 to 40 of `[A-Za-z0-9_.-]`, not starting with a dot.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len()) && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The name a file is imported under when none is given: its directory's name for an
/// `animations.xml` (`…/esheep64/animations.xml` is esheep64), else the file's stem.
pub fn default_name(file: &Path) -> String {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.eq_ignore_ascii_case("animations") {
        if let Some(dir) = file.parent().and_then(Path::file_name) {
            return dir.to_string_lossy().into_owned();
        }
    }
    stem
}

// ------------------------------------------------------------------ a small XML reader

/// An element: its name (without a prefix), attributes, children and text.
#[derive(Debug, Default)]
pub struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<El>,
    pub text: String,
}

impl El {
    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn text_of(&self, name: &str) -> Option<String> {
        self.child(name).map(|c| c.text.trim().to_string())
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// How deeply elements may nest. A pet file needs five levels; a deeper one is refused
/// before it can exhaust the stack (BR-3K's M4 finding D4: a million nested elements).
pub const MAX_XML_DEPTH: usize = 64;

/// Parses well-formed XML (elements, attributes, text, comments, CDATA, processing
/// instructions; the five predefined entities and character references). A DOCTYPE is
/// refused before this runs. Nesting is capped at [`MAX_XML_DEPTH`]; duplicate attributes
/// are found with a set, so the work is linear in the file.
pub fn parse_xml(text: &str) -> Result<El, String> {
    let s = text.as_bytes();
    let mut i = 0;
    let mut stack: Vec<El> = Vec::new();
    let mut root: Option<El> = None;
    let line = |at: usize| text[..at.min(text.len())].matches('\n').count() + 1;
    let skip_ws = |i: &mut usize| {
        while *i < s.len() && matches!(s[*i], b' ' | b'\t' | b'\r' | b'\n') {
            *i += 1;
        }
    };
    let name_end = |mut j: usize| {
        while j < s.len() && !matches!(s[j], b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>' | b'=') {
            j += 1;
        }
        j
    };
    if text.starts_with('\u{feff}') {
        i = 3;
    }
    while i < s.len() {
        if s[i] == b'<' {
            if text[i..].starts_with("<!--") {
                let end = text[i + 4..].find("-->").ok_or_else(|| format!("an unclosed comment at line {}", line(i)))?;
                i += 4 + end + 3;
            } else if text[i..].starts_with("<![CDATA[") {
                let end = text[i + 9..].find("]]>").ok_or_else(|| format!("an unclosed CDATA section at line {}", line(i)))?;
                let Some(top) = stack.last_mut() else { return Err(format!("text outside the root element at line {}", line(i))) };
                top.text.push_str(&text[i + 9..i + 9 + end]);
                i += 9 + end + 3;
            } else if text[i..].starts_with("<?") {
                let end = text[i + 2..].find("?>").ok_or_else(|| format!("an unclosed processing instruction at line {}", line(i)))?;
                i += 2 + end + 2;
            } else if text[i..].starts_with("<!") {
                return Err(format!("a markup declaration (<!…) at line {}", line(i)));
            } else if text[i..].starts_with("</") {
                let start = i + 2;
                let end = name_end(start);
                let name = &text[start..end];
                i = end;
                skip_ws(&mut i);
                if s.get(i) != Some(&b'>') {
                    return Err(format!("a malformed end tag at line {}", line(i)));
                }
                i += 1;
                let el = stack.pop().ok_or_else(|| format!("</{name}> without a start tag at line {}", line(i)))?;
                if el.name != local(name) {
                    return Err(format!("</{name}> closes <{}> at line {}", el.name, line(i)));
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => {
                        if root.is_some() {
                            return Err("more than one root element".into());
                        }
                        root = Some(el);
                    }
                }
            } else {
                let start = i + 1;
                let end = name_end(start);
                if end == start {
                    return Err(format!("a malformed tag at line {}", line(i)));
                }
                let mut el = El { name: local(&text[start..end]), ..El::default() };
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(format!("elements nested more than {MAX_XML_DEPTH} deep at line {}", line(i)));
                }
                let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
                i = end;
                loop {
                    skip_ws(&mut i);
                    match s.get(i) {
                        Some(b'>') => {
                            i += 1;
                            if root.is_some() && stack.is_empty() {
                                return Err("more than one root element".into());
                            }
                            stack.push(el);
                            break;
                        }
                        Some(b'/') if s.get(i + 1) == Some(&b'>') => {
                            i += 2;
                            match stack.last_mut() {
                                Some(parent) => parent.children.push(el),
                                None => {
                                    if root.is_some() {
                                        return Err("more than one root element".into());
                                    }
                                    root = Some(el);
                                }
                            }
                            break;
                        }
                        Some(_) => {
                            let a = i;
                            let ae = name_end(a);
                            if ae == a {
                                return Err(format!("a malformed attribute at line {}", line(i)));
                            }
                            let key = local(&text[a..ae]);
                            i = ae;
                            skip_ws(&mut i);
                            if s.get(i) != Some(&b'=') {
                                return Err(format!("an attribute without a value at line {}", line(i)));
                            }
                            i += 1;
                            skip_ws(&mut i);
                            let q = *s.get(i).ok_or("the file ends inside a tag")?;
                            if q != b'"' && q != b'\'' {
                                return Err(format!("an unquoted attribute value at line {}", line(i)));
                            }
                            let close = text[i + 1..].find(q as char).ok_or_else(|| format!("an unclosed attribute value at line {}", line(i)))?;
                            let raw = &text[i + 1..i + 1 + close];
                            if raw.contains('<') {
                                return Err(format!("'<' in an attribute value at line {}", line(i)));
                            }
                            if !names.insert(key.clone()) {
                                return Err(format!("attribute {key} twice at line {}", line(i)));
                            }
                            el.attrs.push((key, unescape(raw).map_err(|e| format!("{e} at line {}", line(i)))?));
                            i += 1 + close + 1;
                        }
                        None => return Err("the file ends inside a tag".into()),
                    }
                }
            }
        } else {
            let next = text[i..].find('<').map(|n| i + n).unwrap_or(s.len());
            let chunk = &text[i..next];
            match stack.last_mut() {
                Some(top) => top.text.push_str(&unescape(chunk).map_err(|e| format!("{e} at line {}", line(i)))?),
                None if chunk.trim().is_empty() => {}
                None => return Err(format!("text outside the root element at line {}", line(i))),
            }
            i = next;
        }
    }
    if let Some(open) = stack.last() {
        return Err(format!("<{}> is never closed", open.name));
    }
    root.ok_or_else(|| "no root element".into())
}

fn unescape(raw: &str) -> Result<String, String> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(';').ok_or("an unterminated '&' reference")?;
        let name = &rest[at + 1..at + end];
        let c = match name {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32).ok_or("a bad character reference")?,
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32).ok_or("a bad character reference")?,
            _ => return Err(format!("an unknown entity &{name};")),
        };
        out.push(c);
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

// ------------------------------------------------------------------ the pet

/// What an import found in the file.
#[derive(Debug)]
pub struct Pet {
    pub title: String,
    pub tilesx: u32,
    pub tilesy: u32,
    /// Animation names, in file order.
    pub animations: Vec<String>,
}

/// Every check D8 names, on the file's bytes.
pub fn check(bytes: &[u8]) -> Result<Pet, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("it is {} bytes; a pet file may be at most 16 MiB", bytes.len()));
    }
    let text = std::str::from_utf8(bytes).map_err(|e| format!("it isn't UTF-8 ({e})"))?;
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return Err("it declares a DOCTYPE or ENTITY, which pet files never need".into());
    }
    let root = parse_xml(text).map_err(|e| format!("it isn't well-formed XML: {e}"))?;
    if root.name != "animations" {
        return Err(format!("the root element is <{}>, not <animations>: not a desktopPet pet file", root.name));
    }
    let image = root.child("image").ok_or("it has no <image>")?;
    let tile = |n: &str| -> Result<u32, String> {
        let t = image.text_of(n).unwrap_or_default();
        t.parse::<u32>().ok().filter(|v| (1..=64).contains(v)).ok_or_else(|| format!("<{n}> is {t:?}; it must be an integer from 1 to 64"))
    };
    let (tilesx, tilesy) = (tile("tilesx")?, tile("tilesy")?);
    let png: String = image.text_of("png").unwrap_or_default().chars().filter(|c| !c.is_whitespace()).collect();
    let head = base64_prefix(&png, 8).ok_or("<png> isn't base64")?;
    if head != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Err("<png> doesn't hold a PNG image".into());
    }
    let mut animations = Vec::new();
    if let Some(list) = root.child("animations") {
        for a in list.children.iter().filter(|c| c.name == "animation") {
            if a.attr("id").and_then(|v| v.trim().parse::<i64>().ok()).is_some() {
                animations.push(a.text_of("name").unwrap_or_default());
            }
        }
    }
    if animations.is_empty() {
        return Err("it has no <animation> with an integer id".into());
    }
    let title = root.child("header").and_then(|h| h.text_of("title")).unwrap_or_default();
    Ok(Pet { title, tilesx, tilesy, animations })
}

/// The first `n` bytes a base64 text decodes to.
fn base64_prefix(text: &str, n: usize) -> Option<Vec<u8>> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for b in text.bytes() {
        if b == b'=' {
            break;
        }
        let v = A.iter().position(|x| *x == b)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
            if out.len() == n {
                return Some(out);
            }
        }
    }
    (out.len() >= n).then_some(out)
}

/// Text from an imported file made safe to print: control characters (C0, DEL, C1) and
/// bidirectional overrides become `?`, so a title can't drive the terminal.
pub fn printable(s: &str) -> String {
    s.chars().map(|c| if c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}') { '?' } else { c }).collect()
}

/// `pets.animationMap.<name>` by name matching: for each behaviour, the first of its names
/// the pet has (case-insensitive); behaviours with none are left out.
pub fn propose_map(pet: &Pet) -> Obj {
    let mut map = Obj::new();
    for (behaviour, names) in BEHAVIOURS {
        let found: Vec<&String> = names.iter().filter_map(|n| pet.animations.iter().find(|a| a.eq_ignore_ascii_case(n))).collect();
        let Some(first) = found.first() else { continue };
        // sleep takes every matching sleep animation, as esheep64's map does.
        if behaviour == "sleep" && found.len() > 1 {
            let mut all: Vec<Js> = Vec::new();
            for f in &found {
                if !all.iter().any(|x| x.as_str() == Some(f.as_str())) {
                    all.push(Js::Str((*f).clone()));
                }
            }
            map.set(behaviour, Js::Arr(all));
        } else {
            map.set(behaviour, Js::Str((*first).clone()));
        }
    }
    map
}

fn read_lock(home: &Path) -> Obj {
    std::fs::read_to_string(lock_file(home)).ok().and_then(|t| json::parse(&t).ok()).and_then(|v| v.get("pets").as_obj().cloned()).unwrap_or_default()
}

/// Whether an imported pet still matches its pin.
pub fn verify(home: &Path, name: &str) -> Result<(), String> {
    let file = pet_file(home, name);
    let lock = read_lock(home);
    let Some(want) = lock.get(name).and_then(|e| e.get("sha256").as_str().map(str::to_string)) else {
        return Err(format!("{} has no pin in {}: import it again with `bellwether pets import {} --force`", file.display(), lock_file(home).display(), file.display()));
    };
    let bytes = std::fs::read(&file).map_err(|e| format!("{} can't be read ({e})", file.display()))?;
    if sha256_hex(&bytes) != want {
        return Err(format!("{} changed after it was imported (its sha256 no longer matches {}), so the strip won't load it. Import the file again: `bellwether pets import <file> --name {name} --force`", file.display(), lock_file(home).display()));
    }
    Ok(())
}

/// What `pets import` did.
pub struct Imported {
    pub name: String,
    pub file: PathBuf,
    pub sha256: String,
    pub pet: Pet,
    pub map: Obj,
    pub replaced: bool,
}

/// Checks `source`, then copies it and pins it. Nothing is written when a check fails.
pub fn import(home: &Path, source: &Path, name: Option<&str>, force: bool) -> Result<Imported, BrainError> {
    let meta = std::fs::metadata(source).map_err(|e| bad(format!("{}: {}", source.display(), bellwether_core::config::io_code(&e))))?;
    if !meta.is_file() {
        return Err(bad(format!("{} is not a file", source.display())));
    }
    if meta.len() > MAX_BYTES {
        return Err(bad(format!("{} was refused: it is {} bytes; a pet file may be at most 16 MiB. Nothing was imported.", source.display(), meta.len())));
    }
    let name = name.map(str::to_string).unwrap_or_else(|| default_name(source));
    if !valid_name(&name) {
        return Err(bad(format!("{name:?} can't be a pet's name: use 1 to 40 of [A-Za-z0-9_.-], not starting with a dot (--name <name>)")));
    }
    let bytes = std::fs::read(source).map_err(|e| bad(format!("{}: {}", source.display(), bellwether_core::config::io_code(&e))))?;
    let pet = check(&bytes).map_err(|why| bad(format!("{} was refused: {why}. Nothing was imported.", source.display())))?;
    let target = pet_file(home, &name);
    let replaced = target.exists();
    if replaced && !force {
        return Err(bad(format!("a pet named {name} is already imported ({}); pass --force to replace it, or --name <another name>", target.display())));
    }
    let dir = target.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| BrainError::new(format!("can't create {} ({e})", dir.display()), exit::UNEXPECTED))?;
    let tmp = dir.join(format!(".animations.xml.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_replace(&tmp, &bytes, 0o644).and_then(|_| std::fs::rename(&tmp, &target)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        BrainError::new(format!("can't write {} ({e})", target.display()), exit::UNEXPECTED)
    })?;
    let sha256 = sha256_hex(&bytes);
    let mut lock = read_lock(home);
    lock.set(name.clone(), Js::Obj(Obj::new().with("sha256", sha256.as_str()).with("bytes", bytes.len() as f64)));
    let text = json::stringify_pretty(&Js::Obj(Obj::new().with("pets", Js::Obj(lock))), "  ") + "\n";
    let lf = lock_file(home);
    write_atomic(&lf, &home.join(format!(".pets.user.lock.json.{}.tmp", std::process::id())), &text, 0o644).map_err(|e| BrainError::new(format!("can't write {} ({e}); the pet was copied but isn't pinned, so the strip won't load it: run the import again", lf.display()), exit::UNEXPECTED))?;
    let map = propose_map(&pet);
    Ok(Imported { name, file: target, sha256, pet, map, replaced })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn pet(extra: &str, tiles: &str) -> String {
        format!("<?xml version=\"1.0\"?>\n<animations xmlns=\"http://x\"><header><title>T &amp; co</title></header><image>{tiles}<png>{PNG}</png></image><animations><animation id=\"1\"><name>walk</name></animation><animation id='2'><name>Eat</name></animation><animation id=\"3\"><name>sleep1a</name></animation><animation id=\"4\"><name>sleep1b</name></animation>{extra}</animations></animations>")
    }

    #[test]
    fn a_good_pet_passes_and_gets_a_proposed_map() {
        let p = check(pet("", "<tilesx>8</tilesx><tilesy>4</tilesy>").as_bytes()).unwrap();
        assert_eq!((p.title.as_str(), p.tilesx, p.tilesy), ("T & co", 8, 4));
        let m = propose_map(&p);
        assert_eq!(json::stringify(&Js::Obj(m)), r#"{"walk":"walk","graze":"Eat","sleep":["sleep1a","sleep1b"],"chew":"Eat","arrive":"walk"}"#);
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        let good = "<tilesx>8</tilesx><tilesy>4</tilesy>";
        for (text, why) in [
            (pet("", "<tilesx>65</tilesx><tilesy>4</tilesy>"), "from 1 to 64"),
            (pet("", "<tilesx>x</tilesx><tilesy>4</tilesy>"), "from 1 to 64"),
            (pet("<animation>", good), "well-formed"),
            (format!("<!DOCTYPE x [<!ENTITY a \"b\">]>{}", pet("", good)), "DOCTYPE"),
            ("<pet/>".to_string(), "not <animations>"),
            (pet("", good).replace(PNG, "AAAAAAAAAAAA"), "PNG image"),
            (pet("", good).replace("&amp;", "&bogus;"), "unknown entity"),
            ("<animations><image><tilesx>1</tilesx><tilesy>1</tilesy><png>iVBORw0KGgo=</png></image></animations>".to_string(), "no <animation>"),
        ] {
            let e = check(text.as_bytes()).err().unwrap_or_else(|| panic!("accepted: {text}"));
            assert!(e.contains(why), "{why}: {e}");
        }
        assert!(check(&[0xff, 0xfe]).unwrap_err().contains("UTF-8"));
        assert!(check(&vec![b' '; (MAX_BYTES + 1) as usize]).unwrap_err().contains("16 MiB"));
    }

    #[test]
    fn hostile_files_are_refused_quickly_and_nothing_prints_raw() {
        let deep = format!("<animations>{}{}</animations>", "<a>".repeat(1_000_000), "</a>".repeat(1_000_000));
        assert!(check(deep.as_bytes()).unwrap_err().contains("nested more than"));
        let attrs: String = (0..100_000).map(|i| format!(" a{i}=\"x\"")).collect();
        let t = std::time::Instant::now();
        let wide = pet(&format!("<animation id=\"9\"{attrs}><name>w</name></animation>"), "<tilesx>8</tilesx><tilesy>4</tilesy>");
        assert!(check(wide.as_bytes()).is_ok());
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?} for 100 000 attributes", t.elapsed());
        assert!(check(pet("<animation id=\"9\" a=\"1\" a=\"2\"/>", "<tilesx>8</tilesx><tilesy>4</tilesy>").as_bytes()).unwrap_err().contains("twice"));
        let p = check(pet("", "<tilesx>8</tilesx><tilesy>4</tilesy>").replace("T &amp; co", "&#27;]0;owned&#7;\u{202e}x").as_bytes()).unwrap();
        assert_eq!(printable(&p.title), "?]0;owned??x");
    }

    #[test]
    fn names() {
        assert_eq!(default_name(Path::new("/x/esheep64/animations.xml")), "esheep64");
        assert_eq!(default_name(Path::new("/x/my-pet.xml")), "my-pet");
        for n in ["esheep64", "a.b-c_1"] {
            assert!(valid_name(n), "{n}");
        }
        for n in ["", ".hidden", "a/b", "..", &"x".repeat(41)] {
            assert!(!valid_name(n), "{n}");
        }
    }

    #[test]
    fn import_pins_and_a_changed_file_is_refused() {
        let home = std::env::temp_dir().join(format!("bw-pets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let src = home.join("src.xml");
        std::fs::write(&src, pet("", "<tilesx>8</tilesx><tilesy>4</tilesy>")).unwrap();
        let r = import(&home, &src, Some("mine"), false).unwrap();
        assert!(!r.replaced);
        assert!(verify(&home, "mine").is_ok());
        assert!(import(&home, &src, Some("mine"), false).is_err(), "an existing pet needs --force");
        std::fs::write(pet_file(&home, "mine"), "changed").unwrap();
        assert!(verify(&home, "mine").unwrap_err().contains("changed after it was imported"));
        assert!(import(&home, &src, Some("mine"), true).unwrap().replaced);
        assert!(verify(&home, "mine").is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }
}
