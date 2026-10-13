import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { describe, test } from 'node:test';
import { loadConfig } from '../brain/config.mjs';
import { DATA_DIR } from '../brain/util.mjs';
import { runDoctor } from '../brain/doctor.mjs';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';
import { ISOLATED, takeViolations } from './setup.mjs';

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
