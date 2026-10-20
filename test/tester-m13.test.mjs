import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, TOKEN, spawnBrain, tempDir } from './helpers.mjs';
import { call } from './impl.mjs';

// M13 tester: a real `serve` process, its state.json after each way a brain can stop.
// Scratch BELLWETHER_HOME and a free port; nothing here touches the live install.

const freePort = () =>
  new Promise((res) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
  });

async function start(extraEnv = {}) {
  const root = tempDir('bellwether-m13-');
  const port = await freePort();
  const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
  fs.writeFileSync(path.join(root, 'config.example.json'), JSON.stringify({ ...cfg, port }));
  const env = { PATH: process.env.PATH, HOME: root, BELLWETHER_DATA_DIR: path.join(root, 'data'), BELLWETHER_HOOK_TOKEN: TOKEN, ...extraEnv };
  const child = spawnBrain(['serve'], { root, env, stdio: ['ignore', 'ignore', 'pipe'] });
  let err = '';
  child.stderr.on('data', (d) => (err += d));
  const exited = new Promise((r) => child.once('exit', (code, sig) => r({ code, sig })));
  for (let i = 0; i < 500 && !err.includes('listening on'); i++) await new Promise((r) => setTimeout(r, 20));
  assert.ok(err.includes('listening on'), err);
  return { root, port, child, exited, err: () => err, stateFile: path.join(root, 'data', 'state.json') };
}

const control = (port, action) => call(port, { method: 'POST', path: '/control', token: TOKEN, body: { action } });
const stateOf = (f) => JSON.parse(fs.readFileSync(f, 'utf8'));
const leftovers = (f) => fs.readdirSync(path.dirname(f)).filter((n) => /\.tmp$/.test(n));

describe('M13 tester: a stopping brain leaves its last state in state.json', () => {
  for (const sig of ['SIGTERM', 'SIGINT']) {
    test(`${sig} right after a change (the 2 s save timer not yet due): flushed, exit 0, no temp file left`, async () => {
      const b = await start();
      assert.equal((await control(b.port, 'mute')).status, 200);
      assert.ok(!fs.existsSync(b.stateFile), 'not written yet: the timer is 2 s');
      const t0 = Date.now();
      b.child.kill(sig);
      const r = await b.exited;
      assert.equal(r.code, 0, b.err());
      assert.ok(Date.now() - t0 < 3000, `stopped in ${Date.now() - t0} ms`);
      assert.equal(stateOf(b.stateFile).muted, true);
      assert.deepEqual(leftovers(b.stateFile), []);
      assert.equal(fs.statSync(b.stateFile).mode & 0o777, 0o600);
    });
  }

  test('the parent gone (BELLWETHER_PARENT_PID names another process): stops by itself and flushes', async () => {
    const decoy = spawn('sleep', ['60'], { stdio: 'ignore' }); // my own process, killed by pid below
    try {
      const b = await start({ BELLWETHER_PARENT_PID: String(decoy.pid) });
      assert.equal((await control(b.port, 'mute')).status, 200);
      const r = await Promise.race([b.exited, new Promise((res) => setTimeout(() => res('hung'), 5000))]);
      assert.notEqual(r, 'hung', 'the brain did not stop when its parent was not its parent');
      assert.match(b.err(), /is gone; stopping/);
      assert.equal(stateOf(b.stateFile).muted, true);
    } finally {
      decoy.kill('SIGKILL');
    }
  });

  test('mute, unmute, mute then stop: the last one wins (coalescing keeps the newest)', async () => {
    const b = await start();
    await control(b.port, 'mute');
    await control(b.port, 'unmute');
    await control(b.port, 'mute');
    b.child.kill('SIGTERM');
    await b.exited;
    assert.equal(stateOf(b.stateFile).muted, true);
    const b2 = await start({ BELLWETHER_DATA_DIR: path.join(b.root, 'data'), HOME: b.root });
    // a restart on the same data dir reads it back
    assert.equal((await call(b2.port, { path: '/state?after=-1&wait=0', token: TOKEN })).json.muted, true);
    b2.child.kill('SIGTERM');
    await b2.exited;
  });

  test('a SIGKILL at random moments while hooks arrive never leaves a torn state.json', async () => {
    const b0 = await start();
    b0.child.kill('SIGTERM');
    await b0.exited;
    let seen = 0;
    for (let i = 0; i < 6; i++) {
      const b = await start({ BELLWETHER_DATA_DIR: path.join(b0.root, 'data'), HOME: b0.root });
      const hooks = [];
      for (let k = 0; k < 20; k++) {
        const payload = { hook_event_name: 'SessionStart', session_id: `s${i}-${k}`, cwd: `/tmp/p${k}` };
        hooks.push(call(b.port, { method: 'POST', path: '/hook', token: TOKEN, body: payload, headers: { 'X-Bellwether-Token': TOKEN } }).catch(() => {}));
        if (k % 5 === 0) await control(b.port, k % 2 ? 'mute' : 'unmute').catch(() => {});
      }
      await new Promise((r) => setTimeout(r, 2000 + i * 120)); // straddle the 2 s save
      b.child.kill('SIGKILL');
      await b.exited;
      await Promise.all(hooks);
      if (fs.existsSync(b0.stateFile)) {
        const s = stateOf(b0.stateFile); // throws on a torn file
        assert.equal(s.version, 1);
        seen++;
      }
    }
    assert.ok(seen >= 3, `state.json existed after ${seen} of 6 kills`);
  });
});
