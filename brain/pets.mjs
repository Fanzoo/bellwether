import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { AGENT_DIR, CliError, EXIT } from './util.mjs';

/**
 * `bellwether pets fetch`: downloads the pet files bellwether draws, at a pinned commit of
 * Adrianotiger/desktopPet, and checks each against a pinned sha256 before writing
 * it. The art is never committed to the repository or bundled into a build (see
 * NOTICE): it lives in data/pets/, which is gitignored.
 */

// The pins live in pets.lock.json, shared with the shell, which checks the same
// hashes before it loads a pet (Milestone 3 verification D3).
// A missing or unreadable lock file must not stop the rest of the CLI from loading:
// it leaves no pins, and `pets fetch` (and doctor's pet check) say so.
export const LOCK_FILE = path.join(AGENT_DIR, 'pets.lock.json');
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

export function petUrl(entry) {
  return `https://raw.githubusercontent.com/${PET_SOURCE.repo}/${PET_SOURCE.commit}/${entry.path}`;
}

/**
 * Fetches every pinned pet (or the named ones) into `<dataDir>/pets/<folder>/`.
 * A file already present with the right hash is not downloaded again. A download
 * whose hash differs is refused and nothing is written.
 */
export async function fetchPets(dataDir, { only = null, fetchImpl = fetch, log = () => {}, pinned = PINNED_PETS } = {}) {
  if (pinned === PINNED_PETS && LOCK.missing) throw new CliError(`${LOCK_FILE} is missing or invalid, so no pet is pinned; restore it from git`, EXIT.BAD_CONFIG);
  const names = only ?? Object.keys(pinned);
  const results = [];
  for (const folder of names) {
    const entry = Object.hasOwn(pinned, folder) ? pinned[folder] : undefined;
    if (!entry) throw new CliError(`unknown pet "${folder}"; pinned pets: ${Object.keys(pinned).join(', ')}`, EXIT.BAD_CONFIG);
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
      throw new CliError(`${folder}: download failed (${e.cause?.code ?? e.message}). Check the network and try again.`, EXIT.UNEXPECTED);
    }
    if (!res.ok) throw new CliError(`${folder}: ${url} answered HTTP ${res.status}`, EXIT.UNEXPECTED);
    const bytes = await readCapped(res, folder);
    const got = sha256(bytes);
    if (got !== entry.sha256) {
      throw new CliError(`${folder}: sha256 mismatch (expected ${entry.sha256}, got ${got}). Nothing was written.`, EXIT.UNEXPECTED);
    }
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
 * mid-body into a CliError with the cause.
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
        throw new CliError(`${folder}: the download is larger than ${MAX_BYTES / 1024 / 1024} MB; refusing it. Nothing was written.`, EXIT.UNEXPECTED);
      }
      chunks.push(value);
    }
  } catch (e) {
    if (e instanceof CliError) throw e;
    throw new CliError(`${folder}: the download was interrupted (${e.cause?.code ?? e.message}). Nothing was written; try again.`, EXIT.UNEXPECTED);
  }
  return Buffer.concat(chunks.map((c) => Buffer.from(c.buffer, c.byteOffset, c.byteLength)));
}

/** For doctor: which pinned pets are present and verified. */
export function checkPets(dataDir, folder) {
  const entry = Object.hasOwn(PINNED_PETS, folder) ? PINNED_PETS[folder] : undefined;
  const file = path.join(dataDir, 'pets', folder, 'animations.xml');
  if (!fs.existsSync(file)) return { ok: false, detail: `${file} is missing: run \`bellwether pets fetch\`` };
  if (!entry) return { ok: true, detail: `${folder} (not a pinned pet, so not verified)` };
  return sha256(fs.readFileSync(file)) === entry.sha256 ? { ok: true, detail: `${folder} present, sha256 verified` } : { ok: false, detail: `${file} doesn't match its pinned sha256: run \`bellwether pets fetch\` again` };
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
