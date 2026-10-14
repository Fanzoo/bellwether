/**
 * The brain under test, for the tests that call its parts in process (distribution plan
 * D2). The same names as brain/*.mjs:
 *
 *   BELLWETHER_IMPL=node (default)  brain/*.mjs itself
 *   BELLWETHER_IMPL=rust            the Rust brain: pure parts through its hidden
 *                                   `__session` command (one synchronous call per use,
 *                                   over two FIFOs), `startBrain` as a `serve` process
 *
 * So a test written against brain/*.mjs checks the Rust port when the parity table runs it
 * (`npm run test:rust`), call for call. What only the Node CLI has (`.env` writing, doctor
 * before milestone 4) is not here: those tests say so with `notUnderRust` (helpers.mjs).
 *
 * Under rust, JS `undefined` crosses as `{"$undefined": true}`, so `{colourIndex:
 * undefined}` stays a property, as in Node.
 */
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { startBrain as nodeStartBrain } from '../brain/brain.mjs';
import { call } from '../brain/client.mjs';
import { validateConfig as nodeValidateConfig } from '../brain/config.mjs';
import * as nodeDefaults from '../brain/defaults.mjs';
import { envValue as nodeEnvValue } from '../brain/envfile.mjs';
import { Flock as NodeFlock, LIMITS as NODE_LIMITS } from '../brain/flock.mjs';
import { identityOf as nodeIdentityOf } from '../brain/identity.mjs';
import { fingerprint as nodeFingerprint, normalise as nodeNormalise } from '../brain/normalise.mjs';
import { guardOutFile as nodeGuardOutFile, isBellwetherRecording as nodeIsRecording } from '../brain/outfile.mjs';
import { loadState as nodeLoadState } from '../brain/persist.mjs';
import { readShellState as nodeReadShellState, sharingProcessWarnings as nodeSharingProcessWarnings, writeShellState } from '../brain/privacy.mjs';
import { createPublisher as nodeCreatePublisher } from '../brain/publisher.mjs';
import { makeTextCleaner as nodeMakeTextCleaner } from '../brain/redact.mjs';
import { burst, readRecording, rebuild as nodeRebuild, replay } from '../brain/replay.mjs';
import { createRecorder as nodeCreateRecorder } from '../brain/server.mjs';
import { renderStatus as nodeRenderStatus } from '../brain/status.mjs';
import { CliError, maskToken as nodeMaskToken } from '../brain/util.mjs';
import { IMPL, TOKEN, spawnBrain, tempDir } from './helpers.mjs';

export { IMPL };
const RUST = IMPL === 'rust';

// ------------------------------------------------------------------ the session

let session = null;

/** Starts `__session` once per test process: the request FIFO, then the answer FIFO. */
function open() {
  if (session) return session;
  const dir = tempDir('bw-session-');
  const req = path.join(dir, 'req');
  const res = path.join(dir, 'res');
  const mk = spawnSync('mkfifo', [req, res]);
  if (mk.status !== 0) throw new Error(`mkfifo failed: ${mk.stderr}`);
  const child = spawnBrain(['__session', '--in', req, '--out', res], { stdio: ['ignore', 'ignore', 'inherit'] });
  child.unref();
  // Each open waits for the other side, in the order the session opens them.
  const w = fs.openSync(req, 'w');
  const r = fs.openSync(res, 'r');
  session = { w, r, pending: Buffer.alloc(0), buf: Buffer.alloc(1 << 20), child };
  return session;
}

const revive = (v) => {
  if (Array.isArray(v)) return v.map(revive);
  if (v && typeof v === 'object') {
    if (v.$undefined === true && Object.keys(v).length === 1) return undefined;
    const out = {};
    for (const [k, x] of Object.entries(v)) Object.defineProperty(out, k, { value: revive(x), writable: true, enumerable: true, configurable: true });
    return out;
  }
  return v;
};

/** One call to the Rust session; throws a CliError-like error for `{error}`. */
export function rust(fn, ...args) {
  const s = open();
  fs.writeSync(s.w, JSON.stringify({ fn, args: args.map((a) => (a === undefined ? { $undefined: true } : a)) }) + '\n');
  for (;;) {
    const nl = s.pending.indexOf(10);
    if (nl >= 0) {
      const line = s.pending.subarray(0, nl).toString('utf8');
      s.pending = s.pending.subarray(nl + 1);
      const reply = JSON.parse(line);
      if (reply.error) throw new CliError(reply.error.message, reply.error.code);
      return revive(reply.ok);
    }
    const n = fs.readSync(s.r, s.buf, 0, s.buf.length, null);
    if (n === 0) throw new Error('the Rust __session ended');
    s.pending = Buffer.concat([s.pending, s.buf.subarray(0, n)]);
  }
}

// ------------------------------------------------------------------ config

export const PALETTE_UPGRADED = nodeDefaults.PALETTE_UPGRADED;
const palettes = RUST ? rust('palettes') : nodeDefaults;
export const DEFAULT_PALETTE = palettes.DEFAULT_PALETTE;
export const OLD_DEFAULT_PALETTE = palettes.OLD_DEFAULT_PALETTE;

export const withDefaults = RUST
  ? (config) => {
      const { config: merged, paletteUpgraded } = rust('withDefaults', config);
      if (paletteUpgraded) merged[PALETTE_UPGRADED] = true;
      return merged;
    }
  : nodeDefaults.withDefaults;

export const validateConfig = RUST
  ? (config, name = 'config') => {
      rust('validateConfig', config, name);
      return config;
    }
  : nodeValidateConfig;

// ------------------------------------------------------------------ text and events

export const makeTextCleaner = RUST
  ? ({ maxChars = 60, redactPatterns = [] } = {}) => {
      const options = { maxChars, redactPatterns };
      rust('cleaner', options);
      const clean = (text) => rust('clean', options, text);
      clean.options = options;
      return clean;
    }
  : nodeMakeTextCleaner;

export const normalise = RUST
  ? (payload, headers = {}, receivedAt = Date.now(), { clean = null } = {}) => {
      if (clean && !clean.options) throw new Error('under rust, normalise takes a cleaner made by makeTextCleaner from test/impl.mjs');
      return rust('normalise', payload, headers, receivedAt, clean ? clean.options : null);
    }
  : nodeNormalise;

export const fingerprint = RUST ? (toolName, input) => rust('fingerprint', toolName, input) : nodeFingerprint;
export const identityOf = RUST ? (ev) => rust('identityOf', ev) : nodeIdentityOf;
export const LIMITS = RUST ? rust('limits') : NODE_LIMITS;
export const envValue = RUST ? (text, key) => rust('envValue', text, key) : nodeEnvValue;

// ------------------------------------------------------------------ the flock

/** A Flock held by the session; every method is one call. */
class RustFlock {
  constructor({ config, persisted = null, t = 0 }) {
    this.handle = rust('flock.new', config, persisted, t);
  }
  apply(ev) {
    return rust('flock.apply', this.handle, ev);
  }
  tick(t) {
    rust('flock.tick', this.handle, t);
  }
  control(msg, t) {
    return rust('flock.control', this.handle, msg ?? null, t);
  }
  snapshot() {
    return rust('flock.snapshot', this.handle);
  }
  persistable() {
    return rust('flock.persistable', this.handle);
  }
  alertMarks() {
    return new Set(rust('flock.alertMarks', this.handle));
  }
  get muted() {
    return rust('flock.muted', this.handle);
  }
  get persistDirty() {
    return rust('flock.persistDirty', this.handle);
  }
}

export const Flock = RUST ? RustFlock : NodeFlock;

/** The fixture runner (test/fixture-runner.mjs), through the Rust reducer. */
export function runFixtureRust(lines, config = null) {
  return rust('fixture', lines, config);
}

// ------------------------------------------------------------------ persistence, recording, status

export const loadState = RUST
  ? (file, warn = () => {}) => {
      const { value, warnings } = rust('loadState', file);
      for (const w of warnings) warn(w);
      return value;
    }
  : nodeLoadState;

export const createRecorder = RUST
  ? (file) => {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      return { file, write: (payload, headers, receivedAt) => rust('recorder.write', file, payload, headers, receivedAt), close: () => {} };
    }
  : nodeCreateRecorder;

export const guardOutFile = RUST
  ? (target, kind, options) => {
      if (kind !== 'record') throw new Error(`under rust, guardOutFile covers 'record' only until milestone 4 (asked for ${kind})`);
      void options;
      rust('guardRecord', target);
    }
  : nodeGuardOutFile;
export const isBellwetherRecording = RUST ? (text) => rust('isBellwetherRecording', text) : nodeIsRecording;
export const rebuild = RUST ? (line, { tag = null } = {}) => rust('rebuild', line, tag) : nodeRebuild;
export const renderStatus = RUST ? (snap, { palette = [], color = false } = {}) => rust('renderStatus', snap, palette, color) : nodeRenderStatus;
export const readShellState = RUST ? (dataDir) => rust('shellState.read', dataDir) : nodeReadShellState;
export { writeShellState };
// The strip's copy of the sharing-name rules (shell/src-tauri/src/privacy.rs).
export const sharingProcessWarnings = RUST ? (config, platform = process.platform) => rust('sharingProcessWarnings', config ?? null, platform) : nodeSharingProcessWarnings;

export const maskToken = RUST
  ? (text, token = process.env.BELLWETHER_HOOK_TOKEN, seen = undefined) => {
      if (typeof text !== 'string') return text;
      const r = rust('maskToken', text, token ?? null, seen ? [...seen] : null);
      if (seen) for (const v of r.seen) seen.add(v);
      return r.text;
    }
  : nodeMaskToken;

// ------------------------------------------------------------------ the publisher

/** A publisher in the session with a real clock; `build` is read at each markDirty. */
function rustCreatePublisher({ build, intervalMs = 250 }) {
  const handle = rust('publisher.new', build(), intervalMs);
  return {
    markDirty: (urgent = false) => rust('publisher.markDirty', handle, urgent, build()),
    current: () => rust('publisher.current', handle),
    published: () => rust('publisher.published', handle),
    async waitFor(after, timeoutMs) {
      const start = rust('publisher.current', handle);
      if (start.rev > after || timeoutMs <= 0) return start;
      // Like the Node waiters: the next publish (any rev), or the current one at the timeout.
      const until = Date.now() + timeoutMs;
      while (Date.now() < until) {
        await new Promise((r) => setTimeout(r, 2));
        const c = rust('publisher.current', handle);
        if (c.rev !== start.rev) return c;
      }
      return rust('publisher.current', handle);
    },
    close: () => rust('publisher.close', handle),
  };
}

export const createPublisher = RUST ? rustCreatePublisher : nodeCreatePublisher;

// ------------------------------------------------------------------ a running brain

/** A synchronous GET (the Rust brain is another process; tests read its state in line). */
function getSync(port, p) {
  const script = `const http=require('node:http');http.get({host:'127.0.0.1',port:${Number(port)},path:${JSON.stringify(p)},headers:{'X-Bellwether-Token':${JSON.stringify(TOKEN)}}},(r)=>{let b='';r.setEncoding('utf8');r.on('data',(c)=>b+=c);r.on('end',()=>process.stdout.write(b));}).on('error',(e)=>{process.stderr.write(String(e));process.exit(1);});`;
  const r = spawnSync(process.execPath, ['-e', script], { encoding: 'utf8', timeout: 10_000 });
  if (r.status !== 0) throw new Error(`GET ${p}: ${r.stderr}`);
  return JSON.parse(r.stdout);
}

const sleepSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/**
 * `startBrain` as a Rust `serve` (or `record --out`) process: its home is `dataDir`, its
 * config a file written from `config`, its port read from its "listening on" line. The
 * timings a test passes go through the conformance build's test knobs. `flock` answers
 * what the tests read from a running brain: muted (from /state), the identity keys of the
 * sheep present, and what state.json holds.
 */
async function rustStartBrain({ config, token, dataDir, log = () => {}, hang = false, slowMs = 0, recordTo = null, tickMs, persistMs, publishMs }) {
  fs.mkdirSync(dataDir, { recursive: true });
  const cfgFile = path.join(tempDir('bw-rust-cfg-'), 'config.json');
  // A config file names a real port; the one to bind (0 = any free one) is --port.
  fs.writeFileSync(cfgFile, JSON.stringify({ ...config, port: config.port || 1024 }));
  const args = [recordTo ? 'record' : 'serve', ...(recordTo ? ['--out', recordTo] : []), '--port', String(config.port ?? 0)];
  if (hang) args.push('--hang');
  if (slowMs) args.push(`--slow=${slowMs}`);
  const persist = persistMs ?? 200;
  const env = {
    ...process.env,
    BELLWETHER_HOME: dataDir,
    BELLWETHER_CONFIG: cfgFile,
    BELLWETHER_HOOK_TOKEN: token,
    BELLWETHER_TEST_PERSIST_MS: String(persist),
    ...(tickMs ? { BELLWETHER_TEST_TICK_MS: String(tickMs) } : {}),
    ...(publishMs ? { BELLWETHER_TEST_PUBLISH_MS: String(publishMs) } : {}),
  };
  const child = spawnBrain(args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let err = '';
  const port = await new Promise((resolve, reject) => {
    child.stderr.on('data', (d) => {
      err += d;
      for (;;) {
        const nl = err.indexOf('\n');
        if (nl < 0) break;
        const line = err.slice(0, nl).replace(/^\d+ bellwether: /, '');
        err = err.slice(nl + 1);
        log(line);
        const m = /listening on http:\/\/127\.0\.0\.1:(\d+)/.exec(line);
        if (m && !recordTo) resolve(Number(m[1]));
        if (m && recordTo) child.port = Number(m[1]);
        if (/^recording to /.test(line) && child.port) resolve(child.port);
      }
    });
    child.once('exit', (code) => reject(new Error(`the Rust brain exited (${code}) before listening: ${err}`)));
  });
  const stateFile = path.join(dataDir, 'state.json');
  const readState = () => {
    sleepSync(persist + 100);
    try {
      return JSON.parse(fs.readFileSync(stateFile, 'utf8'));
    } catch {
      return { version: 1, muted: false, identities: {} };
    }
  };
  return {
    port,
    child,
    flock: {
      get muted() {
        return getSync(port, '/state?after=-1&wait=0').muted;
      },
      get identities() {
        return new Map(getSync(port, '/state?after=-1&wait=0').flock.map((s) => [s.identityKey, s]));
      },
      persistable: () => readState(),
    },
    // Resolves once stderr is closed too, so the last log lines have reached `log`.
    close: () =>
      new Promise((resolve) => {
        if (child.stderr.closed || child.stderr.destroyed) return resolve();
        child.once('close', () => resolve());
        if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      }),
  };
}

export const startBrain = RUST ? rustStartBrain : nodeStartBrain;

// What tests use as tools, the same under both: the HTTP client, and the JS replay that
// drives whichever brain is listening (`bellwether replay` itself is tested through the CLI).
export { burst, call, CliError, readRecording, replay, TOKEN };
