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

/// Bellwether's own pets (plan D8), in the order the setup window lists them. Their names
/// are reserved: `pets import` refuses them, so an imported file is never shadowed by a
/// bundled pet or mistaken for one.
pub const BUNDLED: [&str; 2] = ["bellwether", "brah"];

/// The pet the strip draws when config.json has no `pets.default`.
pub const DEFAULT_PET: &str = "bellwether";

/// Whether `name` is, or reads as, a bundled pet's name: compared without case (APFS folds
/// case, so `Brah` and `brah` are one folder) and without trailing dots or spaces
/// (`bellwether.` reads as the sheep; M10a review D4).
pub fn reserved(name: &str) -> bool {
    let core = name.trim_end_matches(['.', ' ']);
    BUNDLED.iter().any(|b| b.eq_ignore_ascii_case(core))
}

/// The pets.lock.json compiled into this binary: the same pins the strip checks.
pub const LOCK: &str = include_str!("../../../pets.lock.json");

/// A bundled pet's pinned sha256, by its exact name; None when this version doesn't
/// bundle it or the name isn't exact.
pub fn bundled_sha(name: &str) -> Option<String> {
    if !BUNDLED.contains(&name) {
        return None;
    }
    let lock = json::parse(LOCK).ok()?;
    lock.get("bundled").get(name).get("sha256").as_str().map(str::to_string)
}

/// Where this binary's bundled pet file is: the app's `Contents/Resources/pets/<name>/`,
/// or, for a build that isn't in an app bundle (a repository's dev build), the checkout's
/// `pets/<name>/`, as the strip finds them.
pub fn bundled_file(name: &str) -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let contents = exe.as_ref().and_then(|e| e.parent()).filter(|d| d.file_name().is_some_and(|n| n == "MacOS")).and_then(Path::parent).filter(|c| c.file_name().is_some_and(|n| n == "Contents"));
    match contents {
        Some(c) => c.join("Resources").join("pets").join(name).join("animations.xml"),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("pets").join(name).join("animations.xml"),
    }
}

/// What doctor says about `pets.default` when it names (or nearly names) a bundled pet,
/// by the strip's own rules: the exact name, pinned in this version, its file matching
/// the pin. None for any other name. The Node doctor says the same (brain/pets.mjs).
pub fn bundled_check(name: &str) -> Option<(bool, String)> {
    if !reserved(name) {
        return None;
    }
    let canon = BUNDLED.iter().find(|b| b.eq_ignore_ascii_case(name.trim_end_matches(['.', ' ']))).copied().unwrap_or("bellwether");
    if name != canon {
        return Some((false, format!("pets.default is {name:?}, which the strip won't load: pet names are exact. Did you mean \"{canon}\"?")));
    }
    let Some(want) = bundled_sha(name) else {
        return Some((false, format!("{name} is reserved for one of Bellwether's own pets, which isn't bundled in this version, so the strip draws nothing. Set pets.default to \"bellwether\" (or remove it) in config.json")));
    };
    let file = bundled_file(name);
    Some(match std::fs::read(&file) {
        Err(_) => (false, format!("{name}, one of Bellwether's own pets, is missing its file ({}): reinstall Bellwether", file.display())),
        Ok(bytes) if sha256_hex(&bytes) != want => (false, format!("{name}, one of Bellwether's own pets: {} doesn't match its pinned sha256, so the strip refuses it: reinstall Bellwether", file.display())),
        Ok(_) => (true, format!("{name}, Bellwether's own pet, bundled; sha256 matches its pin")),
    })
}

/// bellwether's behaviours, and the animation names that stand for each, best first
/// (the first are the behaviour's own name; then esheep64's names for it). The last two,
/// `carried` and `land`, are for the planned drag along the strip; nothing plays them yet,
/// and a pet without them falls back to walk (the player's `idsFor`).
pub const BEHAVIOURS: [(&str, &[&str]); 16] = [
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
    ("carried", &["carried"]),
    ("land", &["land"]),
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

/// Unicode 16's format characters (general category Cf): soft hyphen, bidi marks,
/// overrides and isolates, zero-width space, joiners, word joiner, invisible operators, the
/// byte-order mark, interlinear annotation, the tag characters and the rest.
const FORMAT: &[(u32, u32)] = &[
    (0x00AD, 0x00AD), (0x0600, 0x0605), (0x061C, 0x061C), (0x06DD, 0x06DD), (0x070F, 0x070F),
    (0x0890, 0x0891), (0x08E2, 0x08E2), (0x180E, 0x180E), (0x200B, 0x200F), (0x202A, 0x202E),
    (0x2060, 0x2064), (0x2066, 0x206F), (0xFEFF, 0xFEFF), (0xFFF9, 0xFFFB), (0x110BD, 0x110BD),
    (0x110CD, 0x110CD), (0x13430, 0x1343F), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001), (0xE0020, 0xE007F),
];

/// Characters that draw as nothing but aren't Cf: the combining grapheme joiner, the Hangul
/// fillers, the Khmer inherent vowels, the braille blank and the unassigned rest of the tag
/// block.
const INVISIBLE: &[(u32, u32)] = &[
    (0x034F, 0x034F), (0x115F, 0x1160), (0x17B4, 0x17B5), (0x2800, 0x2800), (0x3164, 0x3164),
    (0xFFA0, 0xFFA0), (0xE0000, 0xE0000), (0xE0002, 0xE001F),
];

/// Whether `c` must not reach a terminal from an imported file: controls (Cc), format
/// characters (Cf), the line and paragraph separators (Zl, Zp), and the invisibles above.
pub fn unprintable(c: char) -> bool {
    let n = c as u32;
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || FORMAT.iter().chain(INVISIBLE).any(|(lo, hi)| (*lo..=*hi).contains(&n))
}

/// Text from an imported file made safe to print: every `unprintable` character becomes
/// `?`, so a title can't drive the terminal or hide what it says.
pub fn printable(s: &str) -> String {
    s.chars().map(|c| if unprintable(c) { '?' } else { c }).collect()
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

/// What the onboarding window needs to draw a still of a pet: its sheet (the base64 PNG),
/// the tile grid, the first frame of its first animation, and the colour keyed out.
#[derive(Debug, PartialEq, Eq)]
pub struct Still {
    pub title: String,
    pub png: String,
    pub tilesx: u32,
    pub tilesy: u32,
    pub frame: u32,
    pub transparency: String,
}

/// A still of a pet file that passed [`check`] (or a pinned bundled pet).
pub fn still(bytes: &[u8]) -> Result<Still, String> {
    let pet = check(bytes)?;
    let root = parse_xml(std::str::from_utf8(bytes).map_err(|e| e.to_string())?)?;
    let image = root.child("image").ok_or("it has no <image>")?;
    let png: String = image.text_of("png").unwrap_or_default().chars().filter(|c| !c.is_whitespace()).collect();
    let first = root.child("animations").and_then(|l| l.children.iter().find(|c| c.name == "animation"));
    let frame = first
        .and_then(|a| a.child("sequence"))
        .and_then(|s| s.children.iter().find(|c| c.name == "frame"))
        .and_then(|f| f.text.trim().parse::<u32>().ok())
        .filter(|f| *f < pet.tilesx * pet.tilesy)
        .unwrap_or(0);
    Ok(Still { title: printable(&pet.title), png, tilesx: pet.tilesx, tilesy: pet.tilesy, frame, transparency: printable(&image.text_of("transparency").unwrap_or_default()) })
}

/// The names in `<home>/pets.user.lock.json`, in its order.
pub fn imported_names(home: &Path) -> Vec<String> {
    read_lock(home).keys().iter().map(|k| k.to_string()).filter(|n| valid_name(n)).collect()
}

fn read_lock(home: &Path) -> Obj {
    std::fs::read_to_string(lock_file(home)).ok().and_then(|t| json::parse(&t).ok()).and_then(|v| v.get("pets").as_obj().cloned()).unwrap_or_default()
}

/// Whether an imported pet still matches its pin.
pub fn verify(home: &Path, name: &str) -> Result<(), String> {
    load(home, name).map(|_| ())
}

/// An imported pet's file, read once and checked against its pin: what the strip loads
/// (milestone 5), so the bytes checked are the bytes used.
pub fn load(home: &Path, name: &str) -> Result<Vec<u8>, String> {
    let file = pet_file(home, name);
    let lock = read_lock(home);
    let Some(want) = lock.get(name).and_then(|e| e.get("sha256").as_str().map(str::to_string)) else {
        return Err(format!("{} has no pin in {}: import it again with `bellwether pets import {} --force`", file.display(), lock_file(home).display(), file.display()));
    };
    let bytes = std::fs::read(&file).map_err(|e| format!("{} can't be read ({e})", file.display()))?;
    if sha256_hex(&bytes) != want {
        return Err(format!("{} changed after it was imported (its sha256 no longer matches {}), so the strip won't load it. Import the file again: `bellwether pets import <file> --name {name} --force`", file.display(), lock_file(home).display()));
    }
    Ok(bytes)
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
    if reserved(&name) {
        return Err(bad(format!("{name:?} is the name of one of Bellwether's own pets ({}), so an imported pet can't use it: it would be shadowed by the bundled one, or mistaken for it. Import it under another name with --name <name>. Nothing was imported.", BUNDLED.join(", "))));
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
        // The M4 review's follow-up: the Arabic letter mark, the zero-width space and the
        // byte-order mark are neutralised too; ordinary text, accents and emoji are kept.
        assert_eq!(printable("a\u{061C}b\u{200B}c\u{FEFF}d\u{2067}e\u{200F}f"), "a?b?c?d?e?f");
        assert_eq!(printable("Schäfchen 🐑 x"), "Schäfchen 🐑 x");
        // BR-3K's M5 list, and the category rule behind it: Cc, Cf, Zl, Zp and the invisibles.
        for c in ['\u{2028}', '\u{2029}', '\u{2060}', '\u{00AD}', '\u{180E}', '\u{200C}', '\u{200D}', '\u{034F}', '\u{2800}', '\u{E0000}', '\u{E0001}', '\u{E0041}', '\u{E007F}', '\u{0085}', '\u{009B}', '\u{FFF9}', '\u{1D173}', '\u{3164}'] {
            assert_eq!(printable(&format!("a{c}b")), "a?b", "U+{:04X}", c as u32);
        }
        assert_eq!(printable("A B\u{E0041}C"), "A B?C", "a tag character can't hide ASCII");
        for c in ['é', 'ß', '中', 'ا', '\u{0301}', '\u{FE0F}', '\u{00A0}', '\u{3000}', '\u{E0080}'] {
            assert!(!unprintable(c), "U+{:04X} is visible", c as u32);
        }
        let p = check(pet("", "<tilesx>8</tilesx><tilesy>4</tilesy>").replace("T &amp; co", "&#x61C;&#x200B;&#xFEFF;ok").as_bytes()).unwrap();
        assert_eq!(printable(&p.title), "???ok");
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
    fn the_bundled_names_are_refused_and_nothing_is_written() {
        let home = std::env::temp_dir().join(format!("bw-pets-reserved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let src = home.join("src.xml");
        std::fs::write(&src, pet("", "<tilesx>8</tilesx><tilesy>4</tilesy>")).unwrap();
        for name in ["bellwether", "brah", "Brah", "BELLWETHER", "bellwether.", "brah.."] {
            let e = import(&home, &src, Some(name), true).err().unwrap_or_else(|| panic!("{name} was imported"));
            assert!(e.message.contains("one of Bellwether's own pets") && e.message.contains("Nothing was imported"), "{}", e.message);
        }
        // A trailing space is refused too (a space is never part of a pet's name).
        for name in ["bellwether ", "Brah. "] {
            assert!(import(&home, &src, Some(name), true).is_err(), "{name:?} was imported");
            assert!(reserved(name), "{name:?} reads as a bundled name");
        }
        // A file in a folder named after one (…/brah/animations.xml) is refused too.
        let dir = home.join("brah");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&src, dir.join("animations.xml")).unwrap();
        assert!(import(&home, &dir.join("animations.xml"), None, false).is_err());
        assert!(!home.join("pets").exists() && !lock_file(&home).exists(), "nothing was written");
        assert!(import(&home, &src, Some("brahma"), false).is_ok(), "only the bundled names are reserved");
        assert!(import(&home, &src, Some("bellwether.x"), false).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn doctor_checks_a_bundled_name_as_the_strip_loads_it() {
        let (ok, why) = bundled_check("bellwether").unwrap();
        assert!(ok, "{why}");
        assert!(why.contains("sha256 matches its pin"));
        let (ok, why) = bundled_check("brah").unwrap();
        assert!(ok && why.contains("sha256 matches its pin"), "the goose is bundled since M10b: {why}");
        for name in ["BELLWETHER", "Bellwether", "bellwether.", "BRAH"] {
            let (ok, why) = bundled_check(name).unwrap();
            assert!(!ok && why.contains("pet names are exact") && why.contains("Did you mean"), "{name}: {why}");
        }
        assert_eq!(bundled_check("esheep64"), None);
        assert_eq!(bundled_sha("BELLWETHER"), None, "pins are by exact name");
        assert!(bundled_sha("bellwether").is_some());
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
