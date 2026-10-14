//! The shared vector files (distribution plan D2), answered by the port: every case's
//! answer must be the Node brain's, which `expected` holds (test/fixtures/build-vectors.mjs
//! made them from brain/*.mjs, and the Node tests check them there).

use std::path::PathBuf;

use bellwether_core::json::{self, Js};
use bellwether_core::normalise::normalise;
use bellwether_core::redact::Cleaner;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

fn vectors(name: &str) -> Vec<Js> {
    let path = fixtures().join(format!("{name}-vectors.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match json::parse(&text).unwrap().get("vectors") {
        Js::Arr(a) => a.clone(),
        _ => panic!("{name}: no vectors"),
    }
}

fn cleaner(o: &Js) -> Cleaner {
    let patterns: Vec<String> = o.get("redactPatterns").as_arr().map(|a| a.iter().map(|p| p.as_str().unwrap().to_string()).collect()).unwrap_or_default();
    Cleaner::new(o.get("maxChars").as_num().unwrap() as usize, &patterns).unwrap()
}

#[test]
fn redaction_answers_every_vector_as_node_does() {
    let all = vectors("redact");
    assert!(all.len() >= 160, "{} vectors", all.len());
    let mut wrong = Vec::new();
    for v in &all {
        let got = cleaner(v).clean(v.get("text").as_str().unwrap());
        if got != v.get("expected").as_str().unwrap() {
            wrong.push(format!("{} / {}: got {}, Node {}", v.get("group").as_str().unwrap(), v.get("name").as_str().unwrap(), json::quote(&got), json::quote(v.get("expected").as_str().unwrap())));
        }
        if let Some(secret) = v.get("mustNotContain").as_str() {
            assert!(!got.contains(secret), "{}: keeps {secret:?}", v.get("name").as_str().unwrap());
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_url_fix_is_pinned() {
    let all = vectors("redact");
    let fixed = all.iter().find(|v| v.get("name").as_str() == Some("URL credentials with odd characters")).unwrap();
    assert_eq!(fixed.get("expected").as_str(), Some("curl https://u%40x:•••@h.test/"));
    assert!(all.iter().filter(|v| v.get("group").as_str() == Some("URL credentials")).count() >= 12);
}

#[test]
fn normalising_answers_every_vector_as_node_does() {
    for v in vectors("normalise") {
        let clean = match v.get("clean") {
            Js::Obj(_) => Some(cleaner(v.get("clean"))),
            _ => None,
        };
        let got = normalise(v.get("payload"), v.get("headers").as_obj().unwrap(), v.get("receivedAt").as_num().unwrap(), clean.as_ref()).to_js();
        assert_eq!(json::stringify(&got), json::stringify(v.get("expected")), "{}", v.get("name").as_str().unwrap());
    }
}

/// Config loading answers every vector as Node does: the effective config byte for byte
/// (JSON, in JS key order), or the error and its exit code. For a pattern that doesn't
/// compile, the reason in brackets is the regex crate's, not V8's (test/DIVERGENCES.md).
#[test]
fn config_answers_every_vector_as_node_does() {
    use bellwether_core::config::{validate_config, with_defaults};
    for v in vectors("config") {
        let name = v.get("name").as_str().unwrap();
        let got = match v.get("step").as_str().unwrap() {
            "validateConfig" => validate_config(v.get("config"), "config").map(|_| None),
            _ => with_defaults(v.get("config")).map(Some),
        };
        match (v.get("ok").as_bool().unwrap(), got) {
            (true, Ok(merged)) => {
                if let Some(m) = merged {
                    assert_eq!(json::stringify(&m.config), json::stringify(v.get("effective")), "{name}");
                    assert_eq!(Some(m.palette_upgraded), v.get("paletteUpgraded").as_bool(), "{name}");
                }
            }
            (false, Err(e)) => {
                let want = v.get("error").as_str().unwrap();
                assert_eq!(e.code as f64, v.get("exitCode").as_num().unwrap(), "{name}");
                let marker = " is not a valid regular expression (";
                match want.find(marker) {
                    Some(at) => assert!(e.message.starts_with(&want[..at + marker.len()]) && e.message.ends_with(')'), "{name}: {}", e.message),
                    None => assert_eq!(e.message, want, "{name}"),
                }
            }
            (ok, got) => panic!("{name}: Node ok={ok}, Rust {:?}", got.map(|_| ()).map_err(|e| e.message)),
        }
    }
}
