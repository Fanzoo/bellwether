/**
 * How `node --env-file` reads one key (Node 22.14, which the brain runs with). The one
 * JS definition, checked against the shared vectors in test/fixtures/env-vectors.json;
 * the shell's Rust `parse_env_value` (settings.rs) is checked against the same file.
 *
 * - "\r\n" counts as "\n"; a line starting with "#" (after leading spaces) is skipped;
 * - the key is what precedes the first "=", trimmed of spaces, with one "export "
 *   prefix removed ("export  KEY", two spaces, is not KEY);
 * - a value starting with ", ' or ` is what lies before the matching closing quote
 *   ("\n" becomes a newline inside double quotes); with no closing quote, the rest of
 *   the line, quote included;
 * - an unquoted value stops at "#" and is trimmed of spaces (tabs are kept), so
 *   `KEY= # comment` is empty;
 * - the last assignment wins.
 */

/** Every assignment of `key`, in order: the 0-based line it is on and the value Node gives it. */
export function envAssignments(text, key) {
  const out = [];
  text.replace(/\r\n/g, '\n').split('\n').forEach((raw, line) => {
    const l = raw.replace(/^ +/, '');
    if (l.startsWith('#')) return;
    const eq = l.indexOf('=');
    if (eq < 0) return;
    let k = l.slice(0, eq).replace(/^ +| +$/g, '');
    if (k.startsWith('export ')) k = k.slice('export '.length);
    if (k !== key) return;
    const v = l.slice(eq + 1).replace(/^ +/, '');
    const q = v[0];
    let value;
    if (q === '"' || q === "'" || q === '`') {
      const end = v.indexOf(q, 1);
      if (end >= 0) value = q === '"' ? v.slice(1, end).replace(/\\n/g, '\n') : v.slice(1, end);
      else value = v.replace(/ +$/, '');
    } else {
      const hash = v.indexOf('#');
      value = (hash >= 0 ? v.slice(0, hash) : v).replace(/^ +| +$/g, '');
    }
    out.push({ line, value });
  });
  return out;
}

/** The value Node would set for `key`, or null if the file doesn't set it. */
export function envValue(text, key) {
  const all = envAssignments(text, key);
  return all.length ? all[all.length - 1].value : null;
}
