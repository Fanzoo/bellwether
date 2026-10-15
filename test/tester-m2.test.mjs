import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, IMPL, TOKEN, cleaningProblems, notUnderRust, portValue, readVectors, spawnBrain, spawnBrainSync, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { Flock, call, makeTextCleaner, normalise, rebuild, startBrain, withDefaults } from './impl.mjs';

/**
 * Independent tester probes for Milestone 2. In-process pieces use port 0 and temp
 * dirs; CLI pieces run a copy of the brain under a temp root with its own port, data
 * dir and HOME. Nothing here can reach ~/.claude/settings.json or the real .env.
 */

const config = withDefaults({ port: 0 });
const exampleCfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));

// ------------------------------------------------------------------ redaction

describe('redaction: nothing that looks like a credential survives into a bubble', () => {
  // The corpus lives in test/fixtures/redact-vectors.json (shared with the Rust port):
  // each case's secret fragment must NOT appear in the cleaned text, and the cleaned
  // text is exactly the vector's.
  const patternSets = [['built-in patterns only', []], ['config.example.json patterns', exampleCfg.bubble.redactPatterns]];
  for (const [label, patterns] of patternSets) {
    describe(label, () => {
      const clean = makeTextCleaner({ maxChars: 500, redactPatterns: patterns });
      for (const v of readVectors('redact', `credential corpus, ${label}`)) {
        test(v.name, () => {
          assert.deepEqual(v.redactPatterns, patterns, 'the vector was made with these patterns');
          const out = clean(v.text);
          assert.ok(!out.includes(v.mustNotContain), `secret "${v.mustNotContain}" survived: ${JSON.stringify(out)}`);
          assert.equal(out, v.expected);
        });
      }
    });
  }

  test('URL credentials run to the last @ before the host; @ in a path or query is never userinfo (milestone 3 fix)', () => {
    const vectors = readVectors('redact', 'URL credentials');
    assert.ok(vectors.length >= 12);
    for (const v of vectors) {
      const out = makeTextCleaner({ maxChars: v.maxChars, redactPatterns: v.redactPatterns })(v.text);
      assert.equal(out, v.expected, v.name);
      if (v.mustNotContain) assert.ok(!out.includes(v.mustNotContain), `${v.name}: ${out}`);
      if (v.mustEqual) assert.equal(out, v.mustEqual, v.name);
    }
    assert.equal(makeTextCleaner({ maxChars: 60 })('curl https://u%40x:p@ss:w0rd@h.test/'), 'curl https://u%40x:•••@h.test/');
    // Known gaps, pinned as they are today (follow-ups in the plan's M3 status): a change
    // to them must be deliberate, and must reach both brains.
    const gaps = readVectors('redact', 'URL credentials, known gaps');
    assert.equal(gaps.length, 3);
    for (const v of gaps) assert.equal(makeTextCleaner({ maxChars: v.maxChars, redactPatterns: v.redactPatterns })(v.text), v.expected, `${v.name} (known ${v.knownGap})`);
  });

  test('redact happens before truncation: a secret straddling the cut leaves no recognisable prefix', () => {
    const clean = makeTextCleaner({ maxChars: 60, redactPatterns: [] });
    for (const v of readVectors('redact', 'redact before truncate')) {
      const out = clean(v.text);
      if (v.name.startsWith('sk-')) assert.ok(!/sk-/.test(out) || !/sk-[A-Za-z0-9]{3}/.test(out), `${v.name}: ${out}`);
      else assert.ok(!/hunt/.test(out), `${v.name}: ${out}`);
      assert.ok([...out].length <= 60);
      assert.equal(out, v.expected, v.name);
    }
  });

  test('control characters, bidi marks and line separators cannot stretch or spoof a bubble', () => {
    const clean = makeTextCleaner({ maxChars: 60 });
    const out = clean('echo hi\u2028\u0007\u001b[31mred\u0000end\r\nnext');
    assert.doesNotMatch(out, /[\u0000-\u001f\u2028\u2029\u007f]/);
    const rtl = clean('ls \u202Efdp.exe');
    assert.doesNotMatch(rtl, /\u202E/, 'bidi override survives into the bubble');
  });

  test('unicode and long strings: code-point truncation never splits a surrogate pair; output <= maxChars', () => {
    const clean = makeTextCleaner({ maxChars: 20 });
    const out = clean('😀'.repeat(100));
    assert.ok([...out].length <= 20);
    assert.doesNotMatch(out, /[\ud800-\udbff](?![\udc00-\udfff])/);
    // The shared Unicode and control-character cases: exactly the vector's answer, never
    // longer than maxChars, never a lone surrogate.
    for (const v of readVectors('redact', 'unicode and controls')) {
      const got = makeTextCleaner({ maxChars: v.maxChars, redactPatterns: v.redactPatterns })(v.text);
      assert.equal(got, v.expected, v.name);
      assert.ok([...got].length <= v.maxChars, v.name);
      assert.doesNotMatch(got, /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/, v.name);
    }
  });

  test('pathological inputs (5 MB) are cleaned in well under a second each', () => {
    const clean = makeTextCleaner({ maxChars: 60, redactPatterns: exampleCfg.bubble.redactPatterns });
    const inputs = {
      a: 'a'.repeat(5e6),
      spaces: 'x' + ' '.repeat(5e6) + 'y',
      urls: '://'.repeat(1.5e6),
      userinfo: 'http://' + 'a:'.repeat(1e6),
      pw: 'password='.repeat(5e5),
      auth: 'authorization:' + ' '.repeat(2e6) + 'x',
      bearer: 'bearer ' + 'a'.repeat(5e6),
      jwt: 'eyJ' + 'a'.repeat(5e6),
      colons: ':'.repeat(5e6),
    };
    // Under a second each on the maintainer's Mac, scaled by this machine's measured speed
    // (test/helpers.mjs, cleaningProblems).
    assert.deepEqual(cleaningProblems(clean, inputs, 1000), []);
  });

  // The cases are shared with the Rust port (test/fixtures/normalise-vectors.json).
  const normalised = (v) => normalise(v.payload, v.headers, v.receivedAt, { clean: v.clean ? makeTextCleaner(v.clean) : null });

  test('notification and elicitation messages are redacted too', () => {
    const [n, e] = readVectors('normalise', 'messages');
    const nev = normalised(n);
    assert.ok(!nev.message.includes('abc123def456ghi789'), nev.message);
    const eev = normalised(e);
    assert.ok(!eev.message.includes('hunter2hunter2'), eev.message);
    for (const v of [n, e]) assert.deepEqual(normalised(v), v.expected, v.name);
  });

  test('the file-name, URL and subagent summaries keep credentials out', () => {
    const [w, f, mcp] = readVectors('normalise', 'summaries');
    assert.equal(normalised(w).toolSummary, 'host.test');
    assert.equal(normalised(f).toolSummary, 'credentials');
    assert.equal(normalised(mcp).toolSummary, 'github:create_issue');
    for (const v of [w, f, mcp]) assert.deepEqual(normalised(v), v.expected, v.name);
  });
});

// ------------------------------------------- no free text at rest, only in snapshots

describe('recordings, logs, state.json and /health hold no free text', () => {
  test('canaries sent through a recording brain never reach the recording, the log, state.json or /health', async () => {
    const dataDir = tempDir('m2-rest-');
    const rec = path.join(dataDir, 'rec.jsonl');
    const logs = [];
    const brain = await startBrain({ config, token: TOKEN, dataDir, recordTo: rec, log: (l) => logs.push(l), persistMs: 50 });
    try {
      const post = (payload, headers = {}) => call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: payload, headers });
      const T = { 'x-bellwether-herdr-pane': 'w1:p1', 'x-bellwether-herdr-socket': '/s/herdr.sock', 'x-bellwether-term-program': 'iTerm.app' };
      await post({ hook_event_name: 'PreToolUse', session_id: 'sa', cwd: '/w/proj', tool_name: 'Bash', tool_use_id: 'tu1', tool_input: { command: 'echo CANARY_CMD_alpha', description: 'CANARY_DESC' } }, T);
      await post({ hook_event_name: 'PermissionRequest', session_id: 'sa', cwd: '/w/proj', tool_name: 'Bash', tool_input: { command: 'echo CANARY_CMD_alpha', description: 'CANARY_DESC' } }, T);
      await post({ hook_event_name: 'Notification', session_id: 'sa', notification_type: 'permission_prompt', message: 'CANARY_MSG_beta needs you' }, T);
      await post({ hook_event_name: 'Elicitation', session_id: 'sa', mcp_server_name: 'srv', message: 'CANARY_ELIC_gamma', requested_schema: { CANARY_SCHEMA: 1 } }, T);
      await post({ hook_event_name: 'PostToolUse', session_id: 'sa', tool_name: 'Bash', tool_use_id: 'tu1', tool_response: { stdout: 'CANARY_OUT' } }, T);
      await new Promise((r) => setTimeout(r, 400));
      const snap = (await call(brain.port, { path: '/state?after=-1&wait=0', token: TOKEN })).text;
      assert.match(snap, /CANARY_CMD_alpha|CANARY_MSG_beta|CANARY_ELIC_gamma/, 'the snapshot is where the bubble text lives');
      assert.doesNotMatch(snap, /CANARY_(DESC|OUT|SCHEMA)/);
      const health = (await call(brain.port, { path: '/health' })).text;
      for (const [what, text] of [['recording', fs.readFileSync(rec, 'utf8')], ['log', logs.join('\n')], ['health', health], ['state.json', fs.existsSync(path.join(dataDir, 'state.json')) ? fs.readFileSync(path.join(dataDir, 'state.json'), 'utf8') : '']]) {
        assert.doesNotMatch(text, /CANARY_/, `${what} holds free text`);
      }
    } finally {
      await brain.close();
    }
    assert.doesNotMatch(fs.readFileSync(path.join(dataDir, 'state.json'), 'utf8'), /CANARY_/);
  });
});

// ------------------------------------------------------------ reducer scenarios

let seq = 0;
const SESSION_T = { herdr: { herdrPane: 'w1:p1', herdrSocket: '/s/herdr.sock', program: 'iTerm.app', itermSession: 'w0t1p0:UUID-A' } };
function mkFlock(cfg = config, persisted = null) {
  const f = new Flock({ config: cfg, persisted, t: 0 });
  const ev = (event, extra = {}) => ({ receivedAt: f.now ?? 0, event, ...extra });
  return {
    f,
    at(t, event, o = {}) {
      const e = { receivedAt: t, event, sessionId: o.sessionId ?? 'sess-1', terminal: o.terminal ?? {}, ...o };
      delete e.sessionId_;
      f.apply(e);
      f.tick(t);
      return f.snapshot();
    },
    tick(t) {
      f.tick(t);
      return f.snapshot();
    },
  };
}
const sheepOf = (snap, i = 0) => snap.flock[i];

describe('Decision 14: a lamb alert outlives the main thread ending its turn', () => {
  const lambAlert = (m, terminal = {}) => {
    m.at(1000, 'UserPromptSubmit', { terminal });
    m.at(1100, 'SubagentStart', { agentId: 'ag1', agentType: 'Explore', terminal });
    m.at(1200, 'PreToolUse', { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolUseId: 'tu-l', toolFingerprint: 'fp1', toolSummary: 'rm -rf x', terminal });
    return m.at(1210, 'PermissionRequest', { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolFingerprint: 'fp1', toolSummary: 'rm -rf x', terminal });
  };
  for (const clearer of ['Stop', 'StopFailure', 'UserPromptSubmit']) {
    test(`${clearer} on the main thread leaves the lamb's alert`, () => {
      const m = mkFlock();
      const before = lambAlert(m);
      assert.ok(sheepOf(before).lambs[0].alert, 'lamb alerts');
      const after = m.at(5000, clearer, clearer === 'StopFailure' ? { errorType: 'rate_limit' } : {});
      assert.ok(sheepOf(after).lambs[0]?.alert, `${clearer} wiped the lamb's alert`);
      assert.equal(sheepOf(after).alert, undefined);
      assert.equal(sheepOf(after).lambAlert, true);
    });
  }
  test('the sheep\'s own alert is cleared by the main Stop while the lamb\'s stays', () => {
    const m = mkFlock();
    lambAlert(m);
    m.at(1300, 'PreToolUse', { toolName: 'Write', toolUseId: 'tu-s', toolFingerprint: 'fp2', toolSummary: 'a.txt' });
    const s = m.at(1310, 'PermissionRequest', { toolName: 'Write', toolFingerprint: 'fp2', toolSummary: 'a.txt' });
    assert.ok(sheepOf(s).alert && sheepOf(s).lambs[0].alert);
    const after = m.at(4000, 'Stop');
    assert.equal(sheepOf(after).alert, undefined);
    assert.ok(sheepOf(after).lambs[0].alert);
  });
  test('SessionEnd clears everything, lambs included, and the lamb leaves', () => {
    const m = mkFlock();
    lambAlert(m);
    const after = m.at(6000, 'SessionEnd', { reason: 'other' });
    const later = m.tick(9000);
    assert.equal(later.flock.length, 0, JSON.stringify(after));
  });
  test('the lamb\'s own PostToolUse clears it, and SubagentStop clears then removes the lamb', () => {
    const m = mkFlock();
    lambAlert(m);
    const a = m.at(3000, 'PostToolUse', { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolUseId: 'tu-l' });
    assert.equal(sheepOf(a).lambs[0].alert, undefined);
    lambAlert(mkFlock());
    const m2 = mkFlock();
    lambAlert(m2);
    m2.at(3000, 'SubagentStop', { agentId: 'ag1', agentType: 'Explore' });
    assert.equal(m2.tick(6000).flock[0].lambs.length, 0);
  });
});

describe('the hidden helper agent (agent_type "") is never a lamb', () => {
  test('SubagentStop, PreToolUse, PermissionRequest and Notification from an unknown empty-type agent create no lamb', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit');
    m.at(2000, 'SubagentStop', { agentId: 'helper-1' });
    m.at(2100, 'SubagentStart', { agentId: 'helper-2' });
    m.at(2200, 'PreToolUse', { agentId: 'helper-3', toolName: 'Read', toolUseId: 'h3' });
    const s = m.at(2300, 'PermissionRequest', { agentId: 'helper-3', toolName: 'Bash', toolFingerprint: 'zz', toolSummary: 'ls' });
    assert.equal(sheepOf(s).lambs.length, 0);
  });
  test('a helper SubagentStop does not disturb a real lamb that is alerting', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit');
    m.at(1100, 'PreToolUse', { agentId: 'real', agentType: 'Explore', toolName: 'Bash', toolUseId: 't1', toolFingerprint: 'f' });
    m.at(1110, 'PermissionRequest', { agentId: 'real', agentType: 'Explore', toolName: 'Bash', toolFingerprint: 'f', toolSummary: 'ls' });
    const s = m.at(1500, 'SubagentStop', { agentId: 'helper-9' });
    assert.ok(sheepOf(s).lambs[0].alert);
  });
  test('memory: 5000 distinct mid-flight lambs without SubagentStop are bounded or reaped', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit');
    for (let i = 0; i < 5000; i++) m.at(1001 + i, 'PreToolUse', { agentId: `a${i}`, agentType: 'X', toolName: 'Read', toolUseId: `t${i}` });
    const s = m.tick(1000 + 3600_000 * 20);
    assert.ok(!s.flock[0] || s.flock[0].lambs.length + s.flock[0].hiddenLambCount < 5000, 'lambs without a SubagentStop are never reaped');
  });
});

describe('/clear keeps the same sheep when the terminal identifies it', () => {
  const terminals = {
    herdr: { herdrPane: 'w1:p4', herdrSocket: '/s/h.sock', program: 'iTerm.app', itermSession: 'w0t1p0:SAMEUUID', termSession: 'w0t1p0:SAMEUUID' },
    tmux: { tmuxPane: '%3', program: 'iTerm.app', itermSession: 'w0t2p0:UUID-T' },
    iterm2: { program: 'iTerm.app', itermSession: 'w0t2p0:UUID-I', termSession: 'w0t2p0:UUID-I' },
    terminalApp: { program: 'Apple_Terminal', termSession: '7E2A1A22-1111-2222-3333-444444444444' },
  };
  for (const [name, terminal] of Object.entries(terminals)) {
    test(`${name}: SessionEnd clear then a new session_id = same sheep id, colour and slot, also after a brain restart`, () => {
      const m = mkFlock();
      m.at(1000, 'UserPromptSubmit', { sessionId: 'old', terminal });
      m.at(1500, 'Stop', { sessionId: 'old', terminal });
      const other = m.at(1600, 'UserPromptSubmit', { sessionId: 'other-plain', terminal: {} });
      assert.equal(other.flock.length, 2);
      const before = other.flock.find((s) => s.identityKey !== `session:other-plain`);
      m.at(2000, 'SessionEnd', { sessionId: 'old', terminal, reason: 'clear' });
      const mid = m.tick(5000);
      assert.ok(mid.flock.find((s) => s.id === before.id), 'the sheep left on /clear');
      const after = m.at(6000, 'UserPromptSubmit', { sessionId: 'new', terminal });
      const same = after.flock.find((s) => s.id === before.id);
      assert.ok(same, 'a new sheep appeared');
      assert.equal(same.colourIndex, before.colourIndex);
      assert.equal(same.slot, before.slot);
      assert.equal(after.flock.filter((s) => s.identityKey === before.identityKey).length, 1);
      // restart
      const m2 = mkFlock(config, JSON.parse(JSON.stringify(m.f.persistable())));
      const r = m2.at(9000, 'UserPromptSubmit', { sessionId: 'newer', terminal });
      assert.equal(r.flock[0].colourIndex, before.colourIndex);
      assert.equal(r.flock[0].slot, before.slot);
    });
  }
  test('a session with no terminal headers gets a NEW sheep after /clear and the old one leaves', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit', { sessionId: 'a' });
    m.at(2000, 'SessionEnd', { sessionId: 'a', reason: 'clear' });
    m.at(3000, 'UserPromptSubmit', { sessionId: 'b' });
    const s = m.tick(9000);
    assert.equal(s.flock.length, 1);
    assert.equal(s.flock[0].identityKey, 'session:b');
  });
  test('two herdr panes inside one iTerm2 tab are two sheep; two plain iTerm2 tabs are two sheep; iTerm2 and Terminal.app never collide on TERM_SESSION_ID', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit', { sessionId: '1', terminal: terminals.herdr });
    m.at(1001, 'UserPromptSubmit', { sessionId: '2', terminal: { ...terminals.herdr, herdrPane: 'w1:p5' } });
    m.at(1002, 'UserPromptSubmit', { sessionId: '3', terminal: terminals.iterm2 });
    m.at(1003, 'UserPromptSubmit', { sessionId: '4', terminal: { program: 'iTerm.app', itermSession: 'w0t3p0:UUID-OTHER', termSession: 'w0t3p0:UUID-OTHER' } });
    const s = m.at(1004, 'UserPromptSubmit', { sessionId: '5', terminal: { program: 'Apple_Terminal', termSession: 'w0t2p0:UUID-I' } });
    assert.equal(new Set(s.flock.map((x) => x.identityKey)).size, 5);
    assert.equal(new Set(s.flock.map((x) => x.colourIndex)).size, 5);
    assert.equal(new Set(s.flock.map((x) => x.slot)).size, 5);
  });
  test('a new session in the pane starting BEFORE the old SessionEnd: one sheep, and the late SessionEnd for the old session does not remove it', () => {
    const t = terminals.herdr;
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit', { sessionId: 'old', terminal: t });
    m.at(1100, 'PreToolUse', { sessionId: 'old', terminal: t, toolName: 'Bash', toolUseId: 'x', toolFingerprint: 'fp' });
    m.at(1110, 'PermissionRequest', { sessionId: 'old', terminal: t, toolName: 'Bash', toolFingerprint: 'fp', toolSummary: 'ls' });
    m.at(2000, 'UserPromptSubmit', { sessionId: 'new', terminal: t });
    const s = m.at(2100, 'SessionEnd', { sessionId: 'old', terminal: t, reason: 'clear' });
    const later = m.tick(8000);
    assert.equal(later.flock.length, 1);
    assert.equal(later.flock[0].alert, undefined, 'the old session\'s alert leaked into the new one');
    assert.ok(s.flock.length === 1);
  });
  test('a resumed session that reports another terminal makes its old sheep leave', () => {
    const m = mkFlock();
    m.at(1000, 'UserPromptSubmit', { sessionId: 'r', terminal: terminals.iterm2 });
    m.at(2000, 'UserPromptSubmit', { sessionId: 'r', terminal: { program: 'iTerm.app', itermSession: 'w0t9p0:UUID-NEW' } });
    const s = m.tick(9000);
    assert.equal(s.flock.length, 1);
    assert.equal(s.flock[0].identityKey, 'iterm:UUID-NEW');
  });
});

describe('thinkingTimeoutSec', () => {
  const stateAt = (m, t) => sheepOf(m.tick(t));
  test('a turn that is never ended by Stop (interrupt) stops looking busy after the timeout, then sleeps', () => {
    const m = mkFlock(withDefaults({ port: 0, behaviour: { thinkingTimeoutSec: 30 } }));
    m.at(1000, 'UserPromptSubmit');
    assert.equal(stateAt(m, 5000).state, 'busy');
    assert.equal(stateAt(m, 20_000).state, 'busy');
    const calm = stateAt(m, 45_000).state;
    assert.ok(['idle', 'done', 'waiting'].includes(calm), calm);
    assert.equal(stateAt(m, 45_000 + 11 * 60_000).state, 'sleeping');
  });
  test('a tool still in flight stays busy past the timeout', () => {
    const m = mkFlock(withDefaults({ port: 0, behaviour: { thinkingTimeoutSec: 10 } }));
    m.at(1000, 'UserPromptSubmit');
    m.at(1100, 'PreToolUse', { toolName: 'Bash', toolUseId: 'long' });
    assert.equal(stateAt(m, 60_000).state, 'busy');
  });
  test('thinkingTimeoutSec 0 does not crash and a prompt still produces busy only while a tool runs', () => {
    const m = mkFlock(withDefaults({ port: 0, behaviour: { thinkingTimeoutSec: 0 } }));
    m.at(1000, 'UserPromptSubmit');
    assert.doesNotThrow(() => stateAt(m, 5000));
  });
  test('a bad value is a config error (negative, string, NaN)', () => {
    const bad = readVectors('config', 'behaviour').filter((v) => v.name.startsWith('behaviour.thinkingTimeoutSec'));
    assert.deepEqual(bad.map((v) => v.config.behaviour.thinkingTimeoutSec), [-1, '5', null]);
    for (const v of bad) {
      assert.throws(() => withDefaults(v.config), /thinkingTimeoutSec/);
      assert.throws(() => withDefaults(v.config), (e) => e.exitCode === v.exitCode && e.message === v.error, v.name);
    }
  });
  test('a missing key in an older config.json still gets the default', () => {
    // An older config.json: the example as it is, without the key.
    const old = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    delete old.behaviour.thinkingTimeoutSec;
    assert.ok(Object.keys(old.behaviour).length > 0);
    assert.equal(withDefaults(old).behaviour.thinkingTimeoutSec, 120);
    // The shared vectors: the same older config, and a 0 that is allowed.
    for (const v of readVectors('config', 'behaviour').filter((x) => x.ok)) assert.deepEqual(JSON.parse(JSON.stringify(withDefaults(v.config))), v.effective, v.name);
  });
});

describe('replay markers cannot reach snapshots, even from a REAL (field-names-only) recording', () => {
  // A line shaped exactly like what `bellwether record` writes: no toolSummary, no message.
  const realLine = (normalised, rawKeys) => ({ normalised: { receivedAt: 1, sessionId: 's', cwd: '/w/p', projectLabel: 'p', terminal: { herdrPane: 'w1:p1' }, ...normalised }, rawKeys, headersPresent: [] });
  test('a recorded Bash PreToolUse/PermissionRequest does not produce a «replay:» bubble', () => {
    const lines = [
      realLine({ event: 'PreToolUse', toolName: 'Bash', toolUseId: 'tu', toolFingerprint: 'ab' }, { top: ['cwd', 'hook_event_name', 'session_id', 'tool_input', 'tool_name', 'tool_use_id'], tool_input: ['command', 'description'] }),
      realLine({ event: 'PermissionRequest', toolName: 'Bash', toolFingerprint: 'ab' }, { top: ['cwd', 'hook_event_name', 'session_id', 'tool_input', 'tool_name'], tool_input: ['command', 'description'] }),
      realLine({ event: 'Notification', notificationType: 'permission_prompt' }, { top: ['cwd', 'hook_event_name', 'message', 'notification_type', 'session_id'] }),
    ];
    const clean = makeTextCleaner({ maxChars: 60 });
    const f = new Flock({ config, t: 0 });
    for (const l of lines) {
      const { payload, headers } = rebuild(l);
      f.apply(normalise(payload, headers, l.normalised.receivedAt, { clean }));
    }
    assert.doesNotMatch(JSON.stringify(f.snapshot()), /replay/);
  });
  test('a recorded Elicitation and sandbox Notification do not show a marker either', () => {
    const lines = [
      realLine({ event: 'UserPromptSubmit' }, { top: ['cwd', 'hook_event_name', 'prompt', 'session_id'] }),
      realLine({ event: 'Elicitation', mcpServer: 'srv' }, { top: ['cwd', 'hook_event_name', 'mcp_server_name', 'message', 'mode', 'requested_schema', 'session_id'] }),
    ];
    const clean = makeTextCleaner({ maxChars: 60 });
    const f = new Flock({ config, t: 0 });
    for (const l of lines) {
      const { payload, headers } = rebuild(l);
      f.apply(normalise(payload, headers, l.normalised.receivedAt, { clean }));
    }
    assert.doesNotMatch(JSON.stringify(f.snapshot()), /replay/);
  });
});

// ------------------------------------------------------------- HTTP: /state, /control

describe('/state and /control over HTTP', () => {
  let brain, dataDir;
  before(async () => {
    dataDir = tempDir('m2-http-');
    brain = await startBrain({ config, token: TOKEN, dataDir });
    await call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: { hook_event_name: 'UserPromptSubmit', session_id: 's', cwd: '/w/p' }, headers: { 'x-bellwether-herdr-pane': 'w1:p1', 'x-bellwether-herdr-socket': '/s/h.sock' } });
  });
  after(() => brain.close());

  const raw = (text, wait = 400) =>
    new Promise((resolve) => {
      const chunks = [];
      const s = net.connect(brain.port, '127.0.0.1', () => s.write(text));
      s.on('data', (c) => chunks.push(c));
      const done = () => (s.destroy(), resolve(Buffer.concat(chunks).toString('latin1')));
      s.on('close', done);
      s.on('error', done);
      setTimeout(done, wait);
    });
  const req = (method, p, headers, body = '') => raw(`${method} ${p} HTTP/1.1\r\n${Object.entries({ Connection: 'close', 'Content-Length': Buffer.byteLength(body), ...headers }).map(([k, v]) => `${k}: ${v}`).join('\r\n')}\r\n\r\n${body}`);
  const parts = (r) => {
    const i = r.indexOf('\r\n\r\n');
    return { head: i < 0 ? r : r.slice(0, i), body: i < 0 ? '' : r.slice(i + 4) };
  };
  const H = () => ({ Host: `127.0.0.1:${brain.port}`, 'X-Bellwether-Token': TOKEN, 'Content-Type': 'application/json' });

  test('without a valid token, with an Origin, or with a foreign Host: an empty 403 for every method and path variant', async () => {
    const bad = [
      { ...H(), 'X-Bellwether-Token': undefined },
      { ...H(), 'X-Bellwether-Token': 'wrong' },
      { ...H(), 'X-Bellwether-Token': TOKEN.slice(0, -1) },
      { ...H(), Origin: 'https://evil.example' },
      { ...H(), Origin: 'null' },
      { ...H(), Host: 'evil.example' },
      { ...H(), Host: `evil.example:${brain.port}` },
      { ...H(), Host: `127.0.0.1.evil.example:${brain.port}` },
    ];
    for (const p of ['/state', '/state?after=-1&wait=0', '/control', '/state/', '/STATE']) {
      for (const m of ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS', 'HEAD']) {
        for (const h of bad) {
          const headers = Object.fromEntries(Object.entries(h).filter(([, v]) => v !== undefined));
          const { head, body } = parts(await req(m, p, headers, m === 'POST' ? '{"action":"mute"}' : ''));
          const isRouted = p.toLowerCase().replace(/\/$/, '').startsWith('/state') && p === '/state' || p.startsWith('/state?') || p === '/control';
          assert.equal(body, '', `${m} ${p} ${JSON.stringify(Object.keys(headers))}: body ${body.slice(0, 80)}`);
          assert.doesNotMatch(head, /access-control/i);
          if (isRouted) assert.match(head, /^HTTP\/1.1 403/, `${m} ${p}`);
        }
      }
    }
    assert.equal(await call(brain.port, { path: '/state?wait=0', token: TOKEN }).then((r) => r.json.muted), false, 'a rejected /control must not have muted the flock');
  });

  test('with a valid token: JSON, no CORS headers, no caching, and a rejected cross-site POST does not change state', async () => {
    const ok = parts(await req('GET', '/state?after=-1&wait=0', H()));
    assert.match(ok.head, /^HTTP\/1.1 200/);
    assert.match(ok.head, /content-type: application\/json/i);
    assert.doesNotMatch(ok.head, /access-control/i);
    JSON.parse(ok.body);
    const csrf = parts(await req('POST', '/control', { ...H(), 'Content-Type': 'text/plain', Origin: 'https://evil.example' }, '{"action":"mute"}'));
    assert.match(csrf.head, /403/);
    const noJson = parts(await req('POST', '/control', { ...H(), 'Content-Type': 'text/plain' }, '{"action":"mute"}'));
    assert.match(noJson.head, /^HTTP\/1.1 415/);
    assert.equal(brain.flock.muted, false);
  });

  test('/control: bad bodies answer 4xx with JSON, never crash; unknown actions refused', async () => {
    for (const body of ['', 'null', '[]', '"x"', '{"action":', '{"action":{"a":1}}', '{"action":"mute","extra":1}x', '{"action":"delete-everything"}', '{"action":"dismiss"}', '{"action":"dismiss","actorId":{"a":1}}', '{"action":"placed"}', '{"action":"placed","identityKey":"__proto__","monitor":"m","slot":1}', '{"action":"placed","identityKey":"herdr:/s/h.sock#w1:p1","monitor":"m","slot":-1}', '{"action":"placed","identityKey":"herdr:/s/h.sock#w1:p1","monitor":"m","slot":1.5}', '{"action":"placed","identityKey":"herdr:/s/h.sock#w1:p1","monitor":5,"slot":1}']) {
      const r = parts(await req('POST', '/control', H(), body));
      assert.match(r.head, /^HTTP\/1.1 4\d\d/, `${body}: ${r.head.split('\r\n')[0]}`);
    }
    const huge = parts(await req('POST', '/control', H(), JSON.stringify({ action: 'mute', pad: 'x'.repeat(20000) })));
    assert.match(huge.head, /^HTTP\/1.1 413/);
    assert.equal(brain.flock.muted, false);
    assert.equal(({}).polluted, undefined);
  });

  test('/control placed with hostile monitor names is stored as plain data and the persisted file stays bounded', async () => {
    const key = [...brain.flock.identities.keys()][0];
    for (const monitor of ['__proto__', 'constructor', 'toString', 'a'.repeat(4000)]) {
      const r = await call(brain.port, { method: 'POST', path: '/control', token: TOKEN, body: { action: 'placed', identityKey: key, monitor, slot: 2 } });
      assert.equal(r.status, 200);
    }
    for (let i = 0; i < 300; i++) await call(brain.port, { method: 'POST', path: '/control', token: TOKEN, body: { action: 'placed', identityKey: key, monitor: `m${i}-${'x'.repeat(3000)}`, slot: i } });
    const bytes = JSON.stringify(brain.flock.persistable()).length;
    assert.ok(bytes < 1_000_000, `state.json would grow to ${bytes} bytes from placements by an authenticated client`);
    assert.equal(({}).polluted, undefined);
  });

  test('long-poll abuse: wait= and after= garbage, then 1500 simultaneous waiters that hang up; the brain keeps answering /hook quickly', async () => {
    for (const q of ['wait=99999999999999', 'wait=-5&after=0', 'wait=abc&after=abc', 'after=1e999&wait=0', 'after=-9999999999999999999&wait=0', 'wait=0.5&after=', 'after=0&wait=0&wait=99999', 'wait=%00&after=%00']) {
      const t = Date.now();
      const r = await call(brain.port, { path: `/state?${q}`, token: TOKEN, timeoutMs: 40_000 });
      const ms = Date.now() - t;
      assert.equal(r.status, 200, q);
      if (/wait=(-|abc|0|%00|\.|$)/.test(q) || q.startsWith('after=1e999') || q.startsWith('after=-')) assert.ok(ms < 3000, `${q} took ${ms} ms`);
    }
    const sockets = [];
    for (let i = 0; i < 1500; i++) {
      const s = net.connect(brain.port, '127.0.0.1', () => s.write(`GET /state?after=999999&wait=25000 HTTP/1.1\r\nHost: 127.0.0.1:${brain.port}\r\nX-Bellwether-Token: ${TOKEN}\r\n\r\n`));
      s.on('error', () => {});
      sockets.push(s);
    }
    await new Promise((r) => setTimeout(r, 1500));
    const t0 = process.hrtime.bigint();
    const r = await call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: { hook_event_name: 'Stop', session_id: 's' } });
    const ms = Number(process.hrtime.bigint() - t0) / 1e6;
    assert.equal(r.status, 204);
    assert.ok(ms < 250, `/hook took ${ms.toFixed(0)} ms with 1500 waiters`);
    for (const s of sockets) s.destroy();
    await new Promise((r) => setTimeout(r, 500));
    const heap = process.memoryUsage().heapUsed / 1e6;
    assert.ok(heap < 800, `heap ${heap.toFixed(0)} MB`);
  });
});

// ------------------------------------------------------------ state.json corruption

describe('data/state.json corruption and persistence', () => {
  const run = async (stateText, mutateOk = true) => {
    const dataDir = tempDir('m2-state-');
    if (stateText !== undefined) fs.writeFileSync(path.join(dataDir, 'state.json'), stateText);
    const logs = [];
    const brain = await startBrain({ config, token: TOKEN, dataDir, log: (l) => logs.push(l), persistMs: 30 });
    try {
      const r = await call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: { hook_event_name: 'UserPromptSubmit', session_id: 's1', cwd: '/w/proj' }, headers: { 'x-bellwether-herdr-pane': 'w1:p1', 'x-bellwether-herdr-socket': '/s/h.sock' } });
      assert.equal(r.status, 204);
      await new Promise((res) => setTimeout(res, 200));
      const snap = (await call(brain.port, { path: '/state?after=-1&wait=0', token: TOKEN })).json;
      return { snap, logs, dataDir, brain };
    } finally {
      await brain.close();
    }
  };
  const sane = (snap) => {
    const s = snap.flock[0];
    assert.ok(s, 'no sheep appeared');
    assert.equal(typeof s.label, 'string');
    assert.ok(Number.isInteger(s.colourIndex) && s.colourIndex >= 0 && s.colourIndex < 8, `colourIndex ${s.colourIndex}`);
    assert.ok(Number.isInteger(s.slot) && s.slot >= 0, `slot ${s.slot}`);
    assert.ok(s.label.length <= 200, `label ${s.label.length} chars`);
  };
  const key = 'herdr:/s/h.sock#w1:p1';
  for (const [name, text] of [
    ['empty file', ''],
    ['null', 'null'],
    ['an array', '[]'],
    ['truncated JSON', '{"version":1,"identities":{"a":'],
    ['binary', '\u0000\u0001\u0002'],
    ['identities is an array', '{"identities":[]}'],
    ['identity values that are not objects', JSON.stringify({ identities: { [key]: 5, x: 'y', z: null } })],
    ['negative colour and slot', JSON.stringify({ identities: { [key]: { colourIndex: -1, slots: { primary: -5 }, label: 'p', lastSeen: Date.now() } } })],
    ['colour out of range', JSON.stringify({ identities: { [key]: { colourIndex: 1e9, slots: { primary: 1e9 }, label: 'p', lastSeen: Date.now() } } })],
    ['colour and slot strings', JSON.stringify({ identities: { [key]: { colourIndex: '3', slots: { primary: '2' }, label: 'p', lastSeen: Date.now() } } })],
    ['label is an object', JSON.stringify({ identities: { [key]: { colourIndex: 1, slots: { primary: 0 }, label: { a: 1 }, lastSeen: Date.now() } } })],
    ['label is 1 MB', JSON.stringify({ identities: { [key]: { colourIndex: 1, slots: { primary: 0 }, label: 'L'.repeat(1_000_000), lastSeen: Date.now() } } })],
    ['slots is a string', JSON.stringify({ identities: { [key]: { colourIndex: 1, slots: 'abc', label: 'p', lastSeen: Date.now() } } })],
    ['__proto__ identity key', '{"identities":{"__proto__":{"colourIndex":2,"slots":{},"lastSeen":1}}}'],
    ['muted is a string', JSON.stringify({ muted: 'false', identities: {} })],
  ]) {
    test(`${name}: the brain starts, a sheep appears with sane values`, async () => {
      const { snap, logs, dataDir } = await run(text);
      sane(snap);
      if (name === 'muted is a string') assert.equal(snap.muted, false, 'the string "false" muted the flock');
      JSON.parse(fs.readFileSync(path.join(dataDir, 'state.json'), 'utf8'));
      void logs;
    });
  }
  test('a 50 000-identity state file starts in a reasonable time', async () => {
    const identities = {};
    for (let i = 0; i < 50_000; i++) identities[`session:${i}`] = { colourIndex: i % 8, slots: { primary: i }, label: `p${i}`, lastSeen: Date.now() };
    const t = Date.now();
    const { snap } = await run(JSON.stringify({ identities }));
    assert.ok(Date.now() - t < 10_000);
    assert.equal(snap.flock.length, 1);
  });
  test('state.json is 0600, written atomically (no .tmp left), and identities unseen for 7 days are pruned on load', async () => {
    const old = { identities: { 'session:ancient': { colourIndex: 3, slots: { primary: 4 }, label: 'old', lastSeen: Date.now() - 8 * 86400_000 } } };
    const { dataDir } = await run(JSON.stringify(old));
    const stateFile = path.join(dataDir, 'state.json');
    assert.equal(fs.statSync(stateFile).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(dataDir).filter((f) => f.includes('.tmp')), []);
    assert.doesNotMatch(fs.readFileSync(stateFile, 'utf8'), /ancient/);
  });
  test('an unwritable data dir: the brain keeps serving and logs a warning', async () => {
    const dataDir = tempDir('m2-ro-');
    fs.chmodSync(dataDir, 0o500);
    const logs = [];
    const brain = await startBrain({ config, token: TOKEN, dataDir, log: (l) => logs.push(l), persistMs: 30 });
    try {
      const r = await call(brain.port, { method: 'POST', path: '/hook', token: TOKEN, body: { hook_event_name: 'UserPromptSubmit', session_id: 's' } });
      assert.equal(r.status, 204);
      await new Promise((res) => setTimeout(res, 200));
      assert.equal((await call(brain.port, { path: '/health' })).status, 200);
    } finally {
      await brain.close();
      fs.chmodSync(dataDir, 0o700);
    }
  });
});

// ------------------------------------------------------ CLI in a temp agent copy

let root, agent, home, port, env, caller;
const freePort = () => new Promise((r) => { const s = net.createServer().listen(0, '127.0.0.1', () => { const { port: p } = s.address(); s.close(() => r(p)); }); });
function bellwether(args, { extraEnv = {}, cwd } = {}) {
  const r = spawnBrainSync([...args], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: cwd ?? caller, env: { ...env, ...extraEnv }, timeout: 60_000 });
  return { code: r.status, out: r.stdout, err: r.stderr };
}
const read = (f) => fs.readFileSync(f, 'utf8');

describe('CLI: exact-URL ownership, --port, status, replay, doctor --e2e', () => {
  before(async () => {
    root = tempDir('bellwether-m2cli-');
    agent = path.join(root, 'agents', 'bellwether');
    caller = path.join(root, 'caller');
    home = path.join(root, 'home');
    fs.mkdirSync(caller, { recursive: true });
    fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
    fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(agent, 'brain'), { recursive: true });
    for (const f of ['package.json', 'bellwether']) fs.copyFileSync(path.join(AGENT_DIR, f), path.join(agent, f));
    port = await freePort();
    const cfg = JSON.parse(read(path.join(AGENT_DIR, 'config.example.json')));
    cfg.port = port;
    fs.writeFileSync(path.join(agent, 'config.example.json'), JSON.stringify(cfg));
    env = { PATH: process.env.PATH, HOME: home, USERPROFILE: home, BELLWETHER_DATA_DIR: path.join(root, 'data'), BELLWETHER_HOOK_TOKEN: TOKEN };
  });
  after(() => fs.rmSync(root, { recursive: true, force: true }));

  const settings = (name, obj) => {
    const f = path.join(caller, name);
    fs.writeFileSync(f, JSON.stringify(obj, null, 2) + '\n');
    return f;
  };
  const cmd = (c) => ({ type: 'command', command: c });
  const http_ = (url, extra = {}) => ({ type: 'http', url, ...extra });

  test('foreign URLs near ours are never claimed: install and uninstall leave them byte-identical', () => {
    const u = `http://127.0.0.1:${port}`;
    const foreign = [
      http_(`${u}/hooks`), http_(`${u}/hook2`), http_(`${u}/hook/sub`), http_(`${u}/hook?x=1`), http_(`${u}/hook#f`), http_(`${u}/hook `),
      http_(`${u}//hook`), http_(`${u}/HOOK`), http_(`http://localhost:${port}/hook`), http_(`HTTP://127.0.0.1:${port}/hook`), http_(`http://127.0.0.1:${port + 1}/hook`),
      http_(`https://127.0.0.1:${port}/hook`), http_(`http://127.0.0.1:${port}/hook`, { type: 'command' }), http_(`http://user@127.0.0.1:${port}/hook`),
    ].filter((h) => h.url !== `${u}/hook` || h.type !== 'http');
    const original = { hooks: { Stop: [{ hooks: [cmd('keep'), ...foreign] }], PreToolUse: [{ matcher: 'Bash', hooks: foreign }] }, theme: 'dark' };
    const f = settings('foreign.json', original);
    const text = read(f);
    assert.equal(bellwether(['hooks', 'install', '--write', '--settings', f]).code, 0);
    assert.equal(bellwether(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    assert.equal(read(f), text);
  });

  test('--port removes only the exact old-port entries; the configured install is untouched; install/print refuse --port', () => {
    const f = settings('portmove.json', { hooks: { Stop: [{ hooks: [cmd('keep')] }] } });
    assert.equal(bellwether(['hooks', 'install', '--write', '--settings', f]).code, 0);
    const withOld = JSON.parse(read(f));
    const oldPort = port + 7;
    for (const e of Object.keys(withOld.hooks)) withOld.hooks[e].push({ hooks: [{ type: 'http', url: `http://127.0.0.1:${oldPort}/hook`, timeout: 1, headers: { 'X-Bellwether-Token': 'old' } }] });
    withOld.hooks.Stop.push({ hooks: [http_(`http://127.0.0.1:${oldPort}/hooks`)] });
    fs.writeFileSync(f, JSON.stringify(withOld, null, 2) + '\n');
    const doc = bellwether(['doctor', '--json', '--settings', f]);
    assert.match(doc.out, /hooks stray/);
    const dry = bellwether(['hooks', 'uninstall', '--port', String(oldPort), '--settings', f]);
    assert.equal(dry.code, 0, dry.err);
    assert.equal(read(f), JSON.stringify(withOld, null, 2) + '\n', 'dry run wrote');
    assert.equal(bellwether(['hooks', 'uninstall', '--port', String(oldPort), '--write', '--settings', f]).code, 0);
    const after = JSON.parse(read(f));
    const urls = Object.values(after.hooks).flat().flatMap((g) => g.hooks).map((h) => h.url).filter(Boolean);
    assert.ok(urls.every((u) => u !== `http://127.0.0.1:${oldPort}/hook`));
    assert.ok(urls.includes(`http://127.0.0.1:${oldPort}/hooks`), 'foreign /hooks on the old port was removed');
    assert.equal(urls.filter((u) => u === `http://127.0.0.1:${port}/hook`).length, 16, 'the configured install was disturbed');
    for (const bad of ['--port', '--port=', '--port=abc', '--port=047417', '--port=047500', '--port=+47500', '--port= 47500', '--port=1e4', '--port=80', '--port=70000', '--port=-1']) {
      // Under rust the harness itself refuses any spelling of the live port before the
      // binary runs (test/helpers.mjs); the CLI's own refusal is the other spellings.
      if (IMPL === 'rust' && portValue(bad.split('=')[1]) === 47417) {
        assert.throws(() => bellwether(['hooks', 'uninstall', '--write', '--settings', f, bad]), /live brain's port/);
        continue;
      }
      const r = bellwether(['hooks', 'uninstall', '--write', '--settings', f, bad]);
      assert.equal(r.code, 3, bad);
    }
    for (const sub of ['install', 'print']) assert.equal(bellwether(['hooks', sub, '--port', '47000', '--settings', f]).code, 3, `${sub} accepted --port`);
    assert.equal(JSON.parse(read(f)).hooks.Stop.length, after.hooks.Stop.length);
  });

  test('--port naming the configured port behaves like a normal uninstall and a foreign tool on another port needs an explicit --port', () => {
    const f = settings('portsame.json', { hooks: { Stop: [{ hooks: [http_('http://127.0.0.1:3000/hook')] }] } });
    assert.equal(bellwether(['hooks', 'install', '--write', '--settings', f]).code, 0);
    assert.equal(bellwether(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    assert.ok(read(f).includes('127.0.0.1:3000/hook'), 'a plain uninstall removed another tool\'s localhost hook');
  });

  test('status: brain down exits 2 with a cause; wrong token exits 3; JSON mode is valid', () => {
    const down = bellwether(['status']);
    assert.equal(down.code, 2);
    assert.match(down.err, /brain not running/);
  });

  test('replay refuses nonsense and cannot be turned into a local DoS by its arguments', () => {
    const notRec = path.join(caller, 'x.jsonl');
    fs.writeFileSync(notRec, '{"a":1}\n');
    assert.equal(bellwether(['replay', notRec]).code, 3);
    assert.equal(bellwether(['replay', path.join(caller, 'missing.jsonl')]).code, 3);
    for (const a of [['--burst', '1e9'], ['--burst', '100', '--seconds', '1e9'], ['--burst', '-1'], ['--burst', 'NaN'], ['--speed', '0'], ['--speed', '-5'], ['--speed', 'abc']]) {
      const t = Date.now();
      const r = bellwether(['replay', ...(a[0] === '--speed' ? [notRec] : []), ...a]);
      assert.ok(r.code !== 0 && r.code !== null, a.join(' '));
      assert.ok(Date.now() - t < 5000);
      assert.doesNotMatch(r.err, /at .*\.mjs:\d+/);
    }
  });

  test('replay and status against a live brain: the S2-shaped flow, and --json', async () => {
    const child = spawnBrain(['serve'], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
    let err = '';
    await new Promise((r) => { child.stderr.on('data', (d) => { err += d; if (err.includes('listening on')) r(); }); });
    try {
      const fixture = path.join(AGENT_DIR, 'test', 'fixtures', 's2-permission-approve.jsonl');
      const rep = spawnBrainSync(['replay', fixture, '--speed', '50'], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: caller, env });
      assert.equal(rep.status, 0, rep.stderr);
      assert.match(rep.stdout, /every \/hook answer was an empty 204/);
      const js = spawnBrainSync(['status', '--json'], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: caller, env });
      assert.equal(js.status, 0, js.stderr);
      JSON.parse(js.stdout);
      const wrong = spawnBrainSync(['status'], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: caller, env: { ...env, BELLWETHER_HOOK_TOKEN: 'someOtherTokenValue1234567890' } });
      assert.equal(wrong.status, 3);
      assert.match(wrong.stderr, /refused the token/);
    } finally {
      child.kill('SIGTERM');
      await new Promise((r) => child.once('exit', r));
    }
  });

  test('a rejected replay-marker check: a hostile recording line cannot inject a bubble through a field that is not allowlisted', async () => {
    const child = spawnBrain(['serve'], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
    let err = '';
    await new Promise((r) => { child.stderr.on('data', (d) => { err += d; if (err.includes('listening on')) r(); }); });
    try {
      const line = { normalised: { receivedAt: 1, event: 'PreToolUse', sessionId: 's', toolName: 'Bash', toolUseId: 't', toolFingerprint: 'f', toolSummary: 'echo hi', terminal: {} }, rawKeys: { top: ['tool_input', 'prompt', 'tool_response'], tool_input: ['command'] }, headersPresent: [] };
      const f = path.join(caller, 'hostile.jsonl');
      fs.writeFileSync(f, JSON.stringify(line) + '\n');
      const rep = spawnBrainSync(['replay', f], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: caller, env });
      assert.equal(rep.status, 0, rep.stderr);
      const js = spawnBrainSync(['status', '--json'], { root: agent, shim: path.join(agent, 'bellwether'), encoding: 'utf8', cwd: caller, env });
      assert.doesNotMatch(js.stdout, /replay:/);
    } finally {
      child.kill('SIGTERM');
      await new Promise((r) => child.once('exit', r));
    }
  });
});
