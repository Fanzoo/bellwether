/**
 * Preloaded into every test process and every process a test starts (npm test sets
 * NODE_OPTIONS=--import=<this file>; it passes itself on). This checkout may be a live
 * install, so the suite is isolated from it:
 *
 * - BELLWETHER_ENV, BELLWETHER_DATA_DIR and BELLWETHER_CONFIG point at a temp .env (no
 *   token), a temp data dir and a temp config on a port that isn't 47417, unless a test
 *   sets them itself. A test that starts a process with its own small `env` gets them
 *   too, when that process runs this checkout's code.
 * - A guard fails the process (an error where it happens, and exit code 1 at the end,
 *   even if the error was caught) on any connection to 127.0.0.1:47417, any access to
 *   this checkout's .env, any write under this checkout's data/, and any read there
 *   other than the public pet art in data/pets.
 *
 * BELLWETHER_TEST_GUARD=off turns the guard off (for diagnosing the guard itself).
 */
import childProcess from 'node:child_process';
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SELF = fileURLToPath(import.meta.url);
const REPO = path.resolve(path.dirname(SELF), '..');
const LIVE_PORT = 47417;
// Compared as real paths, so a symlink to .env or into data/ (or a symlinked parent)
// is caught the same as the plain path.
const realOr = (p) => {
  try {
    return fs.realpathSync.native(p);
  } catch {
    return p;
  }
};
const REPO_REAL = realOr(REPO);
const REPO_ENV = path.join(REPO_REAL, '.env');
const REPO_DATA = path.join(REPO_REAL, 'data');
const REPO_PETS = path.join(REPO_DATA, 'pets');
const GUARD = process.env.BELLWETHER_TEST_GUARD !== 'off';

// Kept before patching: the guard's own work must not trip it.
const real = { mkdtempSync: fs.mkdtempSync, writeFileSync: fs.writeFileSync, readFileSync: fs.readFileSync, mkdirSync: fs.mkdirSync, realpathSync: fs.realpathSync.native, lstatSync: fs.lstatSync };

// --- the isolated environment, made once per test run and inherited from then on
if (!process.env.BELLWETHER_TEST_ROOT) {
  const root = real.mkdtempSync.call(fs, path.join(os.tmpdir(), 'bellwether-suite-'));
  const port = 47990 + Math.floor(Math.random() * 900);
  const example = JSON.parse(real.readFileSync.call(fs, path.join(REPO, 'config.example.json'), 'utf8'));
  real.writeFileSync.call(fs, path.join(root, 'config.json'), JSON.stringify({ ...example, port }, null, 2));
  real.writeFileSync.call(fs, path.join(root, 'test.env'), '');
  real.mkdirSync.call(fs, path.join(root, 'data'));
  process.env.BELLWETHER_TEST_ROOT = root;
}
const ROOT = process.env.BELLWETHER_TEST_ROOT;
export const ISOLATED = {
  BELLWETHER_ENV: path.join(ROOT, 'test.env'),
  BELLWETHER_DATA_DIR: path.join(ROOT, 'data'),
  BELLWETHER_CONFIG: path.join(ROOT, 'config.json'),
};
// The runner, the test files and this checkout's own CLI get the isolated paths; a copy
// of the code that a test runs from a temp directory keeps the paths that test gave it.
const main = process.argv[1] ? path.resolve(process.argv[1]) : null;
if (!main || main.startsWith(REPO + path.sep)) for (const [k, v] of Object.entries(ISOLATED)) if (process.env[`PASTURE_${k.slice(11)}`] === undefined) process.env[k] ??= v;
process.env.NODE_OPTIONS = withPreload(process.env.NODE_OPTIONS);

function withPreload(opts = '') {
  const flag = `--import=${pathToFileURL(SELF).href}`;
  const rest = String(opts).split(/\s+/).filter((o) => o && !/^--import=.*test\/setup\.mjs$/.test(o) && o !== '--import=./test/setup.mjs');
  return [...rest, flag].join(' ');
}

// --- every child process: the preload, and the isolated paths when it runs this checkout
// Whether the child runs this checkout's code: the program itself, or the script it is
// given (the first argument that isn't a flag). A data file from this checkout passed
// to a copy of the code doesn't count.
const inRepo = (a) => typeof a === 'string' && [REPO, REPO_REAL].some((r) => a === r || a.startsWith(r + path.sep));
const runsRepo = (cmd, args) => inRepo(cmd) || inRepo((Array.isArray(args) ? args : []).find((a) => typeof a === 'string' && !a.startsWith('-')));
function childEnv(cmd, args, opts) {
  const base = opts?.env ?? process.env;
  const env = { ...base, NODE_OPTIONS: withPreload(base.NODE_OPTIONS), BELLWETHER_TEST_ROOT: ROOT };
  if (opts?.env && runsRepo(cmd, args)) for (const [k, v] of Object.entries(ISOLATED)) if (!(k in opts.env) && !(`PASTURE_${k.slice(11)}` in opts.env)) env[k] = v;
  return env;
}
/**
 * Every way to start a process passes the preload on, even with a custom `env`:
 * spawn, spawnSync, execFile, execFileSync and fork take (file, [args], [options], [cb]);
 * exec and execSync take (command, [options], [cb]) and run it through a shell.
 */
const withArgs = ['spawn', 'spawnSync', 'execFile', 'execFileSync', 'fork'];
const shellForm = ['exec', 'execSync'];
for (const name of [...withArgs, ...shellForm]) {
  const orig = childProcess[name];
  if (typeof orig !== 'function') continue;
  childProcess[name] = function (cmd, ...rest) {
    let args = null;
    if (withArgs.includes(name) && Array.isArray(rest[0])) args = rest.shift();
    let opts = null;
    if (rest[0] && typeof rest[0] === 'object') opts = rest.shift();
    else if (rest[0] === undefined || rest[0] === null) rest.shift();
    const words = shellForm.includes(name) ? String(cmd).trim().split(/\s+/) : null;
    const env = words ? childEnv(words[0], words.slice(1), opts) : childEnv(name === 'fork' ? process.execPath : cmd, name === 'fork' ? [cmd, ...(args ?? [])] : args, opts);
    const o = { ...(opts ?? {}), env };
    return withArgs.includes(name) ? orig.call(this, cmd, args ?? [], o, ...rest) : orig.call(this, cmd, o, ...rest);
  };
}

// --- the guard
const violations = [];
/** For test/isolation.test.mjs, which trips the guard on purpose: the violations so far, cleared. */
export function takeViolations() {
  return violations.splice(0);
}
function violation(what) {
  const msg = `test isolation: ${what} (this checkout may be the live install; see test/setup.mjs)`;
  violations.push(msg);
  return new Error(msg);
}

/** A path as the file system will see it: absolute, `..` folded, symlinks resolved (for a path that doesn't exist yet, its nearest existing parent). */
export function resolveReal(p) {
  let abs;
  if (p instanceof URL) abs = fileURLToPath(p);
  else if (Buffer.isBuffer(p)) abs = p.toString();
  else if (typeof p === 'string') abs = p;
  else return null; // a file descriptor: already open, checked when it was opened
  abs = path.resolve(abs);
  let rest = '';
  for (let dir = abs; ; ) {
    try {
      // lstat first: a dangling symlink is resolved through its target's parent
      real.lstatSync.call(fs, dir);
      return path.join(real.realpathSync.call(fs, dir), rest);
    } catch {}
    const parent = path.dirname(dir);
    if (parent === dir) return abs;
    rest = path.join(path.basename(dir), rest);
    dir = parent;
  }
}
const within = (p, dir) => p === dir || p.startsWith(dir + path.sep);

/**
 * The one check. `kind` is 'meta' (does it exist, what is it), 'read' or 'write'.
 * .env: nothing at all. data/: meta anywhere, reads only in data/pets, no writes.
 */
function check(p, kind, api) {
  const r = resolveReal(p);
  if (r === null) return;
  if (r === REPO_ENV) throw violation(`${api} ${REPO_ENV}`);
  if (!within(r, REPO_DATA) || kind === 'meta') return;
  if (kind === 'read' && within(r, REPO_PETS)) return;
  throw violation(`${api} ${r}`);
}

if (GUARD) {
  const O = fs.constants;
  const opensForWrite = (flags) => {
    if (flags === undefined || flags === null) return false;
    if (typeof flags === 'number') return (flags & (O.O_WRONLY | O.O_RDWR | O.O_APPEND | O.O_CREAT | O.O_TRUNC)) !== 0;
    return !/^(r|rs|sr)$/.test(String(flags));
  };
  // [name, the kind of each path argument]: the sync, callback and promise forms share it.
  const API = {
    existsSync: ['meta'], exists: ['meta'], stat: ['meta'], lstat: ['meta'], access: ['meta'], realpath: ['meta'],
    readFile: ['read'], readdir: ['read'], opendir: ['read'], readlink: ['read'], createReadStream: ['read'], watch: ['read'], watchFile: ['read'],
    writeFile: ['write'], appendFile: ['write'], mkdir: ['write'], mkdtemp: ['write'], rm: ['write'], rmdir: ['write'], unlink: ['write'],
    truncate: ['write'], chmod: ['write'], lchmod: ['write'], chown: ['write'], lchown: ['write'], utimes: ['write'], lutimes: ['write'], createWriteStream: ['write'],
    rename: ['write', 'write'], copyFile: ['read', 'write'], cp: ['read', 'write'], link: ['read', 'write'], symlink: ['meta', 'write'],
    open: ['open'],
  };
  const guardCall = (name, kinds, args) => {
    kinds.forEach((k, i) => {
      if (k === 'open') check(args[0], opensForWrite(args[1]) ? 'write' : 'read', name);
      else if (name === 'createWriteStream' || name === 'createReadStream') check(args[0], k, name);
      else check(args[i], k, name);
    });
  };
  const wrap = (obj, name, kinds, label) => {
    const orig = obj[name];
    if (typeof orig !== 'function') return;
    const wrapped = function (...args) {
      guardCall(label, kinds, args);
      return orig.apply(this, args);
    };
    for (const k of Object.keys(orig)) wrapped[k] = orig[k]; // realpath.native and friends
    obj[name] = wrapped;
  };
  for (const [name, kinds] of Object.entries(API)) {
    wrap(fs, name, kinds, name);
    wrap(fs, `${name}Sync`, kinds, `${name}Sync`);
    wrap(fs.promises, name, kinds, `fs.promises.${name}`);
  }
  wrap(fs.realpathSync, 'native', ['meta'], 'realpathSync.native');
  wrap(fs.realpath, 'native', ['meta'], 'realpath.native');
  const loadEnvFile = process.loadEnvFile;
  process.loadEnvFile = function (p = '.env') {
    check(p, 'read', 'loadEnvFile');
    return loadEnvFile.call(this, p);
  };

  // A connection to the live port, by any loopback spelling, never leaves the process:
  // the socket fails with the violation (asynchronously, like a refused connection),
  // and the process exits 1.
  const connect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function (...a) {
    const o = a[0] && typeof a[0] === 'object' ? (Array.isArray(a[0]) ? a[0][0] : a[0]) : { port: a[0], host: typeof a[1] === 'string' ? a[1] : undefined };
    // (`path` is not a reason to let it through: http passes the request path there too.)
    if (o && o.port !== undefined && Number(o.port) === LIVE_PORT && isLoopback(o.host)) {
      const err = violation(`connection to ${o.host ?? 'localhost'}:${LIVE_PORT}`);
      err.code = 'ECONNREFUSED';
      process.nextTick(() => this.destroy(err));
      return this;
    }
    return connect.apply(this, a);
  };
  process.on('exit', () => {
    if (violations.length) {
      process.stderr.write(`\n${[...new Set(violations)].join('\n')}\n`);
      process.exitCode = 1;
    }
  });
}
syncBuiltinESMExports();

/**
 * Whether a host names this machine: no host, localhost (and *.localhost), any 127/8
 * or 0.0.0.0 address in any inet_aton spelling (127.1, 0x7f.1, 2130706433, 0177.0.0.1),
 * ::1, :: and IPv4-mapped IPv6 (::ffff:127.0.0.1, ::ffff:7f00:1).
 */
export function isLoopback(host) {
  if (host === undefined || host === null || host === '') return true;
  let h = String(host).trim().toLowerCase().replace(/^\[|\]$/g, '').replace(/%.*$/, '');
  if (h === 'localhost' || h.endsWith('.localhost')) return true;
  if (net.isIPv6(h)) {
    const mapped = /^(?:0*:)*:?ffff:(.+)$/.exec(h) ?? /^::(\d+\.\d+\.\d+\.\d+)$/.exec(h);
    if (mapped) {
      const v = mapped[1];
      if (v.includes('.')) return v4Loopback(v);
      const m = /^([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(v);
      return !!m && (parseInt(m[1], 16) >> 8 === 127 || (parseInt(m[1], 16) === 0 && parseInt(m[2], 16) === 0));
    }
    const groups = expand6(h);
    return groups.slice(0, 7).every((g) => g === 0) && (groups[7] === 1 || groups[7] === 0);
  }
  return v4Loopback(h);
}
function v4Loopback(h) {
  const n = inetAton(h);
  return n !== null && (n >>> 24 === 127 || n === 0);
}
/** inet_aton: 1 to 4 parts, each decimal, 0x-hex or 0-octal; the last part fills the remaining bytes. */
function inetAton(h) {
  const parts = h.split('.');
  if (parts.length < 1 || parts.length > 4) return null;
  const nums = [];
  for (const p of parts) {
    if (!/^(0x[0-9a-f]*|0[0-7]*|[1-9][0-9]*)$/.test(p)) return null;
    nums.push(p.startsWith('0x') ? parseInt(p.slice(2) || '0', 16) : p.length > 1 && p.startsWith('0') ? parseInt(p, 8) : Number(p));
  }
  const last = nums.pop();
  if (nums.some((x) => x > 255) || last >= 2 ** (8 * (4 - nums.length))) return null;
  return (nums.reduce((acc, x, i) => acc + x * 2 ** (8 * (3 - i)), 0) + last) >>> 0;
}
function expand6(h) {
  const [a, b] = h.split('::');
  const left = a ? a.split(':') : [];
  const right = b !== undefined ? (b ? b.split(':') : []) : [];
  const fill = b !== undefined ? Array(8 - left.length - right.length).fill('0') : [];
  return [...left, ...fill, ...right].map((g) => parseInt(g || '0', 16));
}
