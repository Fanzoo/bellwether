import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { validateConfig } from '../brain/config.mjs';
import { PALETTE_UPGRADED, withDefaults } from '../brain/defaults.mjs';
import { fingerprint, normalise } from '../brain/normalise.mjs';
import { createRecorder } from '../brain/server.mjs';
import { AGENT_DIR, FIXTURES, readVectors, tempDir } from './helpers.mjs';

describe('package', () => {
  test('has no dependencies', () => {
    const pkg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'package.json'), 'utf8'));
    assert.equal(pkg.dependencies, undefined);
    assert.equal(pkg.devDependencies, undefined);
  });

  test('no brain module opens transcript_path', () => {
    for (const f of fs.readdirSync(path.join(AGENT_DIR, 'brain'))) {
      assert.doesNotMatch(fs.readFileSync(path.join(AGENT_DIR, 'brain', f), 'utf8'), /transcript_path/, f);
    }
  });
});

describe('config', () => {
  // The cases are shared with the Rust port: test/fixtures/config-vectors.json.
  const answer = (v) => {
    try {
      const out = v.step === 'validateConfig' ? validateConfig(structuredClone(v.config)) : withDefaults(structuredClone(v.config));
      return v.step === 'withDefaults' ? { ok: true, effective: JSON.parse(JSON.stringify(out)), paletteUpgraded: out[PALETTE_UPGRADED] === true } : { ok: true };
    } catch (e) {
      return { ok: false, exitCode: e.exitCode, error: e.message };
    }
  };
  const want = (v) => (v.ok ? (v.step === 'withDefaults' ? { ok: true, effective: v.effective, paletteUpgraded: v.paletteUpgraded } : { ok: true }) : { ok: false, exitCode: v.exitCode, error: v.error });

  test('the example config validates', () => {
    const example = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    validateConfig(example);
    const v = readVectors('config', 'ports and hosts').find((x) => x.name === 'the example config');
    assert.deepEqual(v.config, example, 'the vector holds today\'s config.example.json');
    // Defaults merged: every shared case answers as its vector says.
    for (const d of readVectors('config', 'defaults')) assert.deepEqual(answer(d), want(d), d.name);
  });

  test('a host key anywhere is rejected with exit 3', () => {
    for (const bad of [{ port: 47417, host: '0.0.0.0' }, { port: 47417, brain: { host: '0.0.0.0' } }]) {
      assert.throws(() => validateConfig(bad), (e) => e.exitCode === 3 && /not configurable/.test(e.message));
    }
    const hosts = readVectors('config', 'ports and hosts').filter((v) => /host/i.test(v.name));
    assert.ok(hosts.length >= 3 && hosts.every((v) => !v.ok && v.exitCode === 3 && /not configurable/.test(v.error)));
    for (const v of hosts) assert.deepEqual(answer(v), want(v), v.name);
  });

  test('a bad port is rejected', () => {
    for (const port of [undefined, '47417', 80, 70000, 1.5]) {
      assert.throws(() => validateConfig({ port }), (e) => e.exitCode === 3);
    }
    for (const v of readVectors('config', 'ports and hosts').filter((x) => !/host|example/i.test(x.name))) assert.deepEqual(answer(v), want(v), v.name);
  });
});

describe('normalise and record', () => {
  const SECRET = 'hunter2-do-not-keep';
  const payload = {
    hook_event_name: 'PreToolUse',
    session_id: 'sess-1',
    cwd: '/Users/example/projects/demo',
    transcript_path: '/Users/example/.claude/projects/x.jsonl',
    prompt: `please use ${SECRET}`,
    tool_name: 'Bash',
    tool_use_id: 'toolu_01',
    tool_input: { command: `echo ${SECRET}`, description: SECRET },
    tool_output: SECRET,
    last_assistant_message: SECRET,
    some_future_field: SECRET,
    source: `free text ${SECRET}`,
  };
  const headers = { 'x-bellwether-herdr-pane': 'w1:p2', 'x-bellwether-iterm-session': '', 'x-bellwether-token': 'tok', 'x-bellwether-term-program': 'iTerm.app' };

  test('the normalised event comes from the allowlist only', () => {
    const ev = normalise(payload, headers, 123);
    // Shared with the Rust port (test/fixtures/normalise-vectors.json): the same payload,
    // and the whole HookEvent.
    const [v] = readVectors('normalise', 'allowlist');
    assert.deepEqual(v.payload, payload);
    assert.deepEqual(normalise(v.payload, v.headers, v.receivedAt), v.expected);
    assert.deepEqual(Object.keys(ev).sort(), ['cwd', 'event', 'projectLabel', 'receivedAt', 'sessionId', 'terminal', 'toolFingerprint', 'toolName', 'toolUseId']);
    assert.equal(ev.projectLabel, 'demo');
    assert.deepEqual(ev.terminal, { program: 'iTerm.app', herdrPane: 'w1:p2' });
    assert.doesNotMatch(JSON.stringify(ev), new RegExp(SECRET));
  });

  test('an event name that is not an identifier is recorded as "unknown"', () => {
    assert.equal(normalise({ hook_event_name: 'Stop' }).event, 'Stop');
    assert.equal(normalise({ hook_event_name: `free text ${SECRET}` }).event, 'unknown');
    assert.equal(normalise({ hook_event_name: { x: 1 } }).event, 'unknown');
    for (const v of readVectors('normalise', 'event names')) assert.deepEqual(normalise(v.payload, v.headers, v.receivedAt), v.expected, v.name);
  });

  test('fingerprints ignore key order', () => {
    assert.equal(fingerprint('Bash', { a: 1, b: [1, { c: 2, d: 3 }] }), fingerprint('Bash', { b: [1, { d: 3, c: 2 }], a: 1 }));
    assert.notEqual(fingerprint('Bash', { command: 'ls' }), fingerprint('Bash', { command: 'ls -la' }));
  });

  test('a recording holds field names, never values', () => {
    const file = path.join(tempDir(), 'r.jsonl');
    const rec = createRecorder(file);
    rec.write(payload, headers, 123);
    rec.close();
    const text = fs.readFileSync(file, 'utf8');
    assert.doesNotMatch(text, new RegExp(SECRET));
    assert.doesNotMatch(text, /tok"/);
    const line = JSON.parse(text);
    assert.ok(line.rawKeys.top.includes('prompt') && line.rawKeys.top.includes('some_future_field'));
    assert.deepEqual(line.rawKeys.tool_input, ['command', 'description']);
    assert.deepEqual(line.headersPresent, ['x-bellwether-herdr-pane', 'x-bellwether-term-program']);
  });
});

describe('fixtures', () => {
  const files = fs.readdirSync(FIXTURES).filter((f) => /^s\d+-.*\.jsonl$/.test(f) && !f.includes('.expected.'));

  test('S1 to S11 exist', () => {
    for (let n = 1; n <= 11; n++) assert.ok(files.some((f) => f.startsWith(`s${n}-`)), `s${n}-*.jsonl`);
  });

  for (const f of files) {
    test(`${f} is in the recording format`, () => {
      const lines = fs.readFileSync(path.join(FIXTURES, f), 'utf8').trim().split('\n').map((l) => JSON.parse(l));
      assert.ok(lines.length > 0);
      let last = 0;
      for (const l of lines) {
        assert.deepEqual(Object.keys(l).sort(), ['headersPresent', 'normalised', 'rawKeys']);
        assert.equal(typeof l.normalised.event, 'string');
        assert.ok(l.normalised.receivedAt >= last, 'time never goes backwards');
        last = l.normalised.receivedAt;
        assert.ok(Array.isArray(l.rawKeys.top));
      }
    });
  }
});

describe('record --out guard', () => {
  test('guardOutFile: empty file and bellwether recordings may be appended to; anything else is refused', async () => {
    const { guardOutFile, isBellwetherRecording } = await import('../brain/outfile.mjs');
    const dir = tempDir();
    const rec = path.join(dir, 'r.jsonl');
    const r = createRecorder(rec);
    r.write({ hook_event_name: 'Stop', session_id: 's' }, {}, 1);
    r.close();
    assert.ok(isBellwetherRecording(fs.readFileSync(rec, 'utf8')));
    guardOutFile(rec, 'record');
    const empty = path.join(dir, 'empty.jsonl');
    fs.writeFileSync(empty, '');
    guardOutFile(empty, 'record');
    const other = path.join(dir, 'notes.txt');
    fs.writeFileSync(other, 'notes');
    assert.throws(() => guardOutFile(other, 'record'), (e) => e.exitCode === 3 && /not a bellwether recording/.test(e.message));
    assert.throws(() => guardOutFile(dir, 'record'), (e) => e.exitCode === 3 && /is a directory/.test(e.message));
    assert.throws(() => guardOutFile(path.join(dir, '.claude', 'settings.json'), 'record'), (e) => e.exitCode === 3);
  });
});
