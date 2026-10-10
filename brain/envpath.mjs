import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Which `.env` holds PASTURE_HOOK_TOKEN (read by envload.mjs, written by `token init`;
 * the shell's settings.rs `env_file` follows the same rule):
 *
 * 1. PASTURE_ENV, if set (relative to the directory you ran pasture from);
 * 2. `.env` in the repository root, next to `pasture` (the usual place);
 * 3. when the repository sits at `agents/<name>` inside a larger hub and has no `.env`
 *    of its own, the hub's `.env` two levels up, if there is one.
 *
 * With none of them present, the answer is the repository's own `.env`, so that is
 * where `token init` creates one.
 */
export const REPO_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export function envFilePath(env = process.env, root = REPO_DIR) {
  if (env.PASTURE_ENV) return path.resolve(env.PASTURE_CWD ?? process.cwd(), env.PASTURE_ENV);
  const own = path.join(root, '.env');
  if (fs.existsSync(own)) return own;
  if (path.basename(path.dirname(root)) === 'agents') {
    const hub = path.resolve(root, '..', '..', '.env');
    if (fs.existsSync(hub)) return hub;
  }
  return own;
}
