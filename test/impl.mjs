/**
 * The brain under test, for the tests that call its parts in process (distribution plan
 * D2): the Rust brain, its pure parts through the conformance build's hidden `__session`
 * command (one synchronous call per use, over two FIFOs), `startBrain` as a `serve`
 * process and `runDoctor` as a real `doctor --json` run of the binary (its `runClaude`
 * answers a fake `claude` on PATH). The names are the ones the Node brain's modules had
 * (brain/*.mjs until milestone 11), so the tests read as they always did.
 *
 * The tools the tests drive a brain with (the HTTP client, replay, the error class) are
 * in test/support/: test drivers, never the thing under test.
 *
 * JS `undefined` crosses as `{"$undefined": true}`, so `{colourIndex: undefined}` stays a
 * property, as in Node.
 */
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { TOKEN, spawnBrain, tempDir } from './helpers.mjs';
import { call } from './support/client.mjs';
import { ENV_HEADERS, PALETTE_UPGRADED } from './support/hooks.mjs';
import { writeShellState } from './support/privacy.mjs';
import { burst, readRecording, replay } from './support/replay.mjs';
import { CliError } from './support/util.mjs';

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

export { PALETTE_UPGRADED };
const palettes = rust('palettes');
export const DEFAULT_PALETTE = palettes.DEFAULT_PALETTE;
export const OLD_DEFAULT_PALETTE = palettes.OLD_DEFAULT_PALETTE;

export const withDefaults = (config) => {
  const { config: merged, paletteUpgraded } = rust('withDefaults', config);
  if (paletteUpgraded) merged[PALETTE_UPGRADED] = true;
  return merged;
};

export const validateConfig = (config, name = 'config') => {
  rust('validateConfig', config, name);
  return config;
};

// ------------------------------------------------------------------ text and events

export const makeTextCleaner = ({ maxChars = 60, redactPatterns = [] } = {}) => {
  const options = { maxChars, redactPatterns };
  rust('cleaner', options);
  const clean = (text) => rust('clean', options, text);
  clean.options = options;
  return clean;
};

export const normalise = (payload, headers = {}, receivedAt = Date.now(), { clean = null } = {}) => {
  if (clean && !clean.options) throw new Error('normalise takes a cleaner made by makeTextCleaner from test/impl.mjs');
  return rust('normalise', payload, headers, receivedAt, clean ? clean.options : null);
};

export const fingerprint = (toolName, input) => rust('fingerprint', toolName, input);
export const identityOf = (ev) => rust('identityOf', ev);
export const LIMITS = rust('limits');
export const envValue = (text, key) => rust('envValue', text, key);

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

export const Flock = RustFlock;

/** The fixture runner (test/fixture-runner.mjs), through the Rust reducer. */
export function runFixtureRust(lines, config = null) {
  return rust('fixture', lines, config);
}

// ------------------------------------------------------------------ persistence, recording, status

export const loadState = (file, warn = () => {}) => {
  const { value, warnings } = rust('loadState', file);
  for (const w of warnings) warn(w);
  return value;
};

export const createRecorder = (file) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  return { file, write: (payload, headers, receivedAt) => rust('recorder.write', file, payload, headers, receivedAt), close: () => {} };
};

export const guardOutFile = (target, kind, { force = false } = {}) => {
  if (kind === 'print') rust('guardPrint', target, force);
  else rust('guardRecord', target);
};
export const isBellwetherRecording = (text) => rust('isBellwetherRecording', text);
export const rebuild = (line, { tag = null } = {}) => rust('rebuild', line, tag);
export const renderStatus = (snap, { palette = [], color = false } = {}) => rust('renderStatus', snap, palette, color);
export const readShellState = (dataDir) => rust('shellState.read', dataDir);
export { writeShellState };
// The strip's copy of the sharing-name rules (shell/src-tauri/src/privacy.rs).
export const sharingProcessWarnings = (config, platform = process.platform) => rust('sharingProcessWarnings', config ?? null, platform);

export const maskToken = (text, token = process.env.BELLWETHER_HOOK_TOKEN, seen = undefined) => {
  if (typeof text !== 'string') return text;
  const r = rust('maskToken', text, token ?? null, seen ? [...seen] : null);
  if (seen) for (const v of r.seen) seen.add(v);
  return r.text;
};

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

export const createPublisher = rustCreatePublisher;

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

export const startBrain = rustStartBrain;

// ------------------------------------------------------------------ the installer and the diff (M4)

export const EVENTS = rust('hooks.EVENTS');
export const HTTP_UNSUPPORTED_EVENTS = rust('hooks.HTTP_UNSUPPORTED_EVENTS');
export { ENV_HEADERS };
export const hookBlock = (port, token) => rust('hooks.hookBlock', port, token);
export const install = (settings, port, token) => rust('hooks.install', settings, port, token);
export const uninstall = (settings, port, created = null) => rust('hooks.uninstall', settings, port, created);
export const countGroups = (settings, port) => rust('hooks.countGroups', settings ?? null, port);
export const deepEqual = (a, b) => rust('hooks.deepEqual', a ?? null, b ?? null);
export const isBellwetherHandler = (h, port) => rust('hooks.isBellwetherHandler', h ?? null, port);
export const isLegacyHandler = (h, port) => rust('hooks.isLegacyHandler', h ?? null, port);
export const shapeProblems = (settings) => rust('hooks.shapeProblems', settings ?? null);
/** planEdit's answer, without `originalBytes` (the tests read the file). */
export const planEdit = (file, action, { port, token = null, dataDir = null }) => rust('hooks.planEdit', file, action, { port, token, dataDir });
export const unifiedDiff = (oldText, newText, options = {}) => rust('diff.unifiedDiff', oldText, newText, options);
export const diffStats = (oldText, newText) => rust('diff.diffStats', oldText, newText);

// ------------------------------------------------------------------ doctor (M4)

export const checkSettings = (settings, { port, token = null, file = 'settings' }) => rust('doctor.checkSettings', settings ?? null, { port, token, file });
export const checkOtherLevels = ({ projectDir = null, managedFile = null, port, userSettings = null }) => rust('doctor.checkOtherLevels', { projectDir, managedFile, port, userSettings });
export const globMatch = (pattern, value) => rust('doctor.globMatch', String(pattern), value);
export const checkLocatorLog = (dataDir) => rust('doctor.checkLocatorLog', dataDir);
/** doctor's `next` line, in the app's order (no pet art, strip build or LaunchAgent steps). */
export const nextStep = ({ checks, installed }) => rust('doctor.nextStep', checks, !!installed);

/** A port nothing listens on: bound by the OS, then released. */
const closedPort = () =>
  new Promise((resolve) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });

/**
 * `runDoctor`: one `bellwether doctor --json` run of the binary, with the
 * options as its environment (the config the loader returns, the token in `env`, `home`
 * as HOME, `dataDir` as the app's home) and `projectDir` as its directory. A `runClaude`
 * is answered through a fake `claude` on PATH that asks this process over loopback, so
 * the `--e2e` tests drive the Rust doctor's real `claude -p` run. A config port below
 * 1024 (the tests' "nothing listens on port 1") becomes a released free port: the Rust
 * config refuses ports below 1024.
 */
async function rustRunDoctor({ settingsFile, env = process.env, configLoader = null, projectDir = null, e2e = false, runClaude = null, dataDir = null, home = null } = {}) {
  const dir = tempDir('bw-doctor-');
  const childEnv = { PATH: process.env.PATH, HOME: home ?? tempDir('bw-doctor-HOME-'), BELLWETHER_HOME: dataDir ?? tempDir('bw-doctor-home-') };
  if (env.BELLWETHER_HOOK_TOKEN !== undefined) childEnv.BELLWETHER_HOOK_TOKEN = env.BELLWETHER_HOOK_TOKEN;
  if (configLoader) {
    let loaded;
    try {
      loaded = configLoader();
    } catch (e) {
      loaded = { error: e };
    }
    const cfg = path.join(dir, 'config.json');
    if (loaded.error) fs.writeFileSync(cfg, '{ not json');
    else {
      const config = { ...(loaded.config ?? {}) };
      if (!Number.isInteger(config.port) || config.port < 1024) config.port = await closedPort();
      // A config withDefaults() upgraded: the file had the old default palette, so the
      // Rust brain gets that file and finds the upgrade itself.
      if (loaded.config?.[PALETTE_UPGRADED]) config.pets = { ...config.pets, palette: [...OLD_DEFAULT_PALETTE] };
      fs.writeFileSync(cfg, JSON.stringify(config));
    }
    childEnv.BELLWETHER_CONFIG = cfg;
  }
  let server = null;
  if (e2e && runClaude) {
    server = http.createServer((req, res) => {
      let body = '';
      req.on('data', (d) => (body += d));
      req.on('end', async () => {
        let answer;
        try {
          answer = await runClaude(JSON.parse(body));
        } catch (e) {
          answer = { code: 1, stderr: String(e?.message ?? e) };
        }
        res.end(JSON.stringify(answer ?? {}));
      });
    });
    await new Promise((r) => server.listen(0, '127.0.0.1', r));
    const bin = path.join(dir, 'bin');
    fs.mkdirSync(bin);
    const script = `#!${process.execPath}
const http = require('node:http');
const a = process.argv.slice(2);
const hooksFile = a[a.indexOf('--settings') + 1];
const q = http.request({ host: '127.0.0.1', port: ${server.address().port}, method: 'POST', path: '/' }, (r) => {
  let b = ''; r.on('data', (d) => (b += d)); r.on('end', () => {
    const x = JSON.parse(b || '{}');
    if (x.error) { process.stderr.write(x.error + '\\n'); process.exit(127); }
    if (x.stderr) process.stderr.write(x.stderr);
    process.exit(x.code ?? 0);
  });
});
q.end(JSON.stringify({ cwd: process.cwd(), hooksFile }));
`;
    fs.writeFileSync(path.join(bin, 'claude'), script, { mode: 0o755 });
    childEnv.PATH = `${bin}${path.delimiter}${process.env.PATH}`;
  }
  const args = ['doctor', '--json', ...(settingsFile ? ['--settings', settingsFile] : []), ...(e2e ? ['--e2e'] : [])];
  const child = spawnBrain(args, { env: childEnv, cwd: projectDir ?? dir, stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '';
  let err = '';
  child.stdout.on('data', (d) => (out += d));
  child.stderr.on('data', (d) => (err += d));
  const code = await new Promise((r) => child.on('close', r));
  server?.close();
  let result;
  try {
    result = JSON.parse(out);
  } catch {
    throw new Error(`bellwether doctor --json (exit ${code}) printed no JSON: ${err || out}`);
  }
  return result;
}

export const runDoctor = rustRunDoctor;

// What tests use as tools (test/support/): the HTTP client, and the JS replay that drives
// whichever brain is listening (`bellwether replay` itself is tested through the CLI).
export { burst, call, CliError, readRecording, replay, TOKEN };
