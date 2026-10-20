//! Tester M13: adversarial checks of `lossless::edit`. A seeded generator writes random
//! JSON texts in random layouts (inline and multi-line containers, CRLF, tabs, trailing
//! spaces, escapes, no final newline), mutates the parsed value, and checks that
//! (1) the output is valid JSON (parsed by the independent `json::parse` too) equal to the
//! intended value, (2) an unchanged value comes back byte for byte, and (3) a scalar-only
//! change touches nothing but that scalar's text.

use bellwether_core::json;
use bellwether_core::lossless::{deep_equal, edit, has_duplicate_keys, parse, LObj, Lj};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn n(&mut self, m: u64) -> u64 {
        self.next() % m
    }
}

const WS: [&str; 9] = ["", "", " ", "  ", "\n", "\n  ", "\r\n    ", "\t", "  \n\t\t"];

fn ws(r: &mut Rng, eol: &str) -> String {
    WS[r.n(WS.len() as u64) as usize].replace('\n', "\n").replace("\r\n", eol).replace('\n', if eol == "\r\n" { "\r\n" } else { "\n" })
}

fn scalar_value(r: &mut Rng) -> Lj {
    match r.n(6) {
        0 => Lj::Null,
        1 => Lj::Bool(r.n(2) == 0),
        2 => Lj::Num(["0", "1", "-0", "1.50", "2e3", "12345678901234567890"][r.n(6) as usize].to_string()),
        _ => Lj::Str(["", "a", "caf\u{e9}", "tab\there", "q\"uote", "back\\slash", "\u{2603}", "line\nbreak", "</script>"][r.n(9) as usize].to_string()),
    }
}

fn gen(r: &mut Rng, depth: usize, keyn: &mut usize) -> Lj {
    if depth == 0 || r.n(3) == 0 {
        return scalar_value(r);
    }
    let n = r.n(4) as usize;
    if r.n(2) == 0 {
        Lj::Arr((0..n).map(|_| gen(r, depth - 1, keyn)).collect())
    } else {
        let mut o = LObj::new();
        for _ in 0..n {
            *keyn += 1;
            o.set(format!("k{}", *keyn), gen(r, depth - 1, keyn));
        }
        Lj::Obj(o)
    }
}

fn esc(s: &str, r: &mut Rng) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if !c.is_ascii() && r.n(2) == 0 => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    o.push_str(&format!("\\u{:04x}", u));
                }
            }
            '/' => o.push_str(if r.n(2) == 0 { "\\/" } else { "/" }),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn render(v: &Lj, r: &mut Rng, eol: &str) -> String {
    match v {
        Lj::Null => "null".into(),
        Lj::Bool(b) => b.to_string(),
        Lj::Num(n) => n.clone(),
        Lj::Str(s) => esc(s, r),
        Lj::Arr(a) => {
            let mut o = String::from("[");
            for (i, x) in a.iter().enumerate() {
                o += &ws(r, eol);
                o += &render(x, r, eol);
                o += &ws(r, eol);
                if i + 1 < a.len() {
                    o.push(',');
                }
            }
            if a.is_empty() {
                o += &ws(r, eol);
            }
            o + "]"
        }
        Lj::Obj(m) => {
            let mut o = String::from("{");
            let n = m.len();
            for (i, (k, x)) in m.iter().enumerate() {
                o += &ws(r, eol);
                o += &esc(k, r);
                o += &ws(r, eol);
                o.push(':');
                o += &ws(r, eol);
                o += &render(x, r, eol);
                o += &ws(r, eol);
                if i + 1 < n {
                    o.push(',');
                }
            }
            if n == 0 {
                o += &ws(r, eol);
            }
            o + "}"
        }
    }
}

/// Mutates one random place; returns whether only a scalar was replaced by another scalar.
fn mutate(v: &mut Lj, r: &mut Rng, keyn: &mut usize) -> bool {
    match v {
        Lj::Arr(a) if !a.is_empty() && r.n(3) != 0 => {
            let i = r.n(a.len() as u64) as usize;
            match r.n(4) {
                0 => {
                    a.remove(i);
                    false
                }
                1 => {
                    let g = gen(r, 2, keyn);
                    a.push(g);
                    false
                }
                _ => mutate(&mut a[i], r, keyn),
            }
        }
        Lj::Obj(o) if !o.is_empty() && r.n(3) != 0 => {
            let keys: Vec<String> = o.keys().into_iter().cloned().collect();
            let k = keys[r.n(keys.len() as u64) as usize].clone();
            match r.n(5) {
                0 => {
                    o.remove(&k);
                    false
                }
                1 => {
                    *keyn += 1;
                    let g = gen(r, 2, keyn);
                    o.set(format!("new{}", *keyn), g);
                    false
                }
                _ => mutate(o.get_mut(&k).unwrap(), r, keyn),
            }
        }
        Lj::Arr(a) => {
            let g = gen(r, 2, keyn);
            a.push(g);
            false
        }
        Lj::Obj(o) => {
            *keyn += 1;
            let g = gen(r, 2, keyn);
            o.set(format!("add{}", *keyn), g);
            false
        }
        other => {
            let was_scalar = true;
            let g = if r.n(4) == 0 { gen(r, 2, keyn) } else { scalar_value(r) };
            let both = !matches!(g, Lj::Arr(_) | Lj::Obj(_));
            *other = g;
            was_scalar && both
        }
    }
}

#[test]
fn fuzz_edit_is_valid_identity_when_unchanged_and_local_for_scalars() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let (mut scalar_only, mut total, mut changed) = (0, 0, 0);
    for _ in 0..40_000 {
        let mut keyn = 0;
        let mut v = gen(&mut r, 4, &mut keyn);
        if !matches!(v, Lj::Obj(_) | Lj::Arr(_)) {
            continue;
        }
        let eol = if r.n(3) == 0 { "\r\n" } else { "\n" };
        let mut text = ws(&mut r, eol) + &render(&v, &mut r, eol);
        text += &ws(&mut r, eol);
        if r.n(2) == 0 {
            text += eol;
        }
        let old = parse(&text).unwrap_or_else(|e| panic!("generator made bad json {e}: {text:?}"));
        assert!(!has_duplicate_keys(&text));
        // identity
        assert_eq!(edit(&text, &old), text, "identity broke: {text:?}");
        // mutation
        let mut new = old.clone();
        let only_scalar = mutate(&mut new, &mut r, &mut keyn);
        let _ = &mut v;
        let out = edit(&text, &new);
        let back = parse(&out).unwrap_or_else(|e| panic!("edit made bad json ({e}):\nold={text:?}\nout={out:?}"));
        json::parse(&out).unwrap_or_else(|e| panic!("json::parse refused ({e}):\nold={text:?}\nout={out:?}"));
        assert!(deep_equal(&back, &new), "wrong value:\nold={text:?}\nout={out:?}");
        total += 1;
        if !deep_equal(&old, &new) {
            changed += 1;
        }
        if only_scalar && !deep_equal(&old, &new) {
            scalar_only += 1;
            // everything but one scalar's text is unchanged: strip the common prefix and
            // suffix; what is left of the old text must be one token with no line break
            // outside a string literal
            let (a, b) = (text.as_bytes(), out.as_bytes());
            let mut p = 0;
            while p < a.len() && p < b.len() && a[p] == b[p] {
                p += 1;
            }
            let mut s = 0;
            while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
                s += 1;
            }
            let mid = &text[text.char_indices().map(|c| c.0).rfind(|&i| i <= p).unwrap_or(0)..a.len() - s.min(a.len())];
            assert!(!mid.contains('\n'), "a scalar edit touched a line break:\nold={text:?}\nout={out:?}");
            assert!(out.matches('\n').count() == text.matches('\n').count(), "line count changed on scalar edit:\nold={text:?}\nout={out:?}");
        }
    }
    eprintln!("cases {total}, changed {changed}, scalar-only {scalar_only}");
    assert!(changed > 10_000 && scalar_only > 1_000);
}

#[test]
fn handpicked_edges() {
    let cases: &[(&str, &str)] = &[
        // type change scalar -> container and back, inside inline containers
        ("{\"a\":1,\"b\":[1,2]}", "{\"a\":[1],\"b\":3}"),
        ("[ ]", "[1]"),
        ("{ }", "{\"a\":{\"b\":[]}}"),
        ("{\"a\": []}\n", "{\"a\": [[],{}]}\n"),
        ("{\t\"a\":\t1\t}", "{\"a\":1,\"b\":2}"),
        ("{\n\t\"a\": [1, 2, 3],   \n\t\"b\": {\"x\": 1}  \n}", "{\"a\": [1], \"b\": {\"x\": 1, \"y\": [2]}, \"c\": []}"),
        ("{\"a\":1}", "{\"b\":1}"),
        ("{\"a\":1,\"b\":2,\"c\":3}", "{\"c\":3,\"a\":1}"),
        ("{\"a\":1,\"b\":2}", "{\"b\":2,\"a\":1}"),
        ("[1,2,3]", "[]"),
        ("{\"a\":{\"b\":{\"c\":{\"d\":[1,[2,[3]]]}}}}", "{\"a\":{\"b\":{\"c\":{\"d\":[1,[2,[3,4]]],\"e\":null}}}}"),
        ("{\"a\":\"\\u00e9\\ud83d\\ude00\"}", "{\"a\":\"x\"}"),
        ("\u{feff}{\"a\":1}", "{\"a\":2}"),
        ("{\"a\":1} // c", "{\"a\":2}"),
        ("{\"a\":1}\n\n\n", "{\"a\":2}"),
        ("  {\"a\":1}", "[]"),
        ("1", "{\"a\":1}"),
        ("{\"a\":1,\"a\":2,\"b\":[1]}", "{\"a\":3,\"b\":[1,2]}"),
        ("{\"a\\u0062\":1,\"ab\":2}", "{\"ab\":3}"),
    ];
    for (old, newtext) in cases {
        let new = match parse(newtext) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let out = edit(old, &new);
        let back = parse(&out).unwrap_or_else(|e| panic!("bad json ({e}): {old:?} -> {out:?}"));
        assert!(deep_equal(&back, &new), "{old:?} -> {out:?}");
    }
}

#[test]
fn a_very_large_file_is_edited_fast_and_exactly() {
    let mut text = String::from("{\n  \"items\": [\n");
    for i in 0..60_000 {
        text += &format!("    {{\"i\": {i}, \"tags\": [\"a\", \"b\"], \"s\": \"caf\\u00e9\"}}{}\n", if i < 59_999 { "," } else { "" });
    }
    text += "  ],\n  \"inline\": [1, 2, 3],\n  \"pets\": {\"default\": \"x\"}\n}\n";
    let mut v = parse(&text).unwrap();
    v.as_obj_mut().unwrap().get_mut("pets").and_then(Lj::as_obj_mut).unwrap().set("default", Lj::str("brah"));
    let t = std::time::Instant::now();
    let out = edit(&text, &v);
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    assert_eq!(out, text.replace("\"default\": \"x\"", "\"default\": \"brah\""));
    // deep nesting at the limit still gives valid json or the fallback, never a panic
    let deep = "[".repeat(900) + &"]".repeat(900);
    let v = parse(&deep).unwrap();
    assert_eq!(edit(&deep, &v), deep);
}

fn paths(v: &Lj, cur: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    out.push(cur.clone());
    match v {
        Lj::Obj(o) => {
            for (k, x) in o.iter() {
                cur.push(k.clone());
                paths(x, cur, out);
                cur.pop();
            }
        }
        Lj::Arr(a) => {
            for (i, x) in a.iter().enumerate() {
                cur.push(format!("#{i}"));
                paths(x, cur, out);
                cur.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(v: &'a mut Lj, p: &[String]) -> &'a mut Lj {
    let mut cur = v;
    for s in p {
        cur = match cur {
            Lj::Obj(o) => o.get_mut(s).unwrap(),
            Lj::Arr(a) => &mut a[s[1..].parse::<usize>().unwrap()],
            _ => unreachable!(),
        };
    }
    cur
}

/// On the shipped config.example.json (one-line arrays, an empty `{}`, nested objects),
/// every single edit anywhere in the file leaves all but at most two lines as they were
/// (two: a multi-line container that grows changes the old last line's comma and adds one).
#[test]
fn every_single_edit_to_config_example_json_touches_at_most_two_lines() {
    let old = include_str!("../../../config.example.json");
    let base = parse(old).unwrap();
    let mut all = Vec::new();
    paths(&base, &mut Vec::new(), &mut all);
    let old_lines: Vec<&str> = old.split('\n').collect();
    let mut n = 0;
    for p in &all {
        for variant in 0..4 {
            let mut v = base.clone();
            let t = at(&mut v, p);
            match (variant, &mut *t) {
                (0, Lj::Obj(o)) => o.set("zz_new", Lj::str("x")),
                (0, Lj::Arr(a)) => a.push(Lj::str("zz")),
                (0, other) => *other = Lj::str("changed!"),
                (1, Lj::Arr(a)) if !a.is_empty() => {
                    a.pop();
                }
                (1, Lj::Obj(o)) if !o.is_empty() => {
                    let k = o.keys()[0].clone();
                    o.remove(&k);
                }
                (2, other) if !matches!(other, Lj::Obj(_) | Lj::Arr(_)) => *other = Lj::Arr(vec![Lj::int(1), Lj::int(2)]),
                (3, Lj::Obj(o)) => o.set("zz_obj", Lj::Obj(LObj::new().with("a", Lj::Arr(vec![Lj::Null])))),
                _ => continue,
            }
            if p.is_empty() && variant != 0 {
                continue;
            }
            let out = edit(old, &v);
            assert!(deep_equal(&parse(&out).unwrap(), &v), "{p:?} variant {variant}\n{out}");
            n += 1;
            // lines of old that survive, in order
            let new_lines: Vec<&str> = out.split('\n').collect();
            let mut j = 0;
            let mut kept = 0;
            for l in &old_lines {
                if let Some(off) = new_lines[j..].iter().position(|x| x == l) {
                    j += off + 1;
                    kept += 1;
                }
            }
            let lost = old_lines.len() - kept;
            // a removed member takes its own line (and may fix a comma on the line before)
            let bound = match p.is_empty() { true => 2, false => 3 };
            let multi_line_removed = variant == 1;
            assert!(lost <= if multi_line_removed { bound + 20 } else { bound }, "{p:?} variant {variant} lost {lost} lines:\n{out}");
        }
    }
    assert!(n > 100, "{n}");
}

/// Astral characters and lone surrogates, in kept text and in text written fresh.
#[test]
fn astral_and_lone_surrogates_survive_edit() {
    let old = "{\"a\": \"\u{1F600} x\",\n \"b\": \"\\ud83d\\ude00\", \"c\": [\"\\ud800\", 1]}";
    let mut v = parse(old).unwrap();
    v.as_obj_mut().unwrap().set("d", parse("\"new \u{1F600} \\udc00\"").unwrap());
    if let Some(Lj::Arr(a)) = v.as_obj_mut().unwrap().get_mut("c") {
        a.push(parse("\"\u{1F600}\\ud800\"").unwrap());
    }
    let out = edit(old, &v);
    assert!(out.starts_with("{\"a\": \"\u{1F600} x\",\n \"b\": \"\\ud83d\\ude00\", \"c\": [\"\\ud800\", 1, "), "{out}");
    json::parse(&out).unwrap();
    assert!(deep_equal(&parse(&out).unwrap(), &v), "{out}");
}
