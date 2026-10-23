//! The console CLI on Windows (Windows plan WD7): `bin\bellwether.exe`, the same commands,
//! output and exit codes as `Bellwether.exe <command>`, in a console-subsystem program so
//! its output reaches the terminal and the prompt waits for it. `Bellwether.exe` is a GUI
//! program (main.rs), whose output never reaches the terminal that started it.
//!
//! Built only with the `console-cli` feature (scripts/build-windows-test.mjs; the installer
//! from W8), so the macOS builds are unchanged. It never starts the strip: with no
//! subcommand it prints the usage, as the `bellwether` symlink does on macOS.

use bellwether_shell_lib::dispatch::{mode_of, Mode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match mode_of(&args) {
        Mode::Serve => std::process::exit(bellwether_shell_lib::cli::refused_without_home("serve").unwrap_or_else(|| bellwether_brain::cli::run("serve", &args[2..]))),
        // Under any other name or argument too: this program is the CLI only.
        Mode::Cli | Mode::App => std::process::exit(bellwether_shell_lib::cli::run(&args)),
    }
}
