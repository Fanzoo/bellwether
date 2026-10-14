// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bellwether_shell_lib::dispatch::{mode_of, Mode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match mode_of(&args) {
        // The brain never touches Tauri: nothing of the app is initialised.
        Mode::Serve => std::process::exit(bellwether_brain::cli::run("serve", &args[2..])),
        Mode::Cli => std::process::exit(bellwether_shell_lib::cli::run(&args)),
        Mode::App => bellwether_shell_lib::run(),
    }
}
