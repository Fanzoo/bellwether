//! The shared vector files (distribution plan D2), read from the Rust side. The Node
//! tests check every answer in them against brain/*.mjs; test/fixtures/build-vectors.mjs
//! makes them.
//!
//! This checks that each file is complete and readable: every case has what the port
//! needs to answer it and the Node answer to compare with. The answers themselves are
//! compared where each module is ported: redaction, normalising and config in
//! crates/core/tests/vectors.rs (milestone 3); the installer and the diff in milestone 4.
//! The `.env` vectors: crates/core's envfile.rs and the shell's settings.rs.

use std::collections::HashSet;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

fn vectors(name: &str) -> Vec<serde_json::Value> {
    let path = fixtures().join(format!("{name}-vectors.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(json["comment"].as_str().is_some_and(|c| c.contains("build-vectors.mjs")), "{name}: no comment naming its generator");
    json["vectors"].as_array().unwrap_or_else(|| panic!("{name}: no vectors")).clone()
}

/// Every vector has these keys, and (group, name) is unique, so a failure can name it.
fn shape(name: &str, keys: &[&str]) -> Vec<serde_json::Value> {
    let all = vectors(name);
    assert!(!all.is_empty(), "{name}: empty");
    let mut seen = HashSet::new();
    for v in &all {
        for k in keys {
            assert!(v.get(*k).is_some(), "{name}: {} has no {k:?}", v["name"]);
        }
        let id = format!("{} / {}", v["group"], v["name"]);
        assert!(seen.insert(id.clone()), "{name}: {id} twice");
    }
    all
}

#[test]
fn redact_vectors_are_complete() {
    let all = shape("redact", &["group", "name", "text", "maxChars", "redactPatterns", "expected"]);
    assert!(all.len() >= 140, "{} redaction vectors", all.len());
    for v in &all {
        let expected = v["expected"].as_str().unwrap();
        assert!(expected.chars().count() <= v["maxChars"].as_u64().unwrap() as usize, "{}: longer than maxChars", v["name"]);
        if let Some(secret) = v.get("mustNotContain").and_then(|s| s.as_str()) {
            assert!(!expected.contains(secret), "{}: the Node answer keeps {secret:?}", v["name"]);
        }
    }
    // crates/core/tests/vectors.rs compares the cleaner with every `expected` here.
}

#[test]
fn normalise_vectors_are_complete() {
    let all = shape("normalise", &["group", "name", "payload", "headers", "receivedAt", "clean", "expected"]);
    for v in &all {
        assert!(v["expected"]["event"].is_string(), "{}: no event", v["name"]);
        assert!(v["expected"]["terminal"].is_object(), "{}: no terminal", v["name"]);
    }
    // crates/core/tests/vectors.rs compares the normaliser with every `expected` here.
}

#[test]
fn config_vectors_are_complete() {
    let all = shape("config", &["group", "name", "step", "config", "ok"]);
    for v in &all {
        match v["ok"].as_bool() {
            Some(true) if v["step"] == "withDefaults" => assert!(v["effective"].is_object(), "{}", v["name"]),
            Some(true) => {}
            _ => {
                assert_eq!(v["exitCode"], 3, "{}: config errors are exit 3", v["name"]);
                assert!(v["error"].is_string(), "{}", v["name"]);
            }
        }
    }
    // crates/core/tests/vectors.rs compares the config loading with these.
}

#[test]
fn diff_vectors_are_complete() {
    let all = shape("diff", &["group", "name", "old", "new", "options", "expected", "stats"]);
    for v in &all {
        let text = v["expected"].as_str().unwrap();
        assert_eq!(text.is_empty(), v["stats"]["added"] == 0 && v["stats"]["removed"] == 0, "{}", v["name"]);
    }
    // Milestone 4 compares the ported diff with every `expected` here.
}

#[test]
fn installer_vectors_are_complete() {
    let dir = fixtures().join("installer-vectors");
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    assert_ne!(meta["port"], 47417, "never the live port");
    let cases = meta["cases"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        let d = dir.join(c["dir"].as_str().unwrap());
        let read = |f: &str| std::fs::read(d.join(f)).unwrap_or_else(|e| panic!("{}/{f}: {e}", d.display()));
        let (settings, installed, uninstalled) = (read("settings.json"), read("installed.json"), read("uninstalled.json"));
        let (_, _) = (read("install.diff"), read("uninstall.diff"));
        assert_ne!(settings, installed, "{}: install changed nothing", c["name"]);
        assert_eq!(c["roundTrip"].as_bool().unwrap(), settings == uninstalled, "{}: roundTrip doesn't match the bytes", c["name"]);
    }
    // The CRLF case must still have its CRLFs (.gitattributes keeps git from converting them).
    let crlf = std::fs::read(dir.join("4-spaces-crlf").join("settings.json")).unwrap();
    assert!(crlf.windows(2).any(|w| w == b"\r\n"), "4-spaces-crlf lost its CRLF line endings");
    // Milestone 4 runs the ported installer over every case: installed.json and
    // install.diff byte for byte, and uninstalled.json after an uninstall.
}
