import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { envAssignments } from './envfile.mjs';
import { tokenProblem } from './config.mjs';
import { CliError, EXIT } from './util.mjs';

const KEY = 'BELLWETHER_HOOK_TOKEN';
export const OLD_KEY = 'PASTURE_HOOK_TOKEN';

/**
 * `bellwether token init`: puts a fresh BELLWETHER_HOOK_TOKEN into the .env, once. That
 * file holds every account's credentials, so it is never put at risk:
 *
 * - Read the way Node reads it (envfile.mjs): if the *last* BELLWETHER_HOOK_TOKEN line has
 *   a value, nothing changes. If it is empty (`BELLWETHER_HOOK_TOKEN=`, or `= # note`), that
 *   line is filled in place; otherwise one line is appended, after a line break if the
 *   file doesn't end with one.
 * - CRLF files stay CRLF.
 * - The new contents go to a temporary file next to the real file (through a symlink,
 *   so the link survives), with the same mode, flushed to disk, then renamed over it.
 *   The previous contents are kept in `<file>.bak` (mode 0600). A failure leaves the
 *   original untouched and says why, in one line.
 * - A new file is created 0600. The token is never printed.
 * - A file from before the rename, with only PASTURE_HOOK_TOKEN set, gets that same
 *   value under the new name (the old line stays), so the hook entries already
 *   installed keep matching. `value` (from `bellwether migrate`) does the same with a
 *   token read from elsewhere.
 */
export function tokenInit(envFile, { generate = () => crypto.randomBytes(24).toString('base64url'), value = null, force = false } = {}) {
  let real = envFile;
  let text = '';
  let mode = 0o600;
  const exists = fs.existsSync(envFile);
  try {
    if (exists) {
      real = fs.realpathSync(envFile);
      text = fs.readFileSync(real, 'utf8');
      mode = fs.statSync(real).mode & 0o777;
      // Renaming over a read-only file would quietly replace it: refuse instead.
      fs.accessSync(real, fs.constants.W_OK);
    } else if (fs.lstatSync(envFile, { throwIfNoEntry: false })?.isSymbolicLink()) {
      // A link to a file that doesn't exist yet: create the file it points to.
      real = path.resolve(path.dirname(envFile), fs.readlinkSync(envFile));
    }
  } catch (e) {
    throw new CliError(`can't ${e.code === 'EACCES' || e.code === 'EPERM' ? 'write' : 'read'} ${envFile} (${e.code ?? e.message}); nothing was changed`, EXIT.BAD_CONFIG);
  }
  const all = envAssignments(text, KEY);
  const last = all[all.length - 1];
  // A value that is there but can't be used (too short, odd characters, a published
  // example) is left alone too, and said so: only --force replaces it.
  const problem = last && last.value !== '' ? tokenProblem(last.value) : null;
  if (last && last.value !== '' && !(problem && force)) {
    if (problem) return { changed: false, unusable: true, message: `${KEY} is already set in ${envFile} but isn't a usable token (${problem}); left alone. Fix it, or run \`bellwether token init --force\` to replace it` };
    return { changed: false, message: `${KEY} is already set in ${envFile}; left alone (to replace it, edit the file, then re-run \`bellwether hooks install --write\`)` };
  }
  const eol = text.includes('\r\n') ? '\r\n' : '\n';
  const old = problem ? null : envAssignments(text, OLD_KEY).at(-1)?.value || null;
  const from = value ? 'the old install' : old ? `${OLD_KEY} in the same file` : null;
  const token = value || old || generate();
  const assignment = `${KEY}=${token}`;
  let next;
  if (last) {
    const lines = text.split('\n');
    lines[last.line] = assignment + (lines[last.line].endsWith('\r') ? '\r' : '');
    next = lines.join('\n');
  } else {
    next = text + (text && !text.endsWith('\n') ? eol : '') + assignment + eol;
  }
  const dir = path.dirname(real);
  const tmp = path.join(dir, `.${path.basename(real)}.bellwether-${process.pid}-${Date.now()}.tmp`);
  try {
    if (exists) {
      const bak = `${real}.bak`;
      fs.writeFileSync(bak, text, { mode: 0o600 });
      fs.chmodSync(bak, 0o600);
    }
    const fd = fs.openSync(tmp, 'wx', mode);
    try {
      fs.writeSync(fd, next);
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    fs.chmodSync(tmp, mode);
    fs.renameSync(tmp, real);
  } catch (e) {
    try {
      fs.rmSync(tmp, { force: true });
    } catch {}
    throw new CliError(`can't write ${envFile} (${e.code ?? e.message}); it was not changed`, EXIT.BAD_CONFIG);
  }
  const what = problem ? `a new ${KEY} in place of the unusable one` : from ? `${KEY} (the same value as ${from}, so installed hooks keep working)` : `a new ${KEY}`;
  return { changed: true, message: `added ${what} to ${envFile}${last ? ' (filled the empty line)' : ''}${exists ? `; the previous contents are in ${real}.bak` : ''}; the value isn't shown` };
}
