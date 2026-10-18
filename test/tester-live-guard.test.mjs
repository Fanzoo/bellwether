import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, tempDir } from './helpers.mjs';

/**
 * The migrate tests must never touch the maintainer's live machine: no
 * socket to the live brain's port (47417), no launchctl, no lsof. This runs those test
 * files in child processes with a preload that records any such attempt.
 */
const FILES = ['migrate.test.mjs', 'tester-migrate.test.mjs', 'tester-migrate-recheck.test.mjs'].filter((f) => fs.existsSync(path.join(AGENT_DIR, 'test', f)));

describe('live-machine guard', () => {
  test('running the migrate test files records no attempt to reach port 47417, launchctl or lsof', () => {
    const log = path.join(tempDir('liveguard-'), 'attempts.log');
    fs.writeFileSync(log, '');
    const r = spawnSync(process.execPath, ['--import', path.join(AGENT_DIR, 'test', 'live-guard-preload.mjs'), '--test', '--test-concurrency=1', ...FILES.map((f) => path.join(AGENT_DIR, 'test', f))], {
      encoding: 'utf8',
      cwd: AGENT_DIR,
      env: { ...process.env, LIVE_GUARD_LOG: log, BELLWETHER_HOOK_TOKEN: 'liveGuardTokenNotReal_0123456789' },
      timeout: 280000,
    });
    const attempts = fs.readFileSync(log, 'utf8').trim().split('\n').filter(Boolean);
    assert.deepEqual(attempts, [], `these tests reached the live machine:\n${attempts.slice(0, 6).join('\n')}`);
    assert.equal(r.status, 0, `the guarded run failed: ${r.stdout.slice(-400)}`);
  });
  test('the guard itself works: a socket to 47417, a launchctl call and an lsof call are all recorded', () => {
    const log = path.join(tempDir('liveguard2-'), 'a.log');
    fs.writeFileSync(log, '');
    const code = `
      import net from 'node:net'; import cp from 'node:child_process';
      const s = net.connect(47417, '127.0.0.1'); s.on('error', () => {}); s.destroy();
      try { cp.spawnSync('launchctl', ['print', 'gui/0/none']); } catch {}
      try { cp.execFileSync('lsof', ['-v'], { stdio: 'ignore' }); } catch {}
      setTimeout(() => {}, 50);`;
    spawnSync(process.execPath, ['--import', path.join(AGENT_DIR, 'test', 'live-guard-preload.mjs'), '--input-type=module', '-e', code], { env: { ...process.env, LIVE_GUARD_LOG: log }, timeout: 20000 });
    const text = fs.readFileSync(log, 'utf8');
    assert.match(text, /socket to port 47417/);
    assert.match(text, /spawnSync: launchctl/);
    assert.match(text, /execFileSync: lsof/);
  });
});
