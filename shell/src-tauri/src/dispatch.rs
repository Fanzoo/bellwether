//! One executable, three modes (distribution plan, Surface), chosen from argv:
//!
//!   `<exe> serve …`                         the brain: no Tauri, no AppKit windows, no WebKit
//!   `bellwether …` (lower-case name, the CLI symlink on PATH), or any first argument
//!   that isn't an option (`Bellwether doctor`)                      the CLI
//!   anything else (Finder, Login Items, `--demo`, `--action …`)     the app
//!
//! A first argument starting with `-` is the app's (or the dev shell's) own flag, so
//! `bellwether-shell --no-spawn-brain` keeps working exactly as before. The exceptions
//! are `--help`, `-h` and `--version`, which are always the CLI's.

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Serve,
    Cli,
    App,
}

pub fn mode_of(args: &[String]) -> Mode {
    let first = args.get(1).map(String::as_str);
    if first == Some("serve") {
        return Mode::Serve;
    }
    if first.is_some_and(|a| !a.starts_with('-')) {
        return Mode::Cli;
    }
    // Help and version are the CLI's under any name: run as `Bellwether --help` they
    // must print usage, never start a strip on the default port.
    if matches!(first, Some("--help" | "-h" | "--version")) {
        return Mode::Cli;
    }
    let name = args.first().map(|a| std::path::Path::new(a).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()).unwrap_or_default();
    // Exactly lower case: the bundle's own executable is `Bellwether`, and APFS is
    // case-insensitive, so only argv[0] tells the two apart.
    if name == "bellwether" || name == "bellwether.exe" {
        return Mode::Cli;
    }
    Mode::App
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(a: &[&str]) -> Mode {
        mode_of(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn argv_picks_the_mode() {
        assert_eq!(m(&["/Applications/Bellwether.app/Contents/MacOS/Bellwether"]), Mode::App);
        assert_eq!(m(&["/Applications/Bellwether.app/Contents/MacOS/Bellwether", "serve"]), Mode::Serve);
        assert_eq!(m(&["/Applications/Bellwether.app/Contents/MacOS/Bellwether", "serve", "--port", "47418"]), Mode::Serve);
        assert_eq!(m(&["/opt/homebrew/bin/bellwether"]), Mode::Cli, "the symlink with no subcommand prints usage");
        assert_eq!(m(&["bellwether", "--help"]), Mode::Cli);
        assert_eq!(m(&["bellwether", "serve"]), Mode::Serve);
        assert_eq!(m(&["Bellwether", "doctor"]), Mode::Cli);
        assert_eq!(m(&["Bellwether", "__fixture"]), Mode::Cli, "unknown words go to the CLI, which refuses them");
        assert_eq!(m(&["Bellwether", "--action", "login-items-status"]), Mode::App);
        assert_eq!(m(&["Bellwether", "-psn_0_12345"]), Mode::App, "old Finder launch arguments");
        assert_eq!(m(&["target-dev/debug/bellwether-shell", "--no-spawn-brain"]), Mode::App);
        assert_eq!(m(&["target-dev/debug/bellwether-shell", "--ax-prompt"]), Mode::App);
        assert_eq!(m(&["target-dev/debug/bellwether-shell"]), Mode::App);
        assert_eq!(m(&[]), Mode::App);
    }

    #[test]
    fn help_and_version_never_start_the_strip() {
        for flag in ["--help", "-h", "--version"] {
            for exe in ["/Applications/Bellwether.app/Contents/MacOS/Bellwether", "bellwether", "target-dev/debug/bellwether-shell"] {
                assert_eq!(m(&[exe, flag]), Mode::Cli, "{exe} {flag}");
            }
        }
        assert_eq!(m(&["Bellwether", "--demo", "--help"]), Mode::App, "only as the first argument");
    }
}
