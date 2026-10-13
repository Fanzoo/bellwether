import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { describe, test } from 'node:test';
import { LABEL, OLD_LABELS, SHELL_LABEL, autostart, plistPath } from '../brain/autostart.mjs';
import { hookBlock, install, uninstall } from '../brain/hooks.mjs';
import { adoptLegacyEnv } from '../brain/envpath.mjs';
import { migrate } from '../brain/migrate.mjs';
import { startServer } from '../brain/server.mjs';
import { AGENT_DIR, FIXTURES, request, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * BR-3K's probes for the pasture -> bellwether rename and `migrate`. Temp directories
 * and injected fakes only: launchctl is never run, the real HOME, .env and settings are
 * never read.
 */

const OLD = 'oldPastureTokenNotReal_0123456789';
const NEW = 'brandNewBellwetherTok_9876543210';
const PORT = 47990; // a port nothing here listens on: the maintainer's live brain (47417) is never contacted
const legacyBlock = (token, port = PORT) => JSON.parse(JSON.stringify(hookBlock(port, token)).replaceAll('X-Bellwether-', 'X-Pasture-'));
const sha = (f) => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
const snapshot = (...dirs) => {
  const out = {};
  const walk = (d) => {
    if (!fs.existsSync(d)) return;
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isSymbolicLink()) out[p] = `link:${fs.readlinkSync(p)}`;
      else if (e.isDirectory()) walk(p);
      else out[p] = sha(p) + (fs.statSync(p).mode & 0o777).toString(8);
    }
  };
  dirs.forEach(walk);
  return out;
};

function world({ shellAgent = true, built = true, oldEnv, settings, newEnv, port = PORT } = {}) {
  const root = tempDir('bk3-mig-');
  const hub = path.join(root, 'hub');
  const old = path.join(hub, 'agents', 'pasture');
  fs.mkdirSync(path.join(old, 'data'), { recursive: true });
  fs.writeFileSync(path.join(hub, '.env'), oldEnv ?? `OTHER_SECRET=keep-me\nPASTURE_HOOK_TOKEN=${OLD}\n`);
  fs.writeFileSync(path.join(old, 'config.json'), JSON.stringify({ port }));
  fs.writeFileSync(path.join(old, 'data', 'state.json'), JSON.stringify({ version: 1, muted: false, identities: { 'herdr:/s#w1:p1': { colourIndex: 2, slots: { primary: 3 }, label: 'meadow', lastSeen: Date.now() } } }));
  const home = path.join(root, 'home');
  fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
  fs.mkdirSync(path.join(home, '.claude'));
  for (const l of shellAgent ? OLD_LABELS : OLD_LABELS.slice(0, 1)) fs.writeFileSync(plistPath(home, l), '<plist/>');
  const settingsFile = path.join(home, '.claude', 'settings.json');
  const s = settings ?? (() => {
    const b = legacyBlock(OLD, port);
    b.hooks.Stop.unshift({ hooks: [{ type: 'command', command: 'afplay /Users/example/ready.wav' }] });
    b.statusLine = { type: 'command', command: 'echo hi' };
    return b;
  })();
  fs.writeFileSync(settingsFile, JSON.stringify(s, null, 2) + '\n');
  const repo = path.join(root, 'bellwether');
  fs.mkdirSync(repo);
  fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), path.join(repo, 'config.example.json'));
  const shellBin = path.join(repo, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
  if (built) {
    fs.mkdirSync(path.dirname(shellBin), { recursive: true });
    fs.writeFileSync(shellBin, '');
  }
  const targetEnv = path.join(repo, '.env');
  if (newEnv !== undefined) fs.writeFileSync(targetEnv, newEnv);
  const calls = [];
  const opts = { from: old, home, settingsFile, agentDir: repo, dataDir: path.join(repo, 'data'), targetEnv, uid: 501, realHome: () => true, platform: 'darwin', shellBin, portHolder: () => ({ state: 'free' }), run: (c, a) => (calls.push(`${c} ${a.join(' ')}`), { ok: true, out: '' }) };
  return { root, hub, old, home, repo, settingsFile, targetEnv, shellBin, calls, opts };
}
const noTokens = (text, ...tokens) => tokens.every((t) => !String(text).includes(t));

describe('migrate preview: changes nothing and never shows a token', () => {
  test('sha256 of the old install, the new checkout and HOME are identical after a preview, and no launchctl call is made, with any mix of tokens', () => {
    for (const [oldEnv, newEnv] of [[undefined, undefined], [`PASTURE_HOOK_TOKEN=${OLD}\nBELLWETHER_HOOK_TOKEN=${NEW}\n`, undefined], [undefined, `BELLWETHER_HOOK_TOKEN=${NEW}\n`], [`PASTURE_HOOK_TOKEN=\n`, undefined], [`OTHER=1\n`, undefined]]) {
      const w = world({ oldEnv, newEnv });
      const before = snapshot(w.root);
      const r = migrate({ ...w.opts, write: false });
      assert.deepEqual(snapshot(w.root), before, JSON.stringify([oldEnv, newEnv]));
      assert.deepEqual(w.calls, [], 'a preview ran launchctl');
      assert.ok(noTokens(r.lines.join('\n'), OLD, NEW));
    }
  });
  test('error paths do not print the token: a bad --from, an unreadable --env, an invalid settings file containing the token, a read-only target', () => {
    const w = world();
    for (const bad of [{ from: path.join(w.root, 'missing') }, { from: w.hub }]) {
      let msg = '';
      try { migrate({ ...w.opts, ...bad }); } catch (e) { msg = e.message; }
      assert.ok(noTokens(msg, OLD, NEW), msg);
    }
    fs.writeFileSync(w.settingsFile, `{"hooks": {"Stop": [{"hooks": [{"headers": {"X-Pasture-Token": "${OLD}" "oops": 1}}]}]}}`);
    let msg = '';
    try { migrate({ ...w.opts, write: true }); } catch (e) { msg = e.message; }
    assert.ok(noTokens(msg, OLD, NEW), `an invalid settings file made migrate print the token: ${msg.slice(0, 200)}`);
  });
  test('the CLI under temp directories: preview prints no token on stdout or stderr, even for an unknown flag or a missing --from', () => {
    const w = world();
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'd'), BELLWETHER_ENV: w.targetEnv, BELLWETHER_HOOK_TOKEN: NEW };
    for (const args of [['migrate', '--from', w.old, '--settings', w.settingsFile], ['migrate', '--from', path.join(w.root, 'nope')], ['migrate', '--bogus'], ['migrate', '--env', path.join(w.root, 'nope.env')]]) {
      const r = spawnBrainSync([...args], { encoding: 'utf8', env });
      assert.ok(noTokens(r.stdout + r.stderr, OLD, NEW), args.join(' '));
    }
    assert.equal(fs.existsSync(path.join(w.repo, '.env')), false);
  });
});

describe('migrate --write: idempotence, order and recovery', () => {
  test('a second --write changes no file and calls launchctl for nothing that is already done', () => {
    const w = world();
    migrate({ ...w.opts, write: true });
    const before = snapshot(w.root);
    w.calls.length = 0;
    migrate({ ...w.opts, write: true });
    assert.deepEqual(snapshot(w.root), before);
    assert.ok(!w.calls.some((c) => /bootout.*pasture/.test(c)), `second run touched the old agents: ${w.calls}`);
  });
  test('the hooks are never rewritten before the new brain has been loaded: at the first launchctl call the settings file still holds pasture\'s entries, or launchctl failing leaves the hooks as they were', () => {
    const w = world();
    let settingsAtFirstCall = null;
    const run = (c, a) => {
      if (settingsAtFirstCall === null) settingsAtFirstCall = fs.readFileSync(w.settingsFile, 'utf8');
      return { ok: true, out: '' };
    };
    migrate({ ...w.opts, run, write: true });
    assert.match(settingsAtFirstCall, /X-Pasture-Token/, 'the hooks already point at bellwether\'s token before the new brain is loaded');
  });
  test('launchctl failing partway (the new brain cannot be bootstrapped) must not leave new hooks, a removed old brain agent and no brain: either nothing changed or the old setup is still there', () => {
    const w = world();
    const before = snapshot(w.home);
    const run = (c, a) => (a[0] === 'bootstrap' ? { ok: false, out: 'Bootstrap failed: 5: Input/output error' } : { ok: true, out: '' });
    let err;
    try { migrate({ ...w.opts, run, write: true }); } catch (e) { err = e; }
    const settings = fs.readFileSync(w.settingsFile, 'utf8');
    const oldAgentsLeft = OLD_LABELS.filter((l) => fs.existsSync(plistPath(w.home, l)));
    const hooksMigrated = !/X-Pasture-Token/.test(settings);
    assert.ok(err, 'a failed bootstrap was not reported');
    assert.ok(noTokens(err.message, OLD, NEW));
    assert.ok(!(hooksMigrated && oldAgentsLeft.length === 0), 'the hooks were migrated and pasture\'s agents removed, but bellwether\'s brain never loaded: every Claude Code session now prints hook errors');
    void before;
  });
  test('an unwritable settings file fails with a message, leaves the settings, the old agents and the old install untouched, and a re-run after fixing it succeeds', () => {
    const w = world();
    fs.chmodSync(w.settingsFile, 0o444);
    fs.chmodSync(path.dirname(w.settingsFile), 0o555);
    const beforeSettings = sha(w.settingsFile);
    let err;
    try { migrate({ ...w.opts, write: true }); } catch (e) { err = e; }
    fs.chmodSync(path.dirname(w.settingsFile), 0o755);
    fs.chmodSync(w.settingsFile, 0o644);
    assert.ok(err, 'a settings file that cannot be written was reported as success');
    assert.ok(noTokens(err.message, OLD, NEW));
    assert.doesNotMatch(String(err.stack ?? ''), /node_modules/);
    assert.equal(sha(w.settingsFile), beforeSettings);
    for (const l of OLD_LABELS) assert.ok(fs.existsSync(plistPath(w.home, l)), 'the old agent was removed before the settings step failed');
    assert.deepEqual(w.calls, []);
    const r = migrate({ ...w.opts, write: true });
    assert.match(r.lines.join('\n'), /Done/);
    assert.ok(!/X-Pasture-Token/.test(fs.readFileSync(w.settingsFile, 'utf8')));
  });
  test('a missing build moves only the brain, says so, and does not remove the old strip agent without a replacement', () => {
    const w = world({ built: false });
    const r = migrate({ ...w.opts, write: true });
    assert.match(r.lines.join('\n'), /isn't built yet/);
    assert.ok(fs.existsSync(plistPath(w.home, LABEL)));
    assert.ok(!fs.existsSync(plistPath(w.home, SHELL_LABEL)));
    assert.ok(fs.existsSync(plistPath(w.home, OLD_LABELS[1])), 'the old strip agent was removed while the new strip is not built: no strip at the next login');
  });
  test('bootout of the old brain comes before anything of the new one is loaded', () => {
    const w = world();
    migrate({ ...w.opts, write: true });
    const iOld = w.calls.findIndex((c) => /bootout .*local\.pasture\.brain/.test(c));
    const iNew = w.calls.findIndex((c) => /bootstrap .*local\.bellwether\.brain/.test(c));
    assert.ok(iOld >= 0 && iNew > iOld, w.calls.join('\n'));
    const iOldShell = w.calls.findIndex((c) => /bootout .*local\.pasture\.shell/.test(c));
    assert.ok(iOldShell >= 0 && iOldShell < w.calls.findIndex((c) => /bootstrap .*local\.bellwether\.shell/.test(c)));
  });
  test('something already holding the brain\'s port that is not an old brain from these LaunchAgents stops migrate before anything changes, and says what holds it', () => {
    const w = world();
    const before = snapshot(w.home, w.hub);
    for (const holder of [{ state: 'other', pid: 4242, command: 'python3 -m http.server 47990' }, { state: 'brain', name: 'bellwether', pid: 777 }, { state: 'brain', name: 'pasture', pid: 777 }]) {
      // launchctl print says the old brain agent's pid is 1234, so pid 777 is not ours
      const run = (c, a) => (c === 'launchctl' && a[0] === 'print' ? { ok: true, out: 'pid = 1234\n' } : { ok: true, out: '' });
      let err;
      try { migrate({ ...w.opts, run, portHolder: () => holder, write: true }); } catch (e) { err = e; }
      assert.ok(err, JSON.stringify(holder));
      assert.match(err.message, /not changed/);
      assert.deepEqual(snapshot(w.home, w.hub), before, 'the LaunchAgents, the settings or the old install changed before the port check');
    }
  });
});

describe('the token', () => {
  const tokenIn = (f) => /^BELLWETHER_HOOK_TOKEN=(.*)$/m.exec(fs.readFileSync(f, 'utf8'))?.[1];
  test('old token only: copied with the same value, the old .env untouched, mode 0600 for a new file; --env overrides the source', () => {
    const w = world();
    const old = sha(path.join(w.hub, '.env'));
    migrate({ ...w.opts, write: true });
    assert.equal(tokenIn(w.targetEnv), OLD);
    assert.equal(sha(path.join(w.hub, '.env')), old);
    assert.equal(fs.statSync(w.targetEnv).mode & 0o777, 0o600);
    const w2 = world({ oldEnv: 'NOTHING=1\n' });
    const other = path.join(w2.root, 'other.env');
    fs.writeFileSync(other, `export PASTURE_HOOK_TOKEN="${OLD}"\r\n`);
    migrate({ ...w2.opts, envFrom: other, write: true });
    assert.equal(tokenIn(w2.targetEnv), OLD);
  });
  test('both names set: the new one is kept and the hooks use it; the old .env\'s differing value is not copied', () => {
    const w = world({ newEnv: `BELLWETHER_HOOK_TOKEN=${NEW}\n` });
    migrate({ ...w.opts, write: true });
    assert.equal(tokenIn(w.targetEnv), NEW);
    assert.match(fs.readFileSync(w.settingsFile, 'utf8'), new RegExp(NEW));
  });
  test('the last PASTURE_HOOK_TOKEN wins (as Node reads it), an empty last one means none, quotes and export are read as Node reads them', () => {
    const w = world({ oldEnv: `PASTURE_HOOK_TOKEN=first_token_not_used_00000\nPASTURE_HOOK_TOKEN=${OLD}\n` });
    migrate({ ...w.opts, write: true });
    assert.equal(tokenIn(w.targetEnv), OLD);
    const w2 = world({ oldEnv: `PASTURE_HOOK_TOKEN=${OLD}\nPASTURE_HOOK_TOKEN=\n` });
    const r = migrate({ ...w2.opts, write: true });
    assert.match(r.lines.join('\n'), /no PASTURE_HOOK_TOKEN found/);
    assert.equal(fs.existsSync(w2.targetEnv), false);
    assert.match(fs.readFileSync(w2.settingsFile, 'utf8'), /X-Pasture-Token/, 'hooks were rewritten with no token');
  });
  test('a symlinked target .env is written through, stays a link, keeps its mode and other lines, and leaves a .bak', () => {
    const w = world();
    const real = path.join(w.root, 'real.env');
    fs.writeFileSync(real, 'ALPHA=1\r\nBETA=2', { mode: 0o640 });
    fs.chmodSync(real, 0o640);
    fs.symlinkSync(real, w.targetEnv);
    migrate({ ...w.opts, write: true });
    assert.ok(fs.lstatSync(w.targetEnv).isSymbolicLink());
    const text = fs.readFileSync(real, 'utf8');
    assert.match(text, /^ALPHA=1\r\nBETA=2\r\nBELLWETHER_HOOK_TOKEN=/);
    assert.equal(fs.statSync(real).mode & 0o777, 0o640);
    assert.equal(fs.readFileSync(`${real}.bak`, 'utf8'), 'ALPHA=1\r\nBETA=2');
    assert.equal(fs.statSync(`${real}.bak`).mode & 0o777, 0o600);
  });
  test('the .bak copy of the .env (every account\'s secrets) cannot be committed: .env.* is ignored', () => {
    const ig = fs.readFileSync(path.join(AGENT_DIR, '.gitignore'), 'utf8');
    assert.match(ig, /^\.env\.\*$/m);
  });
});

describe('legacy hook entries', () => {
  const others = () => JSON.parse(fs.readFileSync(path.join(FIXTURES, 'settings-sample.json'), 'utf8'));
  const withLegacy = (s, token = OLD) => {
    const b = legacyBlock(token);
    const out = structuredClone(s);
    out.hooks ??= {};
    for (const [e, g] of Object.entries(b.hooks)) out.hooks[e] = [...(out.hooks[e] ?? []), ...g];
    return out;
  };
  test('install replaces all 16 legacy entries, other tools\' entries stay byte-identical and in order, and uninstall of the result restores the other tools\' settings', () => {
    const s = others();
    const mixed = withLegacy(s);
    const after = install(mixed, PORT, NEW);
    assert.doesNotMatch(JSON.stringify(after), /X-Pasture-/);
    for (const [e, groups] of Object.entries(s.hooks ?? {})) assert.deepEqual(after.hooks[e].slice(0, groups.length), groups, e);
    const back = uninstall(after, PORT);
    assert.deepEqual(back.hooks ?? {}, Object.fromEntries(Object.entries(s.hooks ?? {})));
    const direct = uninstall(mixed, PORT);
    assert.doesNotMatch(JSON.stringify(direct), /X-Pasture-/, 'uninstall left a legacy entry');
  });
  test('mixed old and new entries on one event collapse to exactly one current entry; a legacy entry with BOTH headers is still replaced', () => {
    const b = legacyBlock(OLD);
    const n = hookBlock(PORT, NEW);
    const both = JSON.parse(JSON.stringify(n));
    both.hooks.Stop[0].hooks[0].headers['X-Pasture-Token'] = OLD;
    const mixed = { hooks: { Stop: [...b.hooks.Stop, ...n.hooks.Stop, ...both.hooks.Stop] } };
    const after = install(mixed, PORT, NEW);
    assert.equal(after.hooks.Stop.length, 1);
    assert.deepEqual(after.hooks.Stop[0].hooks[0].headers['X-Bellwether-Token'], NEW);
    assert.equal(after.hooks.Stop[0].hooks[0].headers['X-Pasture-Token'], undefined);
  });
  test('the exact-URL rule holds for legacy-looking entries: other ports, /hooks, /hook2, query strings and http hooks of other tools are never claimed', () => {
    const foreign = ['/hooks', '/hook2', '/hook?x=1', '/hook/sub'].map((p) => ({ type: 'http', url: `http://127.0.0.1:${PORT}${p}`, headers: { 'X-Pasture-Token': 'x' } }));
    foreign.push({ type: 'http', url: `http://127.0.0.1:${PORT + 1}/hook`, headers: { 'X-Pasture-Token': 'x' } });
    foreign.push({ type: 'http', url: 'http://127.0.0.1:3000/hook', headers: { Authorization: 'Bearer t' } });
    const s = { hooks: { Stop: [{ hooks: foreign }] } };
    assert.deepEqual(install(s, PORT, NEW).hooks.Stop[0], s.hooks.Stop[0]);
    assert.deepEqual(uninstall(install(s, PORT, NEW), PORT).hooks, s.hooks);
  });
});

describe('legacy header mapping in the server', () => {
  const POST = (port, headers) => request(port, { method: 'POST', path: '/hook', headers: { Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', ...headers }, body: '{"hook_event_name":"Stop","session_id":"s"}' });
  test('only-old, only-new, both-matching, both-different (new wins, never either), and wrong tokens', async () => {
    const seen = [];
    const brain = await startServer({ port: 0, token: NEW, onEvent: (p, h) => seen.push(h) });
    try {
      assert.equal((await POST(brain.port, { 'X-Pasture-Token': NEW })).status, 204, 'old header with the right token');
      assert.equal((await POST(brain.port, { 'X-Bellwether-Token': NEW })).status, 204);
      assert.equal((await POST(brain.port, { 'X-Bellwether-Token': 'wrong_token_value_00000000', 'X-Pasture-Token': NEW })).status, 403, 'a wrong new token must not be rescued by a right old one');
      assert.equal((await POST(brain.port, { 'X-Bellwether-Token': NEW, 'X-Pasture-Token': 'wrong_token_value_00000000' })).status, 204, 'the new header decides');
      assert.equal((await POST(brain.port, { 'X-Pasture-Token': 'wrong_token_value_00000000' })).status, 403);
      assert.equal((await POST(brain.port, {})).status, 403);
      assert.equal((await POST(brain.port, { 'X-Pasture-Token': NEW, 'X-Pasture-Term-Program': 'old', 'X-Bellwether-Term-Program': 'new' })).status, 204);
      await new Promise((r) => setTimeout(r, 30));
      const last = seen.at(-1);
      assert.equal(last['x-bellwether-term-program'], 'new', 'the new header must win for non-token headers too');
      assert.equal(last['x-pasture-term-program'], undefined);
    } finally { await brain.close(); }
  });
});

describe('legacy environment variables', () => {
  test('PASTURE_X fills BELLWETHER_X only when that is unset; a set (even empty) BELLWETHER_X wins; unrelated names are untouched', () => {
    const env = { PASTURE_HOOK_TOKEN: 'a', BELLWETHER_HOOK_TOKEN: 'b', PASTURE_DATA_DIR: '/x', PASTURE_E: 'e', BELLWETHER_E: '', OTHER: '1', PASTURE: 'p' };
    const used = adoptLegacyEnv(env, new Set());
    assert.equal(env.BELLWETHER_HOOK_TOKEN, 'b');
    assert.equal(env.BELLWETHER_DATA_DIR, '/x');
    assert.equal(env.BELLWETHER_E, '');
    assert.equal(env.BELLWETHER_, undefined);
    assert.deepEqual([...used].sort(), ['PASTURE_DATA_DIR']);
  });
  test('an exported PASTURE_HOOK_TOKEN beats BELLWETHER_HOOK_TOKEN in the .env (it is adopted before the file is loaded): the stale old value would win', () => {
    const dir = tempDir('bk3-envprec-');
    const envFile = path.join(dir, '.env');
    fs.writeFileSync(envFile, `BELLWETHER_HOOK_TOKEN=${NEW}\n`);
    const r = spawnSync(process.execPath, ['--input-type=module', '-e', `import('${path.join(AGENT_DIR, 'brain', 'envload.mjs').replace(/\\/g, '/')}').then(() => process.stdout.write(process.env.BELLWETHER_HOOK_TOKEN ?? ''))`], { encoding: 'utf8', env: { PATH: process.env.PATH, BELLWETHER_ENV: envFile, PASTURE_HOOK_TOKEN: OLD } });
    assert.equal(r.stdout, NEW, `a stale exported PASTURE_HOOK_TOKEN overrode the .env's BELLWETHER_HOOK_TOKEN (got ${r.stdout === OLD ? 'the old one' : r.stdout})`);
  });
});

describe('LaunchAgent replacement under injection', () => {
  test('autostart install with old agents present: preview removes nothing; write boots out and removes each old agent before the new ones load; the old plists are gone afterwards', () => {
    const home = tempDir('bk3-la-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    for (const l of OLD_LABELS) fs.writeFileSync(plistPath(home, l), '<plist/>');
    const calls = [];
    const bin = path.join(tempDir('bk3-bin-'), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    const o = { home, uid: 501, realHome: () => true, shell: true, shellBin: bin, run: (c, a) => (calls.push(a.join(' ')), { ok: true, out: '' }) };
    const prev = autostart('install', { ...o, write: false });
    assert.equal(prev.wrote, false);
    assert.deepEqual(calls, []);
    for (const l of OLD_LABELS) assert.ok(fs.existsSync(plistPath(home, l)));
    autostart('install', { ...o, write: true });
    const firstBootstrap = calls.findIndex((c) => /^bootstrap /.test(c));
    assert.ok(calls.slice(0, firstBootstrap).some((c) => /bootout .*pasture\.brain/.test(c)));
    for (const l of OLD_LABELS) assert.ok(!fs.existsSync(plistPath(home, l)));
  });
  test('a temp HOME never reaches launchctl in the default (not forced) guard, old agents or not', () => {
    const home = tempDir('bk3-la2-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    for (const l of OLD_LABELS) fs.writeFileSync(plistPath(home, l), '<plist/>');
    const bin = path.join(tempDir('bk3-bin-'), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    autostart('install', { home, write: true, shell: true, shellBin: bin, run: () => assert.fail('launchctl was reached') });
    autostart('uninstall', { home, write: true, shell: true, shellBin: bin, run: () => assert.fail('launchctl was reached') });
  });
});

describe('the old LaunchAgent labels of a real install', () => {
  test('LaunchAgents named after the old hub (local.<hub>.pasture.brain / .shell, the labels pasture wrote when it lived inside a hub) are found, replaced and removed by migrate and by autostart install', () => {
    const w = world({ shellAgent: false });
    for (const l of OLD_LABELS) fs.rmSync(plistPath(w.home, l), { force: true });
    const hubLabels = ['local.example-hub.pasture.brain', 'local.example-hub.pasture.shell'];
    for (const l of hubLabels) fs.writeFileSync(plistPath(w.home, l), '<plist/>');
    migrate({ ...w.opts, write: true });
    for (const l of hubLabels) assert.ok(!fs.existsSync(plistPath(w.home, l)), `${l} was left installed: the old brain keeps the port and answers only X-Pasture-Token, so every hook gets a 403, while bellwether's brain crash-loops on EADDRINUSE`);
    assert.ok(w.calls.some((c) => /bootout .*example-hub\.pasture\.brain/.test(c)), 'the old brain was never booted out');
  });
});
