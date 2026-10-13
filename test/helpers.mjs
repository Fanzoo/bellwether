// The isolation preload (npm test loads it first; this also covers `node --test <file>`).
import { ISOLATED } from './setup.mjs';
import { spawn, spawnSync } from 'node:child_process';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const AGENT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const FIXTURES = path.join(AGENT_DIR, 'test', 'fixtures');
export const TOKEN = 'testTokenNotReal_abcdefghijklmnop';

export function tempDir(prefix = 'bellwether-test-') {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

// ------------------------------------------------- which brain a test runs (plan D2)
//
// Every test that starts the brain or its CLI goes through spawnBrain / spawnBrainSync,
// so one switch picks the implementation:
//
//   BELLWETHER_IMPL=node (default)  node <root>/brain/cli.mjs …, or the /bin/sh shim
//   BELLWETHER_IMPL=rust            $BELLWETHER_BIN …, by default the dev build
//                                   shell/src-tauri/target-dev/debug/bellwether-shell
//
// `npm test` runs node; `npm run test:rust` (test/conformance.mjs) runs the files that
// spawn the brain against both and prints the parity table.

export const IMPL = process.env.BELLWETHER_IMPL === 'rust' ? 'rust' : 'node';
const DEFAULT_RUST_BIN = path.join(AGENT_DIR, 'shell', 'src-tauri', 'target-dev', 'debug', 'bellwether-shell');
export const RUST_BIN = process.env.BELLWETHER_BIN ? path.resolve(process.env.BELLWETHER_BIN) : DEFAULT_RUST_BIN;
const LIVE_PORT = 47417;

/** Newest mtime (ms) of the Rust sources the binary is built from. */
export function newestRustSource() {
  let newest = 0;
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (/\.(rs|toml)$/.test(e.name)) newest = Math.max(newest, fs.statSync(p).mtimeMs);
    }
  };
  for (const d of [path.join(AGENT_DIR, 'crates', 'brain', 'src'), path.join(AGENT_DIR, 'shell', 'src-tauri', 'src')]) walk(d);
  for (const f of [path.join(AGENT_DIR, 'crates', 'brain', 'Cargo.toml'), path.join(AGENT_DIR, 'shell', 'src-tauri', 'Cargo.toml')]) newest = Math.max(newest, fs.statSync(f).mtimeMs);
  return newest;
}

export const RUST_BUILD_HINT = '`cargo build --manifest-path shell/src-tauri/Cargo.toml --features bellwether-shell/conformance` (a debug build, into target-dev/)';

/**
 * The Rust binary, checked: it must exist, and the default dev build must not be older
 * than its sources (a stale binary would test yesterday's brain). A BELLWETHER_BIN given
 * by hand is taken as it is.
 */
export function rustBin() {
  if (!fs.existsSync(RUST_BIN)) throw new Error(`BELLWETHER_IMPL=rust needs the binary: build it with ${RUST_BUILD_HINT} or set BELLWETHER_BIN (looked for ${RUST_BIN})`);
  if (!process.env.BELLWETHER_BIN && fs.statSync(RUST_BIN).mtimeMs < newestRustSource()) throw new Error(`${RUST_BIN} is older than the Rust sources: rebuild it with ${RUST_BUILD_HINT}, then run \`npm run test:rust\` again`);
  return RUST_BIN;
}

// One Rust home per Node config and data dir, so the calls of one test share a home the
// way its Node calls share a config and a data dir.
const rustHomes = new Map();

/** The port in a config file, or null. */
function portIn(file) {
  try {
    const p = JSON.parse(fs.readFileSync(file, 'utf8')).port;
    return Number.isInteger(p) ? p : null;
  } catch {
    return null;
  }
}

/**
 * The environment for the Rust binary, from the one a test gives the Node CLI. The Rust
 * binary reads BELLWETHER_HOME (config.json, hook-token, logs) and BELLWETHER_HOOK_TOKEN;
 * the Node CLI's BELLWETHER_CONFIG and BELLWETHER_DATA_DIR become a temp home whose
 * config.json carries the same port. The preload's guard can't see inside a Rust
 * process, so the live port is refused here: a home or an argument that would make the
 * binary use 47417 throws.
 */
function rustEnv(args, given, root) {
  const env = { ...given };
  const at = args.findIndex((a) => a === '--port' || a.startsWith('--port='));
  const argPort = at < 0 ? null : Number(args[at].includes('=') ? args[at].split('=')[1] : args[at + 1]);
  if (argPort === LIVE_PORT) throw new Error(`refused under BELLWETHER_IMPL=rust: --port ${LIVE_PORT} is the live brain's port`);
  if (!env.BELLWETHER_HOME) {
    // The config the Node CLI would read: BELLWETHER_CONFIG, else a copied checkout's
    // own config, else the suite's isolated one (never this checkout's config.json).
    let config = env.BELLWETHER_CONFIG;
    if (!config && root !== AGENT_DIR) config = ['config.json', 'config.example.json'].map((f) => path.join(root, f)).find((f) => fs.existsSync(f));
    config ??= ISOLATED.BELLWETHER_CONFIG;
    const key = `${path.resolve(config)}|${env.BELLWETHER_DATA_DIR ?? ''}`;
    if (!rustHomes.has(key)) {
      const home = tempDir('bw-rust-home-');
      let port = portIn(config);
      if (port === null || port === LIVE_PORT) port = portIn(ISOLATED.BELLWETHER_CONFIG);
      fs.writeFileSync(path.join(home, 'config.json'), JSON.stringify({ port }, null, 2) + '\n');
      rustHomes.set(key, home);
    }
    env.BELLWETHER_HOME = rustHomes.get(key);
  }
  const homePort = portIn(path.join(env.BELLWETHER_HOME, 'config.json'));
  if (argPort === null && (homePort === null || homePort === LIVE_PORT)) {
    throw new Error(`refused under BELLWETHER_IMPL=rust: ${env.BELLWETHER_HOME} has no config.json with a test port, so the Rust binary would use the live port ${LIVE_PORT}; pass --port or give the home a config.json`);
  }
  return env;
}

// Commands a test never runs under rust: on this Mac they would reach the real system.
const RUST_REFUSED = {
  autostart: 'the Rust `autostart` registers SMAppService login items on this Mac (milestone 6 ports the command and tests it on a spare account)',
};

/**
 * The command a test runs for `bellwether <args>`:
 * - `root`: the checkout whose brain/cli.mjs runs (a test's temp copy; default this one);
 * - `shim`: run through this /bin/sh shim (a copy's `bellwether`) instead of cli.mjs;
 * - anything else is passed to child_process as options.
 * Returns { file, args, options }.
 */
export function brainCommand(args, { root = AGENT_DIR, shim = null, ...options } = {}) {
  if (IMPL === 'node') {
    if (shim) return { file: '/bin/sh', args: [shim, ...args], options };
    return { file: process.execPath, args: [path.join(root, 'brain', 'cli.mjs'), ...args], options };
  }
  const why = RUST_REFUSED[args[0]];
  if (why) throw new Error(`not run under BELLWETHER_IMPL=rust: ${why}`);
  // argv[0] `bellwether` makes the binary its CLI whatever the arguments, never the
  // strip (no arguments, or a first argument starting with "-", would start the app).
  return { file: rustBin(), args, options: { ...options, argv0: 'bellwether', env: rustEnv(args, options.env ?? process.env, root) } };
}

// The Rust processes a test started are stopped, by pid, when the test process ends: a
// failing test can skip its own cleanup, and the Rust brain has no parent watchdog yet.
const rustChildren = new Set();
function track(child) {
  if (IMPL !== 'rust' || !child?.pid) return child;
  rustChildren.add(child);
  child.once('exit', () => rustChildren.delete(child));
  return child;
}
if (IMPL === 'rust') {
  const stopAll = () => {
    for (const c of rustChildren) if (c.exitCode === null && c.signalCode === null) c.kill('SIGTERM');
  };
  process.on('exit', stopAll);
  for (const sig of ['SIGTERM', 'SIGINT']) {
    process.once(sig, () => {
      stopAll();
      process.exit(sig === 'SIGTERM' ? 143 : 130);
    });
  }
}

/** `spawn` for the brain or its CLI (see brainCommand). */
export function spawnBrain(args, opts = {}) {
  const { file, args: a, options } = brainCommand(args, opts);
  return track(spawn(file, a, options));
}

/** `spawnSync` for the brain or its CLI (see brainCommand). Under rust, at most 60 s. */
export function spawnBrainSync(args, opts = {}) {
  const { file, args: a, options } = brainCommand(args, opts);
  if (IMPL === 'rust') options.timeout ??= 60_000;
  return spawnSync(file, a, options);
}

/**
 * A raw HTTP request where every header is under the test's control (including
 * Host and Origin). Resolves with status, headers and the body as a Buffer.
 */
export function request(port, { method = 'POST', path: p = '/hook', headers = {}, body = '' } = {}) {
  return new Promise((resolve, reject) => {
    const data = Buffer.isBuffer(body) ? body : Buffer.from(body);
    const req = http.request(
      { host: '127.0.0.1', port, method, path: p, headers: { 'Content-Length': data.length, ...headers }, setHost: false },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
      },
    );
    req.on('error', reject);
    req.end(data);
  });
}

export function hookHeaders(port, extra = {}) {
  return { Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', 'X-Bellwether-Token': TOKEN, ...extra };
}

// ------------------------------------------------- shared vectors (plan D2)

/**
 * A shared vector file from test/fixtures (redact, normalise, config, diff): its
 * `vectors`, optionally only one group. The Rust port reads the same files; see
 * test/fixtures/build-vectors.mjs.
 */
export function readVectors(name, group = null) {
  const { vectors } = JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}-vectors.json`), 'utf8'));
  const picked = group === null ? vectors : vectors.filter((v) => v.group === group);
  if (picked.length === 0) throw new Error(`no vectors in ${name}-vectors.json${group === null ? '' : ` for group ${JSON.stringify(group)}`}`);
  return picked;
}

/** The installer vectors: cases.json, plus each case's files as text. */
export function readInstallerVectors() {
  const dir = path.join(FIXTURES, 'installer-vectors');
  const meta = JSON.parse(fs.readFileSync(path.join(dir, 'cases.json'), 'utf8'));
  const file = (c, f) => fs.readFileSync(path.join(dir, c.dir, f), 'utf8');
  return { ...meta, cases: meta.cases.map((c) => ({ ...c, settings: file(c, 'settings.json'), installed: file(c, 'installed.json'), installDiff: file(c, 'install.diff'), uninstallDiff: file(c, 'uninstall.diff'), uninstalled: file(c, 'uninstalled.json') })) };
}
