import assert from 'node:assert/strict';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, TOKEN, notUnderRust, spawnBrain, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Tester re-verification of Milestone 1 after the developer's fixes (D1-D7, M1-6..17).
 * Runs the real shim from a temp copy laid out as <root>/agents/bellwether, with HOME,
 * BELLWETHER_DATA_DIR and the port all isolated. Nothing can reach the real settings file.
 */

let root, agent, home, port, env, defaultSettings, caller;

const freePort = () =>
  new Promise((r) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const { port: p } = s.address();
      s.close(() => r(p));
    });
  });

function bellwether(args, { cwd = caller, extraEnv = {}, bin = path.join(agent, 'bellwether'), pathEnv } = {}) {
  const r = spawnBrainSync([...args], { root: agent, shim: bin, encoding: 'utf8', cwd, env: { ...env, ...(pathEnv ? { PATH: pathEnv } : {}), ...extraEnv }, timeout: 30000 });
  return { code: r.status, out: r.stdout, err: r.stderr };
}
const read = (f) => fs.readFileSync(f, 'utf8');
const dataFile = (n) => path.join(root, 'data', n);

before(async () => {
  root = tempDir('bellwether-reverify-');
  agent = path.join(root, 'agents', 'bellwether');
  caller = path.join(root, 'caller dir');
  home = path.join(root, 'home');
  fs.mkdirSync(caller, { recursive: true });
  fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
  fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(agent, 'brain'), { recursive: true });
  for (const f of ['package.json', 'bellwether']) fs.copyFileSync(path.join(AGENT_DIR, f), path.join(agent, f));
  port = await freePort();
  const cfg = JSON.parse(read(path.join(AGENT_DIR, 'config.example.json')));
  cfg.port = port;
  fs.writeFileSync(path.join(agent, 'config.example.json'), JSON.stringify(cfg));
  defaultSettings = path.join(home, '.claude', 'settings.json');
  env = { PATH: process.env.PATH, HOME: home, USERPROFILE: home, BELLWETHER_DATA_DIR: path.join(root, 'data'), BELLWETHER_HOOK_TOKEN: TOKEN };
});
after(() => fs.rmSync(root, { recursive: true, force: true }));

const mk = (name, text) => {
  const f = path.join(caller, name);
  fs.writeFileSync(f, text);
  return f;
};
const j = (o, indent = 2) => JSON.stringify(o, null, indent) + '\n';
const keep = { hooks: { Stop: [{ hooks: [{ type: 'command', command: 'keep' }] }] }, theme: 'dark' };

describe('D1: no argument shape reaches the default settings file', notUnderRust('hooks install, uninstall and print are ported in milestone 4'), () => {
  const original = j({ theme: 'dark', marker: 1 });
  const shapes = [
    ['hooks', 'install', '--write', '--settings'],
    ['hooks', 'install', '--settings', '--write'],
    ['hooks', 'install', '--write', '--settings='],
    ['hooks', 'install', '--write', '--settings', ''],
    ['hooks', 'install', '--write', '--settings', '   '],
    ['hooks', 'install', '--write', '--setings', 'x.json'],
    ['hooks', 'install', '--write', '--Settings', 'x.json'],
    ['hooks', 'install', '--write', 'x.json'],
    ['hooks', 'install', '--write', '-s', 'x.json'],
    ['hooks', 'install', '--write', '--'],
    ['hooks', 'install', '--write', '--settings', '--settings'],
    ['hooks', 'install', '--write', '--settings=x.json', '--settings'],
    ['hooks', 'install', '--write', '--settings=x.json', '--settings='],
    ['hooks', 'install', '--write', '--settings', 'x.json', '--settings'],
    ['hooks', 'install', '--write', '--settings', 'x.json', 'extra'],
    ['hooks', 'install', '--write', '--settings', 'x.json', '--out', 'y'],
    ['hooks', 'install', '--write=1'],
    ['hooks', 'install', '--write', 'yes'],
    ['hooks', 'install', '--write', ''],
    ['hooks', '--write', 'install', '--settings'],
    ['hooks', 'uninstall', '--write', '--settings'],
    ['hooks', 'uninstall', '--write', 'x.json'],
    ['hooks', 'uninstall', '--write', '--settings', '--write'],
    ['hooks', 'install', '--write', '--__proto__', 'x'],
    ['hooks', 'install', '--write', '--constructor=1'],
    ['hooks', 'install', '--write', '--__proto__=x'],
    ['hooks', 'install', '--write', '--hasOwnProperty', 'x'],
    ['hooks', 'Install', '--write'],
    ['hooks', '--write'],
  ];
  for (const s of shapes) {
    test(`bellwether ${JSON.stringify(s)} leaves the default file alone`, () => {
      fs.writeFileSync(defaultSettings, original);
      fs.rmSync(path.join(root, 'data'), { recursive: true, force: true });
      const r = bellwether(s);
      assert.equal(read(defaultSettings), original, `default settings changed; exit ${r.code}; ${r.err.slice(0, 200)}`);
      assert.equal(fs.existsSync(path.join(root, 'data', 'backups')), false, 'a backup means a write was attempted');
    });
  }

  test('without --write, a bare invocation is a dry run against the default file and writes nothing', () => {
    fs.writeFileSync(defaultSettings, original);
    const r = bellwether(['hooks', 'install']);
    assert.equal(r.code, 0);
    assert.equal(read(defaultSettings), original);
    assert.match(r.err, /dry run/);
  });
});

describe('M1-7: relative paths mean what the caller typed', notUnderRust('hooks --settings and --out are ported in milestone 4, and the shim is the Node CLI\'s'), () => {
  test('--settings and --out resolve against the caller cwd (which has a space in it)', () => {
    const f = mk('rel.json', j({ theme: 'dark' }));
    const r = bellwether(['hooks', 'install', '--write', '--settings', 'rel.json']);
    assert.equal(r.code, 0, r.err);
    assert.ok(JSON.parse(read(f)).hooks.Stop);
    assert.equal(fs.existsSync(path.join(agent, 'rel.json')), false);
    const p = bellwether(['hooks', 'print', '--out', 'sub/dir/hooks-only.json']);
    assert.equal(p.code, 0, p.err);
    assert.ok(fs.existsSync(path.join(caller, 'sub', 'dir', 'hooks-only.json')));
  });

  test('a stale INIT_CWD or BELLWETHER_CWD in the environment cannot redirect the shim', () => {
    const f = mk('rel2.json', j({ theme: 'dark' }));
    const elsewhere = path.join(root, 'elsewhere');
    fs.mkdirSync(elsewhere);
    fs.writeFileSync(path.join(elsewhere, 'rel2.json'), j({ theme: 'light' }));
    const r = bellwether(['hooks', 'install', '--write', '--settings', 'rel2.json'], { extraEnv: { INIT_CWD: elsewhere, BELLWETHER_CWD: elsewhere } });
    assert.equal(r.code, 0, r.err);
    assert.ok(JSON.parse(read(f)).hooks, 'caller file edited');
    assert.equal(JSON.parse(read(path.join(elsewhere, 'rel2.json'))).hooks, undefined, 'the other directory was not touched');
  });

  test('a symlink to the shim on PATH works', () => {
    const bin = path.join(root, 'bin');
    fs.mkdirSync(bin, { recursive: true });
    fs.symlinkSync(path.join(agent, 'bellwether'), path.join(bin, 'bellwether'));
    const r = bellwether(['hooks', 'print'], { bin: path.join(bin, 'bellwether') });
    assert.equal(r.code, 0, r.err);
    assert.ok(JSON.parse(r.out).hooks.Stop);
  });
});

describe('M1-13: Node version floor in the shim', notUnderRust('the /bin/sh shim and bellwether.cmd run the Node CLI; the app has neither'), () => {
  for (const [v, ok] of [['v20.10.0', false], ['v22.8.0', false], ['v22.9.0', true], ['v22.10.1', true], ['v23.0.0', true], ['v100.0.0', true]]) {
    test(`${v} -> ${ok ? 'runs' : 'exit 3 with a message'}`, () => {
      const fake = path.join(root, 'fakenode-' + v);
      fs.mkdirSync(fake, { recursive: true });
      const real = process.execPath;
      // A node that reports `v`: it answers the shim's version probe and --version, and otherwise defers to the real node.
      fs.writeFileSync(
        path.join(fake, 'node'),
        `#!/bin/sh\nif [ "$1" = "-e" ]; then V="${v.slice(1)}"; MA=\${V%%.*}; R=\${V#*.}; MI=\${R%%.*}; if [ "$MA" -gt 22 ] || { [ "$MA" -eq 22 ] && [ "$MI" -ge 9 ]; }; then exit 0; else exit 1; fi; fi\nif [ "$1" = "--version" ]; then echo ${v}; exit 0; fi\nexec "${real}" "$@"\n`,
        { mode: 0o755 },
      );
      const r = bellwether(['hooks', 'print'], { pathEnv: `${fake}:/usr/bin:/bin` });
      if (ok) assert.equal(r.code, 0, r.err);
      else {
        assert.equal(r.code, 3);
        assert.match(r.err, /22\.9/);
      }
    });
  }
});

describe('M1-8/M1-9: lossless layout, numbers and the install record', notUnderRust('hooks install, uninstall and print are ported in milestone 4'), () => {
  const roundTrip = (name, text, { expectIdentical = true } = {}) => {
    const f = mk(name, text);
    const a = bellwether(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(a.code, 0, `${name} install: ${a.err}`);
    const b = bellwether(['hooks', 'uninstall', '--write', '--settings', f]);
    assert.equal(b.code, 0, `${name} uninstall: ${b.err}`);
    if (expectIdentical) assert.equal(read(f), text, `${name} not byte-identical`);
    return f;
  };

  test('tabs, CRLF, no trailing newline, 4-space and 1-space files round-trip byte-identically', () => {
    const o = { theme: 'dark', n: [1, 2], hooks: { Stop: [{ hooks: [{ type: 'command', command: 'k' }] }] } };
    roundTrip('tabs.json', JSON.stringify(o, null, '\t') + '\n');
    roundTrip('crlf.json', JSON.stringify(o, null, 2).replace(/\n/g, '\r\n') + '\r\n');
    roundTrip('nonl.json', JSON.stringify(o, null, 2));
    roundTrip('four.json', JSON.stringify(o, null, 4) + '\n');
    roundTrip('one.json', JSON.stringify(o, null, 1) + '\n');
    roundTrip('crlf4nonl.json', JSON.stringify(o, null, 4).replace(/\n/g, '\r\n'));
  });

  test('odd number literals survive exactly', () => {
    const body = '"a": 12345678901234567890,\n  "b": 1.0,\n  "c": 1e2,\n  "d": -0,\n  "e": 1E+5,\n  "f": 0.10,\n  "g": 1e400,\n  "h": -1e-400,\n  "j": 5e-324,\n  "k": 1.7976931348623157e308';
    const text = `{\n  ${body},\n  "hooks": {\n    "Stop": [\n      {\n        "hooks": [\n          {\n            "type": "command",\n            "command": "k"\n          }\n        ]\n      }\n    ]\n  }\n}\n`;
    roundTrip('numbers.json', text);
  });

  test('numbers inside the part bellwether rewrites (an existing stale bellwether entry) are not corrupted', () => {
    const stale = { type: 'http', url: `http://127.0.0.1:${port}/hook`, timeout: 600, headers: { 'X-Bellwether-Token': 'old' }, extra: 'KEEPNUM' };
    const text = j({ big: 'BIGNUM', hooks: { Stop: [{ hooks: [stale] }] } }).replace('"BIGNUM"', '12345678901234567890').replace('"KEEPNUM"', '12345678901234567890');
    const f = mk('stale-num.json', text);
    bellwether(['hooks', 'install', '--write', '--settings', f]);
    assert.match(read(f), /"big": 12345678901234567890/);
  });

  test('exotic but valid JSON: __proto__ key, integer-like keys, duplicate keys, escapes, astral chars', () => {
    const text = '{\n  "__proto__": {\n    "x": 1\n  },\n  "b": 1,\n  "2": 2,\n  "1": 3,\n  "dup": 1,\n  "dup": 2,\n  "esc": "\\u00e9\\/\\ud83d\\ude00\\u2028",\n  "lone": "\\ud800"\n}\n';
    const f = mk('exotic.json', text);
    const a = bellwether(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(a.code, 0, a.err);
    const after = read(f);
    const lost = [];
    if (!after.includes('"__proto__"')) lost.push('__proto__ key dropped');
    if (!after.includes('"x": 1')) lost.push('__proto__ contents dropped');
    if (!/"lone": "\\ud800"/.test(after)) lost.push('lone surrogate changed');
    if (!after.includes('"esc"')) lost.push('esc');
    assert.deepEqual(lost, []);
  });

  test('install -> uninstall on a minified one-line file keeps the values and warns about layout', () => {
    const f = mk('min.json', '{"theme":"dark","n":12345678901234567890}');
    const dry = bellwether(['hooks', 'install', '--settings', f]);
    assert.match(dry.err, /not laid out the way bellwether writes JSON/);
    bellwether(['hooks', 'install', '--write', '--settings', f]);
    bellwether(['hooks', 'uninstall', '--write', '--settings', f]);
    assert.match(read(f), /12345678901234567890/);
    assert.equal(JSON.parse(read(f)).theme, 'dark');
  });

  test('a large settings file (8000 lines) is planned in reasonable time and memory', () => {
    const o = { perms: Array.from({ length: 8000 }, (_, i) => `Bash(cmd${i}:*)`), hooks: {} };
    const f = mk('big.json', j(o));
    const t = Date.now();
    const r = bellwether(['hooks', 'install', '--settings', f]);
    assert.equal(r.code, 0, r.err.slice(0, 300));
    assert.ok(Date.now() - t < 15000, `took ${Date.now() - t} ms`);
  });

  test('empty "hooks": {} and "Stop": [] that were already there come back after install/uninstall', () => {
    roundTrip('empty-hooks.json', j({ theme: 'dark', hooks: {} }));
    roundTrip('empty-stop.json', j({ hooks: { Stop: [], PreToolUse: [{ hooks: [{ type: 'command', command: 'k' }] }] } }));
  });

  test('a missing file: install creates it 0600; uninstall afterwards leaves valid JSON', () => {
    const f = path.join(caller, 'created', 'settings.json');
    assert.equal(bellwether(['hooks', 'install', '--write', '--settings', f]).code, 0);
    assert.equal(fs.statSync(f).mode & 0o777, 0o600);
    assert.equal(bellwether(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    JSON.parse(read(f));
  });

  describe('the record cannot make uninstall remove anything that is not bellwether\'s', () => {
    const foreignEvents = Object.fromEntries(['SessionEnd', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop', 'Notification'].map((e) => [e, [{ matcher: 'x', hooks: [{ type: 'command', command: `keep-${e}` }] }]]));
    const original = { statusLine: { type: 'command', command: 's' }, hooks: foreignEvents };

    const tamper = [
      ['claims every event and the hooks object were created', (f) => ({ [fs.realpathSync(f)]: { events: ['SessionEnd', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop', 'Notification', 'Bogus'], hooks: true } })],
      ['events is not an array', (f) => ({ [fs.realpathSync(f)]: { events: 'Stop', hooks: true } })],
      ['events holds objects', (f) => ({ [fs.realpathSync(f)]: { events: [{}, null, 5], hooks: 1 } })],
      ['record is null', (f) => ({ [fs.realpathSync(f)]: null })],
      ['record is a string', (f) => ({ [fs.realpathSync(f)]: 'x' })],
    ];
    for (const [what, make] of tamper) {
      test(`tampered installs.json (${what}): other tools' entries survive`, () => {
        const f = mk(`tamper-${what.replace(/\W+/g, '-')}.json`, j(original));
        assert.equal(bellwether(['hooks', 'install', '--write', '--settings', f]).code, 0);
        fs.writeFileSync(dataFile('installs.json'), JSON.stringify(make(f)));
        const u = bellwether(['hooks', 'uninstall', '--write', '--settings', f]);
        assert.equal(u.code, 0, u.err);
        const after = JSON.parse(read(f));
        // a tampered record may leave empty event arrays behind, but must never remove a foreign entry
        for (const [e, groups] of Object.entries(original.hooks)) assert.deepEqual(after.hooks?.[e], groups, e);
        assert.deepEqual(after.statusLine, original.statusLine);
        assert.equal(JSON.stringify(after).includes('127.0.0.1'), false, 'bellwether entries remain');
      });
    }

    for (const [what, content] of [['null', 'null'], ['a string', '"x"'], ['an array', '[]'], ['a number', '5'], ['truncated', '{"a": {'], ['binary', '\u0000\u0001\u0002']]) {
      test(`corrupt installs.json (${what}): install and uninstall still finish cleanly and the file is consistent`, () => {
        fs.mkdirSync(path.join(root, 'data'), { recursive: true });
        fs.writeFileSync(dataFile('installs.json'), content);
        const f = mk(`corrupt-${what.replace(/\W+/g, '-')}.json`, j(original));
        const a = bellwether(['hooks', 'install', '--write', '--settings', f]);
        assert.equal(a.code, 0, `install exit ${a.code}: ${a.err.slice(0, 200)}`);
        assert.doesNotMatch(a.err, /at .*\.mjs:\d+/);
        const u = bellwether(['hooks', 'uninstall', '--write', '--settings', f]);
        assert.equal(u.code, 0, `uninstall exit ${u.code}: ${u.err.slice(0, 200)}`);
        assert.deepEqual(JSON.parse(read(f)), original);
      });
    }

    test('a data dir that cannot be written does not leave a half-reported install', () => {
      const f = mk('rodata.json', j(original));
      const ro = path.join(root, 'ro-data');
      fs.mkdirSync(ro);
      fs.chmodSync(ro, 0o500);
      try {
        const r = bellwether(['hooks', 'install', '--write', '--settings', f], { extraEnv: { BELLWETHER_DATA_DIR: ro } });
        // either it refuses before touching the settings file, or it succeeds; it must not write settings and then crash
        const changed = read(f) !== j(original);
        assert.ok(!(changed && r.code !== 0), `settings were changed but the command failed (exit ${r.code}): ${r.err.slice(0, 200)}`);
      } finally {
        fs.chmodSync(ro, 0o700);
      }
    });

    test('a stale record for a replaced file: uninstall still only removes bellwether entries', () => {
      const f = mk('stale-rec.json', j({ hooks: {} }));
      bellwether(['hooks', 'install', '--write', '--settings', f]);
      fs.writeFileSync(f, j(original)); // the user replaced the file by hand; the record says hooks/all events were created
      const stale = JSON.parse(read(dataFile('installs.json')));
      assert.ok(Object.keys(stale).length > 0);
      const u = bellwether(['hooks', 'uninstall', '--write', '--settings', f]);
      assert.equal(u.code, 0, u.err);
      assert.deepEqual(JSON.parse(read(f)), original);
    });

    test('install/uninstall of file A does not disturb the record or the content of file B', () => {
      const a = mk('A.json', j(original));
      const b = mk('B.json', j({ hooks: { Stop: [] } }));
      bellwether(['hooks', 'install', '--write', '--settings', a]);
      bellwether(['hooks', 'install', '--write', '--settings', b]);
      bellwether(['hooks', 'uninstall', '--write', '--settings', a]);
      bellwether(['hooks', 'uninstall', '--write', '--settings', b]);
      assert.deepEqual(JSON.parse(read(a)), original);
      assert.deepEqual(JSON.parse(read(b)), { hooks: { Stop: [] } });
    });
  });

  test('files holding the token are 0600: backups, installs.json, print --out; an existing settings mode is kept', () => {
    const f = mk('modes.json', j({ theme: 'dark' }));
    fs.chmodSync(f, 0o640);
    const r = bellwether(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(fs.statSync(f).mode & 0o777, 0o640);
    const backup = r.err.match(/backup of the previous file: (.+)/)[1].trim();
    assert.equal(fs.statSync(backup).mode & 0o777, 0o600);
    assert.equal(fs.statSync(dataFile('installs.json')).mode & 0o777, 0o600);
    bellwether(['hooks', 'print', '--out', 'modes-print.json']);
    assert.equal(fs.statSync(path.join(caller, 'modes-print.json')).mode & 0o777, 0o600);
  });
});

describe('new write paths: hooks print --out and record --out', () => {
  test('hooks print --out refuses to overwrite the user settings file (or any existing non-bellwether file)', notUnderRust('hooks install, uninstall and print are ported in milestone 4'), () => {
    const precious = j({ theme: 'dark', hooks: { Stop: [{ hooks: [{ type: 'command', command: 'precious' }] }] } });
    fs.writeFileSync(defaultSettings, precious);
    const r = bellwether(['hooks', 'print', '--out', defaultSettings]);
    assert.equal(read(defaultSettings), precious, `hooks print --out replaced settings.json; exit ${r.code}`);
    const other = mk('notes.txt', 'important notes');
    bellwether(['hooks', 'print', '--out', other]);
    assert.equal(read(other), 'important notes', 'hooks print --out overwrote an arbitrary existing file');
  });

  test('hooks print --out to a directory gives a message, not a stack', notUnderRust('hooks install, uninstall and print are ported in milestone 4'), () => {
    const r = bellwether(['hooks', 'print', '--out', caller]);
    assert.notEqual(r.code, 0);
    assert.doesNotMatch(r.err, /at .*\.mjs:\d+/);
  });

  test('record --out refuses a file that is not a bellwether recording', () => {
    const precious = j({ theme: 'dark' });
    fs.writeFileSync(defaultSettings, precious);
    const child = spawnBrain(['record', '--out', defaultSettings], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
    return new Promise((resolve) => {
      let err = '';
      child.stderr.on('data', (d) => (err += d));
      child.on('exit', () => {
        assert.equal(read(defaultSettings), precious, 'record --out appended to settings.json');
        resolve();
      });
      setTimeout(async () => {
        await fetch(`http://127.0.0.1:${port}/hook`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Bellwether-Token': TOKEN }, body: '{"hook_event_name":"Stop","session_id":"s"}' }).catch(() => {});
        setTimeout(() => child.kill('SIGTERM'), 300);
      }, 1000);
    });
  });
});

describe('M1-14/15: doctor', () => {
  const doctor = (extra = {}, args = []) => bellwether(['doctor', '--json', '--settings', mk(`d-${Math.random().toString(36).slice(2)}.json`, '{}\n'), ...args], { extraEnv: extra });
  const check = (r, n) => JSON.parse(r.out).checks.find((c) => c.name === n);

  test('the old example token is refused by doctor, serve, hooks print and hooks install', notUnderRust('doctor and hooks are ported in milestone 4; serve refusing the example token is checked by crates/core token.rs'), () => {
    const ex = { BELLWETHER_HOOK_TOKEN: 'q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh' };
    for (const a of [['doctor'], ['serve'], ['hooks', 'print'], ['hooks', 'install']]) {
      const r = bellwether(a, { extraEnv: ex });
      assert.equal(r.code, 3, a.join(' '));
      assert.match(r.out + r.err, /example/);
    }
  });

  test('an already-exported token wins over the .env (Node does not override)', notUnderRust('the .env is read by the Node CLI only; the app reads its token file (test/DIVERGENCES.md)'), () => {
    fs.writeFileSync(path.join(root, '.env'), 'BELLWETHER_HOOK_TOKEN=q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh\n');
    try {
      const r = bellwether(['hooks', 'print']);
      assert.equal(r.code, 0, r.err);
      assert.ok(!r.out.includes(TOKEN), 'stdout must be masked (M10-7)');
      assert.match(r.out, new RegExp(`"X-Bellwether-Token": "${TOKEN.slice(0, 4)}…"`));
      const outFile = path.join(caller, 'exported-wins.json');
      assert.equal(bellwether(['hooks', 'print', '--out', outFile]).code, 0);
      assert.ok(read(outFile).includes(TOKEN), 'the exported token (not the .env one) is what the --out file holds');
      assert.ok(!read(outFile).includes('q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh'));
      const n = bellwether(['hooks', 'print'], { extraEnv: { BELLWETHER_HOOK_TOKEN: '' } });
      assert.equal(n.code, 3, 'an empty exported token must not silently fall through to .env');
    } finally {
      fs.rmSync(path.join(root, '.env'));
    }
  });

  function brainProc(args) {
    const c = spawnBrain(['serve', ...args], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
    return new Promise((resolve) => {
      let err = '';
      c.stderr.on('data', (d) => {
        err += d;
        if (err.includes('listening on')) resolve({ stop: () => new Promise((r) => (c.once('exit', r), c.kill('SIGTERM'))) });
      });
    });
  }
  const doctorAsync = (args) =>
    new Promise((resolve) => {
      const c = spawnBrain([...args], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
      let out = '';
      c.stdout.on('data', (d) => (out += d));
      c.on('close', (code) => resolve({ code, out }));
    });

  test('--hang brain: FAIL on brain naming --hang, exit 2', notUnderRust('doctor is ported in milestone 4'), async () => {
    const b = await brainProc(['--hang']);
    try {
      const r = await doctorAsync(['doctor', '--json', '--settings', mk('hang-s.json', '{}')]);
      const c = JSON.parse(r.out).checks.find((x) => x.name === 'brain');
      assert.equal(c.status, 'FAIL');
      assert.match(c.detail, /--hang/);
      assert.equal(r.code, 2);
    } finally {
      await b.stop();
    }
  });

  test('--slow brain longer than the probe: reported as a problem, not a pass', notUnderRust('doctor is ported in milestone 4'), async () => {
    const b = await brainProc(['--slow=3000']);
    try {
      const r = await doctorAsync(['doctor', '--json', '--settings', mk('slow-s.json', '{}')]);
      const checks = JSON.parse(r.out).checks;
      assert.ok(checks.some((c) => c.status === 'FAIL' && /brain/.test(c.name)), r.out);
    } finally {
      await b.stop();
    }
  });
});

describe('M1-16: event names', () => {
  test('a non-identifier hook_event_name is recorded and logged as unknown', async () => {
    const rec = path.join(caller, 'ev.jsonl');
    const c = spawnBrain(['record', '--out', rec], { root: agent, shim: path.join(agent, 'bellwether'), cwd: caller, env });
    let err = '';
    await new Promise((r) => { c.stderr.on('data', (d) => { err += d; if (err.includes('recording to')) r(); }); });
    const body = JSON.stringify({ hook_event_name: 'Pre\nSECRET_EVENT text', session_id: 's' });
    await fetch(`http://127.0.0.1:${port}/hook`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Bellwether-Token': TOKEN }, body });
    await new Promise((r) => setTimeout(r, 300));
    c.kill('SIGTERM');
    await new Promise((r) => c.once('exit', r));
    assert.doesNotMatch(read(rec) + err, /SECRET_EVENT/);
  });
});
