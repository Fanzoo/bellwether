import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { startServer } from '../brain/server.mjs';
import { AGENT_DIR, TOKEN, hookHeaders, request } from './helpers.mjs';

/**
 * The /hook guard matrix. Every response from /hook must be status-only: an empty
 * 204, or an empty 403 for a guard failure. Never a body Claude Code could parse
 * as a permission decision.
 */

function assertEmpty(res, status) {
  assert.equal(res.status, status);
  assert.equal(res.body.length, 0, 'body must be empty');
  assert.equal(res.headers['content-type'], undefined, 'no Content-Type');
  assert.equal(res.headers['transfer-encoding'], undefined, 'no chunked body');
}

describe('/hook', () => {
  let brain;
  const seen = [];
  let throwNext = false;

  before(async () => {
    brain = await startServer({
      port: 0,
      token: TOKEN,
      onEvent: (payload) => {
        if (throwNext) {
          throwNext = false;
          throw new Error('reducer exploded');
        }
        seen.push(payload);
      },
    });
  });
  after(() => brain.close());

  const post = (opts) => request(brain.port, { method: 'POST', path: '/hook', ...opts });
  const settle = () => new Promise((r) => setTimeout(r, 20));

  test('a valid hook gets an empty 204 and reaches the consumer', async () => {
    const res = await post({ headers: hookHeaders(brain.port), body: JSON.stringify({ hook_event_name: 'Stop', session_id: 's1' }) });
    assertEmpty(res, 204);
    await settle();
    assert.equal(seen.at(-1).session_id, 's1');
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
    ['no token', { 'X-Pasture-Token': undefined }, 'token'],
    ['a wrong token', { 'X-Pasture-Token': 'wrong' }, 'token'],
    ['a token that is a prefix of the real one', { 'X-Pasture-Token': TOKEN.slice(0, -1) }, 'token'],
    ['text/plain', { 'Content-Type': 'text/plain' }, 'contentType'],
    ['no Content-Type', { 'Content-Type': undefined }, 'contentType'],
  ];
  for (const [what, override, reason] of rejections) {
    test(`403, empty, with ${what}`, async () => {
      const headers = hookHeaders(brain.port, override);
      for (const k of Object.keys(headers)) if (headers[k] === undefined) delete headers[k];
      const before = brain.counters.rejected[reason];
      const count = seen.length;
      const res = await post({ headers, body: '{"hook_event_name":"Stop","session_id":"spoof"}' });
      assertEmpty(res, 403);
      assert.equal(res.headers['content-length'], '0');
      assert.equal(brain.counters.rejected[reason], before + 1);
      await settle();
      assert.equal(seen.length, count, 'a rejected request never reaches the consumer');
    });
  }

  const failOpen = [
    ['malformed JSON', '{"hook_event_name":', 'malformed'],
    ['a JSON array', '[1,2,3]', 'malformed'],
    ['an empty body', '', 'malformed'],
    ['an unknown event', '{"hook_event_name":"SomethingNew"}', 'unknownEvent'],
  ];
  for (const [what, body, counter] of failOpen) {
    test(`still an empty 204 for ${what}`, async () => {
      const before = brain.counters[counter];
      assertEmpty(await post({ headers: hookHeaders(brain.port), body }), 204);
      await settle();
      assert.equal(brain.counters[counter], before + 1);
    });
  }

  test('still an empty 204 when the consumer throws', async () => {
    throwNext = true;
    const before = brain.counters.consumerErrors;
    assertEmpty(await post({ headers: hookHeaders(brain.port), body: '{"hook_event_name":"Stop"}' }), 204);
    await settle();
    assert.equal(brain.counters.consumerErrors, before + 1);
  });

  test('a body over 8 MiB is drained, not parsed, and still gets an empty 204', async () => {
    const big = Buffer.alloc(9 * 1024 * 1024, 0x20);
    const count = seen.length;
    assertEmpty(await post({ headers: hookHeaders(brain.port), body: big }), 204);
    await settle();
    assert.equal(seen.length, count);
    assert.equal(brain.counters.oversize, 1);
  });

  test('OPTIONS is never answered with CORS headers', async () => {
    const res = await request(brain.port, { method: 'OPTIONS', path: '/hook', headers: { Host: `127.0.0.1:${brain.port}`, Origin: 'https://evil.example' } });
    assert.equal(res.status, 405);
    assert.equal(res.headers['access-control-allow-origin'], undefined);
    assert.equal(res.body.length, 0);
  });

  test('/health needs no token and carries no content', async () => {
    const res = await request(brain.port, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${brain.port}` } });
    assert.equal(res.status, 200);
    const h = JSON.parse(res.body);
    assert.deepEqual(Object.keys(h).sort(), ['counters', 'lastEventAt', 'mode', 'name', 'ok', 'pid', 'port', 'version']);
    assert.equal(h.port, brain.port);
  });
});

describe('/hook debug modes', () => {
  test('--slow answers with an empty 204 after the delay', async () => {
    const brain = await startServer({ port: 0, token: TOKEN, slowMs: 150 });
    try {
      const t0 = Date.now();
      const res = await request(brain.port, { headers: hookHeaders(brain.port), body: '{"hook_event_name":"Stop"}' });
      assertEmpty(res, 204);
      assert.ok(Date.now() - t0 >= 140, 'delayed');
    } finally {
      await brain.close();
    }
  });

  test('--hang never answers', async () => {
    const brain = await startServer({ port: 0, token: TOKEN, hang: true });
    try {
      const outcome = await Promise.race([
        request(brain.port, { headers: hookHeaders(brain.port), body: '{"hook_event_name":"Stop"}' }).then(() => 'answered', () => 'closed'),
        new Promise((r) => setTimeout(() => r('hung'), 300)),
      ]);
      assert.equal(outcome, 'hung');
    } finally {
      await brain.close();
    }
  });
});

test('source scan: the /hook module has no way to write a body', () => {
  const src = fs.readFileSync(path.join(AGENT_DIR, 'brain', 'hook-route.mjs'), 'utf8');
  const code = src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
  assert.doesNotMatch(code, /\.write\s*\(/, 'res.write');
  assert.doesNotMatch(code, /\.end\s*\(\s*[^)\s]/, 'res.end(<arg>)');
  assert.doesNotMatch(code, /JSON\.stringify/, 'JSON.stringify');
  assert.doesNotMatch(code, /\.pipe\s*\(/, 'pipe');
  assert.doesNotMatch(code, /\.(writeContinue|writeEarlyHints|addTrailers|flushHeaders)\b/);
  for (const m of code.matchAll(/writeHead\s*\(([^,)]*)/g)) {
    assert.match(m[1].trim(), /^(204|403)$/, `writeHead only with 204 or 403, found writeHead(${m[1]}`);
  }
});
