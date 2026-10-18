#!/usr/bin/env node
/**
 * Fetches the pinned eSheep art for development and CI (distribution plan, milestone 11,
 * M11-d). A dev script, never shipped: the app has no `pets fetch` (it refuses it and
 * points at `pets import`).
 *
 *   node scripts/fetch-pets.mjs                  every pinned pet
 *   node scripts/fetch-pets.mjs esheep64         one of them
 *   node scripts/fetch-pets.mjs --data <dir>     into <dir>/pets/ (default: $BELLWETHER_HOME,
 *                                                else this checkout's data/)
 *
 * It downloads each pet's animations.xml from Adrianotiger/desktopPet at the commit
 * pets.lock.json pins (`repo`, `commit`, `pets`), checks it against the pinned sha256
 * before writing it, and records it in <dir>/pets/manifest.json. A file already present
 * with the right hash is not downloaded again; a download whose hash differs is refused
 * and nothing is written. The art is never committed or bundled (see NOTICE): it lives in
 * data/pets/, which is gitignored. The clean-room tripwire in test/pets-bundled.test.mjs
 * reads it there.
 *
 * Moved out of the Node brain's `bellwether pets fetch` (brain/pets.mjs at v0.2.0-rc.3).
 * Exit 0 when every pet is present and verified, 1 on a failed or refused download, 3 on
 * bad usage or a missing pin.
 */
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// The pins live in pets.lock.json, shared with the app, which checks the same hashes
// before it loads a pet.
export const LOCK_FILE = path.join(REPO, 'pets.lock.json');
const LOCK = readLock();

function readLock() {
  try {
    const v = JSON.parse(fs.readFileSync(LOCK_FILE, 'utf8'));
    if (v && typeof v.repo === 'string' && typeof v.commit === 'string' && v.pets && typeof v.pets === 'object') return v;
  } catch {}
  return { repo: '', commit: '', pets: {}, missing: true };
}

export const PET_SOURCE = { repo: LOCK.repo, commit: LOCK.commit };

/** folder -> path in the repository and the sha256 of the file at the pinned commit. */
export const PINNED_PETS = Object.freeze(Object.assign(Object.create(null), LOCK.pets));

const MAX_BYTES = 16 * 1024 * 1024;

/** A failure with the exit code the script ends with. */
export class FetchError extends Error {
  constructor(message, exitCode = 1) {
    super(message);
    this.exitCode = exitCode;
  }
}

export function petUrl(entry) {
  return `https://raw.githubusercontent.com/${PET_SOURCE.repo}/${PET_SOURCE.commit}/${entry.path}`;
}

/**
 * Fetches every pinned pet (or the named ones) into `<dataDir>/pets/<folder>/`.
 * A file already present with the right hash is not downloaded again. A download
 * whose hash differs is refused and nothing is written.
 */
export async function fetchPets(dataDir, { only = null, fetchImpl = fetch, log = () => {}, pinned = PINNED_PETS } = {}) {
  if (pinned === PINNED_PETS && LOCK.missing) throw new FetchError(`${LOCK_FILE} is missing or invalid, so no pet is pinned; restore it from git`, 3);
  const names = only ?? Object.keys(pinned);
  const results = [];
  for (const folder of names) {
    const entry = Object.hasOwn(pinned, folder) ? pinned[folder] : undefined;
    if (!entry) throw new FetchError(`unknown pet "${folder}"; pinned pets: ${Object.keys(pinned).join(', ')}`, 3);
    const dir = path.join(dataDir, 'pets', folder);
    const file = path.join(dir, 'animations.xml');
    if (fs.existsSync(file) && sha256(fs.readFileSync(file)) === entry.sha256) {
      log(`${folder}: already present and verified`);
      results.push({ folder, status: 'present' });
      continue;
    }
    const url = petUrl(entry);
    log(`${folder}: downloading ${url}`);
    let res;
    try {
      res = await fetchImpl(url);
    } catch (e) {
      throw new FetchError(`${folder}: download failed (${e.cause?.code ?? e.message}). Check the network and try again.`);
    }
    if (!res.ok) throw new FetchError(`${folder}: ${url} answered HTTP ${res.status}`);
    const bytes = await readCapped(res, folder);
    const got = sha256(bytes);
    if (got !== entry.sha256) throw new FetchError(`${folder}: sha256 mismatch (expected ${entry.sha256}, got ${got}). Nothing was written.`);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(file, bytes);
    results.push({ folder, status: 'downloaded', bytes: bytes.length });
  }
  const manifest = path.join(dataDir, 'pets', 'manifest.json');
  const current = Object.fromEntries(names.map((f) => [f, { folder: f, sourceRepo: PET_SOURCE.repo, sourceCommit: PET_SOURCE.commit, path: pinned[f].path, sha256: pinned[f].sha256 }]));
  let all = {};
  try {
    all = JSON.parse(fs.readFileSync(manifest, 'utf8'));
  } catch {}
  fs.mkdirSync(path.dirname(manifest), { recursive: true });
  fs.writeFileSync(manifest, JSON.stringify({ ...(all && typeof all === 'object' && !Array.isArray(all) ? all : {}), ...current }, null, 2) + '\n');
  return results;
}

/**
 * Reads a response body, refusing as soon as it passes MAX_BYTES (an endless or
 * oversized stream is cancelled, not buffered), and turning a connection that dies
 * mid-body into an error with the cause.
 */
async function readCapped(res, folder) {
  if (!res.body) return Buffer.alloc(0);
  const reader = res.body.getReader();
  const chunks = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > MAX_BYTES) {
        await reader.cancel().catch(() => {});
        throw new FetchError(`${folder}: the download is larger than ${MAX_BYTES / 1024 / 1024} MB; refusing it. Nothing was written.`);
      }
      chunks.push(value);
    }
  } catch (e) {
    if (e instanceof FetchError) throw e;
    throw new FetchError(`${folder}: the download was interrupted (${e.cause?.code ?? e.message}). Nothing was written; try again.`);
  }
  return Buffer.concat(chunks.map((c) => Buffer.from(c.buffer, c.byteOffset, c.byteLength)));
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

/** The arguments: `--data <dir>` (or `--data=<dir>`) and at most one pet name. */
export function parseArgs(argv, env = process.env) {
  let data = null;
  const names = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--data') {
      data = argv[++i];
      if (!data) throw new FetchError('--data needs a directory', 3);
    } else if (a.startsWith('--data=')) data = a.slice(7);
    else if (a === '--help' || a === '-h') return { help: true };
    else if (a.startsWith('-')) throw new FetchError(`unknown option ${JSON.stringify(a)}; usage: node scripts/fetch-pets.mjs [--data <dir>] [<pet>]`, 3);
    else names.push(a);
  }
  if (names.length > 1) throw new FetchError('one pet at a time; usage: node scripts/fetch-pets.mjs [--data <dir>] [<pet>]', 3);
  if (!data) data = env.BELLWETHER_HOME || path.join(REPO, 'data');
  return { data: path.resolve(data), only: names.length ? names : null };
}

async function main() {
  try {
    const o = parseArgs(process.argv.slice(2));
    if (o.help) {
      process.stdout.write('usage: node scripts/fetch-pets.mjs [--data <dir>] [<pet>]\n(see the comment at the top of scripts/fetch-pets.mjs)\n');
      return;
    }
    const results = await fetchPets(o.data, { only: o.only, log: (l) => process.stderr.write(`fetch-pets: ${l}\n`) });
    for (const r of results) process.stdout.write(`${r.folder}: ${r.status}${r.bytes ? ` (${r.bytes} bytes, sha256 verified)` : ''} -> ${path.join(o.data, 'pets', r.folder)}\n`);
  } catch (e) {
    process.stderr.write(`fetch-pets: ${e.message}\n`);
    process.exit(e instanceof FetchError ? e.exitCode : 1);
  }
}

if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) main();
