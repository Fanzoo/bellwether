import fs from 'node:fs';
import path from 'node:path';
import { CliError, EXIT, USER_SETTINGS } from './util.mjs';

/**
 * Guards for the two commands that write to a path the user names: `hooks print
 * --out` (replaces the file) and `record --out` (appends to it). A typo there must
 * not destroy data, so:
 *   - a Claude Code settings file is always refused, --force or not (`hooks install`
 *     is the only way pasture edits settings, with a diff and a backup);
 *   - a directory, or anything that isn't a regular file, is refused with a message;
 *   - an existing file is replaced only if it is already a pasture hooks file
 *     (or with --force), and appended to only if it is already a pasture recording.
 */
export function guardOutFile(target, kind, { force = false } = {}) {
  let real = target;
  let stat = null;
  try {
    stat = fs.statSync(target);
    real = fs.realpathSync(target);
  } catch (e) {
    if (e.code !== 'ENOENT') throw new CliError(`cannot use ${target}: ${e.code}`, EXIT.BAD_CONFIG);
  }
  if (isClaudeSettingsPath(target) || isClaudeSettingsPath(real)) {
    throw new CliError(`refusing to write ${target}: it is a Claude Code settings file. Use \`pasture hooks install\` to change settings (it shows a diff and keeps a backup).`, EXIT.BAD_CONFIG);
  }
  if (stat === null) return;
  if (stat.isDirectory()) throw new CliError(`${target} is a directory; give a file path, e.g. ${path.join(target, kind === 'print' ? 'hooks-only.json' : 'recording.jsonl')}`, EXIT.BAD_CONFIG);
  if (!stat.isFile()) throw new CliError(`${target} is not a regular file; refusing to write it`, EXIT.BAD_CONFIG);
  const text = fs.readFileSync(target, 'utf8');
  if (kind === 'print') {
    if (force || isPastureHooksFile(text)) return;
    throw new CliError(`${target} already exists and is not a pasture hooks file, so it was left alone. Choose another path, or pass --force to replace it.`, EXIT.BAD_CONFIG);
  }
  if (text.length === 0 || isPastureRecording(text)) return;
  throw new CliError(`${target} already exists and is not a pasture recording, so nothing was appended. Choose another path.`, EXIT.BAD_CONFIG);
}

/** settings.json / settings.local.json in a .claude directory, managed-settings.json anywhere, or the user file itself. */
export function isClaudeSettingsPath(p) {
  const base = path.basename(p).toLowerCase();
  const dir = path.basename(path.dirname(p)).toLowerCase();
  if (base === 'managed-settings.json') return true;
  if ((base === 'settings.json' || base === 'settings.local.json') && dir === '.claude') return true;
  return samePath(p, USER_SETTINGS);
}

function samePath(a, b) {
  const norm = (x) => {
    try {
      return fs.realpathSync(x);
    } catch {
      return path.resolve(x);
    }
  };
  return norm(a) === norm(b);
}

/** Exactly what `hooks print` writes: only a `hooks` key, and only pasture http handlers in it. */
export function isPastureHooksFile(text) {
  let v;
  try {
    v = JSON.parse(text);
  } catch {
    return false;
  }
  if (!v || typeof v !== 'object' || Array.isArray(v) || Object.keys(v).join() !== 'hooks') return false;
  const groups = v.hooks && typeof v.hooks === 'object' && !Array.isArray(v.hooks) ? Object.values(v.hooks) : [];
  const handlers = groups.flatMap((g) => (Array.isArray(g) ? g : [null])).flatMap((g) => (Array.isArray(g?.hooks) ? g.hooks : [null]));
  return (
    handlers.length > 0 &&
    handlers.every((h) => h?.type === 'http' && typeof h.url === 'string' && /^http:\/\/127\.0\.0\.1:\d+\/hook/.test(h.url) && typeof h.headers?.['X-Pasture-Token'] === 'string')
  );
}

/** Every line is a recording line: {normalised, rawKeys, headersPresent}. */
export function isPastureRecording(text) {
  const lines = text.split('\n').filter((l) => l.trim() !== '');
  if (lines.length === 0) return false;
  return lines.every((l) => {
    try {
      const o = JSON.parse(l);
      return !!o && typeof o === 'object' && !Array.isArray(o) && Object.keys(o).sort().join() === 'headersPresent,normalised,rawKeys';
    } catch {
      return false;
    }
  });
}
