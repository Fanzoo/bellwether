import fs from 'node:fs';
import path from 'node:path';

/**
 * data/state.json: the only state kept between runs (muted, and per identity its
 * colour, slots and label). A missing or unreadable file starts fresh; the flock
 * itself is rebuilt from incoming events.
 */
export function loadState(file, warn = () => {}) {
  let raw;
  try {
    raw = fs.readFileSync(file, 'utf8');
  } catch (e) {
    if (e.code !== 'ENOENT') warn(`could not read ${file} (${e.code}); starting with no saved colours or slots`);
    return null;
  }
  try {
    const v = JSON.parse(raw);
    if (v && typeof v === 'object' && !Array.isArray(v) && v.identities && typeof v.identities === 'object' && !Array.isArray(v.identities)) return v;
  } catch {}
  warn(`${file} is not a valid state file; starting with no saved colours or slots`);
  return null;
}

export function saveState(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(value, null, 2) + '\n', { mode: 0o600 });
  fs.renameSync(tmp, file);
}
