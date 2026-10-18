//! Hidden `__` subcommands for the conformance harness (distribution plan D2), compiled
//! only with the `conformance` feature, which no release build has.
//!
//! Each one reads cases as JSON lines on stdin and writes one JSON line per case on
//! stdout, in order, so many cases go through one process (`scripts/differential.mjs`
//! compared 10⁵ generated cases a run with the Node functions this way until milestone 11;
//! its frozen sample is crates/core/tests/frozen.rs). A case that can't be read gets
//! `{"error": "…"}` on its line (a difference, never a crash).
//!
//!   __list        the areas and whether each is ported yet (JSON, no input)
//!   __env         {"text", "key"?} -> {"value": string|null}: a .env read the way
//!                 `node --env-file` does (crates/core envfile.rs)
//!   __redact      milestone 3: {"text", "maxChars", "redactPatterns"} -> {"value"}
//!   __normalise   milestone 3: {"payload", "headers", "receivedAt", "clean"} -> the HookEvent
//!   __fixture     milestone 3: a fixture's JSON lines -> its transition lines
//!   __session     the test bridge (test/impl.mjs): one JSON call per line, answered in
//!                 order, over two FIFOs (`--in <path> --out <path>`) so a Node test can
//!                 call the Rust brain's pure parts synchronously: the cleaner, the
//!                 normaliser, the config, a Flock or a publisher by handle, and so on
//!   __json        milestone 4: {"text"} -> {"value"}: a settings file through the lossless
//!                 JSON and back in its own layout (`serialise(parseLossless(text), text)`),
//!                 or {"error": true, "message"} when it isn't JSON
//!   __diff        milestone 4: {"old", "new", "options"} -> {"value", "added", "removed"}:
//!                 the unified diff and its counts
//!
//! An area that isn't ported yet exits 1 with "not ported yet" on stderr, which the
//! harness reports as such; a release build says "unknown command" instead.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use bellwether_brain::exit;
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::redact::Cleaner;

/// Every area, and the milestone that ports it (None: ported).
pub(crate) const AREAS: [(&str, Option<&str>); 6] = [("env", None), ("redact", None), ("normalise", None), ("fixture", None), ("json", None), ("diff", None)];

/// `command` is the whole word (`__env`). Returns the exit code.
pub fn run(command: &str, args: &[String]) -> i32 {
    if command == "__session" {
        return crate::conformance_session::run(args);
    }
    if let Some(a) = args.first() {
        eprintln!("bellwether: {command} takes no arguments (got {a:?}); it reads JSON lines on stdin");
        return exit::BAD_CONFIG;
    }
    match command {
        "__list" => {
            let areas: serde_json::Map<String, serde_json::Value> = AREAS
                .iter()
                .map(|(name, m)| (name.to_string(), serde_json::Value::String(m.map(|m| format!("milestone {m}")).unwrap_or_else(|| "ported".into()))))
                .collect();
            println!("{}", serde_json::json!({ "areas": areas }));
            exit::OK
        }
        "__env" => each_line(env_case),
        "__redact" => each_js_line(redact_case),
        "__normalise" => each_js_line(normalise_case),
        "__fixture" => each_js_line(fixture_case),
        "__json" => each_js_line(json_case),
        "__diff" => each_js_line(diff_case),
        _ => match AREAS.iter().find(|(name, _)| command.strip_prefix("__") == Some(*name)) {
            Some((_, Some(m))) => {
                eprintln!("bellwether: {command} is not ported yet (milestone {m}); this build has the conformance feature");
                exit::UNEXPECTED
            }
            _ => {
                eprintln!("bellwether: unknown hidden command {command:?} (`__list` lists them)");
                exit::UNEXPECTED
            }
        },
    }
}

/// Reads stdin line by line and answers each line, in order.
fn each_line(answer: fn(&str) -> serde_json::Value) -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("bellwether: reading stdin: {e}");
                return exit::UNEXPECTED;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if writeln!(out, "{}", answer(&line)).is_err() {
            return exit::UNEXPECTED;
        }
    }
    if out.flush().is_err() {
        return exit::UNEXPECTED;
    }
    exit::OK
}

/// Like each_line, for the areas whose cases carry JS strings (lone surrogates
/// included): the line is read and the answer written with the JS-faithful JSON of
/// crates/core, never serde_json.
fn each_js_line(mut answer: impl FnMut(&Js) -> Js) -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("bellwether: reading stdin: {e}");
                return exit::UNEXPECTED;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match json::parse(&line) {
            Ok(case) => answer(&case),
            Err(e) => Js::Obj(Obj::new().with("error", format!("not a JSON case: {e}"))),
        };
        if writeln!(out, "{}", json::stringify(&reply)).is_err() {
            return exit::UNEXPECTED;
        }
    }
    if out.flush().is_err() {
        return exit::UNEXPECTED;
    }
    exit::OK
}

/// makeTextCleaner's options, as a case gives them: `{maxChars, redactPatterns}`.
fn cleaner_from(o: &Js, cache: &mut HashMap<String, Result<std::rc::Rc<Cleaner>, String>>) -> Result<std::rc::Rc<Cleaner>, String> {
    let key = json::stringify(o);
    cache
        .entry(key)
        .or_insert_with(|| {
            let max = o.get("maxChars").as_num().filter(|n| *n >= 0.0).unwrap_or(60.0) as usize;
            let patterns: Vec<String> = match o.get("redactPatterns") {
                Js::Arr(a) => a.iter().map(|p| p.as_str().unwrap_or("").to_string()).collect(),
                _ => Vec::new(),
            };
            Cleaner::new(max, &patterns).map(std::rc::Rc::new).map_err(|e| e.to_string())
        })
        .clone()
}

thread_local! {
    static CLEANERS: std::cell::RefCell<HashMap<String, Result<std::rc::Rc<Cleaner>, String>>> = std::cell::RefCell::new(HashMap::new());
}

/// `{"text", "maxChars", "redactPatterns"}` -> `{"value"}`, or `{"error": true, "message"}`
/// when a pattern doesn't compile (the Node answer's shape).
pub fn redact_case(case: &Js) -> Js {
    let cleaner = CLEANERS.with(|c| cleaner_from(case, &mut c.borrow_mut()));
    match cleaner {
        Ok(c) => match case.get("text") {
            Js::Str(t) => Js::Obj(Obj::new().with("value", c.clean(t))),
            _ => Js::Obj(Obj::new()),
        },
        Err(message) => Js::Obj(Obj::new().with("error", true).with("message", message)),
    }
}

/// `{"payload", "headers", "receivedAt", "clean"}` -> the HookEvent, as Node writes it.
pub fn normalise_case(case: &Js) -> Js {
    let headers = case.get("headers").as_obj().cloned().unwrap_or_default();
    let received_at = case.get("receivedAt").as_num().unwrap_or(f64::NAN);
    let clean = match case.get("clean") {
        Js::Obj(_) => match CLEANERS.with(|c| cleaner_from(case.get("clean"), &mut c.borrow_mut())) {
            Ok(c) => Some(c),
            Err(e) => return Js::Obj(Obj::new().with("error", e)),
        },
        _ => None,
    };
    bellwether_core::normalise::normalise(case.get("payload"), &headers, received_at, clean.as_deref()).to_js()
}

/// `{"name", "lines", "snapshots"?}` -> `{"transitions"}` (and `"snapshots"` when asked):
/// the fixture through the Rust reducer with the Node runner's fake clock.
pub fn fixture_case(case: &Js) -> Js {
    let lines = case.get("lines").as_arr().cloned().unwrap_or_default();
    let config = match case.get("config") {
        Js::Obj(_) => match bellwether_core::config::with_defaults(case.get("config")) {
            Ok(m) => m.config,
            Err(e) => return Js::Obj(Obj::new().with("error", e.message)),
        },
        _ => bellwether_core::config::Config::defaults().merged,
    };
    let keep = case.get("snapshots").truthy();
    let run = bellwether_core::fixture::run_fixture(&lines, &config, 250.0, 3000.0, keep);
    let mut out = Obj::new().with("transitions", Js::Arr(run.transitions));
    if keep {
        out.set("snapshots", Js::Arr(run.snapshots));
    }
    Js::Obj(out)
}

/// `{"text"}` -> `{"value"}`: `serialise(parseLossless(text), text)`, the round trip every
/// installer write makes; `{"error": true, "message"}` when the text isn't JSON.
pub fn json_case(case: &Js) -> Js {
    use bellwether_core::{jsstr, lossless};
    let Js::Str(text) = case.get("text") else { return Js::Obj(Obj::new().with("error", "no \"text\"")) };
    // The case's text is a unit string; a settings file is real text (read as UTF-8).
    let text = jsstr::to_string_lossy(text);
    match lossless::parse(&text) {
        Ok(v) => Js::Obj(Obj::new().with("value", jsstr::from_str(&lossless::serialise(&v, &text)))),
        Err(e) => Js::Obj(Obj::new().with("error", true).with("message", e)),
    }
}

/// `{"old", "new", "options"}` -> `{"value", "added", "removed"}`: unifiedDiff and diffStats.
pub fn diff_case(case: &Js) -> Js {
    use bellwether_core::{diff, jsstr};
    let s = |k: &str| jsstr::to_string_lossy(case.get(k).as_str().unwrap_or(""));
    let (old, new) = (s("old"), s("new"));
    let o = case.get("options");
    let name = |k: &str, d: &str| o.get(k).as_str().map(jsstr::to_string_lossy).unwrap_or_else(|| d.to_string());
    let context = o.get("context").as_num().map(|n| n.max(0.0) as usize).unwrap_or(3);
    let stats = diff::diff_stats(&old, &new);
    Js::Obj(
        Obj::new()
            .with("value", jsstr::from_str(&diff::unified_diff(&old, &new, &name("oldName", "a"), &name("newName", "b"), context)))
            .with("added", stats.added as f64)
            .with("removed", stats.removed as f64),
    )
}

fn error(message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({ "error": message.to_string() })
}

/// One `.env` case: `{"text": "...", "key": "BELLWETHER_HOOK_TOKEN"}`.
pub fn env_case(line: &str) -> serde_json::Value {
    let case: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return error(format!("not a JSON case: {e}")),
    };
    let Some(text) = case["text"].as_str() else { return error("no \"text\"") };
    let key = case["key"].as_str().unwrap_or("BELLWETHER_HOOK_TOKEN");
    serde_json::json!({ "value": bellwether_core::envfile::env_value(text, key) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `__env` answers every shared vector the way Node does (the vectors were made
    /// from Node itself; test/env-vectors.test.mjs checks them there).
    #[test]
    fn env_answers_the_shared_vectors() {
        let json: serde_json::Value = serde_json::from_str(include_str!("../../../test/fixtures/env-vectors.json")).unwrap();
        for v in json["vectors"].as_array().unwrap() {
            let line = serde_json::json!({ "text": v["text"] }).to_string();
            assert_eq!(env_case(&line)["value"], v["expected"], "{}", v["name"]);
        }
        assert!(env_case("not json")["error"].is_string());
        assert!(env_case("{}")["error"].is_string());
    }

    #[test]
    fn unported_areas_say_so() {
        assert_eq!(run("__fixture_nope", &[]), exit::UNEXPECTED);
        assert_eq!(run("__unknown", &[]), exit::UNEXPECTED);
        assert_eq!(run("__env", &["x".into()]), exit::BAD_CONFIG);
    }
}
