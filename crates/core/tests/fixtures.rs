//! The 21 golden fixtures (distribution plan D2 point 3): the Rust reducer, driven by the
//! same fake clock as test/fixture-runner.mjs (step 250 ms, tail 3000 ms), writes exactly
//! the transition lines the Node reducer wrote into `*.expected.jsonl`, byte for byte.

use std::path::PathBuf;

use bellwether_core::config::Config;
use bellwether_core::fixture::run_fixture;
use bellwether_core::json::{self, Js};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

#[test]
fn every_golden_fixture_matches_byte_for_byte() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|f| f.starts_with('s') && f.ends_with(".jsonl") && !f.contains(".expected.") && f[1..].starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    names.sort();
    assert_eq!(names.len(), 21, "{names:?}");
    let config = Config::defaults();
    for name in &names {
        let text = std::fs::read_to_string(fixtures().join(name)).unwrap();
        let lines: Vec<Js> = text.trim().lines().filter(|l| !l.is_empty()).map(|l| json::parse(l).unwrap()).collect();
        let run = run_fixture(&lines, &config.merged, 250.0, 3000.0, false);
        let got: Vec<String> = run.transitions.iter().map(json::stringify).collect();
        let expected_file = fixtures().join(name.replace(".jsonl", ".expected.jsonl"));
        let expected: Vec<String> = std::fs::read_to_string(&expected_file).unwrap().lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        assert_eq!(got, expected, "{name}");
    }
}
