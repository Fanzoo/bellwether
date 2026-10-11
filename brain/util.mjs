import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

export const AGENT_DIR = path.resolve(here, '..');
// Tests point this at a temp directory so they never write into the real data/.
export const DATA_DIR = process.env.BELLWETHER_DATA_DIR
  ? path.resolve(process.env.BELLWETHER_DATA_DIR)
  : path.join(AGENT_DIR, 'data');

export const VERSION = '0.1.0';

/** The bind address is a constant, not configuration (see the README). */
export const BIND_HOST = '127.0.0.1';

/** The event name `doctor` posts to check the token; the brain counts it but ignores it. */
export const DOCTOR_PING = 'BellwetherDoctorPing';

export const USER_SETTINGS = path.join(os.homedir(), '.claude', 'settings.json');

/** Exit codes: 0 ok, 1 unexpected, 2 not running / not installed, 3 bad config or .env. */
export const EXIT = { OK: 0, UNEXPECTED: 1, NOT_RUNNING: 2, BAD_CONFIG: 3 };

/** A failure the CLI reports with a specific exit code and no stack trace. */
export class CliError extends Error {
  constructor(message, exitCode = EXIT.UNEXPECTED) {
    super(message);
    this.exitCode = exitCode;
  }
}

export function readJson(file, fallback = null) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return fallback;
  }
}

export function log(...args) {
  process.stderr.write(args.join(' ') + '\n');
}

/**
 * The hook token never appears in anything a person (or a model reading the output)
 * sees: the current token wherever it occurs, and any X-Bellwether-Token header value
 * (an old token in a settings diff, say), become their first 4 characters and "…".
 * Only files bellwether writes on purpose (`hooks print --out`, `--write`) keep the value.
 */
export function maskToken(text, token = process.env.BELLWETHER_HOOK_TOKEN, seen = SEEN_TOKENS) {
  if (typeof text !== 'string') return text;
  const short = (v) => `${v.slice(0, 4)}…`;
  // Learn every value written under a bellwether token header, whatever the header's case,
  // in JSON ("X-Bellwether-Token": "v") or header form (X-Bellwether-Token: v)…
  for (const m of text.matchAll(/["']?x-(?:bellwether|pasture)-token["']?\s*[:=]\s*["']?([^"'\s,}\\]{5,})/gi)) {
    if (!m[1].endsWith('…')) seen.add(m[1]);
  }
  if (token && token.length >= 8) seen.add(token);
  // …and hide each one wherever it appears in the output, under any header or none.
  let out = text;
  for (const v of [...seen].sort((a, b) => b.length - a.length)) out = out.split(v).join(short(v));
  return out;
}

/** Token values seen under a token header in this process's output so far. */
const SEEN_TOKENS = new Set();

/** Masks the token in everything this process writes to stdout and stderr. */
export function installTokenMask() {
  for (const stream of [process.stdout, process.stderr]) {
    if (stream.__bellwetherMasked) continue;
    const write = stream.write.bind(stream);
    stream.write = (chunk, ...rest) => {
      if (typeof chunk === 'string') chunk = maskToken(chunk);
      else if (Buffer.isBuffer(chunk)) chunk = Buffer.from(maskToken(chunk.toString('utf8')));
      return write(chunk, ...rest);
    };
    stream.__bellwetherMasked = true;
  }
}

/** Local wall-clock time with milliseconds, for log lines: 18:41:07.123 */
export function clock(ms = Date.now()) {
  const d = new Date(ms);
  const p = (n, w = 2) => String(n).padStart(w, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

/** Filesystem-safe timestamp: 2026-10-08T18-41-07-123Z */
export function fileStamp(ms = Date.now()) {
  return new Date(ms).toISOString().replace(/[:.]/g, '-');
}

/**
 * Sets an own, enumerable property even when the key is "__proto__" (plain assignment
 * would hit the prototype setter instead). Use it wherever a key comes from input.
 */
export function setOwn(obj, key, value) {
  Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
  return obj;
}

/**
 * Parses `--flag`, `--flag=value` and `--flag value` into a null-prototype object, so
 * a flag such as `--__proto__` or `--toString` is an ordinary own key that the
 * caller's allowlist check sees and refuses. Positional arguments land in `_`. Flags
 * listed in `booleans` never consume the next argument.
 */
export function parseArgs(argv, booleans = []) {
  const out = Object.create(null);
  out._ = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) {
      out._.push(a);
      continue;
    }
    const eq = a.indexOf('=');
    if (eq !== -1) {
      setOwn(out, a.slice(2, eq), a.slice(eq + 1));
    } else {
      const name = a.slice(2);
      const next = argv[i + 1];
      if (!booleans.includes(name) && next !== undefined && !next.startsWith('--')) {
        setOwn(out, name, next);
        i++;
      } else {
        setOwn(out, name, true);
      }
    }
  }
  return out;
}
