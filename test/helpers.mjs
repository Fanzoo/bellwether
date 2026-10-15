// The isolation preload (npm test loads it first; this also covers `node --test <file>`).
import { ISOLATED, isProtected, resolveReal } from './setup.mjs';
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
  for (const d of [path.join(AGENT_DIR, 'crates', 'core', 'src'), path.join(AGENT_DIR, 'crates', 'brain', 'src'), path.join(AGENT_DIR, 'shell', 'src-tauri', 'src')]) walk(d);
  for (const f of [path.join(AGENT_DIR, 'crates', 'core', 'Cargo.toml'), path.join(AGENT_DIR, 'crates', 'brain', 'Cargo.toml'), path.join(AGENT_DIR, 'shell', 'src-tauri', 'Cargo.toml')]) newest = Math.max(newest, fs.statSync(f).mtimeMs);
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

/** The real home directory of this user (from the password database, not HOME). */
export const REAL_HOME = os.userInfo().homedir;

/** Whether a path is the real home directory (symlinks resolved). */
export function isRealHome(p) {
  // By identity first: a firmlink spelling (/System/Volumes/Data/Users/…) or another letter
  // case is the same directory under another name.
  try {
    const a = fs.statSync(p);
    const b = fs.statSync(REAL_HOME);
    if (a.dev === b.dev && a.ino === b.ino) return true;
  } catch {}
  const norm = (x) => {
    let r;
    try {
      r = fs.realpathSync(x);
    } catch {
      r = path.resolve(x);
    }
    return (r.startsWith('/System/Volumes/Data/') ? r.slice('/System/Volumes/Data'.length) : r).toLowerCase();
  };
  return norm(p) === norm(REAL_HOME);
}

/** A `--port` value as a number, however it is spelled (`047417`, `+47417`, ` 47417`). */
export function portValue(v) {
  return Number(String(v ?? '').trim());
}

/**
 * Arguments that would point the CLI at the real Claude Code settings files (`--settings`,
 * `--out`, a positional path, by name or by inode): refused before anything runs. The
 * preload guards Node processes; this guards what it can't see into.
 */
function refuseRealSettings(args, cwd) {
  for (const a of args) {
    if (typeof a !== 'string' || a === '') continue;
    const value = a.startsWith('--') ? (a.includes('=') ? a.slice(a.indexOf('=') + 1) : null) : a;
    if (!value) continue;
    if (isProtected(resolveReal(path.resolve(cwd ?? process.cwd(), value)))) throw new Error(`refused: ${a} names the real Claude Code settings (a test must give a temp file)`);
  }
}

let tempHome = null;
/** The temp HOME the Rust binary gets in place of the real one: one per test process. */
function rustTempHome() {
  tempHome ??= tempDir('bw-rust-HOME-');
  return tempHome;
}

// One Rust home per Node config, for calls that give no data dir: the calls of one test
// share a home the way its Node calls share a data dir.
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

/** Whether a file holds a JSON object (a config the Rust binary would act on). */
function isJsonObject(file) {
  try {
    const v = JSON.parse(fs.readFileSync(file, 'utf8'));
    return !!v && typeof v === 'object' && !Array.isArray(v);
  } catch {
    return false;
  }
}

/**
 * The environment for the Rust binary, from the one a test gives the Node CLI:
 *
 * - its home (state.json, shell-state.json, recordings, logs) is BELLWETHER_HOME if the
 *   test sets one, else the Node CLI's BELLWETHER_DATA_DIR, so files land where the test
 *   looks for them, else a temp home per config;
 * - its config is the file the Node CLI would read (BELLWETHER_CONFIG, else a copied
 *   checkout's own config.json or config.example.json, else the suite's isolated one),
 *   passed whole as BELLWETHER_CONFIG. A config whose port is missing or 47417 is copied
 *   with the suite's test port instead (the Rust brain defaults a missing port to 47417).
 *
 * The preload's guard can't see inside a Rust process, so the live port is refused here:
 * an argument or a config that would make the binary use 47417 throws. A config that isn't
 * a JSON object is passed as it is: the binary refuses it (exit 3) before it binds.
 */
export function rustEnv(args, given, root = AGENT_DIR) {
  const env = { ...given };
  // HOME is never the real one: the Rust binary's default settings file is
  // $HOME/.claude/settings.json, its LaunchAgents $HOME/Library/LaunchAgents, and the
  // preload's guard can't see inside it. A test that gives no HOME, or the real one, gets a
  // temp HOME instead (the guard test in isolation.test.mjs proves it).
  if (!env.HOME || isRealHome(env.HOME)) env.HOME = rustTempHome();
  // Every `--port` (the last one wins in the CLIs), read as a number whatever its spelling.
  const ports = args.flatMap((a, i) => (a === '--port' ? [args[i + 1]] : a.startsWith('--port=') ? [a.slice(7)] : [])).map(portValue);
  const argPort = ports.length ? ports.at(-1) : null;
  if (ports.includes(LIVE_PORT)) throw new Error(`refused under BELLWETHER_IMPL=rust: --port ${LIVE_PORT} (in any spelling) is the live brain's port`);
  let config = env.BELLWETHER_CONFIG;
  if (!config && root !== AGENT_DIR) config = ['config.json', 'config.example.json'].map((f) => path.join(root, f)).find((f) => fs.existsSync(f));
  config = path.resolve(config ?? ISOLATED.BELLWETHER_CONFIG);
  if (isJsonObject(config)) {
    const port = portIn(config);
    if (port === null || port === LIVE_PORT) {
      const copy = path.join(tempDir('bw-rust-config-'), 'config.json');
      fs.writeFileSync(copy, JSON.stringify({ ...JSON.parse(fs.readFileSync(config, 'utf8')), port: portIn(ISOLATED.BELLWETHER_CONFIG) }, null, 2) + '\n');
      config = copy;
    }
  }
  env.BELLWETHER_CONFIG = config;
  if (!env.BELLWETHER_HOME) {
    if (env.BELLWETHER_DATA_DIR) env.BELLWETHER_HOME = path.resolve(env.BELLWETHER_DATA_DIR);
    else {
      if (!rustHomes.has(config)) rustHomes.set(config, tempDir('bw-rust-home-'));
      env.BELLWETHER_HOME = rustHomes.get(config);
    }
  }
  const effective = portIn(config);
  if (argPort === null && isJsonObject(config) && (effective === null || effective === LIVE_PORT)) {
    throw new Error(`refused under BELLWETHER_IMPL=rust: ${config} would make the Rust binary use the live port ${LIVE_PORT}; pass --port or give the config a test port`);
  }
  return env;
}

// Commands a test never runs under rust: on this Mac they would reach the real system.
// `autostart` reaches SMAppService only from inside an app bundle: a development binary
// refuses install and uninstall (exit 2, nothing registered or written; plan milestone 6),
// so it runs, except `autostart settings`, which opens System Settings on screen. Any
// binary inside a `.app` (a BELLWETHER_BIN pointing into a bundle) is refused it.
// The binary's real path decides (symlinks resolved, as the binary resolves its own path
// to find its bundle): a symlink elsewhere to Bellwether.app/Contents/MacOS/Bellwether is
// the bundle. A path that can't be resolved counts as a bundle (refused).
const inBundle = (bin) => {
  let real;
  try {
    real = fs.realpathSync(bin);
  } catch {
    return true;
  }
  return /\.app\/Contents\/MacOS\/[^/]+$/.test(real);
};
/** Why the Rust binary `bin` must not run `args` here, or null (exported for its test). */
export function rustRefusal(args, bin = rustBin()) {
  // The move into the app (plan D9) calls launchctl and the login items; from the suite only
  // a development binary may run it (under the temp HOME rustEnv gives, where the binary
  // reaches them only through stand-ins, and refuses the real HOME in a test process).
  if (args[0] === 'migrate' && args.some((a) => a === '--to-app' || a === '--undo')) {
    return inBundle(bin) ? `the Rust \`migrate ${args.find((a) => a === '--to-app' || a === '--undo')}\` of an app bundle (${bin}) moves login items on this Mac; tests run it only from a development binary` : null;
  }
  if (args[0] !== 'autostart') return null;
  if (args.includes('settings')) return '`autostart settings` opens System Settings on this screen';
  if (inBundle(bin)) return `the Rust \`autostart\` of an app bundle (${bin}) registers SMAppService login items on this Mac; tests run it only from a development binary, which can't`;
  return null;
}

/** Throws when the Rust binary `bin` must not run `args` here (what brainCommand checks). */
export function refuseRust(args, bin = rustBin()) {
  const why = rustRefusal(args, bin);
  if (why) throw new Error(`not run under BELLWETHER_IMPL=rust: ${why}`);
}

/**
 * The command a test runs for `bellwether <args>`:
 * - `root`: the checkout whose brain/cli.mjs runs (a test's temp copy; default this one);
 * - `shim`: run through this /bin/sh shim (a copy's `bellwether`) instead of cli.mjs;
 * - anything else is passed to child_process as options.
 * Returns { file, args, options }.
 */
export function brainCommand(args, { root = AGENT_DIR, shim = null, ...options } = {}) {
  refuseRealSettings(args, options.cwd);
  if (IMPL === 'node') {
    if (shim) return { file: '/bin/sh', args: [shim, ...args], options };
    return { file: process.execPath, args: [path.join(root, 'brain', 'cli.mjs'), ...args], options };
  }
  refuseRust(args);
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

/**
 * Test options for a test that has nothing to check in the Rust brain yet, with the reason
 * (`test(name, notUnderRust('…'), fn)`). Under node it runs as always; under rust the
 * parity table counts it as skipped, never as passed. Each use names the milestone or the
 * test/DIVERGENCES.md entry that explains it.
 */
export function notUnderRust(why) {
  return IMPL === 'rust' ? { skip: `not under rust: ${why}` } : {};
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

// ------------------------------------------------- ReDoS guards that hold on a slow runner

/** Milliseconds for `f`, the best of `runs`. */
function bestOf(runs, f) {
  let best = Infinity;
  for (let i = 0; i < runs; i++) {
    const t = process.hrtime.bigint();
    f();
    best = Math.min(best, Number(process.hrtime.bigint() - t) / 1e6);
  }
  return best;
}

// What the calibration workload takes on the maintainer's Mac (M-series: 174-185 ms).
const CALIBRATION_REFERENCE_MS = 200;
let calibrationScale = null;

/**
 * How much slower this machine is than the maintainer's Mac, measured in this process
 * on a fixed workload of the kind redaction does (a look-behind, bounded classes,
 * captures and a replacement callback over 4 MB) that doesn't use the code under test.
 * Never below 1: a dev machine keeps a budget exactly as written; a shared CI runner
 * gets it scaled by its own measured speed.
 */
export function machineSlowdown({ fresh = false } = {}) {
  // `fresh`: measure again (after a starved attempt, the load may have come since).
  if (calibrationScale === null || fresh) {
    const text = 'k:'.repeat(1e6) + 'x=y '.repeat(5e5);
    const re = /(?<![A-Za-z0-9_.-])([A-Za-z0-9_.-]{1,80})(\s{0,20}[=:]\s{0,20})/g;
    const ms = bestOf(3, () => text.replace(re, (m, k, sep) => k + sep));
    calibrationScale = Math.max(1, ms / CALIBRATION_REFERENCE_MS);
  }
  return calibrationScale;
}

/**
 * The ReDoS guard for a text cleaner: every input (a name and the pathological text, about
 * 5 MB) must clean within `budgetMs` on the maintainer's Mac, times machineSlowdown() on
 * this one. Catastrophic or polynomial backtracking on inputs this size takes minutes, far
 * past any scaled budget; ordinary linear work fits. A run over budget is measured again,
 * the best of 3 (one sample can be stretched by the scheduler), before it counts.
 * Returns the problems found, as lines (empty when all is well).
 */
export function cleaningProblems(clean, inputs, budgetMs) {
  const budget = budgetMs * machineSlowdown();
  const problems = [];
  for (const [name, text] of Object.entries(inputs)) {
    let ms = bestOf(1, () => clean(text));
    if (ms > budget) ms = bestOf(3, () => clean(text));
    if (ms > budget) problems.push(`${name}: ${ms.toFixed(0)} ms, over the ${budget.toFixed(0)} ms budget (${budgetMs} ms × slowdown ${machineSlowdown().toFixed(2)})`);
  }
  return problems;
}

/**
 * Timing tests' lower bounds (how much work the test's own sender and watcher got done)
 * scaled by machineSlowdown(); upper bounds are never scaled. `attempt()` returns
 * `{ starved, why }` after checking its strict bounds itself; a starved attempt is tried
 * again with the slowdown measured afresh, and when every try starves the result is
 * inconclusive (the test skips, saying so), not red. Returns the last attempt's result.
 */
export async function unlessStarved(t, attempt, { tries = 3 } = {}) {
  const seen = [];
  for (let i = 0; i < tries; i++) {
    const r = await attempt(machineSlowdown({ fresh: i > 0 }));
    if (!r.starved) return r;
    seen.push(r.why);
  }
  t.skip(`inconclusive: this machine was too busy to drive the test in ${tries} tries (${seen.join('; ')}); the upper bounds held each time`);
  return null;
}
