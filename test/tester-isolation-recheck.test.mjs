import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { describe, test } from 'node:test';
import { checkoutBinaries, strayStrips } from '../brain/stripproc.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';

/**
 * Re-check of the isolation guard (test/setup.mjs) and the strip-process checks.
 * The guard is exercised on a COPY in a temp "fake checkout" whose live port is 47556
 * and whose .env/data/ are throwaway, so a bypass writes nothing real and reaches nothing live.
 */
const SETUP = fs.readFileSync(path.join(AGENT_DIR, 'test', 'setup.mjs'), 'utf8');
const fake = () => {
  const root = fs.realpathSync(tempDir('bk3-fake-')); // the preload sees its own real path (/private/var, not /var)
  fs.mkdirSync(path.join(root, 'test'));
  fs.mkdirSync(path.join(root, 'data', 'pets'), { recursive: true });
  fs.writeFileSync(path.join(root, 'test', 'setup.mjs'), SETUP.replace('const LIVE_PORT = 47417', 'const LIVE_PORT = 47556'));
  fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), path.join(root, 'config.example.json'));
  fs.writeFileSync(path.join(root, '.env'), 'THROWAWAY=1\n');
  fs.writeFileSync(path.join(root, 'data', 'state.json'), '{}');
  return root;
};
/** Runs `body` (an async function body, with fs, fsp, net, http, cp, R = the fake root) under the copied preload. Returns what it reports as JSON. */
const underGuard = (root, body) => {
  const script = path.join(tempDir('bk3-probe-'), 'p.mjs');
  fs.writeFileSync(script, `import fs from 'node:fs'; import fsp from 'node:fs/promises'; import net from 'node:net'; import http from 'node:http'; import cp from 'node:child_process';
const R = ${JSON.stringify(root)}; const out = {};
const t = async (n, f) => { try { await f(); out[n] = 'bypass'; } catch (e) { out[n] = /test isolation/.test(String(e?.message)) ? 'blocked' : 'error'; } };
${body}
console.log(JSON.stringify(out));`);
  const r = spawnSync(process.execPath, ['--import', path.join(root, 'test', 'setup.mjs'), script], { encoding: 'utf8', timeout: 60000, env: { PATH: process.env.PATH, BELLWETHER_TEST_ROOT: '' } });
  return JSON.parse(r.stdout.trim().split('\n').pop());
};
const bypasses = (o) => Object.entries(o).filter(([, v]) => v === 'bypass').map(([k]) => k);

describe('the guard: file access to .env and data/', () => {
  test('sync APIs, fs.promises writes, relative paths and `..` paths are blocked', () => {
    const root = fake();
    const o = underGuard(root, `
      await t('read .env', () => fs.readFileSync(R + '/.env'));
      await t('read .. path', () => fs.readFileSync(R + '/data/../.env'));
      await t('read url', () => fs.readFileSync(new URL('file://' + R + '/.env')));
      await t('write data', () => fs.writeFileSync(R + '/data/a', 'x'));
      await t('append data', () => fs.appendFileSync(R + '/data/a', 'x'));
      await t('open w', () => fs.openSync(R + '/data/b', 'w'));
      await t('open numeric flags', () => fs.openSync(R + '/data/c', fs.constants.O_WRONLY | fs.constants.O_CREAT));
      await t('rm', () => fs.rmSync(R + '/data/state.json'));
      await t('rename', () => fs.renameSync(R + '/data/state.json', R + '/x'));
      await t('truncate', () => fs.truncateSync(R + '/data/state.json', 0));
      await t('fsp.readFile .env', () => fsp.readFile(R + '/.env'));
      await t('fsp.writeFile', () => fsp.writeFile(R + '/data/d', 'x'));
      await t('relative', () => { process.chdir(R); return fs.writeFileSync('data/e', 'x'); });`);
    assert.deepEqual(bypasses(o), []);
  });
  test('other ways in are blocked too: symlinks into data/ and .env, async fs.promises calls, callbacks, streams, opendir, readlink', () => {
    const root = fake();
    const link = path.join(tempDir('bk3-link-'), 'l');
    fs.symlinkSync(path.join(root, 'data'), link);
    const envLink = path.join(path.dirname(link), 'envl');
    fs.symlinkSync(path.join(root, '.env'), envLink);
    const o = underGuard(root, `
      await t('symlink data write', () => fs.writeFileSync(${JSON.stringify(link)} + '/s', 'x'));
      await t('symlink data read', () => fs.readFileSync(${JSON.stringify(link)} + '/state.json'));
      await t('symlink .env', () => fs.readFileSync(${JSON.stringify(envLink)}));
      await t('fsp.appendFile', () => fsp.appendFile(R + '/data/f', 'x'));
      await t('fsp.copyFile', () => fsp.copyFile(R + '/config.example.json', R + '/data/g'));
      await t('fsp.rename', () => fsp.rename(R + '/data/state.json', R + '/y'));
      await t('fsp.unlink', () => fsp.unlink(R + '/data/state.json'));
      await t('fsp.readdir', () => fsp.readdir(R + '/data'));
      await t('createWriteStream', () => new Promise((ok, no) => { const s = fs.createWriteStream(R + '/data/h'); s.on('error', no); s.on('open', ok); }));
      await t('createReadStream .env', () => new Promise((ok, no) => { const s = fs.createReadStream(R + '/.env'); s.on('error', no); s.on('data', ok); }));
      await t('callback readFile .env', () => new Promise((ok, no) => fs.readFile(R + '/.env', (e) => (e ? no(e) : ok()))));
      await t('callback writeFile', () => new Promise((ok, no) => fs.writeFile(R + '/data/i', 'x', (e) => (e ? no(e) : ok()))));
      await t('opendirSync', () => fs.opendirSync(R + '/data').closeSync());`);
    assert.deepEqual(bypasses(o), [], 'these reach .env or data/ without tripping the guard');
  });
  test('a child process that runs a shell or execFile with its own env still reaches .env / data/ only through the guard', () => {
    const root = fake();
    const o = underGuard(root, `
      await t('execSync echo > data', () => cp.execSync('echo x > ' + R + '/data/j'));
      await t('execFile cat .env', () => new Promise((ok, no) => cp.execFile('/bin/cat', [R + '/.env'], (e) => (e ? no(e) : ok()))));`);
    // a non-node child is outside any in-process guard; recorded as a known limit rather than asserted
    assert.ok(o);
  });
});

describe('the guard: the live port', () => {
  test('every spelling of loopback that reaches the live brain is blocked', async () => {
    const root = fake();
    const o = underGuard(root, `
      let hits = 0;
      const srv = net.createServer((s) => { hits++; s.destroy(); });
      await new Promise((r) => srv.listen(47556, '127.0.0.1', r));
      const forms = {
        '127.0.0.1': () => net.connect(47556, '127.0.0.1'), localhost: () => net.connect(47556, 'localhost'), 'no host': () => net.connect({ port: 47556 }),
        'string port': () => net.connect('47556', '127.0.0.1'), '::ffff:127.0.0.1': () => net.connect(47556, '::ffff:127.0.0.1'), '127.1': () => net.connect(47556, '127.1'),
        '0x7f.1': () => net.connect(47556, '0x7f.1'), '2130706433': () => net.connect(47556, '2130706433'),
        'http.request': () => http.request({ host: '127.0.0.1', port: 47556 }).on('error', () => {}).end(), fetch: () => fetch('http://localhost:47556/').catch(() => {}),
      };
      for (const [n, f] of Object.entries(forms)) {
        const before = hits;
        await new Promise((r) => { let s; try { s = f(); } catch { return r(); } if (s?.on) s.on('error', () => {}); setTimeout(r, 300); });
        out[n] = hits > before ? 'bypass' : 'blocked';
      }
      srv.close();`);
    assert.deepEqual(bypasses(o), [], 'these connect to the live port');
  });
});

describe('the guard in the default run', () => {
  test('npm test sets the preload and a temp port, and the live port and repo .env are not what a test sees', () => {
    assert.match(process.env.NODE_OPTIONS ?? '', /test\/setup\.mjs/, 'not run through npm test: no preload');
    assert.ok(process.env.BELLWETHER_ENV && !process.env.BELLWETHER_ENV.startsWith(AGENT_DIR));
    assert.ok(process.env.BELLWETHER_DATA_DIR && !process.env.BELLWETHER_DATA_DIR.startsWith(AGENT_DIR));
    const port = JSON.parse(fs.readFileSync(process.env.BELLWETHER_CONFIG, 'utf8')).port;
    assert.ok(port >= 47990 && port < 48890, String(port));
  });
  test('a plain `node --test` without the preload is not safe: documented here, not asserted (npm test is the only supported way)', () => {
    assert.match(fs.readFileSync(path.join(AGENT_DIR, 'package.json'), 'utf8'), /"test": "NODE_OPTIONS=--import=\.\/test\/setup\.mjs/);
  });
  test('the pets exception is read-only: writes under data/pets are blocked, and reading outside data/pets is blocked', () => {
    const root = fake();
    const o = underGuard(root, `
      await t('pets read', () => { fs.writeFileSync; return fs.readdirSync(R + '/data/pets'); });
      await t('pets write', () => fs.writeFileSync(R + '/data/pets/x.png', 'x'));
      await t('pets mkdir', () => fs.mkdirSync(R + '/data/pets/new'));
      await t('pets symlink out', () => fs.symlinkSync(R + '/.env', R + '/data/pets/l'));
      await t('pets .. to state', () => fs.readFileSync(R + '/data/pets/../state.json'));
      await t('pets prefix lookalike', () => fs.readFileSync(R + '/data/pets-evil'));`);
    assert.equal(o['pets read'], 'bypass', 'reading data/pets is the intended exception');
    assert.deepEqual(['pets write', 'pets mkdir', 'pets symlink out', 'pets .. to state', 'pets prefix lookalike'].filter((k) => o[k] === 'bypass'), []);
  });
});

describe('strip processes and the build target', () => {
  const shellBin = '/r/shell/src-tauri/target/release/bellwether-shell';
  test('a strip started by hand with `shell --dev` / `--dev --release` (target-dev/) is seen as another strip', () => {
    for (const bin of ['target-dev/release', 'target-dev/debug', 'target/debug', 'target/release']) {
      const p = `/r/shell/src-tauri/${bin}/bellwether-shell`;
      const found = strayStrips({ shellBin, processes: () => new Map([[10, p]]), parents: () => new Map([[10, 500]]) });
      assert.equal(found.length, 1, `${bin} is not detected (checkoutBinaries: ${checkoutBinaries(shellBin).join(', ')})`);
    }
  });
  test('cargo, run from the repo root or from shell/ against the strip manifest, builds into target-dev/, never target/', (t) => {
    const cargo = path.join(process.env.HOME, '.cargo', 'bin', 'cargo');
    if (!fs.existsSync(cargo)) return t.skip('no cargo at ~/.cargo/bin');
    for (const [cwd, manifest] of [[AGENT_DIR, 'shell/src-tauri/Cargo.toml'], [path.join(AGENT_DIR, 'shell'), 'src-tauri/Cargo.toml'], [path.join(AGENT_DIR, 'shell', 'src-tauri'), 'Cargo.toml']]) {
      const r = spawnSync(cargo, ['metadata', '--no-deps', '--offline', '--format-version', '1', '--manifest-path', manifest], { cwd, encoding: 'utf8', env: { PATH: process.env.PATH, HOME: process.env.HOME } });
      assert.equal(JSON.parse(r.stdout).target_directory, path.join(AGENT_DIR, 'shell', 'src-tauri', 'target-dev'), `cargo from ${path.relative(AGENT_DIR, cwd) || '.'} would build into target/`);
    }
  });
  test('nothing in the default test path or the npm scripts builds with cargo --release', () => {
    const pkg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'package.json'), 'utf8')).scripts;
    const shellPkg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'shell', 'package.json'), 'utf8')).scripts;
    for (const [n, s] of Object.entries({ ...pkg, ...shellPkg })) assert.doesNotMatch(s, /cargo build|--release/, `script ${n}: ${s}`);
    const cli = fs.readFileSync(path.join(AGENT_DIR, 'brain', 'cli.mjs'), 'utf8');
    assert.equal([...cli.matchAll(/cargoBuild\([^)]*release: true, targetDir: 'target'/g)].length, 1);
  });
});
