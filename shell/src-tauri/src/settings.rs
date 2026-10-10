//! Where pasture lives, its config, and the token. Read-only: the shell never writes
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
        // PASTURE_AGENT_DIR wins; otherwise the agent directory this binary was built
        // from: the shell runs from a checkout, next to its brain and config.
        let agent_dir = std::env::var_os("PASTURE_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
        let agent_dir = agent_dir.canonicalize().map_err(|e| format!("agent directory {}: {e}", agent_dir.display()))?;
        let data_dir = std::env::var_os("PASTURE_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| agent_dir.join("data"));

        // PASTURE_CONFIG (a test aid, read-only like the rest): another config file,
        // e.g. one with a dummy name in privacy.sharingProcesses (Milestone 7 (b)).
        let real = agent_dir.join("config.json");
        let file = match std::env::var_os("PASTURE_CONFIG") {
            Some(p) => PathBuf::from(p),
            None if real.exists() => real,
            None => agent_dir.join("config.example.json"),
        };
        let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let config: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;
        let port = config.get("port").and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).ok_or("config: port must be 1024–65535")? as u16;
        let node_path = config.pointer("/brain/nodePath").and_then(Value::as_str).unwrap_or("node").to_string();

        // The token: the environment first (PASTURE_HOOK_TOKEN), then the .env (env_file).
        let token = std::env::var("PASTURE_HOOK_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(|| token_from_env_file(&env_file(&agent_dir)));

        Ok(Settings { agent_dir, data_dir, port, node_path, token, config })
    }

    pub fn f64_at(&self, pointer: &str, default: f64) -> f64 {
        self.config.pointer(pointer).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default)
    }
}

/// Reads exactly one key from a dotenv file; every other line is ignored and never
/// kept or logged. Empty means unset.
/// Which .env holds the token: the same rule as the CLI (brain/envpath.mjs). PASTURE_ENV
/// if set; else the repository's own `.env`; else, when the repository sits at
/// `agents/<name>` inside a hub and has no `.env` of its own, the hub's.
pub fn env_file(agent_dir: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("PASTURE_ENV").filter(|p| !p.is_empty()) {
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

fn token_from_env_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_env_value(&text, "PASTURE_HOOK_TOKEN").filter(|v| !v.is_empty())
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
            let got = parse_env_value(text, "PASTURE_HOOK_TOKEN");
            if got != expected {
                failures.push(format!("{}: expected {:?}, got {:?}", v["name"], expected, got));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn empty_means_unset_and_other_lines_are_ignored() {
        let dir = std::env::temp_dir().join(format!("pasture-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(".env");
        std::fs::write(&f, "OTHER=secret\n# comment\nexport PASTURE_HOOK_TOKEN=\"abc_DEF-123\" # mine\n").unwrap();
        assert_eq!(token_from_env_file(&f).as_deref(), Some("abc_DEF-123"));
        std::fs::write(&f, "PASTURE_HOOK_TOKEN=\n").unwrap();
        assert_eq!(token_from_env_file(&f), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_file_is_the_repos_own_then_a_hubs() {
        let base = std::env::temp_dir().join(format!("pasture-envfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let alone = base.join("bellwether");
        std::fs::create_dir_all(&alone).unwrap();
        assert_eq!(env_file(&alone), alone.join(".env"), "no .env anywhere: the repo's own (where token init writes)");
        let agent = base.join("hub").join("agents").join("pasture");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(base.join("hub").join(".env"), "PASTURE_HOOK_TOKEN=x\n").unwrap();
        assert_eq!(env_file(&agent), agent.join("..").join("..").join(".env"), "inside a hub with no .env of its own");
        std::fs::write(agent.join(".env"), "PASTURE_HOOK_TOKEN=y\n").unwrap();
        assert_eq!(env_file(&agent), agent.join(".env"), "its own .env wins");
        let _ = std::fs::remove_dir_all(&base);
    }
}
