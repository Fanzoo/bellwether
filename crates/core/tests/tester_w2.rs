//! BR-3K's independent checks of Windows plan W2 (first part): the pure rules, from the
//! outside. Nothing here touches the environment of the test process, a real home or a port.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bellwether_core::{acl, lossless, normalise, paths, persist};

fn env(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<OsString> {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(*v))
}

#[test]
fn claude_config_dir_wins_over_every_home_and_empty_is_unset() {
    for windows in [false, true] {
        let all = env(vec![("HOME", "/h"), ("USERPROFILE", "C:\\u"), ("CLAUDE_CONFIG_DIR", "D:\\cfg")]);
        assert_eq!(paths::claude_dir_in(&all, windows), PathBuf::from("D:\\cfg"));
        // No home at all and no CLAUDE_CONFIG_DIR: a relative `.claude`, which is what the old code did too.
        assert_eq!(paths::claude_dir_in(env(vec![]), windows), PathBuf::from(".claude"));
    }
    // macOS never falls back to USERPROFILE; Windows without USERPROFILE uses HOME (W2-12).
    assert_eq!(paths::user_home_in(env(vec![("USERPROFILE", "C:\\u")]), false), None);
    assert_eq!(paths::user_home_in(env(vec![("HOME", "/h"), ("USERPROFILE", "")]), true), Some(PathBuf::from("/h")), "an empty USERPROFILE is unset, so HOME");
}

#[test]
fn cwd_names_on_windows_paths_and_not_on_unix_ones() {
    for (cwd, want) in [
        ("C:\\Users\\jeff\\proj", "proj"),
        ("C:/Users/jeff/proj", "proj"),
        ("c:\\Users\\jeff\\proj\\\\", "proj"),
        ("\\\\server\\share\\proj", "proj"),
        ("\\\\?\\C:\\Users\\jeff\\proj", "proj"),
        ("C:\\Users\\jeff\\my project (2)", "my project (2)"),
        ("C:\\Users\\jeff\\ünï", "ünï"),
        // Not a drive path: left to basename, as before.
        ("/mnt/c/Users/jeff/proj", "proj"),
        ("/home/me/odd\\name", "odd\\name"),
        ("relative\\win", "relative\\win"),
        ("C:proj", "C:proj"),
    ] {
        assert_eq!(normalise::cwd_basename(cwd), want, "{cwd:?}");
    }
    // Never panics on junk and never returns something longer than the input.
    for junk in ["", "\\", "\\\\", "C:\\", "C:", ":", "\u{0}", "C:\\\u{202e}x"] {
        assert!(normalise::cwd_basename(junk).len() <= junk.len() + 1, "{junk:?}");
    }
}

#[test]
fn the_bom_split_is_exact() {
    assert_eq!(lossless::split_bom("\u{feff}{}"), ("\u{feff}", "{}"));
    assert_eq!(lossless::split_bom("{}"), ("", "{}"));
    assert_eq!(lossless::split_bom(""), ("", ""));
    // Only one: a second BOM is part of the body, which the parser then refuses.
    let (bom, body) = lossless::split_bom("\u{feff}\u{feff}{}");
    assert_eq!((bom, body), ("\u{feff}", "\u{feff}{}"));
    assert!(lossless::parse(body).is_err(), "a doubled BOM is not JSON");
    // A BOM in the middle is not a BOM.
    assert_eq!(lossless::split_bom("{}\u{feff}").0, "");
}

#[test]
fn a_dacl_with_anyone_else_names_them_and_a_missing_one_names_everyone() {
    let me = "S-1-5-21-1-2-3-1001";
    let d = |s: &[&str]| acl::Dacl { protected: true, allowed: s.iter().map(|x| x.to_string()).collect() };
    assert!(acl::others(Some(&d(&[me])), me).is_empty());
    assert_eq!(acl::others(Some(&d(&["S-1-1-0"])), me), ["Everyone"], "Everyone alone (the user's own entry gone) is still reported");
    assert_eq!(acl::others(Some(&d(&[me, "S-1-5-4", "S-1-5-4"])), me), ["INTERACTIVE"], "repeated entries are reported once");
    assert_eq!(acl::others(None, me).len(), 1);
    assert_eq!(acl::PRIVATE, if cfg!(windows) { "only you can read it" } else { "mode 0600" });
}

#[test]
fn the_rename_retry_ends_at_its_limit_and_never_retries_other_errors() {
    use std::cell::Cell;
    let a = Path::new("a");
    let n = Cell::new(0u32);
    let started = std::time::Instant::now();
    let r = persist::rename_with_retry(a, a, Duration::from_millis(0), |_| true, |_, _| {
        n.set(n.get() + 1);
        Err(std::io::Error::from_raw_os_error(32))
    });
    assert!(r.is_err());
    assert_eq!(n.get(), 1, "a zero limit tries once");
    assert!(started.elapsed() < Duration::from_millis(500));
    // The bound: a persistently busy file gives up after about the limit, not never.
    let started = std::time::Instant::now();
    let r = persist::rename_with_retry(a, a, Duration::from_millis(300), |_| true, |_, _| Err(std::io::Error::from_raw_os_error(5)));
    assert!(r.is_err());
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(250) && took < Duration::from_millis(1500), "{took:?}");
    // On this OS the real predicate retries only Windows' own two codes.
    for code in [2, 13, 17] {
        assert!(!persist::rename_is_transient(&std::io::Error::from_raw_os_error(code)), "{code}");
    }
    assert_eq!(persist::replace_file(Path::new("/nonexistent-bw-tester/x"), Path::new("/nonexistent-bw-tester/y")).unwrap_err().kind(), std::io::ErrorKind::NotFound, "a missing source fails at once");
}
