//! Hidden `__` subcommands for the conformance harness (distribution plan D2), compiled
//! only with the `conformance` feature, which no release build has.
//!
//! Each one reads cases as JSON lines on stdin and writes one JSON line per case on
//! stdout, in order, so `scripts/differential.mjs` can send 10⁵ generated cases through
//! one process and compare them with the Node functions line by line. A case that
//! can't be read gets `{"error": "…"}` on its line (a difference, never a crash).
//!
//!   __list        the areas and whether each is ported yet (JSON, no input)
//!   __env         {"text", "key"?} -> {"value": string|null}: a .env read the way
//!                 `node --env-file` does (settings.rs parse_env_value)
//!   __redact      milestone 3: {"text", "maxChars", "redactPatterns"} -> {"value"}
//!   __normalise   milestone 3: {"payload", "headers", "receivedAt", "clean"} -> the HookEvent
//!   __fixture     milestone 3: a fixture's JSON lines -> its transition lines
//!   __json        milestone 4: settings bytes -> the lossless round trip
//!   __diff        milestone 4: {"old", "new"} -> the unified diff
//!
//! An area that isn't ported yet exits 1 with "not ported yet" on stderr, which the
//! harness reports as such; a release build says "unknown command" instead.

use std::io::{BufRead, Write};

use bellwether_brain::exit;

/// Every area, and the milestone that ports it (None: ported).
pub(crate) const AREAS: [(&str, Option<&str>); 6] = [("env", None), ("redact", Some("3")), ("normalise", Some("3")), ("fixture", Some("3")), ("json", Some("4")), ("diff", Some("4"))];

/// `command` is the whole word (`__env`). Returns the exit code.
pub fn run(command: &str, args: &[String]) -> i32 {
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
    serde_json::json!({ "value": crate::settings::parse_env_value(text, key) })
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
        assert_eq!(run("__redact", &[]), exit::UNEXPECTED);
        assert_eq!(run("__unknown", &[]), exit::UNEXPECTED);
        assert_eq!(run("__env", &["x".into()]), exit::BAD_CONFIG);
    }
}
