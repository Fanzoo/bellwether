//! The config: defaults merged under `config.json`, and the checks the brain makes (a port
//! of brain/defaults.mjs and brain/config.mjs, pinned by test/fixtures/config-vectors.json).
//!
//! Where it is read (distribution plan D3, Configuration): `BELLWETHER_CONFIG` if set (a
//! read-only alternative file, for tests), else `config.json` in the app's home, else the
//! compiled-in defaults. Every key is optional in the app, `port` included (47417); the
//! repository's Node brain requires `port` (test/DIVERGENCES.md).

use std::path::{Path, PathBuf};

use crate::json::{self, Js, Obj};
use crate::redact::{self, PatternError};

pub const DEFAULT_PORT: u16 = 47417;

/// Session colours (brain/defaults.mjs): orange, blue, black, red, cyan, brown, green, pink.
pub const DEFAULT_PALETTE: [&str; 8] = ["#FF8C00", "#1F5BFF", "#202020", "#E8202A", "#00C2D4", "#8B4513", "#12A84A", "#FF4FB0"];

/// Milestone 3 to 10's default; a config.json copied from the old example has it verbatim.
pub const OLD_DEFAULT_PALETTE: [&str; 8] = ["#ffffff", "#7fb3ff", "#ffb36b", "#9be37d", "#f28bd0", "#c9a2ff", "#ffe066", "#ff7b7b"];

/// Exit 3: bad config or token.
pub const BAD_CONFIG: i32 = 3;

/// A config problem: the message the CLI prints, and its exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub message: String,
    pub code: i32,
}

impl ConfigError {
    pub fn new(message: impl Into<String>) -> ConfigError {
        ConfigError { message: message.into(), code: BAD_CONFIG }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<PatternError> for ConfigError {
    fn from(e: PatternError) -> ConfigError {
        ConfigError::new(e.to_string())
    }
}

fn palette(colours: &[&str]) -> Js {
    Js::Arr(colours.iter().map(|c| Js::str(c)).collect())
}

/// The defaults for every key the brain reads (DEFAULTS in brain/defaults.mjs).
pub fn defaults() -> Js {
    let behaviour = Obj::new()
        .with("busyWindowSec", 8.0)
        .with("grazeAfterSec", 20.0)
        .with("sleepAfterMin", 10.0)
        .with("minDwellMs", 1500.0)
        .with("reapAfterHours", 12.0)
        .with("maxLambsShown", 5.0)
        .with("thinkingTimeoutSec", 120.0)
        .with("lambTimeoutMin", 30.0)
        .with("toolTimeoutMin", 30.0);
    Js::Obj(
        Obj::new()
            .with("port", DEFAULT_PORT as f64)
            .with("pets", Obj::new().with("maxLambsShown", 5.0).with("palette", palette(&DEFAULT_PALETTE)))
            .with("behaviour", behaviour)
            .with("bubble", Obj::new().with("detail", "full").with("maxChars", 60.0).with("redactPatterns", Js::Arr(vec![]))),
    )
}

/// Objects merge, everything else replaces.
fn merge(base: &Js, over: &Js) -> Js {
    match (base, over) {
        (Js::Obj(b), Js::Obj(o)) => {
            let mut out = b.clone();
            for (k, v) in o.iter() {
                let merged = merge(b.get(k).unwrap_or(&Js::Undefined), v);
                out.set(k.clone(), merged);
            }
            Js::Obj(out)
        }
        (_, Js::Undefined) => base.clone(),
        _ => over.clone(),
    }
}

fn is_old_default_palette(p: &Js) -> bool {
    match p {
        Js::Arr(a) => a.len() == OLD_DEFAULT_PALETTE.len() && a.iter().zip(OLD_DEFAULT_PALETTE).all(|(c, old)| c.as_str().is_some_and(|c| c.to_lowercase() == old)),
        _ => false,
    }
}

/// The merged config, and whether the old default palette was swapped for the new one.
#[derive(Clone, Debug)]
pub struct Merged {
    pub config: Js,
    pub palette_upgraded: bool,
}

/// `withDefaults(config)`: merged over the defaults, the old default palette upgraded, and
/// the keys the brain reads checked.
pub fn with_defaults(config: &Js) -> Result<Merged, ConfigError> {
    let mut c = merge(&defaults(), config);
    let mut upgraded = false;
    if is_old_default_palette(c.get("pets").get("palette")) {
        if let Js::Obj(o) = &mut c {
            if let Some(Js::Obj(pets)) = o.get_mut("pets") {
                pets.set("palette", palette(&DEFAULT_PALETTE));
                upgraded = true;
            }
        }
    }
    validate_brain_config(&c)?;
    Ok(Merged { config: c, palette_upgraded: upgraded })
}

fn validate_brain_config(c: &Js) -> Result<(), ConfigError> {
    let b = c.get("behaviour");
    for k in ["busyWindowSec", "grazeAfterSec", "sleepAfterMin", "minDwellMs", "reapAfterHours", "thinkingTimeoutSec", "lambTimeoutMin", "toolTimeoutMin"] {
        if !b.get(k).as_num().is_some_and(|n| n >= 0.0) {
            return Err(ConfigError::new(format!("config: behaviour.{k} must be a number ≥ 0")));
        }
    }
    if !(b.get("maxLambsShown").is_integer() && b.get("maxLambsShown").as_num().unwrap() >= 0.0) {
        return Err(ConfigError::new("config: behaviour.maxLambsShown must be an integer ≥ 0"));
    }
    let bubble = c.get("bubble");
    if !matches!(bubble.get("detail").as_str(), Some("full" | "tool")) {
        return Err(ConfigError::new("config: bubble.detail must be \"full\" or \"tool\""));
    }
    let mc = bubble.get("maxChars");
    if !(mc.is_integer() && (8.0..=500.0).contains(&mc.as_num().unwrap())) {
        return Err(ConfigError::new("config: bubble.maxChars must be an integer from 8 to 500"));
    }
    let Some(patterns) = bubble.get("redactPatterns").as_arr() else {
        return Err(ConfigError::new("config: bubble.redactPatterns must be an array of regular expressions"));
    };
    for p in patterns {
        match p.as_str() {
            Some(s) if !s.is_empty() => {
                redact::compile_pattern(s)?;
            }
            _ => return Err(PatternError::NotAString.into()),
        }
    }
    let pal = c.get("pets").get("palette");
    if !pal.as_arr().is_some_and(|a| !a.is_empty()) {
        return Err(ConfigError::new("config: pets.palette must be a non-empty array"));
    }
    let tint = c.get("pets").get("tint");
    if !tint.is_undefined() && !matches!(tint.as_str(), Some("horns" | "whole")) {
        return Err(ConfigError::new("config: pets.tint must be \"horns\" (colour only the horns) or \"whole\" (tint the whole sheep)"));
    }
    Ok(())
}

/// Depth-first search for a key anywhere in the config, by `Object.entries` order (arrays
/// included); its dotted path.
fn find_key(value: &Js, key: &str, prefix: &str) -> Option<String> {
    let entries: Vec<(String, &Js)> = match value {
        Js::Obj(o) => o.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Js::Arr(a) => a.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => return None,
    };
    for (k, v) in entries {
        let here = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        if k.to_lowercase() == key {
            return Some(here);
        }
        if let Some(deeper) = find_key(v, key, &here) {
            return Some(deeper);
        }
    }
    None
}

/// `validateConfig(config, name)`: an object, no `host` anywhere, a port from 1024 to 65535.
pub fn validate_config(config: &Js, name: &str) -> Result<(), ConfigError> {
    if !config.is_plain_object() {
        return Err(ConfigError::new(format!("{name} must be a JSON object")));
    }
    if let Some(host) = find_key(config, "host", "") {
        return Err(ConfigError::new(format!("{name} sets \"{host}\". The listener always binds 127.0.0.1 and that is not configurable; remove the key.")));
    }
    let port = config.get("port");
    if !(port.is_integer() && (1024.0..=65535.0).contains(&port.as_num().unwrap())) {
        let got = if port.is_undefined() { "undefined".to_string() } else { json::stringify(port) };
        return Err(ConfigError::new(format!("{name}: \"port\" must be an integer between 1024 and 65535 (got {got})")));
    }
    Ok(())
}

/// The config the brain and the CLI run with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The merged config (defaults under the file), as JS values.
    pub merged: Js,
    pub palette_upgraded: bool,
    /// The file it came from; None when there was none (compiled-in defaults).
    pub file: Option<PathBuf>,
}

impl Config {
    pub fn port(&self) -> u16 {
        self.merged.get("port").as_num().unwrap_or(DEFAULT_PORT as f64) as u16
    }

    pub fn max_chars(&self) -> usize {
        self.merged.get("bubble").get("maxChars").as_num().unwrap_or(60.0) as usize
    }

    pub fn redact_patterns(&self) -> Vec<String> {
        self.merged.get("bubble").get("redactPatterns").as_arr().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    pub fn palette(&self) -> Vec<String> {
        self.merged.get("pets").get("palette").as_arr().map(|a| a.iter().map(|c| c.as_str().map(str::to_string).unwrap_or_else(|| json::stringify(c))).collect()).unwrap_or_default()
    }

    /// The defaults alone (no file).
    pub fn defaults() -> Config {
        let m = with_defaults(&Js::Obj(Obj::new())).expect("the defaults are valid");
        Config { merged: m.config, palette_upgraded: false, file: None }
    }

    /// From JS values (a test, or a config already parsed): validated and merged.
    pub fn from_js(v: &Js, name: &str) -> Result<Config, ConfigError> {
        let mut v = v.clone();
        if let Js::Obj(o) = &mut v {
            if !o.contains("port") {
                o.set("port", DEFAULT_PORT as f64);
            }
        }
        validate_config(&v, name)?;
        let m = with_defaults(&v)?;
        Ok(Config { merged: m.config, palette_upgraded: m.palette_upgraded, file: None })
    }
}

/// Which file holds the config: `BELLWETHER_CONFIG` (resolved against the current
/// directory), else `<home>/config.json` when it exists.
pub fn config_file(home: &Path, env_config: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = env_config.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return Some(if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) });
    }
    let f = home.join("config.json");
    f.exists().then_some(f)
}

/// Loads and checks the config (`loadConfig` in brain/config.mjs).
pub fn load(home: &Path, env_config: Option<&str>) -> Result<Config, ConfigError> {
    let Some(file) = config_file(home, env_config) else { return Ok(Config::defaults()) };
    let raw = std::fs::read_to_string(&file).map_err(|e| ConfigError::new(format!("cannot read {}: {}", file.display(), io_code(&e))))?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "config".into());
    let v = json::parse(&raw).map_err(|e| ConfigError::new(format!("{name} is not valid JSON: {e}")))?;
    let mut c = Config::from_js(&v, &name)?;
    c.file = Some(file);
    Ok(c)
}

/// Node's `e.code` for the common I/O errors, else the error's own words.
pub fn io_code(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => "ENOENT".into(),
        PermissionDenied => "EACCES".into(),
        IsADirectory => "EISDIR".into(),
        AlreadyExists => "EEXIST".into(),
        _ => e.raw_os_error().map(|n| format!("errno {n}")).unwrap_or_else(|| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_keeps_js_key_order() {
        let c = with_defaults(&json::parse(r#"{"port":47999,"pets":{"default":"x"},"strip":{"a":1}}"#).unwrap()).unwrap();
        assert_eq!(c.config.as_obj().unwrap().keys(), ["port", "pets", "behaviour", "bubble", "strip"]);
        assert_eq!(c.config.get("pets").as_obj().unwrap().keys(), ["maxLambsShown", "palette", "default"]);
    }

    #[test]
    fn a_missing_port_is_the_default_in_the_app() {
        let c = Config::from_js(&json::parse("{}").unwrap(), "config.json").unwrap();
        assert_eq!(c.port(), DEFAULT_PORT);
        assert!(validate_config(&json::parse("{}").unwrap(), "config.json").is_err(), "the Node rule, for the shared vectors");
    }
}
