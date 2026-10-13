import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { withDefaults } from '../brain/defaults.mjs';
import { Flock } from '../brain/flock.mjs';
import { identityOf } from '../brain/identity.mjs';
import { normalise } from '../brain/normalise.mjs';
import { makeTextCleaner } from '../brain/redact.mjs';
import { readJsonl, runFixture } from './fixture-runner.mjs';
import { AGENT_DIR, FIXTURES, readVectors } from './helpers.mjs';

/**
 * The reducer, with a fake clock throughout: every event carries its own time and
 * tick(t) is called explicitly, so nothing here depends on wall-clock time.
 */

const CONFIG = withDefaults({ port: 47417 });
const HERDR = { herdrPane: 'w1:p1', herdrSocket: '/x/herdr.sock', itermSession: 'w0t1p0:SAME', program: 'iTerm.app' };
const scenarios = fs.readdirSync(FIXTURES).filter((f) => /^s\d+-.*\.jsonl$/.test(f) && !f.includes('.expected.'));

describe('fixtures reproduce their expected transitions exactly', () => {
  for (const f of scenarios) {
    test(f, () => {
      const expectedFile = path.join(FIXTURES, f.replace(/\.jsonl$/, '.expected.jsonl'));
      const expected = fs.readFileSync(expectedFile, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
      assert.deepEqual(runFixture(readJsonl(path.join(FIXTURES, f))).transitions, expected);
    });
  }
});

describe('every row of the correlation and clearing table is exercised by a fixture', () => {
  const tr = (f) => runFixture(readJsonl(path.join(FIXTURES, f))).transitions;
  const alertAt = (ts, actorPrefix) => ts.filter((t) => t.actor.startsWith(actorPrefix) && t.alert);
  const rows = [
    ['PermissionRequest raises on the PreToolUse actor (sheep)', 's2-permission-approve.jsonl', (ts) => alertAt(ts, 'sheep:').some((t) => t.alert === 'permission: Bash: touch meadow-approved.txt?')],
    ['PermissionRequest raises on the lamb (S5)', 's5-subagent-permission.jsonl', (ts) => alertAt(ts, 'lamb:general-purpose').length > 0 && alertAt(ts, 'sheep:').length === 0],
    ['Notification permission_prompt confirms', 's2-permission-approve.jsonl', (ts) => ts.some((t) => t.alert?.endsWith('(confirmed)'))],
    ['Notification permission_prompt with no request raises on the sheep', 's15-sandbox-notification.jsonl', (ts) => ts.some((t) => t.alert === 'permission: Claude wants to reach status.example.com (confirmed)')],
    ['Elicitation raises; elicitation_dialog confirms; ElicitationResult clears', 's12-elicitation.jsonl', (ts) => ts.some((t) => t.alert?.startsWith('elicitation: deploy:')) && ts.at(-1).alert === undefined],
    ['agent_needs_input raises an input alert', 's16-agent-needs-input.jsonl', (ts) => ts.some((t) => t.alert?.startsWith('input:'))],
    ['PostToolUse with the alert\'s tool_use_id clears', 's2-permission-approve.jsonl', (ts) => ts.find((t) => t.atMs > 10000).alert === undefined],
    ['PermissionDenied with the tool_use_id clears', 's13-auto-mode-denied.jsonl', (ts) => ts.find((t) => t.atMs === 2500)?.alert === undefined],
    ['UserPromptSubmit clears the sheep (manual deny)', 's3-permission-deny.jsonl', (ts) => ts.at(-1).alert === undefined],
    ['StopFailure clears the sheep and shows the error', 's14-stop-failure.jsonl', (ts) => ts.some((t) => t.state === 'error' && t.flavour === 'rate_limit' && !t.alert)],
    ['Stop does not clear a lamb (Decision 14)', 's17-lamb-alert-outlives-main-stop.jsonl', (ts) => ts.some((t) => t.actor.startsWith('lamb:') && t.atMs > 3000 && t.alert)],
    ['SessionEnd clears lambs too', 's18-session-end-clears-lamb.jsonl', (ts) => ts.some((t) => t.actor.startsWith('lamb:') && t.atMs === 9000 && t.state === 'leaving' && !t.alert)],
    ['SubagentStop clears the lamb, which leaves', 's5-subagent-permission.jsonl', (ts) => ts.some((t) => t.actor.startsWith('lamb:') && t.state === 'leaving')],
    ['A later PreToolUse from the same actor (> 2 s) clears; one within 2 s does not', 's19-progress-clears.jsonl', (ts) => ts.find((t) => t.atMs === 1500).alert && ts.find((t) => t.atMs === 6000).alert === undefined],
    ['Manual deny: nothing clears until the next prompt', 's3-permission-deny.jsonl', (ts) => ts.filter((t) => t.atMs > 7000 && t.atMs < 26000).length === 0],
    ['Two identical calls: each prompt alerts once and clears', 's20-duplicate-fingerprints.jsonl', (ts) => ts.filter((t) => t.alert && !ts[ts.indexOf(t) - 1]?.alert).length === 2 && !ts.at(-1).alert],
    ['The helper agent (agent_type "") never becomes a lamb', 's17-lamb-alert-outlives-main-stop.jsonl', (ts) => !ts.some((t) => t.actor.includes('a0000000000000012'))],
    ['/clear keeps the same sheep', 's8-clear.jsonl', (ts) => new Set(ts.map((t) => t.actor)).size === 1 && !ts.some((t) => t.state === 'leaving')],
  ];
  for (const [row, f, check] of rows) test(`${row} (${f})`, () => assert.ok(check(tr(f))));

  test('S6: no alert flapping (raised once, cleared once)', () => {
    const ts = tr('s6-parallel.jsonl');
    const changes = ts.filter((t, i) => i > 0 && Boolean(t.alert) !== Boolean(ts[i - 1].alert));
    assert.equal(changes.length, 2, JSON.stringify(ts));
  });
});

describe('snapshots carry only short, redacted text', () => {
  test('over every fixture, no snapshot string is longer than bubble.maxChars plus the label', () => {
    const max = CONFIG.bubble.maxChars;
    for (const f of scenarios) {
      for (const snap of runFixture(readJsonl(path.join(FIXTURES, f))).snapshots) {
        for (const s of snap.flock) {
          const limit = max + s.label.length;
          walk(s, (key, v) => {
            if (['identityKey', 'herdrSocket', 'itermSession', 'termSession'].includes(key)) return;
            assert.ok(v.length <= limit, `${f}: ${key} is ${v.length} chars: ${v}`);
          });
        }
      }
    }
  });

  test('a secret in a Bash command never reaches the snapshot; long commands are truncated', () => {
    const clean = makeTextCleaner({ maxChars: 60, redactPatterns: ['(?i)hunter\\d'] });
    const flock = new Flock({ config: CONFIG });
    const base = { session_id: 's', cwd: '/p/proj', tool_name: 'Bash' };
    const input = { command: `curl -H "Authorization: Bearer abc.def.ghi" https://u:pw@h.example/x?token=SEKRET1 && echo hunter2 ${'x'.repeat(200)}` };
    flock.apply(normalise({ ...base, hook_event_name: 'PreToolUse', tool_use_id: 't1', tool_input: input }, { 'x-bellwether-herdr-pane': 'w1:p1' }, 0, { clean }));
    flock.apply(normalise({ ...base, hook_event_name: 'PermissionRequest', tool_input: input }, { 'x-bellwether-herdr-pane': 'w1:p1' }, 10, { clean }));
    const text = JSON.stringify(flock.snapshot());
    for (const secret of ['abc.def.ghi', 'SEKRET1', 'hunter2', 'u:pw']) assert.doesNotMatch(text, new RegExp(secret.replace('.', '\\.')));
    const alert = flock.snapshot().flock[0].alert.text;
    assert.ok([...alert].length <= 60, alert);
    assert.match(alert, /^Bash: curl -H "Authorization: Bearer •••"/);
    const [user] = readVectors('redact', 'user patterns');
    assert.equal(makeTextCleaner({ maxChars: user.maxChars, redactPatterns: user.redactPatterns })(user.text), user.expected, user.name);
  });

  test('bubble.detail "tool" shows the tool name only', () => {
    const flock = new Flock({ config: withDefaults({ port: 47417, bubble: { detail: 'tool' } }) });
    flock.apply({ event: 'PermissionRequest', sessionId: 's', receivedAt: 0, toolName: 'Bash', toolSummary: 'git push origin main', toolFingerprint: 'f', terminal: HERDR });
    assert.equal(flock.snapshot().flock[0].alert.text, 'Bash?');
  });
});

describe('identity', () => {
  const id = (terminal, sessionId = 's1') => identityOf({ sessionId, terminal }).key;

  test('innermost wins: herdr pane over the iTerm2 tab every herdr pane shares', () => {
    assert.equal(id(HERDR), 'herdr:/x/herdr.sock#w1:p1');
    assert.notEqual(id({ ...HERDR, herdrPane: 'w1:p2' }), id(HERDR));
  });
  test('tmux pane next, then the iTerm2 tab (uuid only)', () => {
    assert.equal(id({ tmuxPane: '%3', itermSession: 'w0t0p0:U' }), 'tmux:%3');
    assert.equal(id({ itermSession: 'w0t2p0:5E551011-1', termSession: 'w0t2p0:5E551011-1', program: 'iTerm.app' }), 'iterm:5E551011-1');
    assert.equal(id({ itermSession: 'w1t5p0:5E551011-1' }), 'iterm:5E551011-1', 'moving the tab keeps the sheep');
  });
  test("Terminal.app's TERM_SESSION_ID counts only with TERM_PROGRAM=Apple_Terminal", () => {
    assert.equal(id({ program: 'Apple_Terminal', termSession: '7E2A1A22-1' }), 'term:7E2A1A22-1');
    assert.equal(id({ program: 'vscode', termSession: '7E2A1A22-1' }), 'session:s1');
  });
  test('Windows Terminal, then the session', () => {
    assert.equal(id({ wtSession: 'abc' }), 'wt:abc');
    assert.equal(id({}), 'session:s1');
  });

  test('/clear: a terminal-keyed sheep keeps its colour and slot under the new session_id', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'old', receivedAt: 0, terminal: HERDR, projectLabel: 'p' });
    const before = f.snapshot().flock[0];
    f.apply({ event: 'SessionEnd', sessionId: 'old', reason: 'clear', receivedAt: 5000, terminal: HERDR });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'new', receivedAt: 6000, terminal: HERDR });
    const after = f.snapshot().flock;
    assert.equal(after.length, 1);
    assert.equal(after[0].id, before.id);
    assert.equal(after[0].colourIndex, before.colourIndex);
  });

  test('/clear without terminal headers: the old sheep leaves and a new one arrives', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'old', receivedAt: 0, terminal: {} });
    f.apply({ event: 'SessionEnd', sessionId: 'old', reason: 'clear', receivedAt: 5000, terminal: {} });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'new', receivedAt: 5100, terminal: {} });
    const states = f.snapshot().flock.map((s) => `${s.identityKey}=${s.state}`).sort();
    assert.deepEqual(states, ['session:new=arriving', 'session:old=leaving']);
    f.tick(7000);
    assert.deepEqual(f.snapshot().flock.map((s) => s.identityKey), ['session:new']);
  });

  test('a new session in the same pane takes the sheep over, dropping the old alerts and lambs', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'SubagentStart', sessionId: 'a', agentId: 'x', agentType: 'Explore', receivedAt: 0, terminal: HERDR });
    f.apply({ event: 'PermissionRequest', sessionId: 'a', receivedAt: 10, toolName: 'Bash', toolFingerprint: 'f', terminal: HERDR });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'b', receivedAt: 3000, terminal: HERDR });
    const s = f.snapshot().flock[0];
    assert.equal(s.alert, undefined);
    assert.equal(s.lambs.length, 0);
  });

  test('colour and slot persist across a brain restart', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'a', receivedAt: 0, terminal: { herdrPane: 'p1' } });
    f.apply({ event: 'UserPromptSubmit', sessionId: 'b', receivedAt: 0, terminal: { herdrPane: 'p2' } });
    const saved = JSON.parse(JSON.stringify(f.persistable()));
    const g = new Flock({ config: CONFIG, persisted: saved, t: 1000 });
    g.apply({ event: 'UserPromptSubmit', sessionId: 'c', receivedAt: 1000, terminal: { herdrPane: 'p2' } });
    const s = g.snapshot().flock[0];
    const before = f.snapshot().flock.find((x) => x.identityKey === s.identityKey);
    assert.equal(s.colourIndex, before.colourIndex);
    assert.equal(s.slot, before.slot);
  });

  test('identities unseen for 7 days are pruned on load', () => {
    const g = new Flock({ config: CONFIG, persisted: { identities: { 'herdr:#old': { colourIndex: 3, slots: { primary: 2 }, lastSeen: 0 } } }, t: 8 * 24 * 3600_000 });
    assert.equal(g.persistable().identities['herdr:#old'], undefined);
  });
});

describe('lambs', () => {
  test('the helper agent: SubagentStop with agent_type "" and no SubagentStart creates nothing', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'Stop', sessionId: 's', receivedAt: 0, terminal: HERDR });
    f.apply({ event: 'SubagentStop', sessionId: 's', agentId: 'helper', receivedAt: 900, terminal: HERDR });
    f.apply({ event: 'SubagentStop', sessionId: 's', agentId: 'helper2', agentType: 'Explore', receivedAt: 950, terminal: HERDR });
    assert.equal(f.snapshot().flock[0].lambs.length, 0);
  });

  test('tool events from an agent with no agent_type create no lamb either', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'PreToolUse', sessionId: 's', agentId: 'helper', receivedAt: 0, toolName: 'Read', toolUseId: 't', terminal: HERDR });
    assert.equal(f.snapshot().flock[0].lambs.length, 0);
  });

  test('a lamb first seen mid-flight appears on its first event', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'PreToolUse', sessionId: 's', agentId: 'a1', agentType: 'Explore', receivedAt: 0, toolName: 'Read', toolUseId: 't', terminal: HERDR });
    assert.deepEqual(f.snapshot().flock[0].lambs.map((l) => l.label), ['Explore']);
  });

  test('more than maxLambsShown: alerting lambs first, the rest counted', () => {
    const f = new Flock({ config: CONFIG });
    for (let i = 0; i < 7; i++) f.apply({ event: 'SubagentStart', sessionId: 's', agentId: `a${i}`, agentType: `T${i}`, receivedAt: i, terminal: HERDR });
    f.apply({ event: 'PermissionRequest', sessionId: 's', agentId: 'a6', receivedAt: 10, toolName: 'Bash', toolFingerprint: 'f', terminal: HERDR });
    const s = f.snapshot().flock[0];
    assert.equal(s.lambs.length, 5);
    assert.equal(s.hiddenLambCount, 2);
    assert.equal(s.lambs[0].label, 'T6');
    assert.equal(s.lambAlert, true);
  });
});

describe('Decision 14: turn-level events clear only the sheep', () => {
  for (const ev of ['UserPromptSubmit', 'Stop', 'StopFailure']) {
    test(`${ev} clears the sheep's alert and leaves the lamb's`, () => {
      const f = new Flock({ config: CONFIG });
      f.apply({ event: 'SubagentStart', sessionId: 's', agentId: 'a', agentType: 'Explore', receivedAt: 0, terminal: HERDR });
      f.apply({ event: 'PreToolUse', sessionId: 's', agentId: 'a', agentType: 'Explore', receivedAt: 10, toolName: 'Bash', toolUseId: 'tl', toolFingerprint: 'fl', terminal: HERDR });
      f.apply({ event: 'PermissionRequest', sessionId: 's', agentId: 'a', agentType: 'Explore', receivedAt: 20, toolName: 'Bash', toolFingerprint: 'fl', terminal: HERDR });
      f.apply({ event: 'Notification', sessionId: 's', notificationType: 'agent_needs_input', receivedAt: 30, terminal: HERDR });
      f.apply({ event: ev, sessionId: 's', receivedAt: 40, errorType: 'overloaded', terminal: HERDR });
      const s = f.snapshot().flock[0];
      assert.equal(s.alert, undefined);
      assert.equal(s.lambs[0].alert.kind, 'permission');
    });
  }
  test('SessionEnd clears everything', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'SubagentStart', sessionId: 's', agentId: 'a', agentType: 'Explore', receivedAt: 0, terminal: HERDR });
    f.apply({ event: 'PermissionRequest', sessionId: 's', agentId: 'a', receivedAt: 20, toolName: 'Bash', toolFingerprint: 'fl', terminal: HERDR });
    f.apply({ event: 'SessionEnd', sessionId: 's', reason: 'other', receivedAt: 40, terminal: HERDR });
    const s = f.snapshot().flock[0];
    assert.equal(s.state, 'leaving');
    assert.equal(s.lambs[0].alert, undefined);
  });
});

describe('smoothing, hysteresis and reaping', () => {
  test('calm states change no faster than minDwellMs, while alerts appear at once', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'UserPromptSubmit', sessionId: 's', receivedAt: 0, terminal: HERDR });
    f.tick(1500);
    for (let t = 1600; t < 3000; t += 100) {
      f.apply({ event: t % 200 ? 'PreToolUse' : 'PostToolUse', sessionId: 's', receivedAt: t, toolName: t % 400 ? 'Read' : 'Bash', toolUseId: `t${Math.floor(t / 200)}`, terminal: HERDR });
    }
    const changes = [];
    let last = null;
    for (let t = 1500; t < 3000; t += 50) {
      f.tick(t);
      const s = f.snapshot().flock[0];
      if (last && (s.state !== last.state || s.flavour !== last.flavour)) changes.push(t);
      last = s;
    }
    for (let i = 1; i < changes.length; i++) assert.ok(changes[i] - changes[i - 1] >= CONFIG.behaviour.minDwellMs, String(changes));
    f.apply({ event: 'PermissionRequest', sessionId: 's', receivedAt: 3001, toolName: 'Bash', toolFingerprint: 'zz', terminal: HERDR });
    assert.equal(f.snapshot().flock[0].alert.kind, 'permission');
  });

  test('an interrupted turn (no Stop) stops looking busy after thinkingTimeoutSec, then sleeps', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'UserPromptSubmit', sessionId: 's', receivedAt: 0, terminal: HERDR });
    f.tick(60_000);
    assert.equal(f.snapshot().flock[0].state, 'busy');
    f.tick(130_000);
    assert.equal(f.snapshot().flock[0].state, 'idle');
    f.tick(11 * 60_000);
    assert.equal(f.snapshot().flock[0].state, 'sleeping');
  });

  test('a silent sheep is reaped after reapAfterHours', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'Stop', sessionId: 's', receivedAt: 0, terminal: HERDR });
    f.tick(11 * 3600_000);
    assert.equal(f.snapshot().flock.length, 1);
    f.tick(12 * 3600_000 + 1);
    assert.equal(f.snapshot().flock.length, 0);
  });

  test('no timeout on alerts: a prompt still bounces hours later', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'PermissionRequest', sessionId: 's', receivedAt: 0, toolName: 'Bash', toolFingerprint: 'f', terminal: HERDR });
    f.tick(5 * 3600_000);
    assert.equal(f.snapshot().flock[0].alert.kind, 'permission');
  });
});

describe('control', () => {
  test('mute, dismiss and placed', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'PermissionRequest', sessionId: 's', receivedAt: 0, toolName: 'Bash', toolFingerprint: 'f', terminal: HERDR });
    const s = f.snapshot().flock[0];
    assert.deepEqual(f.control({ action: 'mute' }, 1), { ok: true });
    assert.equal(f.snapshot().muted, true);
    assert.deepEqual(f.control({ action: 'dismiss', actorId: s.id }, 2), { ok: true });
    assert.equal(f.snapshot().flock[0].alert, undefined);
    assert.equal(f.control({ action: 'dismiss', actorId: 'nope' }, 2).ok, false);
    assert.deepEqual(f.control({ action: 'placed', identityKey: s.identityKey, monitor: 'DELL U2720Q', slot: 4 }, 3), { ok: true });
    assert.equal(f.persistable().identities[s.identityKey].slots['DELL U2720Q'], 4);
    assert.equal(f.control({ action: 'placed', identityKey: s.identityKey, monitor: 'x', slot: -1 }, 3).ok, false);
    assert.equal(f.control({ action: 'explode' }, 3).ok, false);
  });
});

describe('content never kept', () => {
  test('no brain module reads prompt, tool_output, tool_response, last_assistant_message, compact_summary, agent_transcript_path or transcript_path', () => {
    const names = ['prompt', 'tool_output', 'tool_response', 'last_assistant_message', 'compact_summary', 'agent_transcript_path', 'transcript_path', 'custom_instructions', 'requested_schema', 'form_values', 'error_details'];
    for (const f of fs.readdirSync(path.join(AGENT_DIR, 'brain')).filter((f) => f.endsWith('.mjs'))) {
      const code = fs.readFileSync(path.join(AGENT_DIR, 'brain', f), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
      for (const n of names) {
        assert.doesNotMatch(code, new RegExp(`(\\.${n}\\b|\\[\\s*['"\`]${n}['"\`]\\s*\\]|['"\`]${n}['"\`])`), `${f} names ${n}`);
      }
    }
  });
});

function walk(o, fn, key = '') {
  if (typeof o === 'string') fn(key, o);
  else if (Array.isArray(o)) o.forEach((x) => walk(x, fn, key));
  else if (o && typeof o === 'object') for (const [k, v] of Object.entries(o)) walk(v, fn, k);
}

describe('M2 verification fixes', () => {
  test('redaction keeps the key, flag or scheme readable and leaves ordinary commands alone', () => {
    // Shared with the Rust port: test/fixtures/redact-vectors.json.
    const clean = makeTextCleaner({ maxChars: 200 });
    const cases = readVectors('redact', 'readable key');
    assert.ok(cases.length >= 10);
    for (const v of cases) {
      assert.equal(clean(v.text), v.mustEqual, v.text);
      assert.equal(v.expected, v.mustEqual, `${v.text}: the vector's answer`);
    }
  });

  test('bidi controls are stripped', () => {
    const clean = makeTextCleaner({ maxChars: 60 });
    const cases = readVectors('redact', 'bidi controls');
    assert.equal(cases.length, 11);
    for (const v of cases) {
      assert.ok(!clean(v.text).includes(v.mustNotContain), v.name);
      assert.equal(clean(v.text), v.expected, v.name);
    }
  });

  test('a tampered state.json is repaired field by field', () => {
    const now = 1_000_000;
    const f = new Flock({
      config: CONFIG,
      t: now,
      persisted: {
        muted: 'false',
        identities: {
          'herdr:#a': { colourIndex: -1, slots: { primary: -5, ok: 3, bad: 'x' }, label: { x: 1 }, lastSeen: now },
          'herdr:#b': { colourIndex: 99, slots: 'abc', label: 'L'.repeat(1000), lastSeen: 'yesterday' },
          'herdr:#c': 5,
        },
      },
    });
    assert.equal(f.snapshot().muted, false);
    const ids = f.persistable().identities;
    assert.deepEqual(Object.keys(ids).sort(), ['herdr:#a', 'herdr:#b']);
    assert.deepEqual(ids['herdr:#a'], { colourIndex: undefined, slots: { ok: 3 }, label: undefined, lastSeen: now });
    assert.equal(ids['herdr:#b'].colourIndex, undefined);
    assert.equal([...ids['herdr:#b'].label].length, 200, 'an over-long label is cleaned and truncated');
    assert.ok(ids['herdr:#b'].label.endsWith('…'));
    f.apply({ event: 'Stop', sessionId: 's', receivedAt: now, terminal: { herdrPane: 'a' } });
    const s = f.snapshot().flock[0];
    assert.ok(s.colourIndex >= 0 && s.slot >= 0);
  });

  test('placed: at most 16 monitors per sheep; an over-long name or slot is refused', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'Stop', sessionId: 's', receivedAt: 0, terminal: HERDR });
    const key = f.snapshot().flock[0].identityKey;
    for (let i = 0; i < 15; i++) assert.ok(f.control({ action: 'placed', identityKey: key, monitor: `m${i}`, slot: i }, 1).ok);
    assert.equal(f.control({ action: 'placed', identityKey: key, monitor: 'one-too-many', slot: 1 }, 1).ok, false);
    assert.ok(f.control({ action: 'placed', identityKey: key, monitor: 'm3', slot: 9 }, 1).ok, 'an existing monitor can still move');
    assert.equal(f.control({ action: 'placed', identityKey: key, monitor: 'x'.repeat(5000), slot: 1 }, 1).ok, false);
    assert.equal(f.control({ action: 'placed', identityKey: key, monitor: 'm1', slot: 1e9 }, 1).ok, false);
  });

  test('identities are capped: the least recently seen absent ones go first', async () => {
    const { LIMITS } = await import('../brain/flock.mjs');
    const f = new Flock({ config: CONFIG });
    // Each sheep arrives, exits and is gone before the next, so none is present.
    for (let i = 0; i <= LIMITS.identities + 5; i++) {
      const t = i * 3000;
      f.apply({ event: 'Stop', sessionId: `s${i}`, receivedAt: t, terminal: { herdrPane: `p${i}` } });
      f.apply({ event: 'SessionEnd', sessionId: `s${i}`, reason: 'other', receivedAt: t + 1, terminal: { herdrPane: `p${i}` } });
      f.tick(t + 2000);
    }
    const ids = Object.keys(f.persistable().identities);
    assert.equal(ids.length, LIMITS.identities);
    assert.ok(!ids.includes('herdr:#p0') && ids.includes(`herdr:#p${LIMITS.identities + 5}`));
  });

  test('a lamb without SubagentStop leaves after lambTimeoutMin, unless it holds an alert', () => {
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'SubagentStart', sessionId: 's', agentId: 'quiet', agentType: 'Explore', receivedAt: 0, terminal: HERDR });
    f.apply({ event: 'PermissionRequest', sessionId: 's', agentId: 'asking', agentType: 'Explore', receivedAt: 0, toolName: 'Bash', toolFingerprint: 'f', terminal: HERDR });
    f.tick(29 * 60_000);
    assert.equal(f.snapshot().flock[0].lambs.length, 2);
    f.tick(31 * 60_000);
    f.tick(31 * 60_000 + 2000);
    assert.deepEqual(f.snapshot().flock[0].lambs.map((l) => l.id.replace(/^l-[^-]+-/, '')), ['asking']);
  });

  test('an in-flight tool whose PostToolUse never came stops counting after toolTimeoutMin', () => {
    const f = new Flock({ config: withDefaults({ port: 47417, behaviour: { thinkingTimeoutSec: 60 } }) });
    f.apply({ event: 'PreToolUse', sessionId: 's', receivedAt: 0, toolName: 'Bash', toolUseId: 't', terminal: HERDR });
    f.tick(20 * 60_000);
    assert.equal(f.snapshot().flock[0].state, 'busy');
    f.tick(31 * 60_000);
    f.tick(31 * 60_000 + 2000);
    assert.notEqual(f.snapshot().flock[0].state, 'busy');
  });

  test('at most 64 lambs per sheep; the oldest without an alert makes room', () => {
    const f = new Flock({ config: CONFIG });
    for (let i = 0; i < 70; i++) f.apply({ event: 'SubagentStart', sessionId: 's', agentId: `a${i}`, agentType: 'X', receivedAt: i, terminal: HERDR });
    const s = f.snapshot().flock[0];
    assert.equal(s.lambs.length + s.hiddenLambCount, 64);
  });
});

describe('M2 re-check fixes', () => {
  test('labels from the folder name and from state.json lose control, bidi and zero-width characters', () => {
    const [v] = readVectors('normalise', 'labels');
    const ev = normalise(v.payload, v.headers, v.receivedAt);
    assert.equal(ev.projectLabel, 'ab cd', JSON.stringify(ev.projectLabel));
    assert.deepEqual(ev, v.expected);
    const [t] = readVectors('normalise', 'terminals');
    assert.deepEqual(normalise(t.payload, t.headers, t.receivedAt), t.expected, t.name);
    const f = new Flock({ config: CONFIG, t: 0, persisted: { identities: { 'herdr:#w1:p1': { label: 'x⁦y\nz', slots: {}, lastSeen: 0 } } } });
    assert.equal(f.persistable().identities['herdr:#w1:p1'].label, 'xy z');
  });

  test('at most 200 sheep at once; the least recently seen one without an alert makes room', async () => {
    const { LIMITS } = await import('../brain/flock.mjs');
    const f = new Flock({ config: CONFIG });
    f.apply({ event: 'PermissionRequest', sessionId: 'alerting', receivedAt: 0, toolName: 'Bash', toolFingerprint: 'f', terminal: {} });
    for (let i = 0; i < LIMITS.sheep + 50; i++) f.apply({ event: 'UserPromptSubmit', sessionId: `x${i}`, receivedAt: 1 + i, terminal: {} });
    const flock = f.snapshot().flock;
    assert.equal(flock.length, LIMITS.sheep);
    assert.ok(flock.some((s) => s.identityKey === 'session:alerting'), 'the alerting sheep is never the one evicted');
  });

  test('bubble.detail "tool" holds end to end: normaliser, flock and snapshot show only the tool name', () => {
    const config = withDefaults({ port: 47417, bubble: { detail: 'tool' } });
    const clean = makeTextCleaner(config.bubble);
    const f = new Flock({ config });
    const headers = { 'x-bellwether-herdr-pane': 'w1:p1' };
    const input = { command: 'git push origin main', description: 'Push' };
    f.apply(normalise({ hook_event_name: 'PreToolUse', session_id: 's', cwd: '/p/x', tool_name: 'Bash', tool_use_id: 't', tool_input: input }, headers, 1, { clean }));
    f.apply(normalise({ hook_event_name: 'PermissionRequest', session_id: 's', cwd: '/p/x', tool_name: 'Bash', tool_input: input }, headers, 2, { clean }));
    const snap = JSON.stringify(f.snapshot());
    assert.equal(f.snapshot().flock[0].alert.text, 'Bash?');
    assert.doesNotMatch(snap, /git push/);
  });
});

describe('S21: six parallel subagents (Milestone 6)', () => {
  test('five lambs show and "+1"; the lamb itself alerts and is confirmed; the helper never becomes a lamb; all leave', () => {
    const lines = fs.readFileSync(path.join(FIXTURES, 's21-six-lambs.jsonl'), 'utf8').trim().split('\n').map((l) => JSON.parse(l).normalised);
    const f = new Flock({ config: withDefaults({}), persisted: null, t: lines[0].receivedAt });
    const at = {};
    for (const ev of lines) {
      f.apply(ev);
      const s = f.snapshot().flock[0];
      at[`${ev.event}@${ev.receivedAt - lines[0].receivedAt}`] = s;
    }
    const raised = at['PermissionRequest@3010'];
    assert.equal(raised.lambs.length, 5);
    assert.equal(raised.hiddenLambCount, 1);
    assert.equal(raised.alert, undefined, 'the sheep itself does not alert');
    assert.equal(raised.lambAlert, true);
    const plan = raised.lambs.find((l) => l.alert);
    assert.equal(plan.label, 'Plan');
    assert.equal(plan.alert.text, 'Bash: ls?');
    assert.ok(!raised.lambs.some((l) => l.id.endsWith('-a0000000000000039')));
    assert.equal(at['Notification@9010'].lambs.find((l) => l.alert).alert.confirmed, true);
    assert.equal(at['PostToolUse@13000'].lambAlert, undefined);
    assert.equal(at['PostToolUse@13000'].hiddenLambCount, 1, 'leaving lambs still count while they trot back');
    assert.equal(at['SubagentStop@14000'].hiddenLambCount, 0, 'the sixth shows once two have gone');
    f.tick(lines.at(-1).receivedAt + 5000);
    assert.equal(f.snapshot().flock[0].lambs.length, 0);
  });
});
