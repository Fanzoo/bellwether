//! The brain on its own, without Tauri, AppKit or WebKit linked in: the baseline for
//! milestone 1's memory comparison (distribution plan R14). Same arguments as
//! `Bellwether serve` (the word `serve` itself is optional). Never shipped.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).skip_while(|a| a == "serve").collect();
    std::process::exit(bellwether_brain::cli_serve(&args));
}
