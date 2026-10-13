import assert from 'node:assert/strict';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, IMPL, TOKEN, hookHeaders, request, spawnBrain, tempDir } from './helpers.mjs';

/**
 * The /hook guard matrix from hook-route.test.mjs, black-box: against a brain process,
 * reading its counters from /health instead of from memory. It runs against either
 * implementation (distribution plan D2), through the switch in helpers.mjs:
 *
 *   npm test                 the Node brain (brain/cli.mjs serve)
 *   npm run test:rust        the Rust brain: $BELLWETHER_BIN serve, by default the dev
 *                            build shell/src-tauri/target-dev/debug/bellwether-shell,
 *                            refused when it is older than the Rust sources
 *
 * Every response from /hook must be status-only: an empty 204, or an empty 403.
 */

function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once('error', reject);
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

/** Starts one brain on a free port (never 47417) and waits for /health. */
async function startBrain(extra = []) {
  const port = await freePort();
  const dir = tempDir('bw-blackbox-');
  const env = { ...process.env, BELLWETHER_HOOK_TOKEN: TOKEN };
  let args;
  if (IMPL === 'rust') {
    // The Rust brain takes the port as an argument and keeps its files in a home.
    args = ['serve', '--port', String(port), ...extra];
    env.BELLWETHER_HOME = dir;
  } else {
    const config = path.join(dir, 'config.json');
    fs.writeFileSync(config, JSON.stringify({ ...JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8')), port }));
    args = ['serve', ...extra];
    env.BELLWETHER_CONFIG = config;
    env.BELLWETHER_DATA_DIR = path.join(dir, 'data');
  }
  const child = spawnBrain(args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  const deadline = Date.now() + 15000;
  for (;;) {
    try {
      const r = await request(port, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${port}` } });
      if (r.status === 200) break;
    } catch {}
    if (child.exitCode !== null || Date.now() > deadline) {
      child.kill();
      throw new Error(`the ${IMPL} brain didn't start: ${stderr}`);
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  return {
    port,
    stop: () => new Promise((resolve) => {
      if (child.exitCode !== null) return resolve();
      child.once('exit', resolve);
      child.kill('SIGTERM');
    }),
  };
}

function assertEmpty(res, status) {
  assert.equal(res.status, status);
  assert.equal(res.body.length, 0, 'body must be empty');
  assert.equal(res.headers['content-type'], undefined, 'no Content-Type');
  assert.equal(res.headers['transfer-encoding'], undefined, 'no chunked body');
}

describe(`/hook, black-box (${IMPL} brain)`, () => {
  let brain;
  before(async () => {
    brain = await startBrain();
  });
  after(() => brain.stop());

  const post = (opts) => request(brain.port, { method: 'POST', path: '/hook', ...opts });
  const health = async () => JSON.parse((await request(brain.port, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${brain.port}` } })).body);
  const settle = () => new Promise((r) => setTimeout(r, 40));

  test('a valid hook gets an empty 204 and counts as accepted', async () => {
    const before = (await health()).counters.accepted;
    assertEmpty(await post({ headers: hookHeaders(brain.port), body: JSON.stringify({ hook_event_name: 'Stop', session_id: 's1' }) }), 204);
    await settle();
    const h = await health();
    assert.equal(h.counters.accepted, before + 1);
    assert.equal(typeof h.lastEventAt, 'number');
  });

  test('localhost:<port> is an accepted Host, and a charset on the media type is fine', async () => {
    const res = await post({
      headers: hookHeaders(brain.port, { Host: `localhost:${brain.port}`, 'Content-Type': 'application/json; charset=utf-8' }),
      body: '{"hook_event_name":"Stop"}',
    });
    assertEmpty(res, 204);
  });

  const rejections = [
    ['an Origin header', { Origin: 'https://evil.example' }, 'origin'],
    ['a null Origin', { Origin: 'null' }, 'origin'],
    ['Host: evil.example', { Host: 'evil.example' }, 'host'],
    ['Host on another port', { Host: '127.0.0.1:1' }, 'host'],
    ['no token', { 'X-Bellwether-Token': undefined }, 'token'],
    ['a wrong token', { 'X-Bellwether-Token': 'wrong' }, 'token'],
    ['a token that is a prefix of the real one', { 'X-Bellwether-Token': TOKEN.slice(0, -1) }, 'token'],
    ['text/plain', { 'Content-Type': 'text/plain' }, 'contentType'],
    ['no Content-Type', { 'Content-Type': undefined }, 'contentType'],
  ];
  for (const [what, override, reason] of rejections) {
    test(`403, empty, with ${what}`, async () => {
      const headers = hookHeaders(brain.port, override);
      for (const k of Object.keys(headers)) if (headers[k] === undefined) delete headers[k];
      const before = await health();
      const res = await post({ headers, body: '{"hook_event_name":"Stop","session_id":"spoof"}' });
      assertEmpty(res, 403);
      assert.equal(res.headers['content-length'], '0');
      const now = await health();
      assert.equal(now.counters.rejected[reason], before.counters.rejected[reason] + 1);
      assert.equal(now.counters.accepted, before.counters.accepted, 'a rejected request is never accepted');
      assert.equal(typeof now.counters.lastRejectedAt, 'number');
    });
  }

  test('two X-Bellwether-Token headers are refused, even when one of them is right', async () => {
    // Raw bytes: the request helper can't send a header twice.
    const raw = (tokens) =>
      new Promise((resolve, reject) => {
        const body = '{"hook_event_name":"Stop","session_id":"dup"}';
        const sock = net.connect(brain.port, '127.0.0.1');
        let data = '';
        sock.on('data', (d) => (data += d));
        sock.on('end', () => resolve(data));
        sock.on('error', reject);
        const lines = [`POST /hook HTTP/1.1`, `Host: 127.0.0.1:${brain.port}`, 'Content-Type: application/json', ...tokens.map((t) => `X-Bellwether-Token: ${t}`), `Content-Length: ${body.length}`, 'Connection: close', '', body];
        sock.write(lines.join('\r\n'));
      });
    for (const pair of [[TOKEN, 'wrong'], ['wrong', TOKEN], [TOKEN, TOKEN]]) {
      const before = (await health()).counters;
      const text = await raw(pair);
      assert.match(text, /^HTTP\/1\.1 403 /, pair.join(' + '));
      assert.match(text, /content-length: 0\r\n/i);
      const now = (await health()).counters;
      assert.equal(now.rejected.token, before.rejected.token + 1);
      assert.equal(now.accepted, before.accepted);
    }
  });

  const failOpen = [
    ['malformed JSON', '{"hook_event_name":', 'malformed'],
    ['a JSON array', '[1,2,3]', 'malformed'],
    ['an empty body', '', 'malformed'],
    ['an unknown event', '{"hook_event_name":"SomethingNew"}', 'unknownEvent'],
  ];
  for (const [what, body, counter] of failOpen) {
    test(`still an empty 204 for ${what}`, async () => {
      const before = (await health()).counters[counter];
      assertEmpty(await post({ headers: hookHeaders(brain.port), body }), 204);
      await settle();
      assert.equal((await health()).counters[counter], before + 1);
    });
  }

  test('a body over 8 MiB is drained, not parsed, and still gets an empty 204', async () => {
    const big = Buffer.alloc(9 * 1024 * 1024, 0x20);
    assertEmpty(await post({ headers: hookHeaders(brain.port), body: big }), 204);
    await settle();
    assert.equal((await health()).counters.oversize, 1);
  });

  test('OPTIONS is never answered with CORS headers; other routes are an empty 404', async () => {
    const res = await request(brain.port, { method: 'OPTIONS', path: '/hook', headers: { Host: `127.0.0.1:${brain.port}`, Origin: 'https://evil.example' } });
    assert.equal(res.status, 405);
    assert.equal(res.headers['access-control-allow-origin'], undefined);
    assert.equal(res.body.length, 0);
    const nf = await request(brain.port, { method: 'GET', path: '/hook', headers: { Host: `127.0.0.1:${brain.port}` } });
    assert.equal(nf.status, 404);
    assert.equal(nf.body.length, 0);
  });

  test('/health needs no token and carries no content', async () => {
    const res = await request(brain.port, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${brain.port}` } });
    assert.equal(res.status, 200);
    assert.equal(res.headers['cache-control'], 'no-store');
    const h = JSON.parse(res.body);
    assert.deepEqual(Object.keys(h).sort(), ['counters', 'lastEventAt', 'mode', 'name', 'ok', 'pid', 'port', 'version']);
    assert.equal(h.port, brain.port);
    assert.equal(h.name, 'bellwether');
    assert.equal(h.mode, 'normal');
    for (const k of ['accepted', 'malformed', 'oversize', 'unknownEvent', 'consumerErrors']) assert.equal(typeof h.counters[k], 'number', k);
    assert.deepEqual(Object.keys(h.counters.rejected).sort(), ['contentType', 'host', 'origin', 'token']);
  });
});

describe(`/hook debug modes, black-box (${IMPL} brain)`, () => {
  test('--slow answers with an empty 204 after the delay', async () => {
    const brain = await startBrain(['--slow=150']);
    try {
      const t0 = Date.now();
      assertEmpty(await request(brain.port, { headers: hookHeaders(brain.port), body: '{"hook_event_name":"Stop"}' }), 204);
      assert.ok(Date.now() - t0 >= 140, 'delayed');
    } finally {
      await brain.stop();
    }
  });

  test('--hang never answers', async () => {
    const brain = await startBrain(['--hang']);
    try {
      const outcome = await Promise.race([
        request(brain.port, { headers: hookHeaders(brain.port), body: '{"hook_event_name":"Stop"}' }).then(() => 'answered', () => 'closed'),
        new Promise((r) => setTimeout(() => r('hung'), 300)),
      ]);
      assert.equal(outcome, 'hung');
    } finally {
      await brain.stop();
    }
  });
});
