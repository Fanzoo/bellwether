//! The hook token (distribution plan D3): one line in `<home>/hook-token`, 0600, in a 0700
//! directory. `BELLWETHER_HOOK_TOKEN` in the environment overrides it (tests, CI). Format:
//! 16 or more of `[A-Za-z0-9_-]`, and never a value published as an example.
//!
//! `token init` writes the file, never a `.env` (the in-place `.env` writer is retired with
//! the Node CLI; test/DIVERGENCES.md). It can import a legacy token from a repository
//! install's `.env`, so the hook entries already installed keep matching.

use std::path::{Path, PathBuf};

use crate::config::{io_code, ConfigError};
use crate::persist::{create_private_dir, write_atomic_sync};

/// Token values that have been committed as examples somewhere. Never accepted.
pub const KNOWN_EXAMPLE_TOKENS: [&str; 1] = ["q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh"];

pub fn file(home: &Path) -> PathBuf {
    home.join("hook-token")
}

/// Where the token in use came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Env,
    File(PathBuf),
}

/// Why a token can't be used, in a few words (tokenProblem in brain/config.mjs).
pub fn problem(token: &str) -> Option<&'static str> {
    if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Some("shorter than 16 characters, or not all of [A-Za-z0-9_-]");
    }
    KNOWN_EXAMPLE_TOKENS.contains(&token).then_some("a published example value")
}

/// The format check, with the fix named.
pub fn check(token: &str) -> Result<(), ConfigError> {
    if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(ConfigError::new("the hook token must be at least 16 characters of [A-Za-z0-9_-] (base64url); make a new one with `bellwether token rotate --write`"));
    }
    if KNOWN_EXAMPLE_TOKENS.contains(&token) {
        return Err(ConfigError::new("the hook token is a published example value, so anyone can read it; make your own with `bellwether token rotate --write`"));
    }
    Ok(())
}

/// The token from the file alone: its first line, trimmed.
pub fn read_file(home: &Path) -> Result<Option<String>, ConfigError> {
    let f = file(home);
    match std::fs::read_to_string(&f) {
        Ok(text) => Ok(Some(text.lines().next().unwrap_or("").trim().to_string()).filter(|t| !t.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::new(format!("no hook token: {} can't be read ({})", f.display(), io_code(&e)))),
    }
}

/// The token in use: `env` (BELLWETHER_HOOK_TOKEN) if set and not empty, else the file.
/// Checked. Never logged or printed.
pub fn read(home: &Path, env: Option<String>) -> Result<(String, Source), ConfigError> {
    if let Some(t) = env.filter(|t| !t.is_empty()) {
        check(&t)?;
        return Ok((t, Source::Env));
    }
    let f = file(home);
    let token = read_file(home)?.ok_or_else(|| ConfigError::new(format!("no hook token: BELLWETHER_HOOK_TOKEN is not set, and {} doesn't exist or is empty. Make one with `bellwether token init`", f.display())))?;
    check(&token)?;
    Ok((token, Source::File(f)))
}

/// 24 random bytes, base64url without padding (32 characters), as `token init` makes them.
pub fn generate() -> Result<String, ConfigError> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|e| ConfigError { message: format!("no randomness for a token: {e}"), code: 1 })?;
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(32);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32;
        for shift in [18, 12, 6, 0] {
            out.push(A[((n >> shift) & 63) as usize] as char);
        }
    }
    Ok(out)
}

/// Writes the token file: the directory 0700, the file 0600, through a temp file.
pub fn write(home: &Path, token: &str) -> Result<PathBuf, ConfigError> {
    let f = file(home);
    create_private_dir(home).map_err(|e| ConfigError::new(format!("can't create {} ({})", home.display(), io_code(&e))))?;
    let tmp = home.join(format!(".hook-token.{}.tmp", std::process::id()));
    write_atomic_sync(&f, &tmp, &format!("{token}\n"), 0o600, true).map_err(|e| ConfigError::new(format!("can't write {} ({}); nothing was changed", f.display(), io_code(&e))))?;
    Ok(f)
}

/// The token in a legacy `.env` (BELLWETHER_HOOK_TOKEN, else PASTURE_HOOK_TOKEN), read the
/// way `node --env-file` reads it.
pub fn from_env_file(path: &Path) -> Result<Option<String>, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::new(format!("can't read {} ({})", path.display(), io_code(&e))))?;
    Ok(crate::envfile::env_value(&text, "BELLWETHER_HOOK_TOKEN").filter(|t| !t.is_empty()).or_else(|| crate::envfile::env_value(&text, "PASTURE_HOOK_TOKEN").filter(|t| !t.is_empty())))
}

/// What `token init` did, and the message (which never holds the token).
#[derive(Debug, PartialEq, Eq)]
pub struct Init {
    pub changed: bool,
    /// A token is there but can't be used, and was left alone (the CLI exits 3).
    pub unusable: bool,
    pub message: String,
}

/// `bellwether token init [--from <.env>] [--force]`: a token file, only if there is none.
/// One that is there but unusable is left alone and named, as the Node CLI does with its
/// `.env`; `--force` replaces it.
pub fn init(home: &Path, from: Option<&Path>, force: bool) -> Result<Init, ConfigError> {
    let f = file(home);
    let existing = read_file(home)?;
    let unusable = existing.as_deref().and_then(problem);
    if existing.is_some() {
        match unusable {
            None => return Ok(Init { changed: false, unusable: false, message: format!("a hook token is already set in {}; left alone (to replace it, `bellwether token rotate --write`)", f.display()) }),
            Some(why) if !force => {
                return Ok(Init {
                    changed: false,
                    unusable: true,
                    message: format!("a hook token is already set in {} but isn't a usable token ({why}); left alone. Fix it, or run `bellwether token init --force` to replace it", f.display()),
                });
            }
            Some(_) => {}
        }
    }
    let (token, what) = match from {
        Some(env) => match from_env_file(env)? {
            Some(t) => {
                check(&t).map_err(|e| ConfigError::new(format!("the token in {} can't be used: {}", env.display(), e.message)))?;
                (t, format!("the token from {} (the same value, so installed hooks keep working)", env.display()))
            }
            None => return Err(ConfigError::new(format!("{} has no BELLWETHER_HOOK_TOKEN (or PASTURE_HOOK_TOKEN); nothing was written", env.display()))),
        },
        None => (generate()?, if unusable.is_some() { "a new hook token in place of the unusable one".to_string() } else { "a new hook token".to_string() }),
    };
    write(home, &token)?;
    Ok(Init { changed: true, unusable: false, message: format!("wrote {what} to {} (mode 0600); the value isn't shown", f.display()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_pass_the_check() {
        let t = generate().unwrap();
        assert_eq!(t.len(), 32);
        check(&t).unwrap();
        assert_ne!(t, generate().unwrap());
        assert!(check("short").is_err() && check("has spaces in it but is long").is_err() && check(KNOWN_EXAMPLE_TOKENS[0]).is_err());
    }

    #[test]
    fn init_writes_once_and_imports() {
        let home = std::env::temp_dir().join(format!("bw-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let env = std::env::temp_dir().join(format!("bw-token-{}.env", std::process::id()));
        std::fs::write(&env, "OTHER=1\nexport BELLWETHER_HOOK_TOKEN=\"legacyToken_0123456789\"\n").unwrap();
        let msg = init(&home, Some(&env), false).unwrap().message;
        assert!(msg.contains("the same value") && !msg.contains("legacyToken"), "{msg}");
        assert_eq!(read(&home, None).unwrap().0, "legacyToken_0123456789");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(file(&home)).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert!(init(&home, None, false).unwrap().message.contains("already set"));
        assert!(!init(&home, None, true).unwrap().changed, "--force never replaces a usable token");
        assert_eq!(read(&home, None).unwrap().0, "legacyToken_0123456789");
        assert_eq!(read(&home, Some("envTokenValue_0123456".into())).unwrap().1, Source::Env);
        // An unusable token is left alone and named; --force replaces it.
        write(&home, "short").unwrap();
        let r = init(&home, None, false).unwrap();
        assert!(r.unusable && !r.changed && r.message.contains("isn't a usable token"), "{r:?}");
        assert_eq!(read_file(&home).unwrap().as_deref(), Some("short"));
        let r = init(&home, None, true).unwrap();
        assert!(r.changed && r.message.contains("in place of the unusable one"), "{r:?}");
        check(&read_file(&home).unwrap().unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_file(&env);
    }
}
