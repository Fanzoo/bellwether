import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, describe, test } from 'node:test';
import { request, spawnBrain, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Independent tester probes for Windows plan W2 (first part), run on any OS against the
 * Rust binary. Everything is in temp directories: a scratch BELLWETHER_HOME, HOME and
 * USERPROFILE, a test port; nothing reaches ~/.claude or port 47417.
 *
 * Windows-only behaviour (DACLs, the watchdog by handle, SO_EXCLUSIVEADDRUSE) is covered by
 * the VM runs in TEST-REPORT.md and by the cargo tests; this file covers what the Mac can.
 */

const freePort = () =>
  new Promise((resolve) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });

const BOM = '﻿';
const roots = [];
function scratch() {
  const root = tempDir('bw-w2-');
  roots.push(root);
  fs.mkdirSync(path.join(root, 'home'));
  fs.mkdirSync(path.join(root, 'cc'));
  return root;
}
after(() => {
  for (const r of roots) fs.rmSync(r, { recursive: true, force: true });
});

async function setup() {
  const root = scratch();
  const port = await freePort();
  fs.writeFileSync(path.join(root, 'config.json'), JSON.stringify({ port }));
  const env = { PATH: process.env.PATH, HOME: root, USERPROFILE: root, BELLWETHER_HOME: path.join(root, 'home'), BELLWETHER_CONFIG: path.join(root, 'config.json') };
  const cli = (args, extra = {}, input) => {
    const r = spawnBrainSync(args, { root, env: { ...env, ...extra }, cwd: root, encoding: 'utf8', input, timeout: 30000 });
    return { code: r.status, out: r.stdout, err: r.stderr };
  };
  assert.equal(cli(['token', 'init']).code, 0);
  return { root, port, env, cli, settings: (name = 's.json') => path.join(root, name) };
}

describe('BOM and CRLF in a settings file (W2-6, divergence 27)', () => {
  test('a BOM + CRLF + tab-indented file: install keeps all three, uninstall gives the exact bytes back', async () => {
    const { cli, settings } = await setup();
    const f = settings();
    const original = Buffer.from(`${BOM}{\r\n\t"model": "x",\r\n\t"hooks": {}\r\n}\r\n`);
    fs.writeFileSync(f, original);
    assert.equal(cli(['hooks', 'install', '--settings', f]).code, 0, 'a dry run is fine');
    assert.deepEqual(fs.readFileSync(f), original, 'a dry run writes nothing');
    const r = cli(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(r.code, 0, r.err);
    const after = fs.readFileSync(f);
    assert.deepEqual([...after.subarray(0, 3)], [0xef, 0xbb, 0xbf], 'BOM first');
    assert.notDeepEqual([...after.subarray(3, 6)], [0xef, 0xbb, 0xbf], 'exactly one BOM');
    const text = after.toString('utf8');
    assert.ok(!/[^\r]\n/.test(text), 'every newline is CRLF, including the ones the installer added');
    assert.ok(text.includes('\r\n\t"model"'), 'the file\'s own tab indent is kept');
    assert.ok(JSON.parse(text.slice(1)).hooks.Stop, 'valid JSON after the BOM');
    // Idempotent: a second install changes nothing and writes no second backup.
    const again = cli(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(again.code, 0, again.err);
    assert.deepEqual(fs.readFileSync(f), after);
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    assert.deepEqual(fs.readFileSync(f), original);
  });

  test('a BOM with nothing after it, two BOMs, and UTF-16 are refused with a cause, exit 3, nothing written', async () => {
    const { root, cli, settings } = await setup();
    const cases = {
      bomOnly: Buffer.from(BOM),
      twoBoms: Buffer.from(`${BOM}${BOM}{}`),
      bomComment: Buffer.from(`${BOM}// hi\n{}`),
      utf16le: Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from('{}', 'utf16le')]),
    };
    for (const [name, bytes] of Object.entries(cases)) {
      const f = settings(`${name}.json`);
      fs.writeFileSync(f, bytes);
      const r = cli(['hooks', 'install', '--write', '--settings', f]);
      assert.equal(r.code, 3, `${name}: ${r.out}${r.err}`);
      assert.match(r.err, /not valid JSON|does not hold a JSON object/, name);
      assert.doesNotMatch(r.err, /panicked|RUST_BACKTRACE/, name);
      assert.deepEqual(fs.readFileSync(f), bytes, `${name}: untouched`);
    }
    assert.ok(!fs.existsSync(path.join(root, 'home', 'backups')) || fs.readdirSync(path.join(root, 'home', 'backups')).length === 0, 'a refused file leaves no backup');
  });

  test('a BOM before an empty object installs and round-trips; doctor reads a BOM file without calling it invalid', async () => {
    const { cli, settings, port } = await setup();
    const f = settings();
    fs.writeFileSync(f, `${BOM}{}`);
    assert.equal(cli(['hooks', 'install', '--write', '--settings', f]).code, 0);
    const d = cli(['doctor', '--settings', f, '--port', String(port)]);
    assert.doesNotMatch(d.out + d.err, /not valid JSON|invalid JSON/i, d.out);
    // W2-19 (the coordinator's fix round): doctor says the file has a BOM, as INFO only.
    assert.match(d.out, /INFO\s+settings BOM\s+.*byte order mark/, d.out);
    assert.match(d.out, /PASS\s+hooks/i, d.out);
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    assert.equal(fs.readFileSync(f, 'utf8'), `${BOM}{}`);
  });
});

describe('CLAUDE_CONFIG_DIR moves the default settings file on every OS (WD8)', () => {
  test('hooks install --write with no --settings edits $CLAUDE_CONFIG_DIR/settings.json and never $HOME/.claude', async () => {
    const { root, cli } = await setup();
    const cc = path.join(root, 'cc');
    fs.writeFileSync(path.join(cc, 'settings.json'), '{"model":"x"}\n');
    fs.mkdirSync(path.join(root, '.claude'));
    fs.writeFileSync(path.join(root, '.claude', 'settings.json'), '{"decoy":true}\n');
    const r = cli(['hooks', 'install', '--write'], { CLAUDE_CONFIG_DIR: cc });
    assert.equal(r.code, 0, r.err);
    assert.ok(JSON.parse(fs.readFileSync(path.join(cc, 'settings.json'), 'utf8')).hooks.Stop, 'the config dir\'s file was edited');
    assert.equal(fs.readFileSync(path.join(root, '.claude', 'settings.json'), 'utf8'), '{"decoy":true}\n', '~/.claude untouched');
    // Empty means unset.
    const e = cli(['hooks', 'install', '--write'], { CLAUDE_CONFIG_DIR: '' });
    assert.equal(e.code, 0, e.err);
    assert.ok(JSON.parse(fs.readFileSync(path.join(root, '.claude', 'settings.json'), 'utf8')).hooks.Stop, 'empty CLAUDE_CONFIG_DIR falls back to ~/.claude');
  });

  test('hooks print --out refuses the config dir\'s settings.json (the out-file guard follows it), but writes an ordinary file', async () => {
    const { root, cli } = await setup();
    const cc = path.join(root, 'cc');
    for (const name of ['settings.json', 'settings.local.json']) {
      const r = cli(['hooks', 'print', '--out', path.join(cc, name)], { CLAUDE_CONFIG_DIR: cc });
      assert.equal(r.code, 3, `${name}: ${r.out}${r.err}`);
      assert.ok(!fs.existsSync(path.join(cc, name)), `${name} not created`);
    }
    // Spelled through a dot-dot and a different case on a case-insensitive disk is the same file: the guard canonicalises.
    const sneaky = path.join(cc, '..', 'cc', 'settings.json');
    assert.equal(cli(['hooks', 'print', '--out', sneaky], { CLAUDE_CONFIG_DIR: cc }).code, 3);
    const ok = cli(['hooks', 'print', '--out', path.join(root, 'hooks.json')], { CLAUDE_CONFIG_DIR: cc });
    assert.equal(ok.code, 0, ok.err);
  });
});

describe('POST /control stop (W2, divergence 26)', () => {
  test('refused without the token, with a wrong one, with a look-alike action or the wrong media type; accepted once, saved, port freed', async () => {
    const { root, port, env, cli } = await setup();
    const home = path.join(root, 'home');
    const token = fs.readFileSync(path.join(home, 'hook-token'), 'utf8').trim();
    const child = spawnBrain(['serve', '--port', String(port)], { root, env, cwd: root });
    let log = '';
    child.stderr.on('data', (d) => (log += d));
    const exited = new Promise((res) => child.once('exit', (code, sig) => res({ code, sig })));
    try {
      for (let i = 0; i < 100; i++) {
        try {
          const h = await request(port, { method: 'GET', path: '/health' });
          if (h.status === 200) break;
        } catch {}
        await new Promise((r) => setTimeout(r, 100));
      }
      const hdr = (extra = {}) => ({ Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', ...extra });
      const stop = (headers, body = '{"action":"stop"}', method = 'POST') => request(port, { method, path: '/control', headers, body });
      const alive = async () => (await request(port, { method: 'GET', path: '/health' })).status === 200;
      assert.equal((await stop(hdr())).status, 403, 'no token');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': 'wrongTokenNotReal_0123456789abcdef' }))).status, 403, 'wrong token');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': token, Origin: 'http://evil.example' }))).status, 403, 'a browser origin, even with the token');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': token, Host: 'evil.example' }))).status, 403, 'a foreign Host (DNS rebinding), even with the token');
      assert.ok(await alive(), 'refused stops left it serving');
      const bad = await stop(hdr({ 'X-Bellwether-Token': token }), '{"action":"STOP"}');
      assert.equal(bad.status, 400, 'actions are case-sensitive and unknown ones are 400');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': token, 'Content-Type': 'text/plain' }))).status, 415, 'a no-preflight text/plain POST cannot stop it');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': token }), '{"action":"stop"', 'POST')).status, 400, 'torn JSON');
      assert.equal((await stop(hdr({ 'X-Bellwether-Token': token }), '{"action":["stop"]}')).status, 400, 'an array is not the string');
      assert.ok(await alive(), 'still serving after every malformed attempt');
      const mute = await stop(hdr({ 'X-Bellwether-Token': token }), '{"action":"mute"}');
      assert.equal(mute.status, 200);
      const ok = await stop(hdr({ 'X-Bellwether-Token': token }));
      assert.equal(ok.status, 200);
      assert.deepEqual(JSON.parse(ok.body.toString()), { ok: true });
      const r = await Promise.race([exited, new Promise((res) => setTimeout(() => res('timeout'), 5000))]);
      assert.notEqual(r, 'timeout', `the brain did not exit after stop; log:\n${log}`);
      assert.equal(r.code, 0, `a stop is a clean exit (${JSON.stringify(r)}), log:\n${log}`);
      assert.equal(JSON.parse(fs.readFileSync(path.join(home, 'state.json'), 'utf8')).muted, true, 'state.json saved on the way out');
      assert.match(fs.readFileSync(path.join(home, 'logs', 'brain.log'), 'utf8'), /asked to stop \(POST \/control stop\)/);
    } finally {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    }
  });

  test('a second serve on the same port exits 2 with the cause (no silent double listener)', async () => {
    const { root, port, env, cli } = await setup();
    const first = spawn_serve(root, env, port);
    try {
      await waitHealth(port);
      const r = cli(['serve', '--port', String(port)]);
      assert.equal(r.code, 2, r.err);
      assert.match(r.err + r.out, /already in use/);
    } finally {
      first.kill('SIGTERM');
    }
  });
});

function spawn_serve(root, env, port) {
  return spawnBrain(['serve', '--port', String(port)], { root, env, cwd: root, stdio: 'ignore' });
}
async function waitHealth(port) {
  for (let i = 0; i < 100; i++) {
    try {
      if ((await request(port, { method: 'GET', path: '/health' })).status === 200) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('the brain never answered /health');
}

describe('the token the strip holds for its tokened stop (W2, brain.rs child_control)', { skip: !process.env.BELLWETHER_SLOW && 'set BELLWETHER_SLOW=1 (takes about 70 s)' }, () => {
  // A defect probe, not a pass/fail of the plan: brain.rs keeps the token it started the
  // child with and sends that one in `stop`. After `token rotate`, the brain accepts the old
  // token for one minute only (reload-token), so a strip that is quit more than a minute
  // after a rotation sends a refused stop and falls back to TerminateProcess (state.json is
  // not saved). This pins the brain half: the old token is refused after the grace, the
  // new one stops it and saves.
  test('after a rotation the old token stops being accepted within about a minute, so a stop with it is a 403', async () => {
    const { root, port, env, cli, settings } = await setup();
    const home = path.join(root, 'home');
    const old = fs.readFileSync(path.join(home, 'hook-token'), 'utf8').trim();
    const child = spawnBrain(['serve', '--port', String(port)], { root, env, cwd: root, stdio: 'ignore' });
    try {
      await waitHealth(port);
      const f = settings();
      fs.writeFileSync(f, '{}\n');
      const r = cli(['token', 'rotate', '--write', '--settings', f]);
      assert.equal(r.code, 0, r.err + r.out);
      const fresh = fs.readFileSync(path.join(home, 'hook-token'), 'utf8').trim();
      assert.notEqual(fresh, old);
      const stop = (token) => request(port, { path: '/control', headers: { Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', 'X-Bellwether-Token': token }, body: '{"action":"mute"}' });
      assert.equal((await stop(old)).status, 200, 'the old token is accepted during the grace');
      await new Promise((res) => setTimeout(res, 62_000));
      assert.equal((await stop(old)).status, 403, 'and refused after it');
      assert.equal((await stop(fresh)).status, 200);
    } finally {
      child.kill('SIGTERM');
    }
  });
});

describe('the parent watchdog (BELLWETHER_PARENT_PID; the unix path, unchanged by W2)', { skip: process.platform === 'win32' }, () => {
  test('a pid that is not this brain\'s parent stops it within a second; its real parent keeps it running', async () => {
    const { root, port, env } = await setup();
    const log = () => (fs.existsSync(path.join(root, 'home', 'logs', 'brain.log')) ? fs.readFileSync(path.join(root, 'home', 'logs', 'brain.log'), 'utf8') : '');
    // Wrong parent: the brain's real parent is this process.
    const wrong = spawnBrain(['serve', '--port', String(port)], { root, env: { ...env, BELLWETHER_PARENT_PID: String(process.pid + 1) }, cwd: root, stdio: 'ignore' });
    const gone = await Promise.race([new Promise((res) => wrong.once('exit', (c) => res(c))), new Promise((res) => setTimeout(() => res('timeout'), 4000))]);
    if (gone === 'timeout') wrong.kill('SIGTERM');
    assert.notEqual(gone, 'timeout', 'it should have stopped at once');
    assert.match(log(), /is gone; stopping/);
    // Right parent: keeps serving.
    const right = spawnBrain(['serve', '--port', String(port)], { root, env: { ...env, BELLWETHER_PARENT_PID: String(process.pid) }, cwd: root, stdio: 'ignore' });
    try {
      await waitHealth(port);
      await new Promise((r) => setTimeout(r, 1200));
      assert.equal(right.exitCode, null, 'still running with its real parent');
    } finally {
      right.kill('SIGTERM');
    }
  });
});

/**
 * The Windows test folder's build refuses to touch the maintainer's own Claude Code
 * settings (WD12 5, W2-2). The rule is `--features test-build`, which also builds on a Mac:
 *   CARGO_TARGET_DIR=<dir> cargo build --manifest-path shell/src-tauri/Cargo.toml --features test-build
 *   BELLWETHER_TESTBUILD_BIN=<dir>/debug/bellwether-shell npm test -- ... test/tester-w2.test.mjs
 * HOME, USERPROFILE and CLAUDE_CONFIG_DIR are scratch, so even a missing refusal could only
 * write there. The live port is deliberately not probed on a Mac (a missing refusal would
 * reach the real brain); the VM run covers it.
 */
describe('the test-folder build refuses what would reach the live install (W2-2)', { skip: !process.env.BELLWETHER_TESTBUILD_BIN && 'set BELLWETHER_TESTBUILD_BIN to a --features test-build binary' }, () => {
  const run = (root, args) => {
    const env = { PATH: process.env.PATH, HOME: root, USERPROFILE: root, CLAUDE_CONFIG_DIR: path.join(root, 'cc'), BELLWETHER_HOME: path.join(root, 'home'), BELLWETHER_CONFIG: path.join(root, 'config.json') };
    const r = spawnSyncRaw(process.env.BELLWETHER_TESTBUILD_BIN, args, env, root);
    return { code: r.status, out: r.stdout, err: r.stderr };
  };
  test('hooks install|uninstall, token rotate and migrate need --settings, in every spelling of its absence; autostart is refused; nothing is written', async () => {
    const root = scratch();
    fs.writeFileSync(path.join(root, 'config.json'), JSON.stringify({ port: await freePort() }));
    fs.mkdirSync(path.join(root, '.claude'));
    fs.writeFileSync(path.join(root, '.claude', 'settings.json'), '{"standin":1}\n');
    fs.writeFileSync(path.join(root, 'cc', 'settings.json'), '{"standin":2}\n');
    assert.equal(run(root, ['token', 'init']).code, 0);
    const before = () => [fs.readFileSync(path.join(root, '.claude', 'settings.json'), 'utf8'), fs.readFileSync(path.join(root, 'cc', 'settings.json'), 'utf8'), fs.readdirSync(path.join(root, 'home')).sort().join()].join('|');
    const snap = before();
    for (const args of [
      ['hooks', 'install', '--write'],
      ['hooks', 'uninstall', '--write'],
      ['hooks', 'install'],
      ['token', 'rotate', '--write'],
      ['migrate'],
      ['migrate', '--write'],
      ['migrate', '--undo', '--write'],
      ['hooks', 'install', '--write', '--settingsx=foo'],
      ['hooks', 'install', '--write', '--settings'],
      ['hooks', 'install', '--write', '--settings='],
      ['autostart', 'install'],
      ['autostart', 'uninstall'],
    ]) {
      const r = run(root, args);
      assert.notEqual(r.code, 0, `${args.join(' ')} should not succeed: ${r.out}${r.err}`);
      assert.match(r.err, /this test build never|--settings needs a value/, args.join(' '));
      assert.equal(before(), snap, `${args.join(' ')} changed something`);
    }
    // With a scratch file it works, and only that file changes.
    const scratchFile = path.join(root, 'scratch.json');
    fs.writeFileSync(scratchFile, '{}\n');
    const ok = run(root, ['hooks', 'install', '--write', '--settings', scratchFile]);
    assert.equal(ok.code, 0, ok.err);
    assert.ok(JSON.parse(fs.readFileSync(scratchFile, 'utf8')).hooks.Stop);
    assert.equal(before().split('|').slice(0, 2).join('|'), snap.split('|').slice(0, 2).join('|'));
    // help and version always run; hooks print writes only its own file.
    assert.equal(run(root, ['version']).code, 0);
    assert.equal(run(root, ['hooks', 'print', '--out', path.join(root, 'hooks.json')]).code, 0);
  });
});

import { spawnSync } from 'node:child_process';
function spawnSyncRaw(bin, args, env, cwd) {
  return spawnSync(bin, args, { argv0: 'bellwether', env, cwd, encoding: 'utf8', timeout: 30000 });
}
