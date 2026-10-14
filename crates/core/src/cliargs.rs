//! The CLI's argument rules (a port of parseArgs and args() in brain/util.mjs and
//! brain/cli.mjs). Strict on purpose, and a tested contract: `--flag value`,
//! `--flag=value` and `--flag`; a flag that takes a value must have a non-empty one;
//! unknown flags and extra positional arguments are refused (exit 3), so a typo is never
//! read as a request to act on a default.

use crate::config::ConfigError;
use crate::json::{Js, Obj};

pub struct Args {
    flags: Obj,
    pub positional: Vec<String>,
}

impl Args {
    pub fn has(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    /// A value flag's value (validated non-empty by `parse`).
    pub fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(Js::as_str)
    }
}

/// `args(argv, {booleans, values, positionals})`.
pub fn parse(argv: &[String], booleans: &[&str], values: &[&str], positionals: usize) -> Result<Args, ConfigError> {
    let mut flags = Obj::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        let Some(rest) = a.strip_prefix("--") else {
            positional.push(a.clone());
            i += 1;
            continue;
        };
        match rest.find('=') {
            Some(eq) => flags.set(&rest[..eq], &rest[eq + 1..]),
            None => match argv.get(i + 1) {
                Some(next) if !booleans.contains(&rest) && !next.starts_with("--") => {
                    flags.set(rest, next.as_str());
                    i += 1;
                }
                _ => flags.set(rest, true),
            },
        }
        i += 1;
    }
    for (k, v) in flags.iter() {
        if booleans.contains(&k.as_str()) {
            if v != &Js::Bool(true) {
                return Err(ConfigError::new(format!("--{k} takes no value")));
            }
        } else if values.contains(&k.as_str()) {
            if !v.as_str().is_some_and(|s| !crate::jsstr::trim(s).is_empty()) {
                let what = if k == "slow" { "ms" } else { "path" };
                return Err(ConfigError::new(format!("--{k} needs a value, e.g. --{k} <{what}>. Nothing was done.")));
            }
        } else {
            return Err(ConfigError::new(format!("unknown option --{k}. Nothing was done.")));
        }
    }
    if positional.len() > positionals {
        return Err(ConfigError::new(format!("unexpected argument \"{}\". Nothing was done.", positional[positionals])));
    }
    Ok(Args { flags, positional })
}

/// JavaScript's `Number(s)` for a command-line value: decimal (with an exponent), hex,
/// octal and binary literals, `Infinity`; surrounding whitespace ignored; "" is 0;
/// anything else NaN.
pub fn js_number(s: &str) -> f64 {
    let t = crate::jsstr::trim(s);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = t.strip_prefix(prefix) {
            return if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) { u128::from_str_radix(digits, radix).map(|n| n as f64).unwrap_or(f64::INFINITY) } else { f64::NAN };
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let valid = !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) && body.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.');
    if !valid {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn strict_like_node() {
        let a = parse(&s(&["file", "--speed", "2", "--json"]), &["json"], &["speed"], 1).unwrap();
        assert_eq!(a.value("speed"), Some("2"));
        assert!(a.has("json"));
        assert_eq!(a.positional, ["file"]);
        assert_eq!(parse(&s(&["--json=1"]), &["json"], &[], 0).err().unwrap().message, "--json takes no value");
        assert!(parse(&s(&["--speed"]), &[], &["speed"], 0).err().unwrap().message.starts_with("--speed needs a value"));
        assert!(parse(&s(&["--speed="]), &[], &["speed"], 0).is_err());
        assert_eq!(parse(&s(&["--nope"]), &[], &[], 0).err().unwrap().message, "unknown option --nope. Nothing was done.");
        assert_eq!(parse(&s(&["a", "b"]), &[], &[], 1).err().unwrap().message, "unexpected argument \"b\". Nothing was done.");
        assert_eq!(parse(&s(&["--__proto__"]), &[], &[], 0).err().unwrap().code, 3);
    }

    #[test]
    fn numbers_like_javascript() {
        for (t, want) in [("2", 2.0), (" 5 ", 5.0), ("1e3", 1000.0), ("0x10", 16.0), ("", 0.0), (".5", 0.5), ("-1", -1.0)] {
            assert_eq!(js_number(t), want, "{t:?}");
        }
        for t in ["abc", "NaN", "inf", "1_000", "0x", "1e", "e5", "--1"] {
            assert!(js_number(t).is_nan(), "{t:?}");
        }
        assert!(js_number("1e999").is_infinite());
    }
}
