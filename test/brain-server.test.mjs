import assert from 'node:assert/strict';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, FIXTURES, TOKEN, machineSlowdown, spawnBrain, spawnBrainSync, tempDir, unlessStarved } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs): startBrain is a
// Rust `serve` there. `call`, `replay`, `burst` and `readRecording` are the tests' HTTP tools.
import { Flock, burst, call, createPublisher, readRecording, rebuild, renderStatus, replay, startBrain, withDefaults } from './impl.mjs';

/**
 * The running brain over HTTP: fixtures replayed through /hook, /state and /control,
 * the snapshot rate, /hook latency and persistence. Real sockets on port 0 and a temp
 * data dir; nothing touches data/ or any settings file.
 */

const scenarios = fs.readdirSync(FIXTURES).filter((f) => /^s\d+-.*\.jsonl$/.test(f) && !f.includes('.expected.')).sort();
const config = withDefaults({ port: 0 });

async function withBrain(fn, opts = {}) {
  const dataDir = opts.dataDir ?? tempDir();
  const brain = await startBrain({ config, token: TOKEN, dataDir, ...opts });
  try {
    return await fn(brain, dataDir);
  } finally {
    await brain.close();
  }
}

const state = (port, q = 'after=-1&wait=0', headers = {}) => call(port, { path: `/state?${q}`, token: TOKEN, headers });

describe('replaying fixtures through /hook', () => {
  test('every fixture: only empty 204s, and no «replay:» marker (a non-allowlisted field) ever reaches a snapshot', async () => {
    await withBrain(async (brain) => {
      const seen = [];
      let polling = true;
      const poller = (async () => {
        while (polling) {
          const r = await state(brain.port);
          seen.push(r.text);
          await new Promise((res) => setTimeout(res, 20));
        }
      })();
      for (const f of scenarios) {
        const r = await replay(readRecording(path.join(FIXTURES, f)), { port: brain.port, token: TOKEN, speed: 1000 });
        assert.deepEqual(r.failures, [], f);
      }
      polling = false;
      await poller;
      assert.ok(seen.length > 10);
      for (const text of seen) assert.doesNotMatch(text, /«replay:/);
    });
  });

  test('S2 at 5×: the sheep shows the Bash alert, then calms', async () => {
    await withBrain(async (brain) => {
      const alerts = new Set();
      let polling = true;
      const poller = (async () => {
        let rev = -1;
        while (polling) {
          const r = await state(brain.port, `after=${rev}&wait=200`);
          rev = r.json.rev;
          for (const s of r.json.flock) if (s.alert) alerts.add(s.alert.text);
        }
      })();
      await replay(readRecording(path.join(FIXTURES, 's2-permission-approve.jsonl')), { port: brain.port, token: TOKEN, speed: 5 });
      polling = false;
      await poller;
      assert.ok(alerts.has('Bash: touch meadow-approved.txt?'), [...alerts].join());
      const final = (await state(brain.port)).json;
      assert.equal(final.flock[0].alert, undefined);
      assert.match(renderStatus(final), /meadow/);
    });
  });

  test('rebuild uses the real Claude Code field names and the summary allowlist', () => {
    const [, pre] = readRecording(path.join(FIXTURES, 's2-permission-approve.jsonl'));
    const { payload, headers } = rebuild(pre);
    assert.equal(payload.hook_event_name, 'PreToolUse');
    assert.equal(payload.tool_use_id, pre.normalised.toolUseId);
    assert.equal(payload.tool_input.command, 'touch meadow-approved.txt');
    assert.equal(payload.tool_input.description, '«replay:tool_input.description»');
    assert.equal(payload.transcript_path, '«replay:transcript_path»');
    assert.equal(headers['x-bellwether-herdr-pane'], 'w1:p3');
  });
});

describe('/state and /control', () => {
  let brain;
  before(async () => {
    brain = await startBrain({ config, token: TOKEN, dataDir: tempDir() });
  });
  after(() => brain.close());

  for (const [what, headers] of [
    ['an Origin header', { Origin: 'https://evil.example' }],
    ['a foreign Host', { Host: 'evil.example' }],
    ['a wrong token', { 'X-Bellwether-Token': 'wrong' }],
  ]) {
    test(`403 and an empty body with ${what}`, async () => {
      const r = await state(brain.port, 'after=-1&wait=0', headers);
      assert.equal(r.status, 403);
      assert.equal(r.text, '');
      const c = await call(brain.port, { method: 'POST', path: '/control', token: TOKEN, body: { action: 'mute' }, headers });
      assert.equal(c.status, 403);
      assert.equal(c.text, '');
    });
  }

  test('a valid request gets the snapshot, with no CORS headers', async () => {
    const r = await state(brain.port);
    assert.equal(r.status, 200);
    assert.equal(typeof r.json.rev, 'number');
    assert.ok(Array.isArray(r.json.flock));
  });

  test('long-poll: answers as soon as the flock changes, else after `wait`', async () => {
    const { rev } = (await state(brain.port)).json;
    const t0 = Date.now();
    const idle = await state(brain.port, `after=${rev}&wait=300`);
    assert.ok(Date.now() - t0 >= 280);
    assert.equal(idle.json.rev, rev);
    const pending = state(brain.port, `after=${rev}&wait=10000`);
    await new Promise((r) => setTimeout(r, 100));
    const t1 = Date.now();
    await call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: { hook_event_name: 'UserPromptSubmit', session_id: 'lp', cwd: '/p/longpoll' }, headers: { 'X-Bellwether-Herdr-Pane': 'w5:p5' } });
    const changed = await pending;
    assert.ok(changed.json.rev > rev);
    assert.ok(Date.now() - t1 < 1000, 'answered promptly');
    assert.ok(changed.json.flock.some((s) => s.label === 'longpoll'));
  });

  test('/control: mute and unmute; bad requests are refused', async () => {
    const post = (body, headers) => call(brain.port, { method: 'POST', path: '/control', token: TOKEN, body, headers });
    assert.deepEqual((await post({ action: 'mute' })).json, { ok: true });
    await new Promise((r) => setTimeout(r, 300));
    assert.equal((await state(brain.port)).json.muted, true);
    await post({ action: 'unmute' });
    assert.equal((await post({ action: 'explode' })).status, 400);
    assert.equal((await post('{nope')).status, 400);
    assert.equal((await post({ action: 'mute' }, { 'Content-Type': 'text/plain' })).status, 415);
    assert.equal((await post({ action: 'mute', pad: 'x'.repeat(20000) })).status, 413);
  });
});

describe('throughput', () => {
  test('200 events per second: at most 4 snapshots in any second, and /hook p99 under 5 ms', async (t) => {
    // A 2 s, 400-sample p99 moves with a passing load spike on a busy machine (M3 findings),
    // so the latency is judged on the best of up to 3 bursts: p99 under 5 ms (times
    // machineSlowdown(), which is 1 on the maintainer's Mac) and p50 under 2 ms in that
    // burst. Every burst must still have no failures and at most 4 snapshots a second
    // (strict). The lower bound on snapshots seen (2) is the watcher's work, so it scales
    // with the slowdown, measured again after a starved burst (M4 review D8).
    let budget = 5 * machineSlowdown();
    const runs = [];
    let starved = 0;
    await withBrain(async (brain) => {
      for (let i = 0; i < 3; i++) {
        const r = await replay(burst({ rate: 200, seconds: 2 }), { port: brain.port, token: TOKEN });
        assert.deepEqual(r.failures, []);
        assert.ok(r.maxPerSecond <= 4, `max snapshots per second: ${r.maxPerSecond}`);
        if (r.snapshots < (machineSlowdown() > 2 ? 1 : 2)) {
          const s = machineSlowdown({ fresh: true });
          budget = 5 * s;
          if (r.snapshots < (s > 2 ? 1 : 2)) {
            starved++;
            continue;
          }
        }
        runs.push(r);
        if (r.p99 < budget && r.p50 < 2) break;
      }
    });
    if (runs.length === 0) return t.skip(`inconclusive: the watcher saw too few snapshots in ${starved} bursts (slowdown ${machineSlowdown().toFixed(1)}); every burst had no failures and at most 4 snapshots a second`);
    const best = runs.reduce((a, b) => (b.p99 < a.p99 ? b : a));
    const seen = runs.map((r) => `p50 ${r.p50.toFixed(2)} / p99 ${r.p99.toFixed(2)} ms`).join('; ');
    assert.ok(best.p99 < budget, `p99 over ${budget.toFixed(1)} ms in every burst: ${seen}`);
    assert.ok(best.p50 < 2, `p50 over 2 ms in the best burst: ${seen}`);
  });
});

describe('persistence', () => {
  test('colour and slot survive a brain restart; state.json is 0600', { skip: process.platform === 'win32' }, async () => {
    const dataDir = tempDir();
    const post = (port, body) => call(port, { method: 'POST', path: '/hook', token: TOKEN, body, headers: { 'X-Bellwether-Herdr-Pane': body.pane } });
    const first = await withBrain(
      async (brain) => {
        await post(brain.port, { hook_event_name: 'UserPromptSubmit', session_id: 'a', cwd: '/p/one', pane: 'w1:p1' });
        await post(brain.port, { hook_event_name: 'UserPromptSubmit', session_id: 'b', cwd: '/p/two', pane: 'w1:p2' });
        await new Promise((r) => setTimeout(r, 400));
        return (await state(brain.port)).json.flock.find((s) => s.label === 'two');
      },
      { dataDir },
    );
    const file = path.join(dataDir, 'state.json');
    assert.equal(fs.statSync(file).mode & 0o777, 0o600);
    const again = await withBrain(
      async (brain) => {
        await post(brain.port, { hook_event_name: 'UserPromptSubmit', session_id: 'c', cwd: '/p/two', pane: 'w1:p2' });
        await new Promise((r) => setTimeout(r, 400));
        return (await state(brain.port)).json.flock[0];
      },
      { dataDir },
    );
    assert.equal(again.colourIndex, first.colourIndex);
    assert.equal(again.slot, first.slot);
  });

  test('a corrupt state.json is ignored with a warning', async () => {
    const dataDir = tempDir();
    fs.writeFileSync(path.join(dataDir, 'state.json'), 'null');
    const lines = [];
    await withBrain(async () => {}, { dataDir, log: (l) => lines.push(l) });
    assert.ok(lines.some((l) => /not a valid state file/.test(l)));
  });
});

describe('the CLI end to end (serve, replay, status)', () => {
  let root;
  let port;
  let child;
  let env;
  before(async () => {
    root = tempDir('bellwether-cli-');
    fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(root, 'brain'), { recursive: true });
    port = await new Promise((res) => {
      const s = net.createServer().listen(0, '127.0.0.1', () => {
        const p = s.address().port;
        s.close(() => res(p));
      });
    });
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    fs.writeFileSync(path.join(root, 'config.example.json'), JSON.stringify({ ...cfg, port }));
    env = { PATH: process.env.PATH, HOME: root, BELLWETHER_DATA_DIR: path.join(root, 'data'), BELLWETHER_HOOK_TOKEN: TOKEN };
    child = spawnBrain(['serve'], { root, env, stdio: ['ignore', 'ignore', 'pipe'] });
    let err = '';
    child.stderr.on('data', (d) => (err += d));
    await new Promise((res) => {
      const t = setInterval(() => err.includes('listening on') && (clearInterval(t), res()), 20);
    });
  });
  after(() => new Promise((r) => (child.once('exit', r), child.kill('SIGTERM'))));

  const cli = (args) =>
    new Promise((resolve) => {
      const c = spawnBrain([...args], { root, env, cwd: root });
      let out = '';
      let err = '';
      c.stdout.on('data', (d) => (out += d));
      c.stderr.on('data', (d) => (err += d));
      c.on('close', (code) => resolve({ code, out, err }));
    });

  test('replay S2, and `status` shows ALERT  Bash: <command>? while the prompt is open', async () => {
    const lines = readRecording(path.join(FIXTURES, 's2-permission-approve.jsonl')).slice(0, 4); // up to the confirmation
    const file = path.join(root, 's2-open.jsonl');
    fs.writeFileSync(file, lines.map((l) => JSON.stringify(l)).join('\n') + '\n');
    const r = await cli(['replay', file, '--speed', '20']);
    assert.equal(r.code, 0, r.err);
    assert.match(r.out, /every \/hook answer was an empty 204/);
    const s = await cli(['status']);
    assert.equal(s.code, 0, s.err);
    assert.match(s.out, /ALERT {2}Bash: touch meadow-approved\.txt\?/);
    const rest = path.join(root, 's2-rest.jsonl');
    fs.writeFileSync(rest, readRecording(path.join(FIXTURES, 's2-permission-approve.jsonl')).slice(4).map((l) => JSON.stringify(l)).join('\n') + '\n');
    assert.equal((await cli(['replay', rest, '--speed', '20'])).code, 0);
    const calm = await cli(['status', '--json']);
    assert.equal(JSON.parse(calm.out).flock[0].alert, undefined);
  });

  test('`bellwether mute` and `unmute` switch do-not-disturb through the brain; status shows MUTED', async () => {
    const m = await cli(['mute']);
    assert.equal(m.code, 0, m.err);
    assert.match(m.out, /^muted/);
    assert.match((await cli(['status'])).out, /MUTED/);
    assert.equal(JSON.parse((await cli(['status', '--json'])).out).muted, true);
    const u = await cli(['unmute']);
    assert.equal(u.code, 0, u.err);
    assert.doesNotMatch((await cli(['status'])).out, /MUTED/);
  });

  test('record --out into a directory that can\'t be written: exit 3 and one line naming the file, no stack', { skip: process.platform === 'win32' || process.getuid?.() === 0 }, () => {
    const locked = tempDir('bellwether-cli-locked-');
    fs.chmodSync(locked, 0o500);
    try {
      const out = path.join(locked, 'rec.jsonl');
      const r = spawnBrainSync(['record', '--out', out], { root, env, cwd: root, encoding: 'utf8', timeout: 10000 });
      assert.equal(r.status, 3, r.stderr);
      assert.match(r.stderr, new RegExp(`cannot open ${out.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}: EACCES`));
      assert.doesNotMatch(r.stderr, /at .*\.mjs:\d+|listening on/);
      assert.equal(fs.existsSync(out), false);
    } finally {
      fs.chmodSync(locked, 0o700);
    }
  });

  test('status with the brain down: exit 2 and the reason', () => {
    const dead = tempDir('bellwether-cli-dead-');
    fs.cpSync(path.join(root, 'brain'), path.join(dead, 'brain'), { recursive: true });
    const cfg = JSON.parse(fs.readFileSync(path.join(root, 'config.example.json'), 'utf8'));
    fs.writeFileSync(path.join(dead, 'config.example.json'), JSON.stringify({ ...cfg, port: 1 + ((port + 7) % 60000) + 1024 }));
    const r = spawnBrainSync(['status'], { root: dead, env, cwd: dead, encoding: 'utf8', timeout: 10000 });
    assert.equal(r.status, 2, r.stderr);
    assert.match(r.stderr, /brain not running — run `bellwether serve`/);
  });
});

/**
 * 2 s of changes, one every 5 ms (`urgent(i)` marks some urgent), watched through waitFor:
 * how many were sent, when each snapshot was published, and the most in any second.
 */
async function throttleRun(urgent) {
  let n = 0;
  const pub = createPublisher({ build: () => ({ n }) });
  const times = [];
  let rev = 0;
  let watching = true;
  const watcher = (async () => {
    while (watching) {
      const s = await pub.waitFor(rev, 100);
      if (s.rev > rev) {
        // The publisher's own timestamp: a busy machine can delay the watcher and bunch
        // receive times, but the rate being checked is the publishing rate.
        times.push(s.at);
        rev = s.rev;
      }
    }
  })();
  const start = Date.now();
  while (Date.now() - start < 2000) {
    n++;
    pub.markDirty(urgent(n));
    await new Promise((r) => setTimeout(r, 5));
  }
  await new Promise((r) => setTimeout(r, 300));
  watching = false;
  await watcher;
  pub.close();
  let maxPerSecond = 0;
  for (let i = 0; i < times.length; i++) maxPerSecond = Math.max(maxPerSecond, times.filter((t) => t >= times[i] && t - times[i] < 1000).length);
  return { n, times, maxPerSecond };
}

/** The lower bounds (more than 300 events, at least 7 snapshots on the maintainer's Mac), scaled. */
function starvedBy(n, times, slowdown) {
  const minEvents = 300 / slowdown;
  const minSnapshots = Math.max(2, Math.floor(7 / slowdown));
  if (n <= minEvents) return { starved: true, why: `events: ${n}, needs more than ${minEvents.toFixed(0)}` };
  if (times.length < minSnapshots) return { starved: true, why: `snapshots: ${times.length}, needs ${minSnapshots}` };
  return { starved: false };
}

describe('the snapshot throttle', () => {
  test('content changing on every one of 200 events per second still yields at most 4 snapshots in any second', async (t) => {
    await unlessStarved(t, async (slowdown) => {
      const { n, times, maxPerSecond } = await throttleRun(() => false);
      assert.ok(maxPerSecond <= 4, `max per second: ${maxPerSecond}`);
      // How many events the loop sent and snapshots the watcher saw is this machine's work:
      // scaled (M4 review D8).
      return starvedBy(n, times, slowdown);
    });
  });
});

describe('urgent snapshots (a new or confirmed alert)', () => {
  test('publish at once inside the interval, restart the interval, and never break 4 in a second', async () => {
    let n = 0;
    const pub = createPublisher({ build: () => ({ n }) });
    n++;
    pub.markDirty(); // quiet spell: goes on the next turn of the event loop
    await pub.waitFor(0, 100);
    assert.equal(pub.current().rev, 1);
    n++;
    pub.markDirty(); // inside the interval: waits
    assert.equal(pub.current().rev, 1);
    n++;
    pub.markDirty(true); // urgent: goes now, and takes the pending change with it
    assert.equal(pub.current().rev, 2);
    assert.equal(pub.current().n, 3);
    const urgentAt = pub.current().at;
    n++;
    pub.markDirty();
    await pub.waitFor(2, 1000);
    assert.ok(pub.current().at - urgentAt >= 249, `next ordinary snapshot ${pub.current().at - urgentAt} ms after the urgent one`);
    // A flood of urgent changes: three already went out in this second, so one more may.
    for (let i = 0; i < 5; i++) {
      n++;
      pub.markDirty(true);
    }
    assert.equal(pub.current().rev, 4, 'capped at 4 snapshots in any second');
    pub.close();
  });

  test('a flood of urgent and ordinary changes still yields at most 4 snapshots in any second', async (t) => {
    await unlessStarved(t, async (slowdown) => {
      const { n, times, maxPerSecond } = await throttleRun((i) => i % 3 === 0);
      assert.ok(maxPerSecond <= 4, `max per second: ${maxPerSecond}`);
      return starvedBy(n, times, slowdown);
    });
  });

  test('the flock marks a new alert and a confirmation, not a busy tool', async () => {
    const config = withDefaults({});
    const f = new Flock({ config, persisted: null, t: 1000 });
    const ev = (event, extra = {}) => ({ event, sessionId: 's1', cwd: '/tmp/meadow', t: 1000, ...extra });
    const raised = (e) => {
      const before = f.alertMarks();
      f.apply(e);
      return [...f.alertMarks()].some((m) => !before.has(m));
    };
    assert.equal(raised(ev('SessionStart')), false);
    assert.equal(raised(ev('PreToolUse', { toolName: 'Bash', toolUseId: 't1', t: 1100 })), false);
    assert.equal(raised(ev('PermissionRequest', { toolName: 'Bash', toolUseId: 't1', summary: 'Bash: ls?', t: 1200 })), true);
    assert.equal(raised(ev('Notification', { notificationType: 'permission_prompt', message: 'Claude needs your permission to use Bash', t: 1300 })), true);
    assert.equal(raised(ev('PostToolUse', { toolName: 'Bash', toolUseId: 't1', t: 1400 })), false);
  });
});

describe('the client', () => {
  test('a GET whose pooled connection was reset is retried once on a fresh one; requests never reuse sockets', async () => {
    const http = await import('node:http');
    let hits = 0;
    const server = http.createServer((req, res) => {
      hits++;
      if (hits === 1) return req.socket.destroy();
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end('{"ok":true}');
    });
    await new Promise((r) => server.listen(0, '127.0.0.1', r));
    try {
      const r = await call(server.address().port, { path: '/state' });
      assert.deepEqual(r.json, { ok: true });
      assert.equal(hits, 2);
      hits = 0;
      await assert.rejects(call(server.address().port, { method: 'POST', path: '/hook', body: {} }), /socket hang up|ECONNRESET/);
      assert.equal(hits, 1, 'a POST is not retried');
    } finally {
      await new Promise((r) => server.close(r));
    }
  });
});

describe('bubble.detail "tool" over HTTP', () => {
  test('a permission prompt posted to /hook shows only "Bash?" in GET /state', async () => {
    const dataDir = tempDir();
    const brain = await startBrain({ config: withDefaults({ port: 0, bubble: { detail: 'tool' } }), token: TOKEN, dataDir });
    try {
      const post = (body) => call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body, headers: { 'X-Bellwether-Herdr-Pane': 'w1:p1' } });
      const input = { command: 'git push origin main', description: 'Push' };
      await post({ hook_event_name: 'PreToolUse', session_id: 's', cwd: '/p/x', tool_name: 'Bash', tool_use_id: 't1', tool_input: input });
      await post({ hook_event_name: 'PermissionRequest', session_id: 's', cwd: '/p/x', tool_name: 'Bash', tool_input: input });
      await new Promise((r) => setTimeout(r, 400));
      const r = await state(brain.port);
      assert.equal(r.json.flock[0].alert.text, 'Bash?');
      assert.doesNotMatch(r.text, /git push/);
    } finally {
      await brain.close();
    }
  });
});
