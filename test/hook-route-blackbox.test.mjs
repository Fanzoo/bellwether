import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, TOKEN, hookHeaders, request, tempDir } from './helpers.mjs';

/**
 * The /hook guard matrix from hook-route.test.mjs, black-box: against a brain process,
 * reading its counters from /health instead of from memory. It runs against either
 * implementation (distribution plan D2, milestone 1):
 *
 *   npm test                 the Node brain (brain/cli.mjs serve)
 *   npm run test:rust        the Rust brain: $BELLWETHER_BIN serve, by default the dev
 *                            build shell/src-tauri/target-dev/debug/bellwether-shell,
 *                            refused when it is older than the Rust sources
 *
 * Every response from /hook must be status-only: an empty 204, or an empty 403.
 */

const IMPL = process.env.BELLWETHER_IMPL === 'rust' ? 'rust' : 'node';
const RUST_BIN = process.env.BELLWETHER_BIN || path.join(AGENT_DIR, 'shell', 'src-tauri', 'target-dev', 'debug', 'bellwether-shell');

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

/** Newest mtime (ms) of the Rust sources the binary is built from. */
function newestSource() {
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

/** Starts one brain on a free port (never 47417) and waits for /health. */
async function startBrain(extra = []) {
  const port = await freePort();
  const dir = tempDir('bw-blackbox-');
  let cmd;
  let args;
  const env = { ...process.env, BELLWETHER_HOOK_TOKEN: TOKEN };
  if (IMPL === 'rust') {
    if (!fs.existsSync(RUST_BIN)) throw new Error(`BELLWETHER_IMPL=rust needs the binary: build it with \`cargo build --manifest-path shell/src-tauri/Cargo.toml\` (a debug build, into target-dev/) or set BELLWETHER_BIN (looked for ${RUST_BIN})`);
    // The default dev build must not be older than its sources: a stale binary would
    // test yesterday's brain. (A BELLWETHER_BIN given by hand is taken as it is.)
    if (!process.env.BELLWETHER_BIN && fs.statSync(RUST_BIN).mtimeMs < newestSource()) throw new Error(`${RUST_BIN} is older than the Rust sources: rebuild it with \`cargo build --manifest-path shell/src-tauri/Cargo.toml\` (a debug build, into target-dev/), then run \`npm run test:rust\` again`);
    cmd = RUST_BIN;
    args = ['serve', '--port', String(port), ...extra];
    env.BELLWETHER_HOME = dir;
  } else {
    const config = path.join(dir, 'config.json');
    fs.writeFileSync(config, JSON.stringify({ ...JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8')), port }));
    cmd = process.execPath;
    args = [path.join(AGENT_DIR, 'brain', 'cli.mjs'), 'serve', ...extra];
    env.BELLWETHER_CONFIG = config;
    env.BELLWETHER_DATA_DIR = path.join(dir, 'data');
  }
  const child = spawn(cmd, args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
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
