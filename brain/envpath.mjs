import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Which `.env` holds BELLWETHER_HOOK_TOKEN (read by envload.mjs, written by `token init`;
 * the shell's settings.rs `env_file` follows the same rule):
 *
 * 1. BELLWETHER_ENV (or the old PASTURE_ENV), if set (relative to the directory you ran
 *    bellwether from);
 * 2. `.env` in the repository root, next to `bellwether` (the usual place);
 * 3. when the repository sits at `agents/<name>` inside a larger hub and has no `.env`
 *    of its own, the hub's `.env` two levels up, if there is one.
 *
 * With none of them present, the answer is the repository's own `.env`, so that is
 * where `token init` creates one.
 */
export const REPO_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export function envFilePath(env = process.env, root = REPO_DIR) {
  // The old name counts here too: this runs before adoptLegacyEnv (envload.mjs).
  const named = env.BELLWETHER_ENV !== undefined ? env.BELLWETHER_ENV : env.PASTURE_ENV;
  if (named) return path.resolve(env.BELLWETHER_CWD ?? env.PASTURE_CWD ?? process.cwd(), named);
  const own = path.join(root, '.env');
  if (fs.existsSync(own)) return own;
  if (path.basename(path.dirname(root)) === 'agents') {
    const hub = path.resolve(root, '..', '..', '.env');
    if (fs.existsSync(hub)) return hub;
  }
  return own;
}

/** The PASTURE_* names this process read in place of BELLWETHER_* (doctor shows them). */
export const LEGACY_ENV_USED = new Set();

/**
 * Before the rename every variable was PASTURE_*. For one release each is still read:
 * PASTURE_X fills BELLWETHER_X when BELLWETHER_X isn't set (a set one, even "", wins).
 * Changes `env` in place and remembers which old names it used.
 */
export function adoptLegacyEnv(env = process.env, used = LEGACY_ENV_USED) {
  for (const k of Object.keys(env)) {
    if (!k.startsWith('PASTURE_')) continue;
    const now = `BELLWETHER_${k.slice('PASTURE_'.length)}`;
    if (env[now] === undefined) {
      env[now] = env[k];
      used.add(k);
    }
  }
  return used;
}
