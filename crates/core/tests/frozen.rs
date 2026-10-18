//! The frozen sample of the differential (distribution plan, milestone 11, M11-f): 2 000
//! generated cases per area, each with the answer the Node brain gave, written by
//! `scripts/differential.mjs --freeze` before the Node brain was deleted (its last version
//! is tag v0.2.0-rc.3). Only cases where the Rust brain already gave the same bytes went
//! in, so every one of them must still be answered the same way: the redaction and
//! normaliser areas keep the differential's reach (R1) as a fixed regression set.
//!
//! Each area is answered as the conformance build's `__<area>` command answers it
//! (shell/src-tauri/src/conformance.rs), with the crates/core functions it calls.

use std::collections::HashMap;
use std::path::PathBuf;

use bellwether_core::json::{self, Js, Obj};
use bellwether_core::redact::Cleaner;
use bellwether_core::{diff, envfile, jsstr, lossless, normalise};

fn frozen(area: &str) -> (Js, Vec<Js>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../test/fixtures/frozen/{area}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file = json::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(file.get("area").as_str(), Some(area));
    let cases = file.get("cases").as_arr().cloned().unwrap_or_else(|| panic!("{area}: no cases"));
    (file, cases)
}

/// Every case through `answer`; the answers that differ from Node's, at most 5 shown.
fn check(area: &str, at_least: usize, mut answer: impl FnMut(&Js) -> Js) {
    let (file, cases) = frozen(area);
    assert!(cases.len() >= at_least, "{area}: {} cases, expected at least {at_least}", cases.len());
    let skipped: f64 = file.get("skipped").as_obj().map(|o| o.iter().map(|(_, n)| n.as_num().unwrap_or(0.0)).sum()).unwrap_or(0.0);
    assert_eq!(cases.len() as f64 + skipped, file.get("generated").as_num().unwrap(), "{area}: kept and skipped add up to the cases generated");
    let mut wrong = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let got = json::stringify(&answer(c.get("case")));
        let want = json::stringify(c.get("expected"));
        if got != want {
            wrong.push(format!("case {i}: {}\n  Node: {}\n  Rust: {}", json::stringify(c.get("case")).chars().take(300).collect::<String>(), want.chars().take(300).collect::<String>(), got.chars().take(300).collect::<String>()));
        }
    }
    assert!(wrong.is_empty(), "{area}: {} of {} frozen answers differ:\n{}", wrong.len(), cases.len(), wrong.iter().take(5).cloned().collect::<Vec<_>>().join("\n"));
}

/// makeTextCleaner's options as a case gives them, one cleaner per distinct set.
fn cleaner<'a>(o: &Js, cache: &'a mut HashMap<String, Result<Cleaner, String>>) -> &'a Result<Cleaner, String> {
    cache.entry(json::stringify(o)).or_insert_with(|| {
        let max = o.get("maxChars").as_num().filter(|n| *n >= 0.0).unwrap_or(60.0) as usize;
        let patterns: Vec<String> = match o.get("redactPatterns") {
            Js::Arr(a) => a.iter().map(|p| p.as_str().unwrap_or("").to_string()).collect(),
            _ => Vec::new(),
        };
        Cleaner::new(max, &patterns).map_err(|e| e.to_string())
    })
}

#[test]
fn env_answers_the_frozen_sample_as_node_did() {
    check("env", 2000, |c| {
        let key = c.get("key").as_str().unwrap_or("BELLWETHER_HOOK_TOKEN");
        let value = envfile::env_value(c.get("text").as_str().unwrap(), key).map(Js::Str).unwrap_or(Js::Null);
        Js::Obj(Obj::new().with("value", value))
    });
}

#[test]
fn redaction_answers_the_frozen_sample_as_node_did() {
    let mut cache = HashMap::new();
    check("redact", 2000, |c| match cleaner(c, &mut cache) {
        Ok(cl) => Js::Obj(Obj::new().with("value", cl.clean(c.get("text").as_str().unwrap()))),
        Err(message) => Js::Obj(Obj::new().with("error", true).with("message", message.clone())),
    });
}

#[test]
fn normalising_answers_the_frozen_sample_as_node_did() {
    let mut cache = HashMap::new();
    check("normalise", 2000, |c| {
        let headers = c.get("headers").as_obj().cloned().unwrap_or_default();
        let received_at = c.get("receivedAt").as_num().unwrap_or(f64::NAN);
        let clean = match c.get("clean") {
            Js::Obj(_) => Some(cleaner(c.get("clean"), &mut cache).as_ref().expect("a frozen case's cleaner compiles")),
            _ => None,
        };
        normalise::normalise(c.get("payload"), &headers, received_at, clean).to_js()
    });
}

#[test]
fn settings_json_round_trips_the_frozen_sample_as_node_did() {
    check("json", 1500, |c| {
        let text = jsstr::to_string_lossy(c.get("text").as_str().unwrap());
        match lossless::parse(&text) {
            Ok(v) => Js::Obj(Obj::new().with("value", jsstr::from_str(&lossless::serialise(&v, &text)))),
            Err(e) => Js::Obj(Obj::new().with("error", true).with("message", e)),
        }
    });
}

#[test]
fn diffs_answer_the_frozen_sample_as_node_did() {
    check("diff", 2000, |c| {
        let s = |k: &str| jsstr::to_string_lossy(c.get(k).as_str().unwrap_or(""));
        let (old, new) = (s("old"), s("new"));
        let o = c.get("options");
        let name = |k: &str, d: &str| o.get(k).as_str().map(jsstr::to_string_lossy).unwrap_or_else(|| d.to_string());
        let context = o.get("context").as_num().map(|n| n.max(0.0) as usize).unwrap_or(3);
        let stats = diff::diff_stats(&old, &new);
        Js::Obj(
            Obj::new()
                .with("value", jsstr::from_str(&diff::unified_diff(&old, &new, &name("oldName", "a"), &name("newName", "b"), context)))
                .with("added", stats.added as f64)
                .with("removed", stats.removed as f64),
        )
    });
}
