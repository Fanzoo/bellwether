import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, cleaningProblems, readVectors, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { Flock, loadState, makeTextCleaner, normalise, rebuild, withDefaults } from './impl.mjs';

const example = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
const clean = makeTextCleaner({ maxChars: 500, redactPatterns: [] });
const cleanEx = makeTextCleaner({ maxChars: 500, redactPatterns: example.bubble.redactPatterns });

describe('re-check: new secret shapes must be redacted (built-in patterns only)', () => {
  // The cases live in test/fixtures/redact-vectors.json, shared with the Rust port.
  const missed = [];
  for (const v of readVectors('redact', 'new secret shapes')) {
    test(v.name, () => {
      const out = clean(v.text);
      if (out.includes(v.mustNotContain)) missed.push(v.name);
      assert.ok(!out.includes(v.mustNotContain), `leaked "${v.mustNotContain}" in: ${out}`);
      assert.equal(out, v.expected);
    });
  }
});

describe('re-check: ordinary commands are not mangled', () => {
  for (const v of readVectors('redact', 'ordinary commands')) {
    test(v.text, () => {
      const out = clean(v.text);
      assert.ok(!out.includes('•••'), `false positive: ${out}`);
      assert.equal(out, v.expected);
    });
  }
  test('redacted output keeps the readable key so the bubble still says what ran', () => {
    assert.match(clean('DB_PASSWORD=hunter2hunter2 psql -h db'), /DB_PASSWORD=•••.*psql -h db/);
  });
});

describe('re-check: ReDoS and size', () => {
  const inputs = () => ({
    a: 'a'.repeat(5e6),
    spaces: 'x' + ' '.repeat(5e6) + 'y',
    pwWords: 'password '.repeat(5e5),
    pwEq: 'password='.repeat(5e5),
    pwEqQuote: 'password="'.repeat(5e5),
    tokenChain: 'token_'.repeat(8e5),
    dashes: '-'.repeat(5e6),
    dashP: ' -p'.repeat(1.5e6),
    dashU: ' -u a'.repeat(1e6),
    dashUColon: ' -u ' + 'a'.repeat(5e6),
    auth: 'authorization:' + ' '.repeat(3e6) + 'x',
    authQuote: 'authorization"'.repeat(3e5),
    bearer: 'bearer ' + 'a'.repeat(5e6),
    bearerSpaces: 'bearer' + ' '.repeat(5e6),
    urls: '://a:'.repeat(1e6),
    userinfo: 'http://' + 'a:'.repeat(2e6),
    jwt: 'eyJ' + 'a'.repeat(5e6),
    jwtDots: 'eyJ' + 'aaaaaaaaaa.'.repeat(4e5),
    quotes: '"'.repeat(5e6),
    singleQ: "password='" + 'a'.repeat(5e6),
    nested: ('token' + '-').repeat(1e6),
    flagLike: (' --' + 'a'.repeat(30) + 'token' + 'b'.repeat(30)).repeat(8e4),
    ws: ('\t \n').repeat(2e6),
  });
  const cleaners = { builtin: clean, example: cleanEx };
  for (const [cn, fn] of Object.entries(cleaners)) {
    test(`every pathological 5 MB input cleans in under 1.5 s (${cn})`, () => {
      // 1.5 s each on the maintainer's Mac, scaled by this machine's measured speed
      // (test/helpers.mjs, cleaningProblems).
      assert.deepEqual(cleaningProblems(fn, inputs(), 1500), []);
    });
  }
  test('output is capped at maxChars even when every character is redacted', () => {
    const c = makeTextCleaner({ maxChars: 20 });
    assert.ok([...c('password=x '.repeat(100))].length <= 20);
  });
});

describe('re-check: D2 replay of a real recording', () => {
  test('every real recording in data/recordings replays with no marker in any snapshot, and no marker is sent in an allowlisted field', (t) => {
    const dir = path.join(AGENT_DIR, 'data', 'recordings');
    if (!fs.existsSync(dir)) return t.skip('no real recordings in data/recordings (private, never committed)');
    const c = makeTextCleaner({ maxChars: 60 });
    const f = new Flock({ config: withDefaults({ port: 0 }), t: 0 });
    let n = 0;
    for (const file of fs.readdirSync(dir).filter((x) => x.endsWith('.jsonl'))) {
      for (const l of fs.readFileSync(path.join(dir, file), 'utf8').split('\n').filter(Boolean)) {
        const line = JSON.parse(l);
        const { payload, headers } = rebuild(line);
        const ev = normalise(payload, headers, line.normalised.receivedAt, { clean: c });
        f.apply(ev);
        n++;
        assert.doesNotMatch(JSON.stringify(ev), /replay/, `${file}: ${JSON.stringify(ev).slice(0, 120)}`);
      }
      f.tick(1e12);
    }
    assert.ok(n > 0);
    assert.doesNotMatch(JSON.stringify(f.snapshot()), /replay/);
  });
  test('the synthetic shapes from the M2 report: Bash PreToolUse/PermissionRequest, Elicitation, sandbox Notification, WebFetch, Task', () => {
    const base = { sessionId: 's', cwd: '/w/p', terminal: { herdrPane: 'w1:p1' } };
    const mk = (n, rk) => ({ normalised: { receivedAt: 1, ...base, ...n }, rawKeys: rk, headersPresent: [] });
    const lines = [
      mk({ event: 'PreToolUse', toolName: 'Bash', toolUseId: 't', toolFingerprint: 'ab' }, { top: ['tool_input', 'tool_name'], tool_input: ['command', 'description'] }),
      mk({ event: 'PermissionRequest', toolName: 'Bash', toolFingerprint: 'ab' }, { top: ['tool_input'], tool_input: ['command'] }),
      mk({ event: 'PreToolUse', toolName: 'WebFetch', toolUseId: 't2', toolFingerprint: 'cd' }, { top: ['tool_input'], tool_input: ['url', 'prompt'] }),
      mk({ event: 'PermissionRequest', toolName: 'WebFetch', toolFingerprint: 'cd' }, { top: ['tool_input'], tool_input: ['url'] }),
      mk({ event: 'PreToolUse', toolName: 'Task', toolUseId: 't3' }, { top: ['tool_input'], tool_input: ['subagent_type', 'prompt'] }),
      mk({ event: 'PreToolUse', toolName: 'mcp__srv__tool', toolUseId: 't4' }, { top: ['tool_input'], tool_input: ['x'] }),
      mk({ event: 'Elicitation', mcpServer: 'srv' }, { top: ['message', 'mcp_server_name', 'requested_schema'] }),
      mk({ event: 'Notification', notificationType: 'permission_prompt' }, { top: ['message', 'notification_type'] }),
      mk({ event: 'Notification', notificationType: 'agent_needs_input' }, { top: ['message', 'notification_type'] }),
    ];
    const c = makeTextCleaner({ maxChars: 60 });
    const f = new Flock({ config: withDefaults({ port: 0 }), t: 0 });
    for (const l of lines) {
      const { payload, headers } = rebuild(l);
      f.apply(normalise(payload, headers, 1, { clean: c }));
    }
    assert.doesNotMatch(JSON.stringify(f.snapshot()), /replay/);
  });
});

describe('re-check: D3/D4 state.json validation and caps', () => {
  const load = (obj) => {
    const d = tempDir('rc-');
    fs.writeFileSync(path.join(d, 'state.json'), typeof obj === 'string' ? obj : JSON.stringify(obj));
    const warns = [];
    return { v: loadState(path.join(d, 'state.json'), (w) => warns.push(w)), warns };
  };
  const mkFlock = (persisted) => new Flock({ config: withDefaults({ port: 0 }), persisted, t: Date.now() });
  const ident = (extra) => ({ colourIndex: 1, slots: { primary: 0 }, label: 'p', lastSeen: Date.now(), ...extra });
  const sheepFor = (f, key = 'herdr:/s/h.sock#w1:p1') => {
    f.apply({ receivedAt: Date.now(), event: 'UserPromptSubmit', sessionId: 's', cwd: '/w/p', projectLabel: 'p', terminal: { herdrPane: 'w1:p1', herdrSocket: '/s/h.sock' } });
    return f.snapshot().flock.find((s) => s.identityKey === key);
  };
  const KEY = 'herdr:/s/h.sock#w1:p1';

  test('negative, fractional, huge, NaN-ish and string colour/slot values never reach a snapshot', () => {
    for (const bad of [-1, 1.5, 1e9, '3', null, {}, [], true]) {
      const { v } = load({ identities: { [KEY]: ident({ colourIndex: bad, slots: { primary: bad } }) } });
      const s = sheepFor(mkFlock(v));
      assert.ok(Number.isInteger(s.colourIndex) && s.colourIndex >= 0 && s.colourIndex < 8, `colour ${JSON.stringify(bad)} -> ${s.colourIndex}`);
      assert.ok(Number.isInteger(s.slot) && s.slot >= 0 && s.slot < 1000, `slot ${JSON.stringify(bad)} -> ${s.slot}`);
    }
  });
  test('muted is only true for boolean true', () => {
    for (const m of ['false', 'true', 1, 'yes', {}, []]) assert.equal(mkFlock(load({ muted: m, identities: {} }).v).muted, false, JSON.stringify(m));
    assert.equal(mkFlock(load({ muted: true, identities: {} }).v).muted, true);
  });
  test('labels: object, number, 1 MB, control characters and bidi marks become a short clean string', () => {
    for (const label of [{ a: 1 }, 5, 'L'.repeat(1e6), 'a‮b\u0007c\nd', ['x']]) {
      const { v } = load({ identities: { [KEY]: ident({ label }) } });
      const f = mkFlock(v);
      f.apply({ receivedAt: Date.now(), event: 'Stop', sessionId: 's', terminal: { herdrPane: 'w1:p1', herdrSocket: '/s/h.sock' } });
      const s = f.snapshot().flock[0];
      assert.equal(typeof s.label, 'string');
      assert.ok(s.label.length <= 200, `label ${s.label.length}`);
      assert.doesNotMatch(s.label, /[\u0000-\u001f‪-‮]/);
    }
  });
  test('slots: too many monitors, huge names and prototype keys are bounded on load and on placed', () => {
    const slots = {};
    for (let i = 0; i < 5000; i++) slots[`m${i}-${'x'.repeat(200)}`] = i;
    const { v } = load({ identities: { [KEY]: ident({ slots }) } });
    const f = mkFlock(v);
    assert.ok(JSON.stringify(f.persistable()).length < 200_000, 'loaded state is not bounded');
    sheepFor(f);
    for (let i = 0; i < 300; i++) f.control({ action: 'placed', identityKey: KEY, monitor: `n${i}-${'y'.repeat(3000)}`, slot: i }, Date.now());
    assert.ok(JSON.stringify(f.persistable()).length < 200_000, `placed grew the state to ${JSON.stringify(f.persistable()).length} bytes`);
    for (const m of ['__proto__', 'constructor']) assert.doesNotThrow(() => f.control({ action: 'placed', identityKey: KEY, monitor: m, slot: 1 }, Date.now()));
    assert.equal(({}).polluted, undefined);
  });
  test('identities: 50 000 on load are capped (most recent kept), and 5000 new identities in a session stay bounded', () => {
    const identities = {};
    for (let i = 0; i < 50_000; i++) identities[`session:${i}`] = ident({ lastSeen: Date.now() - i });
    const f = mkFlock(load({ identities }).v);
    assert.ok(f.persistable().identities && Object.keys(f.persistable().identities).length <= 10_000, `kept ${Object.keys(f.persistable().identities).length}`);
    const g = mkFlock(null);
    for (let i = 0; i < 20_000; i++) g.apply({ receivedAt: Date.now() + i, event: 'UserPromptSubmit', sessionId: `x${i}`, terminal: {} });
    assert.ok(Object.keys(g.persistable().identities).length <= 10_000, `identities ${Object.keys(g.persistable().identities).length}`);
  });
  test('lambs: 5000 mid-flight lambs are capped or time out; tools in flight time out (lambTimeoutMin, toolTimeoutMin)', () => {
    const cfg = withDefaults({ port: 0, behaviour: { lambTimeoutMin: 1, toolTimeoutMin: 1 } });
    const f = new Flock({ config: cfg, t: 0 });
    const term = { herdrPane: 'w1:p1', herdrSocket: '/s/h.sock' };
    f.apply({ receivedAt: 1000, event: 'UserPromptSubmit', sessionId: 's', terminal: term });
    for (let i = 0; i < 5000; i++) f.apply({ receivedAt: 1001 + i, event: 'PreToolUse', sessionId: 's', agentId: `a${i}`, agentType: 'X', toolName: 'Read', toolUseId: `t${i}`, terminal: term });
    f.apply({ receivedAt: 7000, event: 'PreToolUse', sessionId: 's', toolName: 'Bash', toolUseId: 'main', terminal: term });
    const peak = f.snapshot().flock[0];
    assert.ok(peak.lambs.length + peak.hiddenLambCount <= 1000, `${peak.lambs.length + peak.hiddenLambCount} lambs held`);
    // keep the sheep alive with events but silent for the lambs and the main tool
    for (let t = 20_000; t <= 200_000; t += 10_000) f.apply({ receivedAt: t, event: 'Notification', sessionId: 's', notificationType: 'idle_prompt', terminal: term });
    f.tick(200_000);
    f.tick(205_000);
    const later = f.snapshot().flock[0];
    assert.equal(later.lambs.length + later.hiddenLambCount, 0, 'silent lambs never timed out');
    assert.notEqual(later.state === 'busy' && later.flavour === 'running', true, 'a tool in flight never timed out');
  });
  test('an alerting lamb or tool is NOT timed out: a permission prompt waits for hours', () => {
    const cfg = withDefaults({ port: 0, behaviour: { lambTimeoutMin: 1, toolTimeoutMin: 1 } });
    const f = new Flock({ config: cfg, t: 0 });
    const term = { herdrPane: 'w1:p1', herdrSocket: '/s/h.sock' };
    f.apply({ receivedAt: 1000, event: 'UserPromptSubmit', sessionId: 's', terminal: term });
    f.apply({ receivedAt: 1100, event: 'PreToolUse', sessionId: 's', agentId: 'ag', agentType: 'Explore', toolName: 'Bash', toolUseId: 'tl', toolFingerprint: 'fp', terminal: term });
    f.apply({ receivedAt: 1110, event: 'PermissionRequest', sessionId: 's', agentId: 'ag', agentType: 'Explore', toolName: 'Bash', toolFingerprint: 'fp', toolSummary: 'ls', terminal: term });
    f.apply({ receivedAt: 1200, event: 'PreToolUse', sessionId: 's', toolName: 'Write', toolUseId: 'ts', toolFingerprint: 'fp2', terminal: term });
    f.apply({ receivedAt: 1210, event: 'PermissionRequest', sessionId: 's', toolName: 'Write', toolFingerprint: 'fp2', toolSummary: 'a.txt', terminal: term });
    for (let t = 60_000; t <= 3 * 3600_000; t += 600_000) f.apply({ receivedAt: t, event: 'Notification', sessionId: 's', notificationType: 'idle_prompt', terminal: term });
    f.tick(3 * 3600_000);
    const s = f.snapshot().flock[0];
    assert.ok(s.alert, 'the sheep\'s permission alert was timed out');
    assert.ok(s.lambs[0]?.alert, 'the lamb\'s permission alert was timed out');
  });
  test('config validation: lambTimeoutMin and toolTimeoutMin reject negatives and strings', () => {
    for (const k of ['lambTimeoutMin', 'toolTimeoutMin']) {
      const bad = readVectors('config', 'behaviour').filter((v) => v.name.startsWith(`behaviour.${k} `));
      assert.deepEqual(bad.map((v) => v.config.behaviour[k]), [-1, '5', null], k);
      for (const v of bad) assert.throws(() => withDefaults(v.config), (e) => new RegExp(k).test(e.message) && e.message === v.error && e.exitCode === v.exitCode, v.name);
    }
  });
});
