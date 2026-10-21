//! The installer vectors and the diff vectors (test/fixtures/installer-vectors/ and
//! diff-vectors.json, made from the Node brain by test/fixtures/build-vectors.mjs),
//! answered by the port: the bytes after `hooks install`, the preview, the bytes after
//! `hooks uninstall`, and the diff text and counts.
//!
//! One vector differs on purpose (test/DIVERGENCES.md): "exotic numbers, escapes and
//! integer-like keys", where `JSON.parse` moves the keys "1" and "2" first and the port
//! keeps file order.
//!
//! Named `hook_vectors`, not `installer`: Windows asks for elevation before running an exe
//! whose name has "install", "setup", "update" or "patch" in it (Windows plan, deviation
//! W1-11), and every test target is an exe there.

use std::path::{Path, PathBuf};

use bellwether_core::diff::{diff_stats, unified_diff};
use bellwether_core::hooks::{apply_plan, plan_edit, Action};
use bellwether_core::json::{self, Js};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bw-installer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const EXOTIC: &str = "exotic-numbers-escapes-and-integer-like-keys";

#[test]
fn every_installer_vector_installs_previews_and_uninstalls_as_node_does() {
    let dir = fixtures().join("installer-vectors");
    let meta = json::parse(&read(&dir.join("cases.json"))).unwrap();
    let port = meta.get("port").as_num().unwrap() as u16;
    let token = meta.get("token").as_str().unwrap().to_string();
    let cases = meta.get("cases").as_arr().unwrap();
    assert!(cases.len() >= 11);
    let mut round_trips = 0;
    for c in cases {
        let name = c.get("dir").as_str().unwrap();
        let case = dir.join(name);
        let settings = read(&case.join("settings.json"));
        let work = temp(name);
        let file = work.join("settings.json");
        std::fs::write(&file, &settings).unwrap();
        let data = work.join("data");
        let mut plan = plan_edit(&file, Action::Install, port, &token, Some(&data)).unwrap();
        let preview = unified_diff(&plan.old_text, &plan.new_text, "settings.json (current)", "settings.json (after install)", 3);
        if name == EXOTIC {
            // The port keeps file order: the keys stay b, 2, 1; Node's reorder is the only
            // difference in the preview and the installed bytes.
            let keys: Vec<&str> = plan.new_text.lines().skip(1).take(3).map(str::trim).collect();
            assert_eq!(keys, ["\"b\": 1,", "\"2\": \"two\",", "\"1\": \"one\","], "{name}");
            let node_text = read(&case.join("installed.json"));
            let node: Vec<&str> = node_text.lines().collect();
            let mut ours: Vec<&str> = plan.new_text.lines().collect();
            ours[1..4].sort();
            let mut theirs = node.clone();
            theirs[1..4].sort();
            assert_eq!(ours, theirs, "{name}: only the order of the first three keys differs");
        } else {
            assert_eq!(plan.new_text, read(&case.join("installed.json")), "{name}: installed bytes");
            assert_eq!(preview, read(&case.join("install.diff")), "{name}: the preview");
        }
        assert_eq!(plan.layout_kept, c.get("layoutKept").as_bool().unwrap(), "{name}: layoutKept");
        apply_plan(&mut plan, &work.join("backups-install"), Some(&data), "2026-01-01T00-00-00-000Z").unwrap();
        let mut un = plan_edit(&file, Action::Uninstall, port, "", Some(&data)).unwrap();
        let un_preview = unified_diff(&un.old_text, &un.new_text, "settings.json (current)", "settings.json (after uninstall)", 3);
        if name != EXOTIC {
            assert_eq!(un_preview, read(&case.join("uninstall.diff")), "{name}: the uninstall preview");
        }
        apply_plan(&mut un, &work.join("backups-uninstall"), Some(&data), "2026-01-01T00-00-00-001Z").unwrap();
        let back = read(&file);
        if c.get("roundTrip") == &Js::Bool(true) {
            assert_eq!(back, settings, "{name}: install then uninstall gives back the bytes");
            round_trips += 1;
        } else if name == EXOTIC {
            // Escapes are rewritten as JSON.stringify writes them (`\/` as `/`, `é` as
            // é); everything else, the key order included, comes back as it was.
            let changed: Vec<(&str, &str)> = settings.lines().zip(back.lines()).filter(|(a, b)| a != b).collect();
            assert_eq!(changed, [("  \"s\": \"tab\\there \\u00e9 \\ud83d\\ude00 \\/ slash\",", "  \"s\": \"tab\\there é 😀 / slash\",")], "{name}");
        } else {
            assert_eq!(back, read(&case.join("uninstalled.json")), "{name}: uninstalled bytes");
        }
        let _ = std::fs::remove_dir_all(&work);
    }
    assert!(round_trips >= 9, "{round_trips} round trips");
}

#[test]
fn every_diff_vector_is_node_s_diff() {
    let v = json::parse(&read(&fixtures().join("diff-vectors.json"))).unwrap();
    let all = v.get("vectors").as_arr().unwrap();
    assert!(all.len() >= 20);
    for c in all {
        let name = c.get("name").as_str().unwrap();
        let o = c.get("options");
        let old = c.get("old").as_str().unwrap();
        let new = c.get("new").as_str().unwrap();
        let got = unified_diff(old, new, o.get("oldName").as_str().unwrap_or("a"), o.get("newName").as_str().unwrap_or("b"), o.get("context").as_num().map(|n| n as usize).unwrap_or(3));
        assert_eq!(got, c.get("expected").as_str().unwrap(), "{name}");
        let s = diff_stats(old, new);
        assert_eq!((s.added as f64, s.removed as f64), (c.get("stats").get("added").as_num().unwrap(), c.get("stats").get("removed").as_num().unwrap()), "{name}");
    }
}
