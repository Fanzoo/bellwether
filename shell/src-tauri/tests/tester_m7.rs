//! BR-3K's independent checks of milestone 7 (onboarding): the preview/apply protocol and the
//! hostile-input edges, driven through the public `Onboarding` API under a temp HOME. Nothing
//! here touches the real home, ~/.claude, the repository's data/ or a port.

use std::path::{Path, PathBuf};

use bellwether_core::hooks::Action;
use bellwether_core::token as tok;
use bellwether_shell_lib::onboarding::{Ctx, Layout, Onboarding};

const T1: &str = "testTokenOne_abcdefghijklmnopqrstu";
const T2: &str = "testTokenTwo_zyxwvutsrqponmlkjihgf";
const FOREIGN: &str = "foreignTokenFromOldInstall_0123456789";

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bw-tester-m7-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ob(base: &Path) -> Onboarding {
    Onboarding::new(Ctx { layout: Layout::App, home: base.join("app"), port: 47481, resources: None, user_home: base.join("user"), env_config: None, env_token: None })
}

fn settings(o: &Onboarding) -> PathBuf {
    let f = o.ctx.settings_file();
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    f
}

#[test]
fn a_newer_preview_voids_the_older_plan_id_and_junk_ids_are_refused() {
    let base = temp("ids");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    let a = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    let b = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    assert_ne!(a, b);
    assert!(o.hooks_apply(&a).is_err(), "the older preview is no longer current");
    for junk in ["", "plan-1", "../../etc/passwd", "plan-1-\u{0}", &"x".repeat(100_000)] {
        assert!(o.hooks_apply(junk).is_err(), "{junk:.20?}");
    }
    assert!(!o.ctx.settings_file().exists(), "nothing was written by any refusal");
    // The current one still applies, once.
    assert_eq!(o.hooks_apply(&b).unwrap()["outcome"], "applied");
    assert!(o.hooks_apply(&b).is_err());
    // An install preview then an uninstall preview: applying the install id is refused.
    let u = o.hooks_preview(Action::Uninstall).unwrap()["planId"].as_str().unwrap().to_string();
    assert!(o.hooks_apply(&b).is_err());
    assert_eq!(o.hooks_apply(&u).unwrap()["outcome"], "applied");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_token_rotated_between_preview_and_install_is_refused_and_nothing_is_written() {
    let base = temp("rotate");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    let file = settings(&o);
    std::fs::write(&file, "{}\n").unwrap();
    let id = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    tok::write(&o.ctx.home, T2).unwrap();
    let r = o.hooks_apply(&id).unwrap();
    assert_eq!(r["outcome"], "changed");
    assert!(r["message"].as_str().unwrap().contains("token changed"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}\n", "settings untouched");
    let shown = r["preview"]["diffText"].as_str().unwrap();
    assert!(!shown.contains(T1) && !shown.contains(T2), "masked in the re-preview");
    let backups = o.ctx.home.join("backups");
    assert!(!backups.exists() || std::fs::read_dir(&backups).unwrap().count() == 0, "no backup for a refused install");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_settings_file_appearing_or_vanishing_between_preview_and_install_is_caught() {
    let base = temp("appear");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    // Preview with no settings file; one appears (Claude Code made it); Install refuses.
    let id = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    let file = settings(&o);
    std::fs::write(&file, "{\"model\": \"x\"}\n").unwrap();
    assert_eq!(o.hooks_apply(&id).unwrap()["outcome"], "changed");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"model\": \"x\"}\n");
    // Preview with a file; it is deleted; Install refuses instead of resurrecting its old content.
    let id = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    std::fs::remove_file(&file).unwrap();
    let r = o.hooks_apply(&id).unwrap();
    assert_eq!(r["outcome"], "changed");
    assert!(!file.exists());
    // A one-byte same-length edit (hash, not size or mtime) is caught as well.
    std::fs::write(&file, "{\"model\": \"x\"}\n").unwrap();
    let id = o.hooks_preview(Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    std::fs::write(&file, "{\"model\": \"y\"}\n").unwrap();
    assert_eq!(o.hooks_apply(&id).unwrap()["outcome"], "changed");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"model\": \"y\"}\n");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_foreign_token_in_the_file_is_masked_in_install_and_uninstall_previews() {
    // Hooks from an older install carry a token the app does not hold.
    let base = temp("foreign");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    let file = settings(&o);
    let mut text = String::from("{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"http\",\n            \"url\": \"http://127.0.0.1:47481/hook\",\n            \"headers\": {\n              \"X-Bellwether-Token\": \"");
    text.push_str(FOREIGN);
    text.push_str("\"\n            }\n          }\n        ]\n      }\n    ]\n  }\n}\n");
    std::fs::write(&file, &text).unwrap();
    for action in [Action::Install, Action::Uninstall] {
        let p = o.hooks_preview(action).unwrap();
        let all = p.to_string();
        assert!(!all.contains(FOREIGN), "{action:?}: the file's own token leaked into the window's reply: {all}");
        assert!(!all.contains(T1), "{action:?}: the app token leaked");
    }
    // And with no app token at all (uninstall from a fresh app home).
    let base2 = temp("foreign2");
    let o2 = ob(&base2);
    let file2 = settings(&o2);
    std::fs::write(&file2, &text).unwrap();
    let p = o2.hooks_preview(Action::Uninstall).unwrap();
    assert!(!p.to_string().contains(FOREIGN), "uninstall with no app token: {p}");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&base2);
}

#[test]
fn other_secrets_in_the_settings_file_are_not_echoed_by_the_status() {
    // setup_status must not carry file contents (only counts and a path).
    let base = temp("status");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    let file = settings(&o);
    std::fs::write(&file, "{\"env\": {\"ANTHROPIC_API_KEY\": \"sk-ant-SECRETSECRET\"}}\n").unwrap();
    let s = o.status(&serde_json::Value::Null).to_string();
    assert!(!s.contains("SECRETSECRET") && !s.contains(T1), "{s}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_unparseable_settings_file_is_a_reported_cause_and_never_overwritten() {
    let base = temp("badjson");
    let o = ob(&base);
    tok::write(&o.ctx.home, T1).unwrap();
    let file = settings(&o);
    std::fs::write(&file, "{ not json").unwrap();
    for action in [Action::Install, Action::Uninstall] {
        let e = o.hooks_preview(action).unwrap_err();
        assert!(!e.is_empty() && !e.contains("panicked"), "{e}");
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
    let st = o.status(&serde_json::Value::Null);
    assert!(st["hooks"]["problem"].is_string(), "{st}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn pet_select_refuses_names_that_could_escape_and_writes_nothing() {
    let base = temp("petnames");
    let o = ob(&base);
    for n in ["..", "../x", "a/b", "", ".", "/etc/passwd", "bellwether\u{0}", "__proto__", "constructor"] {
        assert!(o.pet_select(n, &serde_json::Value::Null).is_err(), "{n:?}");
    }
    assert!(!o.ctx.home.join("config.json").exists());
    let _ = std::fs::remove_dir_all(&base);
}

// A symlink to /dev/null: Unix only (Windows plan W1).
#[cfg(unix)]
#[test]
fn a_symlinked_pet_source_or_unreadable_one_is_a_message_not_a_panic() {
    let base = temp("petsrc");
    let o = ob(&base);
    let link = base.join("l.xml");
    std::os::unix::fs::symlink("/dev/null", &link).unwrap();
    assert!(o.pet_import_file(&link).is_err());
    assert!(o.pet_import_file(Path::new("/nonexistent/animations.xml")).is_err());
    assert!(o.pet_import_file(&base).is_err(), "a directory");
    assert!(!o.ctx.home.join("pets").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_repository_layout_never_makes_a_token_or_writes_config() {
    let base = temp("repo");
    let mut c = Ctx { layout: Layout::Repository, home: base.join("data"), port: 47481, resources: None, user_home: base.join("user"), env_config: None, env_token: None };
    c.layout = Layout::Repository;
    let o = Onboarding::new(c);
    assert!(o.login_items_set(true).is_err());
    assert!(o.pet_select("esheep64", &serde_json::Value::Null).is_err());
    assert!(!base.join("data").exists(), "nothing created in a repository's data/");
    let _ = std::fs::remove_dir_all(&base);
}

// Unix permission bits: Unix only; the Windows ACL check is W2's.
#[cfg(unix)]
#[test]
fn an_unwritable_settings_dir_fails_with_a_cause_and_leaves_the_file_and_no_backup() {
    use std::os::unix::fs::PermissionsExt;
    let base = temp("readonly");
    let o = ob(&base);
    let file = settings(&o);
    std::fs::write(&file, "{\"model\": \"x\"}\n").unwrap();
    let id = o.hooks_preview(bellwether_core::hooks::Action::Install).unwrap()["planId"].as_str().unwrap().to_string();
    let dir = file.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let r = o.hooks_apply(&id);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let e = r.expect_err("the write can't succeed");
    assert!(!e.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"model\": \"x\"}\n");
    // Recorded as an observation: a token made for the failed install.
    eprintln!("token file left after a failed first install: {}", tok::file(&o.ctx.home).exists());
    let _ = std::fs::remove_dir_all(&base);
}

const TINY: &str = "<animations><header><title>tiny</title></header><image><tilesx>2</tilesx><tilesy>1</tilesy><png>iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==</png><transparency>Magenta</transparency></image><spawns><spawn id=\"1\"><x>0</x><y>0</y><next probability=\"100\">1</next></spawn></spawns><animations><animation id=\"1\"><name>walk</name><start><x>0</x><y>0</y><interval>200</interval></start><end><x>0</x><y>0</y><interval>200</interval></end><sequence repeat=\"0\" repeatfrom=\"0\"><frame>1</frame><frame>0</frame></sequence></animation></animations></animations>";

#[test]
fn a_corrupt_config_json_is_never_overwritten_by_a_pet_choice() {
    let base = temp("badconfig");
    let o = ob(&base);
    let pet = base.join("tiny.xml");
    std::fs::write(&pet, TINY).unwrap();
    o.pet_import_file(&pet).unwrap();
    std::fs::write(o.ctx.home.join("config.json"), "{ broken").unwrap();
    let e = o.pet_select("tiny", &serde_json::Value::Null).unwrap_err();
    assert!(e.contains("config.json"), "{e}");
    assert_eq!(std::fs::read_to_string(o.ctx.home.join("config.json")).unwrap(), "{ broken");
    // A config the brain would refuse (a bad port) is not made worse or replaced by a write.
    std::fs::write(o.ctx.home.join("config.json"), "{\"port\": 80}\n").unwrap();
    let r = o.pet_select("tiny", &serde_json::Value::Null);
    eprintln!("pet_select over a config with port 80: {r:?}");
    let _ = std::fs::remove_dir_all(&base);
}
