//! Times the cleaner on the pathological inputs the Node tests use (tester-m2,
//! tester-m2-recheck). `cargo run -p bellwether-core --example redact_timing`.
fn main() {
    let inputs: Vec<(&str, String)> = vec![
        ("a", "a".repeat(5_000_000)),
        ("spaces", format!("x{}y", " ".repeat(5_000_000))),
        ("urls", "://".repeat(1_500_000)),
        ("userinfo2", format!("http://{}", "a:".repeat(2_000_000))),
        ("pwWords", "password ".repeat(500_000)),
        ("dashP", " -p".repeat(1_500_000)),
        ("dashU", " -u a".repeat(1_000_000)),
        ("quotes", "\"".repeat(5_000_000)),
        ("nested", "token-".repeat(1_000_000)),
        ("ws", "\t \n".repeat(2_000_000)),
    ];
    let builtin = bellwether_core::redact::Cleaner::new(60, &[]).unwrap();
    let example = bellwether_core::redact::Cleaner::new(60, &["(?i)(token|secret|password|api[_-]?key)=\\S+".to_string()]).unwrap();
    for (name, text) in &inputs {
        for (label, c) in [("builtin", &builtin), ("example", &example)] {
            let t = std::time::Instant::now();
            let out = c.clean(text);
            println!("{label:8} {name:10} {:6} ms  {}", t.elapsed().as_millis(), out.chars().take(20).collect::<String>());
        }
    }
}
