//! `bellwether token init` and `token rotate` through the CLI functions (one test, since
//! they read BELLWETHER_HOME and HOME from the process environment): the token file is
//! made once, 0600; a legacy `.env` token is imported; rotate is a dry run without
//! --write, refuses a settings file whose hooks carry the current token (the installer
//! that rewrites them is milestone 4), and with --write tells the running brain, which
//! keeps accepting the old token for a minute.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn status_of(port: u16, token: &str) -> u16 {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let body = r#"{"hook_event_name":"Stop"}"#;
    write!(s, "POST /hook HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nX-Bellwether-Token: {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
}

#[test]
fn token_init_and_rotate() {
    let root = std::env::temp_dir().join(format!("bw-token-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(root.join("user").join(".claude")).unwrap();
    std::env::set_var("BELLWETHER_HOME", &home);
    std::env::set_var("HOME", root.join("user"));
    std::env::remove_var("BELLWETHER_HOOK_TOKEN");
    std::env::remove_var("BELLWETHER_CONFIG");
    let run = |args: &[&str]| bellwether_brain::cli::run(args[0], &args[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>());

    // init imports a repository install's token, once.
    let env = root.join("legacy.env");
    std::fs::write(&env, "BELLWETHER_HOOK_TOKEN=legacyToken_0123456789abc\n").unwrap();
    assert_eq!(run(&["token", "init", "--from", env.to_str().unwrap()]), 0);
    assert_eq!(bellwether_core::token::read_file(&home).unwrap().as_deref(), Some("legacyToken_0123456789abc"));
    assert_eq!(run(&["token", "init"]), 0, "a second init leaves the file alone");
    assert_eq!(bellwether_core::token::read_file(&home).unwrap().as_deref(), Some("legacyToken_0123456789abc"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(bellwether_core::token::file(&home)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(run(&["token", "bogus"]), 3);
    assert_eq!(run(&["token", "init", "--nope"]), 3);

    // rotate: a dry run changes nothing; hooks carrying the token are refused.
    std::fs::write(home.join("config.json"), r#"{"port": 47993}"#).unwrap();
    assert_eq!(run(&["token", "rotate"]), 0);
    assert_eq!(bellwether_core::token::read_file(&home).unwrap().as_deref(), Some("legacyToken_0123456789abc"));
    let settings = root.join("user").join(".claude").join("settings.json");
    std::fs::write(&settings, r#"{"hooks":{"Stop":[{"hooks":[{"type":"http","url":"http://127.0.0.1:47993/hook","headers":{"X-Bellwether-Token":"legacyToken_0123456789abc"}}]}]}}"#).unwrap();
    assert_eq!(run(&["token", "rotate", "--write"]), 3);
    assert_eq!(bellwether_core::token::read_file(&home).unwrap().as_deref(), Some("legacyToken_0123456789abc"), "refused, so unchanged");
    std::fs::remove_file(&settings).unwrap();

    // rotate --write with a running brain: it switches, and the old token works a minute more.
    let bound = bellwether_brain::bind(0).unwrap();
    let port = bound.port;
    std::fs::write(home.join("config.json"), format!(r#"{{"port": {port}}}"#)).unwrap();
    let mut opts = bellwether_brain::Options::new("legacyToken_0123456789abc".into(), bellwether_brain::Mode::Normal, bellwether_brain::Logger::stderr_only());
    opts.token_file = Some(bellwether_core::token::file(&home));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || bellwether_brain::serve(bound, opts, Some(rx)).unwrap());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(run(&["token", "rotate", "--write"]), 0);
    let new = bellwether_core::token::read_file(&home).unwrap().unwrap();
    assert_ne!(new, "legacyToken_0123456789abc");
    bellwether_core::token::check(&new).unwrap();
    assert_eq!(status_of(port, &new), 204);
    assert_eq!(status_of(port, "legacyToken_0123456789abc"), 204, "the old token, within the minute");
    assert_eq!(status_of(port, "anotherToken_0123456789"), 403);
    let _ = tx.send(());
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&root);
}
