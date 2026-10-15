#![cfg(unix)]
//! BR-3K, distribution milestone 6: legacy agent detection under hostile plist shapes and
//! hostile `launchctl print` answers. Offline; everything under a temp dir, launchd is a
//! closure (never asked).

use bellwether_brain::legacy::{parse_print, scan, scan_here, target, Loaded, LABELS};
use std::cell::RefCell;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;

fn home(name: &str) -> (PathBuf, PathBuf) {
    let h = std::env::temp_dir().join(format!("bw-t6-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&h);
    let a = h.join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&a).unwrap();
    (h, a)
}

#[test]
fn every_plist_shape_is_reported_without_hanging_or_opening_it() {
    let (h, a) = home("shapes");
    // malformed content
    std::fs::write(a.join("local.bellwether.brain.plist"), b"\xff\xfe<not xml").unwrap();
    // dangling symlink
    symlink("/nonexistent/target", a.join("local.bellwether.shell.plist")).unwrap();
    // a directory
    std::fs::create_dir(a.join("local.pasture.brain.plist")).unwrap();
    // a FIFO: opening it for read would block forever
    let fifo = a.join("local.pasture.shell.plist");
    let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // lookalikes
    for n in ["local.bellwether.brainx.plist", "local.bellwether.brain", "xlocal.bellwether.brain.plist", "LOCAL.BELLWETHER.BRAIN.plist"] {
        std::fs::write(a.join(n), "").unwrap();
    }
    let asked = RefCell::new(Vec::new());
    let t = std::time::Instant::now();
    let found = scan(&h, &|l| {
        asked.borrow_mut().push(l.to_string());
        Loaded::No
    });
    assert!(t.elapsed().as_secs() < 2);
    assert_eq!(found.iter().map(|x| x.label).collect::<Vec<_>>(), LABELS.to_vec(), "all four shapes counted; lookalikes not");
    assert_eq!(*asked.borrow(), LABELS.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "launchd asked about the four labels and nothing else");
    for x in &found {
        assert!(x.describe().contains("plist only"), "{}", x.describe());
    }
    let _ = std::fs::remove_dir_all(h);
}

#[test]
fn odd_modes_and_an_unreadable_folder() {
    let (h, a) = home("modes");
    let p = a.join("local.bellwether.brain.plist");
    std::fs::write(&p, "<plist/>").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(scan(&h, &|_| Loaded::No).len(), 1, "mode 000 file still counts (stat only)");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    // Folder with no search permission: the plist can't be statted. It must not panic; loaded
    // still counts via launchd.
    std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = scan(&h, &|l| if l == "local.bellwether.brain" { Loaded::Running(9) } else { Loaded::No });
    std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.len(), 1);
    assert!(r[0].plist.is_none() && r[0].loaded.is_loaded());
    // Library missing altogether.
    let none = std::env::temp_dir().join(format!("bw-t6-{}-nolib", std::process::id()));
    std::fs::create_dir_all(&none).unwrap();
    assert!(scan(&none, &|_| Loaded::No).is_empty());
    let _ = std::fs::remove_dir_all(h);
    let _ = std::fs::remove_dir_all(none);
}

#[test]
fn hostile_launchctl_answers_never_read_as_loaded() {
    let t = target(501, "local.bellwether.brain");
    let ok = format!("{t} = {{\n\tpid = 12\n}}\n");
    assert_eq!(parse_print(&t, true, &ok, ""), Loaded::Running(12));
    assert_eq!(parse_print(&t, true, &ok.replace('\n', "\r\n"), ""), Loaded::Running(12), "CRLF");
    // first line must be exactly the target
    for bad in [
        format!("\n{ok}"),
        format!(" {ok}"),
        ok.replace("gui/501/", "gui/5011/"),
        ok.replace("gui/501/", "user/501/"),
        ok.replace("gui/501/", "system/"),
        ok.replace("brain =", "brainx ="),
        format!("junk\n{ok}"),
        String::new(),
    ] {
        assert!(matches!(parse_print(&t, true, &bad, ""), Loaded::Unknown(_)), "{bad:?}");
    }
    // pids: 0 and negative are not running; nested pid ignored; garbage ignored
    for (body, want) in [("\tpid = 0\n", Loaded::Idle), ("\tpid = -1\n", Loaded::Idle), ("\t\tpid = 5\n", Loaded::Idle), ("\tpid = abc\n", Loaded::Idle), ("\tpid = 99999999999\n", Loaded::Idle)] {
        assert_eq!(parse_print(&t, true, &format!("{t} = {{\n{body}}}\n"), ""), want, "{body:?}");
    }
    // failures
    assert_eq!(parse_print(&t, false, "", ""), Loaded::Unknown("no answer".into()));
    assert!(matches!(parse_print(&t, false, "", "Operation not permitted"), Loaded::Unknown(_)));
    assert!(!parse_print(&t, false, &ok, "").is_loaded(), "failure exit with a plausible body is not loaded");
}

#[test]
fn a_temp_home_is_never_asked() {
    let (h, _a) = home("tempreal");
    std::fs::write(h.join("Library").join("LaunchAgents").join("local.bellwether.brain.plist"), "").unwrap();
    let f = scan_here(&h);
    assert_eq!(f[0].loaded, Loaded::NotAsked);
    // a symlink to a temp dir is still not the real home
    let l = std::env::temp_dir().join(format!("bw-t6-{}-link", std::process::id()));
    let _ = std::fs::remove_file(&l);
    symlink(&h, &l).unwrap();
    assert_eq!(scan_here(&l)[0].loaded, Loaded::NotAsked);
    let _ = std::fs::remove_file(l);
    let _ = std::fs::remove_dir_all(h);
}
