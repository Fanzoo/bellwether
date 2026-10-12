//! Where bellwether lives, its config, and the token. Read-only: the shell never writes
//! config.json or the .env.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Settings {
    pub agent_dir: PathBuf,
    pub data_dir: PathBuf,
    pub port: u16,
    pub node_path: String,
    pub token: Option<String>,
    pub config: Value,
}

impl Settings {
    pub fn load() -> Result<Settings, String> {
        // BELLWETHER_AGENT_DIR wins; otherwise the agent directory this binary was built
        // from: the shell runs from a checkout, next to its brain and config.
        let agent_dir = env_os("AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
        let agent_dir = agent_dir.canonicalize().map_err(|e| format!("agent directory {}: {e}", agent_dir.display()))?;
        let data_dir = env_os("DATA_DIR").map(PathBuf::from).unwrap_or_else(|| agent_dir.join("data"));

        // BELLWETHER_CONFIG (a test aid, read-only like the rest): another config file,
        // e.g. one with a dummy name in privacy.sharingProcesses (Milestone 7 (b)).
        let real = agent_dir.join("config.json");
        let file = match env_os("CONFIG") {
            Some(p) => PathBuf::from(p),
            None if real.exists() => real,
            None => agent_dir.join("config.example.json"),
        };
        let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let config: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;
        let port = config.get("port").and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).ok_or("config: port must be 1024–65535")? as u16;
        let node_path = config.pointer("/brain/nodePath").and_then(Value::as_str).unwrap_or("node").to_string();

        // The token, in the CLI's order (brain/envload.mjs): an exported
        // BELLWETHER_HOOK_TOKEN, then one in the .env (env_file), then the old name,
        // exported, then in the .env.
        let _ = AGENT_DIR.set(agent_dir.clone());
        let token = token_now();

        Ok(Settings { agent_dir, data_dir, port, node_path, token, config })
    }

    pub fn f64_at(&self, pointer: &str, default: f64) -> f64 {
        self.config.pointer(pointer).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default)
    }
}

/// `BELLWETHER_<name>` from the environment, or else the name it had before the
/// rename, `PASTURE_<name>` (read for one release; brain/envpath.mjs does the same).
pub fn env_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("BELLWETHER_{name}")).or_else(|| std::env::var_os(format!("PASTURE_{name}")))
}

/// Which .env holds the token: the same rule as the CLI (brain/envpath.mjs). BELLWETHER_ENV
/// if set; else the repository's own `.env`; else, when the repository sits at
/// `agents/<name>` inside a hub and has no `.env` of its own, the hub's.
pub fn env_file(agent_dir: &Path) -> PathBuf {
    if let Some(p) = env_os("ENV").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let own = agent_dir.join(".env");
    if own.exists() {
        return own;
    }
    if agent_dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "agents") {
        let hub = agent_dir.join("..").join("..").join(".env");
        if hub.exists() {
            return hub;
        }
    }
    own
}

static AGENT_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The hook token as it is right now: the environment, then the .env, read again on
/// every call. A strip started before `bellwether token init` uses it to connect once
/// the token appears (brain.rs), without a restart.
pub fn token_now() -> Option<String> {
    token_from(AGENT_DIR.get().map(PathBuf::as_path), |k| std::env::var(k).ok())
}

fn token_from(agent_dir: Option<&Path>, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let text = agent_dir.and_then(|d| std::fs::read_to_string(env_file(d)).ok()).unwrap_or_default();
    token_in_order(var, &text)
}

/// The hook token: an exported BELLWETHER_HOOK_TOKEN, then the .env's, then the old
/// name exported, then the old name in the .env. Only those keys are read from the
/// file; every other line is ignored and never kept or logged. Empty means unset.
fn token_in_order(var: impl Fn(&str) -> Option<String>, env_text: &str) -> Option<String> {
    let set = |v: Option<String>| v.filter(|v| !v.is_empty());
    set(var("BELLWETHER_HOOK_TOKEN"))
        .or_else(|| set(parse_env_value(env_text, "BELLWETHER_HOOK_TOKEN")))
        .or_else(|| set(var("PASTURE_HOOK_TOKEN")))
        .or_else(|| set(parse_env_value(env_text, "PASTURE_HOOK_TOKEN")))
}

#[cfg(test)]
fn token_from_env_file(path: &Path) -> Option<String> {
    token_in_order(|_| None, &std::fs::read_to_string(path).ok()?)
}

/// Reads one key the way Node's `--env-file` does (Node 22.14, which the brain runs
/// with), so the shell and the brain always see the same token. The rules, pinned by
/// the shared vectors in test/fixtures/env-vectors.json:
/// - `\r\n` counts as `\n`; a line starting with `#` (after leading spaces) is skipped;
/// - the key is what precedes the first `=`, trimmed of spaces, with one `export `
///   prefix removed (`export  KEY`, two spaces, is not KEY);
/// - a value starting with `"`, `'` or `` ` `` is what lies before the matching
///   closing quote (`\n` becomes a newline inside double quotes); with no closing
///   quote the value is the rest of the line, quote included;
/// - an unquoted value stops at `#` and is trimmed of spaces (tabs are kept);
/// - the last assignment wins.
pub fn parse_env_value(text: &str, wanted: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let mut found = None;
    for raw in text.split('\n') {
        let line = raw.trim_start_matches(' ');
        if line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let mut key = line[..eq].trim_matches(' ');
        if let Some(rest) = key.strip_prefix("export ") {
            key = rest;
        }
        if key != wanted {
            continue;
        }
        let v = line[eq + 1..].trim_start_matches(' ');
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'' | '`')) => match v[1..].find(q) {
                Some(end) => {
                    let inner = &v[1..1 + end];
                    if q == '"' { inner.replace("\\n", "\n") } else { inner.to_string() }
                }
                None => v.trim_end_matches(' ').to_string(),
            },
            _ => v[..v.find('#').unwrap_or(v.len())].trim_matches(' ').to_string(),
        };
        found = Some(value);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same vectors test/env-vectors.test.mjs checks against Node itself.
    #[test]
    fn matches_nodes_env_file_parser() {
        let json: serde_json::Value = serde_json::from_str(include_str!("../../../test/fixtures/env-vectors.json")).unwrap();
        let vectors = json["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 20);
        let mut failures = Vec::new();
        for v in vectors {
            let text = v["text"].as_str().unwrap();
            let expected = v["expected"].as_str().map(str::to_string);
            let got = parse_env_value(text, "BELLWETHER_HOOK_TOKEN");
            if got != expected {
                failures.push(format!("{}: expected {:?}, got {:?}", v["name"], expected, got));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn empty_means_unset_and_other_lines_are_ignored() {
        let dir = std::env::temp_dir().join(format!("bellwether-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(".env");
        std::fs::write(&f, "OTHER=secret\n# comment\nexport BELLWETHER_HOOK_TOKEN=\"abc_DEF-123\" # mine\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("abc_DEF-123"));
        std::fs::write(&f, "BELLWETHER_HOOK_TOKEN=\n").unwrap();
        assert_eq!(token_from_env_file(&f), None);
        // the name before the rename is still read, and the new one wins over it
        std::fs::write(&f, "PASTURE_HOOK_TOKEN=old_TOKEN-123\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("old_TOKEN-123"));
        std::fs::write(&f, "PASTURE_HOOK_TOKEN=old_TOKEN-123\nBELLWETHER_HOOK_TOKEN=new_TOKEN-456\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("new_TOKEN-456"));
        // the order with the environment: exported new, file new, exported old, file old
        let file_new_and_old = "PASTURE_HOOK_TOKEN=file_old\nBELLWETHER_HOOK_TOKEN=file_new\n";
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(token_in_order(env(&[("BELLWETHER_HOOK_TOKEN", "env_new"), ("PASTURE_HOOK_TOKEN", "env_old")]), file_new_and_old).as_deref(), Some("env_new"));
        assert_eq!(token_in_order(env(&[("PASTURE_HOOK_TOKEN", "env_old")]), file_new_and_old).as_deref(), Some("file_new"), "a stale exported old name doesn't beat the .env's new one");
        assert_eq!(token_in_order(env(&[("PASTURE_HOOK_TOKEN", "env_old")]), "PASTURE_HOOK_TOKEN=file_old\n").as_deref(), Some("env_old"));
        assert_eq!(token_in_order(env(&[]), "PASTURE_HOOK_TOKEN=file_old\n").as_deref(), Some("file_old"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_written_later_is_found_on_the_next_look() {
        let dir = std::env::temp_dir().join(format!("bellwether-later-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(token_from(Some(&dir), |_| None), None, "no token yet");
        std::fs::write(dir.join(".env"), "BELLWETHER_HOOK_TOKEN=appearedLater_0123456789\n").unwrap();
        assert_eq!(token_from(Some(&dir), |_| None).as_deref(), Some("appearedLater_0123456789"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_file_is_the_repos_own_then_a_hubs() {
        let base = std::env::temp_dir().join(format!("bellwether-envfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let alone = base.join("bellwether");
        std::fs::create_dir_all(&alone).unwrap();
        assert_eq!(env_file(&alone), alone.join(".env"), "no .env anywhere: the repo's own (where token init writes)");
        let agent = base.join("hub").join("agents").join("bellwether");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(base.join("hub").join(".env"), "BELLWETHER_HOOK_TOKEN=x\n").unwrap();
        assert_eq!(env_file(&agent), agent.join("..").join("..").join(".env"), "inside a hub with no .env of its own");
        std::fs::write(agent.join(".env"), "BELLWETHER_HOOK_TOKEN=y\n").unwrap();
        assert_eq!(env_file(&agent), agent.join(".env"), "its own .env wins");
        let _ = std::fs::remove_dir_all(&base);
    }
}
