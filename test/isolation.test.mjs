import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { describe, test } from 'node:test';
import { loadConfig } from '../brain/config.mjs';
import { DATA_DIR } from '../brain/util.mjs';
import { AGENT_DIR, IMPL, REAL_HOME, TOKEN, brainCommand, isRealHome, refuseRust, rustEnv, spawnBrainSync, tempDir } from './helpers.mjs';
import { runDoctor } from './impl.mjs';
import { ISOLATED, isProtected, protectForTest, resolveReal, takeViolations } from './setup.mjs';

/**
 * The suite never touches this checkout's live install (test/setup.mjs): its .env, its
 * data/ and the live brain on 47417. These tests prove the preload is in place and that
 * the guard stops each of those, in this process and in the processes tests start.
 * Each probe is stopped by the guard before anything is read, written or connected.
 */
const guarded = process.env.BELLWETHER_TEST_GUARD !== 'off' && !!process.env.BELLWETHER_TEST_ROOT;

describe('the test run is isolated from the live install', () => {
  test('the preload is loaded: .env, data dir and config are temp files, on a port that isn\'t 47417', () => {
    assert.ok(process.env.BELLWETHER_TEST_ROOT, 'run the suite with `npm test` (it preloads test/setup.mjs)');
    for (const [k, v] of Object.entries(ISOLATED)) {
      assert.equal(process.env[k], v, k);
      assert.ok(!v.startsWith(AGENT_DIR + path.sep), `${k} points into the checkout`);
    }
    assert.equal(DATA_DIR, ISOLATED.BELLWETHER_DATA_DIR);
    const { config, file } = loadConfig();
    assert.equal(file, ISOLATED.BELLWETHER_CONFIG);
    assert.notEqual(config.port, 47417);
  });

  test('the guard stops a read of the checkout\'s .env, a write or a state read under its data/, and a connection to 47417', { skip: !guarded && 'guard off' }, async () => {
    takeViolations();
    assert.throws(() => fs.readFileSync(path.join(AGENT_DIR, '.env'), 'utf8'), /test isolation: readFileSync .*\.env/);
    assert.throws(() => fs.existsSync(path.join(AGENT_DIR, '.env')), /test isolation/);
    assert.throws(() => process.loadEnvFile(path.join(AGENT_DIR, '.env')), /test isolation/);
    const probe = path.join(AGENT_DIR, 'data', `guard-probe-${process.pid}`);
    assert.throws(() => fs.writeFileSync(probe, 'x'), /test isolation: writeFileSync/);
    assert.throws(() => fs.mkdirSync(path.join(AGENT_DIR, 'data', 'x'), { recursive: true }), /test isolation/);
    assert.throws(() => fs.readFileSync(path.join(AGENT_DIR, 'data', 'state.json')), /test isolation/);
    assert.ok(!fs.existsSync(probe), 'the probe file must not exist');
    const err = await new Promise((resolve) => {
      const s = net.connect(47417, '127.0.0.1');
      s.on('connect', () => resolve(new Error('connected to 47417')));
      s.on('error', resolve);
    });
    assert.match(err.message, /test isolation: connection to 127\.0\.0\.1:47417/);
    const fetched = await fetch('http://127.0.0.1:47417/health').then(() => 'answered', (e) => String(e.cause?.message ?? e.message));
    assert.match(fetched, /test isolation/);
    assert.ok(takeViolations().length >= 7, 'every probe was recorded');
  });

  test('a process a test starts is guarded and isolated too, even with its own small env', { skip: !guarded && 'guard off' }, () => {
    const read = spawnSync(process.execPath, ['-e', `require('node:fs').readFileSync(${JSON.stringify(path.join(AGENT_DIR, '.env'))})`], { encoding: 'utf8', env: { PATH: process.env.PATH } });
    assert.notEqual(read.status, 0);
    assert.match(read.stderr, /test isolation: readFileSync .*\.env/);
    // This checkout's CLI with nothing but PATH: it gets the temp .env (no token), not the live one.
    const home = tempDir('iso-home-');
    const r = spawnBrainSync(['hooks', 'print'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: home } });
    assert.equal(r.status, 3, r.stderr);
    assert.match(r.stderr, /BELLWETHER_HOOK_TOKEN is not set/);
  });

  test('no test process can reach the real ~/.claude/settings.json: a read is stopped here and in a child, and the Rust binary always gets a temp HOME', { skip: !guarded && 'guard off' }, () => {
    // Only reads are tried against the real file: a write must never be attempted, guard or not.
    takeViolations();
    const real = path.join(REAL_HOME, '.claude', 'settings.json');
    assert.throws(() => fs.readFileSync(real, 'utf8'), /test isolation: readFileSync .*the real Claude Code settings/);
    assert.throws(() => fs.readFileSync(path.join(REAL_HOME, '.claude', 'settings.local.json')), /test isolation/);
    assert.throws(() => fs.createReadStream(real), /test isolation/);
    assert.ok(takeViolations().length >= 3, 'every probe was recorded');
    const child = spawnSync(process.execPath, ['-e', `require('node:fs').readFileSync(${JSON.stringify(real)})`], { encoding: 'utf8', env: { PATH: process.env.PATH } });
    assert.notEqual(child.status, 0);
    assert.match(child.stderr, /test isolation: readFileSync .*the real Claude Code settings/);
    // The Rust binary can't be guarded from in here: the helper replaces a missing or real
    // HOME with a temp one, so its default settings file is never the real one.
    for (const given of [{ PATH: process.env.PATH }, { PATH: process.env.PATH, HOME: REAL_HOME }, { PATH: process.env.PATH, HOME: `${REAL_HOME}/` }]) {
      const env = rustEnv(['hooks', 'install'], given);
      assert.ok(env.HOME && !isRealHome(env.HOME), JSON.stringify(given));
      assert.notEqual(path.join(env.HOME, '.claude', 'settings.json'), real);
    }
    const mine = tempDir('iso-own-home-');
    assert.equal(rustEnv(['hooks', 'install'], { HOME: mine }).HOME, mine, "a test's own temp HOME is kept");
    // And end to end: `hooks install` with no HOME and no --settings. The Node CLI resolves
    // the real file from the password database and the guard stops it there; the Rust CLI
    // gets the temp HOME and previews a file under it.
    const r = spawnBrainSync(['hooks', 'install'], { encoding: 'utf8', env: { PATH: process.env.PATH, BELLWETHER_HOOK_TOKEN: TOKEN } });
    if (IMPL === 'rust') {
      assert.equal(r.status, 0, r.stderr);
      assert.ok(!(r.stdout + r.stderr).includes(real), 'the Rust CLI named the real settings file');
      assert.match(r.stdout, /bw-rust-HOME-.*\/\.claude\/settings\.json \(current\)/);
    } else {
      assert.notEqual(r.status, 0);
      assert.match(r.stderr, /test isolation: readFileSync .*the real Claude Code settings/);
    }
  });

  test('the guard sees the real settings under other names, and stops the APIs that bypass fs wrappers (BR-3K M4 D2); nothing here reads the real file', { skip: !guarded && 'guard off' }, () => {
    takeViolations();
    const claude = path.join(REAL_HOME, '.claude');
    // Other spellings of the real files: identity (stat only) or the normalised name.
    const spellings = [path.join(claude, 'settings.json'), path.join(claude.toUpperCase(), 'SETTINGS.JSON'), path.join(claude, 'settings.local.json'), path.join(claude, 'Settings.Local.Json'), path.join(claude, '..', '.claude', 'settings.json')];
    if (fs.existsSync('/System/Volumes/Data' + REAL_HOME)) spellings.push(path.join('/System/Volumes/Data' + REAL_HOME, '.claude', 'settings.json'), path.join('/System/Volumes/Data' + REAL_HOME, '.claude', 'settings.local.json'));
    for (const p of spellings) assert.ok(isProtected(resolveReal(p)), p);
    assert.ok(!isProtected(resolveReal(path.join(tempDir('iso-not-'), '.claude', 'settings.json'))), 'a temp settings file is not protected');
    // The wrappers, on a temp file protected the same way.
    const f = path.join(tempDir('iso-protected-'), 'decoy.json');
    fs.writeFileSync(f, '{}');
    protectForTest(f);
    for (const [what, attempt] of [
      ['readFileSync', () => fs.readFileSync(f)],
      ['openAsBlob', () => fs.openAsBlob(f)],
      ['new fs.ReadStream', () => new fs.ReadStream(f)],
      ['new fs.WriteStream', () => new fs.WriteStream(f)],
      ["process.binding('fs')", () => process.binding('fs').open(f, 0, 0o666)],
      ['firmlink spelling', () => fs.existsSync('/System/Volumes/Data') && fs.readFileSync('/System/Volumes/Data' + resolveReal(f))],
    ]) {
      if (what === 'firmlink spelling' && !fs.existsSync('/System/Volumes/Data' + resolveReal(f))) continue;
      assert.throws(attempt, /test isolation: .*protected like them/, what);
    }
    assert.ok(takeViolations().length >= 5, 'every attempt was recorded');
  });

  test('the harness refuses a real HOME in any spelling, and arguments that name the real settings (BR-3K M4 D2)', () => {
    const homes = [REAL_HOME, `${REAL_HOME}/`, REAL_HOME.toUpperCase(), path.join(REAL_HOME, '..', path.basename(REAL_HOME))];
    if (fs.existsSync('/System/Volumes/Data' + REAL_HOME)) homes.push('/System/Volumes/Data' + REAL_HOME);
    for (const h of homes) {
      assert.ok(isRealHome(h), h);
      assert.ok(!isRealHome(rustEnv(['doctor'], { PATH: process.env.PATH, HOME: h }).HOME), `rustEnv kept ${h}`);
    }
    const real = path.join(REAL_HOME, '.claude', 'settings.json');
    const other = [real, real.toUpperCase(), path.join(REAL_HOME, '.claude', 'settings.local.json')];
    if (fs.existsSync('/System/Volumes/Data' + REAL_HOME)) other.push('/System/Volumes/Data' + real);
    for (const p of other) {
      for (const args of [['hooks', 'uninstall', '--write', '--settings', p], ['hooks', 'install', `--settings=${p}`], ['hooks', 'print', '--out', p], ['migrate', '--settings', p], ['doctor', '--settings', p]]) {
        assert.throws(() => brainCommand(args), /names the real Claude Code settings/, args.join(' '));
      }
    }
    assert.throws(() => brainCommand(['hooks', 'install', '--settings', path.join('.claude', 'settings.json')], { cwd: REAL_HOME }), /names the real Claude Code settings/, 'a relative path, against its cwd');
    // Any spelling of the live port, for every command.
    if (IMPL === 'rust') for (const p of ['47417', '047417', '+47417', ' 47417', '47417.0']) assert.throws(() => brainCommand(['doctor', '--port', p]), /live brain's port/, p);
  });

  test('the strip build never goes to target/: plain cargo builds, from the root, shell/ or src-tauri, use target-dev', () => {
    // cargo reads .cargo/config.toml from the directory it runs in and its parents: one in each.
    for (const [dir, want] of [['', 'shell/src-tauri/target-dev'], ['shell', 'src-tauri/target-dev'], ['shell/src-tauri', 'target-dev']]) {
      const cfg = fs.readFileSync(path.join(AGENT_DIR, dir, '.cargo', 'config.toml'), 'utf8');
      assert.match(cfg, new RegExp(`^target-dir = "${want}"$`, 'm'), dir || '.');
    }
    assert.match(fs.readFileSync(path.join(AGENT_DIR, '.gitignore'), 'utf8'), /^shell\/src-tauri\/target-dev\/$/m);
    const cli = fs.readFileSync(path.join(AGENT_DIR, 'brain', 'cli.mjs'), 'utf8');
    assert.equal([...cli.matchAll(/targetDir: 'target'(?!-)/g)].length, 1, 'only `shell build --release` builds into target/');
  });
});

describe('doctor and old rejections', () => {
  // A stand-in brain on a free port, answering /health with given counters.
  const fakeBrain = async (counters) => {
    const s = http.createServer((req, res) => {
      if (req.url === '/health') {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ ok: true, name: 'bellwether', version: '0.1.0', pid: 1, mode: 'normal', counters }));
      } else {
        res.writeHead(204);
        res.end();
      }
    });
    await new Promise((r) => s.listen(0, '127.0.0.1', r));
    return s;
  };
  const doctorAt = async (counters) => {
    const s = await fakeBrain(counters);
    try {
      const settings = path.join(tempDir('iso-doc-'), 'settings.json');
      fs.writeFileSync(settings, '{}');
      const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: 'rejectionsTokenNotReal_012345' }, settingsFile: settings, configLoader: () => ({ config: { port: s.address().port }, usingExample: false }), home: tempDir('iso-h-'), dataDir: tempDir('iso-d-'), findCargoFor: () => null });
      return r.checks.find((c) => c.name === 'rejections');
    } finally {
      await new Promise((r) => s.close(r));
    }
  };
  const rejected = { origin: 0, host: 0, contentType: 0, token: 9 };
  test('a refusal in the last 15 minutes is a WARN; older ones are INFO; a brain without the timestamp still warns', async () => {
    assert.equal((await doctorAt({ accepted: 1, rejected, lastRejectedAt: Date.now() - 60_000 })).status, 'WARN');
    const old = await doctorAt({ accepted: 1, rejected, lastRejectedAt: Date.now() - 60 * 60_000 });
    assert.equal(old.status, 'INFO');
    assert.match(old.detail, /token=9 refused since the brain started[\s\S]*none in the last 15 minutes/);
    assert.equal((await doctorAt({ accepted: 1, rejected })).status, 'WARN');
    assert.equal(await doctorAt({ accepted: 1, rejected: { origin: 0, host: 0, contentType: 0, token: 0 }, lastRejectedAt: null }), undefined);
  });
});

describe('the harness never lets an app bundle run `autostart` (plan deviation 56)', () => {
  // BR-3K's M6 repro: BELLWETHER_BIN as a symlink, outside any bundle, to a bundle's
  // executable. The binary resolves its own path to find its bundle, so it would register
  // real login items; the harness must resolve it the same way. A stand-in file, never run.
  test('a symlink to Bellwether.app/Contents/MacOS/Bellwether is refused', () => {
    const dir = tempDir('iso-bundle-');
    const exe = path.join(dir, 'Bellwether.app', 'Contents', 'MacOS', 'Bellwether');
    fs.mkdirSync(path.dirname(exe), { recursive: true });
    fs.writeFileSync(exe, '#!/bin/sh\nexit 99\n', { mode: 0o755 });
    const link = path.join(dir, 'bin', 'bellwether-shell');
    fs.mkdirSync(path.dirname(link));
    fs.symlinkSync(exe, link);
    for (const args of [['autostart', 'install', '--write'], ['autostart', 'install', '--write', '--brain-only'], ['autostart', 'uninstall', '--write'], ['autostart']]) {
      assert.throws(() => refuseRust(args, link), /app bundle/, args.join(' '));
      assert.throws(() => refuseRust(args, exe), /app bundle/, args.join(' '));
    }
    // A chain of links, and a relative one, resolve the same way.
    const link2 = path.join(dir, 'bin', 'again');
    fs.symlinkSync('bellwether-shell', link2);
    assert.throws(() => refuseRust(['autostart', 'install', '--write'], link2), /app bundle/);
    // A development binary outside a bundle may run it; `settings` never; other commands always.
    const dev = path.join(dir, 'bin', 'dev-bellwether-shell');
    fs.writeFileSync(dev, '', { mode: 0o755 });
    assert.doesNotThrow(() => refuseRust(['autostart', 'install', '--write'], dev));
    assert.throws(() => refuseRust(['autostart', 'settings'], dev), /System Settings/);
    assert.doesNotThrow(() => refuseRust(['doctor'], link));
    // A path that doesn't resolve is refused rather than guessed at.
    assert.throws(() => refuseRust(['autostart', 'status'], path.join(dir, 'missing')), /app bundle/);
  });
});
