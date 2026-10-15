import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { LABEL, SHELL_LABEL, findOldAgents, plistPath } from '../brain/autostart.mjs';
import { hookBlock } from '../brain/hooks.mjs';
import { migrate } from '../brain/migrate.mjs';
import { startServer } from '../brain/server.mjs';
import { AGENT_DIR, notUnderRust, spawnBrainSync, tempDir } from './helpers.mjs';

// The repository-to-repository migration in process, launchd injected. The app's
// `migrate` (its own files, LaunchAgents named and left alone) runs in the CLI test of D1
// and in migrate.test.mjs, against both brains.
const REPO_MIGRATE = notUnderRust("brain/migrate.mjs and autostart.mjs in process (LaunchAgents swapped, launchctl injected); the app never writes or boots out a LaunchAgent (plan D5; its swap is `migrate --to-app`, milestone 8, cargo-tested)");

/**
 * Re-check of `migrate` after the fixes, laid out like the maintainer's real install:
 * a hub with agents/pasture, hub-prefixed LaunchAgents running `<hub>/agents/pasture/brain/cli.mjs
 * serve` and `<hub>/.../target/release/pasture-shell`, 16 old hook entries, the token in the hub
 * .env. Temp directories, injected `run` and `portHolder`; nothing reaches the live machine.
 */

const OLD = 'oldPastureTokenNotReal_0123456789';
const PORT = 47991;
const OLD_BRAIN = 'local.example-hub.pasture.brain';
const OLD_STRIP = 'local.example-hub.pasture.shell';
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;');
const plist = (label, args, extra = '') => `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>Label</key><string>${esc(label)}</string><key>ProgramArguments</key><array>${args.map((a) => `<string>${esc(a)}</string>`).join('')}</array>${extra}</dict></plist>\n`;

function world({ built = true, binaryPlists = false } = {}) {
  const root = tempDir('bk3-re-');
  const hub = path.join(root, 'hub');
  const old = path.join(hub, 'agents', 'pasture');
  fs.mkdirSync(path.join(old, 'data'), { recursive: true });
  fs.mkdirSync(path.join(old, 'brain'));
  fs.mkdirSync(path.join(old, 'shell', 'src-tauri', 'target', 'release'), { recursive: true });
  fs.writeFileSync(path.join(old, 'shell', 'src-tauri', 'target', 'release', 'pasture-shell'), '');
  fs.writeFileSync(path.join(hub, '.env'), `OTHER=1\nPASTURE_HOOK_TOKEN=${OLD}\n`);
  fs.writeFileSync(path.join(old, 'config.json'), JSON.stringify({ port: PORT }));
  const home = path.join(root, 'home');
  const la = path.join(home, 'Library', 'LaunchAgents');
  fs.mkdirSync(la, { recursive: true });
  fs.mkdirSync(path.join(home, '.claude'));
  const brainPlist = path.join(la, `${OLD_BRAIN}.plist`);
  const stripPlist = path.join(la, `${OLD_STRIP}.plist`);
  fs.writeFileSync(brainPlist, plist(OLD_BRAIN, ['/usr/local/bin/node', `--env-file-if-exists=${hub}/.env`, `${old}/brain/cli.mjs`, 'serve']));
  fs.writeFileSync(stripPlist, plist(OLD_STRIP, [`${old}/shell/src-tauri/target/release/pasture-shell`, '--no-spawn-brain']));
  if (binaryPlists) for (const f of [brainPlist, stripPlist]) spawnSync('plutil', ['-convert', 'binary1', f]);
  const settingsFile = path.join(home, '.claude', 'settings.json');
  const legacy = JSON.parse(JSON.stringify(hookBlock(PORT, OLD)).replaceAll('X-Bellwether-', 'X-Pasture-'));
  legacy.hooks.Stop.unshift({ hooks: [{ type: 'command', command: 'afplay /Users/example/ready.wav' }] });
  fs.writeFileSync(settingsFile, JSON.stringify(legacy, null, 2) + '\n');
  const repo = path.join(root, 'bellwether');
  fs.mkdirSync(repo);
  fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), path.join(repo, 'config.example.json'));
  const shellBin = path.join(repo, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
  if (built) {
    fs.mkdirSync(path.dirname(shellBin), { recursive: true });
    fs.writeFileSync(shellBin, '');
  }
  const calls = [];
  const state = { failLoad: false, failProbe: false, failRestoreOf: null, probeCalls: 0 };
  const run = (c, a) => {
    calls.push(`${path.basename(c)} ${a.map((x) => (x.includes('probe.mjs') ? 'probe.mjs' : x)).join(' ')}`.trim());
    if (c === 'launchctl' && a[0] === 'print') return { ok: true, out: 'state = running\npid = 1234\n' };
    if (c === 'launchctl' && a[0] === 'bootstrap' && state.failLoad && /bellwether/.test(a[2] ?? '')) return { ok: false, out: 'Bootstrap failed: 5: Input/output error' };
    if (c === 'launchctl' && a[0] === 'bootstrap' && state.failRestoreOf && String(a[2]).includes(state.failRestoreOf)) return { ok: false, out: 'Bootstrap failed: 5' };
    if (a.some((x) => String(x).includes('probe.mjs'))) {
      state.probeCalls++;
      return state.failProbe ? { ok: false, out: 'a pasture brain (pid 1234) still holds port' } : { ok: true, out: 'the bellwether brain answers' };
    }
    return { ok: true, out: '' };
  };
  const opts = { from: old, home, settingsFile, agentDir: repo, dataDir: path.join(repo, 'data'), targetEnv: path.join(repo, '.env'), uid: 501, realHome: () => true, platform: 'darwin', shellBin, portHolder: () => ({ state: 'brain', name: 'pasture', pid: 1234 }), run };
  return { root, hub, old, home, la, brainPlist, stripPlist, settingsFile, repo, shellBin, calls, state, opts };
}
const read = (f) => fs.readFileSync(f, 'utf8');
const hooksAre = (w) => (/X-Pasture-Token/.test(read(w.settingsFile)) ? 'pasture' : /X-Bellwether-Token/.test(read(w.settingsFile)) ? 'bellwether' : 'none');
const ok = (text) => !text.includes(OLD);

describe('D1: the old agents are found by what they run', () => {
  for (const binary of [false, true]) {
    test(`the CLI preview names the maintainer's layout (hub-prefixed labels, ${binary ? 'binary' : 'XML'} plists) and no one else's agent, and changes nothing`, () => {
      const w = world({ binaryPlists: binary });
      fs.writeFileSync(path.join(w.la, 'com.acme.pasture.brain.plist'), plist('com.acme.pasture.brain', ['/usr/bin/python3', '-m', 'http.server', '8080']));
      fs.writeFileSync(path.join(w.la, 'com.other.brain.plist'), plist('com.other.brain', ['/usr/local/bin/node', '/opt/other/brain/cli.mjs', 'serve']));
      fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(w.repo, 'brain'), { recursive: true });
      const files = () => fs.readdirSync(w.la).sort().map((f) => [f, read(path.join(w.la, f))]);
      const before = { agents: files(), settings: read(w.settingsFile) };
      const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'cli-data'), BELLWETHER_ENV: path.join(w.root, 'cli.env') };
      const r = spawnBrainSync(['migrate', '--from', w.old, '--settings', w.settingsFile], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
      assert.equal(r.status, 0, r.stderr);
      assert.match(r.stdout, new RegExp(OLD_BRAIN.replace(/\./g, '\\.')));
      assert.match(r.stdout, new RegExp(OLD_STRIP.replace(/\./g, '\\.')));
      assert.doesNotMatch(r.stdout, /com\.acme\.pasture\.brain|com\.other\.brain/, 'another program\'s agent was named');
      assert.ok(ok(r.stdout + r.stderr), 'the token is never shown');
      assert.deepEqual({ agents: files(), settings: read(w.settingsFile) }, before, 'a preview changed something');
    });
    test(`the maintainer's layout (hub-prefixed labels, ${binary ? 'binary' : 'XML'} plists) is recognised as brain and strip, with and without --from`, REPO_MIGRATE, () => {
      const w = world({ binaryPlists: binary });
      const found = findOldAgents(w.home, { from: w.old });
      assert.deepEqual(found.map((a) => [a.label, a.kind]).sort(), [[OLD_BRAIN, 'brain'], [OLD_STRIP, 'shell']]);
      assert.deepEqual(findOldAgents(w.home).map((a) => a.kind).sort(), ['brain', 'shell'], 'without --from, the pattern and content still find them');
    });
  }
  test('a .pasture.brain label that runs something else, other tools\' agents, and bellwether\'s own agents are left alone', REPO_MIGRATE, () => {
    const w = world();
    const mk = (label, args) => fs.writeFileSync(path.join(w.la, `${label}.plist`), plist(label, args));
    mk('com.acme.pasture.brain', ['/usr/bin/python3', '-m', 'http.server', '8080']);
    mk('com.acme.pasture.shell', ['/usr/bin/ssh', '-N', 'host']);
    mk('com.other.brain', ['/usr/local/bin/node', '/opt/other/brain/cli.mjs', 'serve']);
    mk('com.other.sheep', [`${w.old}/../other/pasture-shell-ish`]);
    mk(LABEL, ['/x/node', '/x/brain/cli.mjs', 'serve']);
    mk(SHELL_LABEL, ['/x/bellwether-shell']);
    const labels = findOldAgents(w.home, { from: w.old }).map((a) => a.label).sort();
    assert.deepEqual(labels, [OLD_BRAIN, OLD_STRIP]);
  });
  test('an unreadable plist is matched by label only for a .pasture.* name; garbage in other names is ignored; nothing throws', REPO_MIGRATE, () => {
    const w = world();
    fs.writeFileSync(path.join(w.la, 'garbage.plist'), '\u0000\u0001not a plist');
    fs.writeFileSync(path.join(w.la, 'bplist-broken.plist'), 'bplist00broken');
    assert.doesNotThrow(() => findOldAgents(w.home, { from: w.old }));
    assert.deepEqual(findOldAgents(w.home, { from: w.old }).map((a) => a.label).sort(), [OLD_BRAIN, OLD_STRIP]);
  });
});

describe('D2: order, health wait and rollback (hub-shaped install)', REPO_MIGRATE, () => {
  test('success: agents swapped (brain first, old booted out before the new loads), the new brain is probed, and only then are the hooks rewritten; a re-run changes nothing', () => {
    const w = world();
    const r = migrate({ ...w.opts, write: true });
    const iBootout = w.calls.findIndex((c) => c === `launchctl bootout gui/501/${OLD_BRAIN}`);
    const iLoad = w.calls.findIndex((c) => c.startsWith('launchctl bootstrap') && c.includes('local.bellwether.brain'));
    const iProbe = w.calls.findIndex((c) => c.startsWith(`${path.basename(process.execPath)} probe.mjs`));
    assert.ok(iBootout >= 0 && iLoad > iBootout && iProbe > iLoad, w.calls.join('\n'));
    assert.equal(hooksAre(w), 'bellwether');
    assert.ok(!fs.existsSync(w.brainPlist) && !fs.existsSync(w.stripPlist));
    assert.ok(ok(r.lines.join('\n')));
    const before = fs.readdirSync(w.la).sort();
    const calls = w.calls.length;
    migrate({ ...w.opts, portHolder: () => ({ state: 'brain', name: 'bellwether', pid: 99 }), write: true });
    assert.deepEqual(fs.readdirSync(w.la).sort(), before);
    assert.ok(!w.calls.slice(calls).some((c) => /bootout .*pasture/.test(c)));
  });
  test('failure at LOAD: old agents restored (plists back, bootstrapped again), hooks untouched, message lists what changed and was undone, and a re-run after the cause is fixed converges', () => {
    const w = world();
    w.state.failLoad = true;
    let err;
    try { migrate({ ...w.opts, write: true }); } catch (e) { err = e; }
    assert.ok(err);
    assert.ok(ok(err.message));
    assert.equal(hooksAre(w), 'pasture');
    assert.ok(fs.existsSync(w.brainPlist) && fs.existsSync(w.stripPlist), 'the old plists are not back');
    assert.ok(w.calls.some((c) => c === `launchctl bootstrap gui/501 ${w.brainPlist}`), 'the old brain was not loaded again');
    assert.ok(!fs.existsSync(plistPath(w.home, LABEL)), 'a half-installed bellwether agent was left');
    assert.match(err.message, /hooks were not touched/);
    assert.match(err.message, /back as they were/);
    w.state.failLoad = false;
    migrate({ ...w.opts, write: true });
    assert.equal(hooksAre(w), 'bellwether');
  });
  test('failure at the HEALTH WAIT (the new brain never answers): same rollback, and the printed message names the cause', () => {
    const w = world();
    w.state.failProbe = true;
    let err;
    try { migrate({ ...w.opts, write: true }); } catch (e) { err = e; }
    assert.ok(err);
    assert.match(err.message, /didn't come up/);
    assert.equal(hooksAre(w), 'pasture');
    assert.ok(fs.existsSync(w.brainPlist));
    assert.ok(!fs.existsSync(plistPath(w.home, LABEL)));
    w.state.failProbe = false;
    migrate({ ...w.opts, write: true });
    assert.equal(hooksAre(w), 'bellwether');
  });
  test('failure PARTWAY THROUGH RESTORING: the message prints the exact launchctl commands to run by hand, those files exist, and a re-run converges', () => {
    const w = world();
    w.state.failProbe = true;
    w.state.failRestoreOf = OLD_STRIP;
    let err;
    try { migrate({ ...w.opts, write: true }); } catch (e) { err = e; }
    assert.ok(err);
    const cmds = [...err.message.matchAll(/^\s+(launchctl bootstrap gui\/501 (\S+))$/gm)];
    assert.ok(cmds.length >= 1, `no recovery command printed:\n${err.message}`);
    for (const [, , file] of cmds) assert.ok(fs.existsSync(file), `the printed recovery command points at a missing file: ${file}`);
    assert.ok(cmds.some(([, , f]) => f === w.stripPlist), 'the strip agent that could not be loaded is not in the recovery commands');
    assert.ok(!cmds.some(([, , f]) => f === w.brainPlist), 'the brain agent restored fine but its command is printed');
    assert.equal(hooksAre(w), 'pasture');
    w.state.failProbe = false;
    w.state.failRestoreOf = null;
    migrate({ ...w.opts, write: true });
    assert.equal(hooksAre(w), 'bellwether');
  });
  test('failure at the HOOK WRITE after a good swap: the new brain is up and still accepts the old header, nothing is rolled back, the message is clear, and a re-run finishes the hooks', () => {
    const w = world();
    // make the backup directory impossible to create: applyPlan fails after the swap
    fs.mkdirSync(path.join(w.repo, 'data'), { recursive: true });
    fs.writeFileSync(path.join(w.repo, 'data', 'backups'), 'a file where the backup directory should be');
    let err;
    try { migrate({ ...w.opts, write: true }); } catch (e) { err = e; }
    assert.ok(err, 'a failed hook write was reported as success');
    assert.ok(ok(err.message));
    assert.equal(hooksAre(w), 'pasture');
    assert.ok(fs.existsSync(plistPath(w.home, LABEL)), 'the new brain agent was rolled back although only the hook write failed');
    assert.ok(!fs.existsSync(w.brainPlist));
    assert.doesNotMatch(String(err.stack ?? ''), /at .*node_modules/);
    fs.rmSync(path.join(w.repo, 'data', 'backups'));
    const r2 = migrate({ ...w.opts, portHolder: () => ({ state: 'brain', name: 'bellwether', pid: 99 }), write: true });
    assert.equal(hooksAre(w), 'bellwether');
    assert.match(r2.lines.join('\n'), /replaced 16 pasture entries/);
  });
});

describe('D3: the strip is not built', REPO_MIGRATE, () => {
  test('the old strip agent is left alone (no bootout, plist kept), only the brain moves, and the old strip keeps working against the new brain', () => {
    const w = world({ built: false });
    const r = migrate({ ...w.opts, write: true });
    assert.ok(fs.existsSync(w.stripPlist));
    assert.ok(!w.calls.some((c) => c.includes(OLD_STRIP) && /bootout|bootstrap/.test(c)));
    assert.match(r.lines.join('\n'), /stays as it is/);
    assert.equal(hooksAre(w), 'bellwether');
  });
});

describe('D5: who holds the port', REPO_MIGRATE, () => {
  const attempt = (holder, launchdPid = 1234) => {
    const w = world();
    const run = (c, a) => {
      if (c === 'launchctl' && a[0] === 'print') return launchdPid ? { ok: true, out: `pid = ${launchdPid}\n` } : { ok: false, out: 'Could not find service' };
      return w.opts.run(c, a);
    };
    let err;
    let r;
    try { r = migrate({ ...w.opts, run, portHolder: () => holder, write: true }); } catch (e) { err = e; }
    return { w, err, r };
  };
  test('the old brain launchd runs (pid matches) is ours; a bellwether brain, another pasture brain and any other process are refused with nothing changed', () => {
    assert.ok(!attempt({ state: 'brain', name: 'pasture', pid: 1234 }).err);
    for (const h of [{ state: 'brain', name: 'bellwether', pid: 5 }, { state: 'brain', name: 'pasture', pid: 5 }, { state: 'other', pid: 6, command: 'node server.js' }, { state: 'other', pid: null, command: null }]) {
      const { w, err } = attempt(h);
      assert.ok(err, JSON.stringify(h));
      assert.equal(hooksAre(w), 'pasture');
      assert.ok(fs.existsSync(w.brainPlist) && fs.existsSync(w.stripPlist));
      assert.ok(!w.calls.some((c) => /bootout|bootstrap/.test(c)), 'something was booted out or loaded');
    }
  });
  test('the exception: a pasture brain holds the port but launchd has no pid for the old agent. It is treated as ours; is that safe? The swap proceeds, the new brain cannot bind, the health wait fails, and everything is rolled back with the hooks untouched.', () => {
    const w = world();
    w.state.failProbe = true; // the hand-started pasture brain still holds the port, so the new brain never answers
    const run = (c, a) => (c === 'launchctl' && a[0] === 'print' ? { ok: false, out: 'Could not find service' } : w.opts.run(c, a));
    let err;
    try { migrate({ ...w.opts, run, portHolder: () => ({ state: 'brain', name: 'pasture', pid: 4321 }), write: true }); } catch (e) { err = e; }
    assert.ok(err, 'a swap that cannot work was reported as success');
    assert.equal(hooksAre(w), 'pasture');
    assert.ok(fs.existsSync(w.brainPlist));
    assert.match(err.message, /hooks were not touched/);
  });
  test('the "other process" refusal shows the listener\'s command line: a secret on that command line must not be echoed', () => {
    const { err } = attempt({ state: 'other', pid: 6, command: 'someserver --token=sk-abcdefghijklmnopqrstuvwx --port 47991' });
    assert.ok(err);
    assert.doesNotMatch(err.message, /sk-abcdefghijklmnopqrstuvwx/, 'another process\'s secret was printed');
  });
});

describe('probe.mjs', notUnderRust("brain/probe.mjs, the repository migration's health probe; the app's migrate pings a brain itself (crates/brain migrate.rs `brain_takes`), only when HOME is the real one"), () => {
  // async: the servers under test live in this process, so a synchronous spawn would block them
  const probe = (args) =>
    new Promise((resolve) => {
      const c = spawn(process.execPath, [path.join(AGENT_DIR, 'brain', 'probe.mjs'), ...args], { env: { PATH: process.env.PATH } });
      let stdout = '';
      let stderr = '';
      c.stdout.on('data', (d) => (stdout += d));
      c.stderr.on('data', (d) => (stderr += d));
      c.on('close', (status) => resolve({ status, stdout, stderr }));
    });
  test('the token is read from the file: never in argv, stdout or stderr, whether the brain accepts it, refuses it, or is absent', async () => {
    const TOKEN = 'probeTokenNotReal_0123456789abcdef';
    const WRONG = 'wrongProbeTokenNotReal_0123456789ab';
    const brain = await startServer({ port: 0, token: TOKEN, onEvent: () => {} });
    try {
      const dir = tempDir('bk3-probe-');
      const good = path.join(dir, 'good.env');
      const bad = path.join(dir, 'bad.env');
      fs.writeFileSync(good, `BELLWETHER_HOOK_TOKEN=${TOKEN}\n`);
      fs.writeFileSync(bad, `BELLWETHER_HOOK_TOKEN=${WRONG}\n`);
      const ok1 = await probe(['--port', String(brain.port), '--env', good, '--timeout', '5']);
      assert.equal(ok1.status, 0, ok1.stdout);
      const refused = await probe(['--port', String(brain.port), '--env', bad, '--timeout', '1']);
      assert.equal(refused.status, 1);
      const none = await probe(['--port', '1', '--env', good, '--timeout', '1']);
      assert.equal(none.status, 1);
      const missing = await probe(['--port', String(brain.port), '--env', path.join(dir, 'nope.env')]);
      assert.equal(missing.status, 1);
      for (const r of [ok1, refused, none, missing]) assert.ok(!(r.stdout + r.stderr).includes(TOKEN) && !(r.stdout + r.stderr).includes(WRONG));
      // argv of a running probe: spawn one against a closed port and read it with ps
      const child = spawn(process.execPath, [path.join(AGENT_DIR, 'brain', 'probe.mjs'), '--port', '1', '--env', good, '--timeout', '3'], { stdio: 'ignore' });
      await new Promise((r) => setTimeout(r, 300));
      const ps = spawnSync('ps', ['-o', 'command=', '-p', String(child.pid)], { encoding: 'utf8' }).stdout;
      child.kill();
      assert.ok(!ps.includes(TOKEN), 'the token is on the probe\'s command line');
      assert.ok(ps.includes(good));
    } finally { await brain.close(); }
  });
  test('an old pasture brain answering /health is reported as a pasture brain still holding the port, not accepted as the new one', async () => {
    const http = await import('node:http');
    const srv = http.createServer((q, s) => { s.writeHead(200, { 'Content-Type': 'application/json' }); s.end(JSON.stringify({ ok: true, name: 'pasture', pid: 4242 })); });
    await new Promise((r) => srv.listen(0, '127.0.0.1', r));
    const dir = tempDir('bk3-probe2-');
    const env = path.join(dir, 'e.env');
    fs.writeFileSync(env, 'BELLWETHER_HOOK_TOKEN=probeTokenNotReal_0123456789abcdef\n');
    const r = await probe(['--port', String(srv.address().port), '--env', env, '--timeout', '1']);
    srv.close();
    assert.equal(r.status, 1);
    assert.match(r.stdout, /pasture brain \(pid 4242\) still holds port/);
  });
});
