//! The `.env` reader, for importing a legacy token only (distribution plan D3: the app
//! writes no `.env` files). It reads one key the way `node --env-file` does, and matches
//! brain/envfile.mjs on test/fixtures/env-vectors.json and on 10⁵ generated cases (the
//! differential's `env` area). Two shapes where Node itself differs are listed in
//! test/DIVERGENCES.md.

/// The value of the last `wanted=` line (`export` allowed), or None.
pub fn env_value(text: &str, wanted: &str) -> Option<String> {
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
                    if q == '"' {
                        inner.replace("\\n", "\n")
                    } else {
                        inner.to_string()
                    }
                }
                None => v.trim_end_matches(' ').to_string(),
            },
            _ => v[..v.find('#').unwrap_or(v.len())].trim_matches(' ').to_string(),
        };
        found = Some(value);
    }
    found
}
