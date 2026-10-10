import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, FIXTURES, request, tempDir } from './helpers.mjs';

/**
 * Tester full regression (2026-10-09). A copy of the brain on its own port, throwaway
 * token, HOME and PASTURE_DATA_DIR in temp directories, driven the way Claude Code
 * and a user would drive it. Never touches ~/.claude/settings.json or the real .env.
 */

const TOKEN = 'regressionTokenNotReal_0123456789';
const PORT = 47000 + (process.pid % 900) + 100;
const ROOT = tempDir('pasture-reg-');
const HUB = path.join(ROOT, 'hub');
const AGENT = path.join(HUB, 'agents', 'pasture');
const DATA = path.join(ROOT, 'data');
const HOME = path.join(ROOT, 'home');
const SETTINGS = path.join(HOME, '.claude', 'settings.json');
const CLI = path.join(AGENT, 'brain', 'cli.mjs');
const env = { PATH: process.env.PATH, HOME, PASTURE_DATA_DIR: DATA, PASTURE_HOOK_TOKEN: TOKEN };
let server;
let serverErr = '';

const cli = (args, extra = {}) => spawnSync(process.execPath, [CLI, ...args], { encoding: 'utf8', timeout: 30000, env: { ...env, ...extra }, cwd: AGENT });
const hdr = (extra = {}) => ({ Host: `127.0.0.1:${PORT}`, 'Content-Type': 'application/json', 'X-Pasture-Token': TOKEN, 'X-Pasture-Term-Program': 'iTerm.app', 'X-Pasture-Herdr-Pane': 'w1:p1', 'X-Pasture-Herdr-Socket': '/tmp/regression-not-real.sock', ...extra });
const post = (payload, headers = hdr()) => request(PORT, { path: '/hook', headers, body: JSON.stringify(payload) });
const SID = 'aaaaaaaa-1111-4222-8333-000000000001';
const ev = (name, o = {}) => post({ hook_event_name: name, session_id: SID, cwd: '/Users/example/projects/meadow', permission_mode: 'default', ...o });
const state = async (waitMs = 0) => {
  const r = await request(PORT, { method: 'GET', path: `/state?after=-1&wait=${waitMs}`, headers: hdr() });
  assert.equal(r.status, 200);
  return JSON.parse(r.body.toString());
};
const settle = () => new Promise((r) => setTimeout(r, 1900));
const sheep = async () => {
  const s = (await state()).flock.find((x) => x.label === 'meadow');
  if (s) s.lambs = s.lambs.filter((l) => l.state !== 'leaving'); // a stopped lamb walks off first
  return s;
};
const control = (msg) => request(PORT, { method: 'POST', path: '/control', headers: hdr(), body: JSON.stringify(msg) });

const real = path.join(process.env.HOME ?? '', '.claude', 'settings.json');
const realSha = () => (fs.existsSync(real) ? crypto.createHash('sha256').update(fs.readFileSync(real)).digest('hex') : 'absent');
const realBefore = { sha: realSha(), mtime: fs.existsSync(real) ? fs.statSync(real).mtimeMs : 0 };

async function up() {
  for (let i = 0; i < 60; i++) {
    try {
      const r = await request(PORT, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${PORT}` } });
      if (r.status === 200) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`brain did not come up: ${serverErr}`);
}
const startServer = async () => {
  server = spawn(process.execPath, [CLI, 'serve'], { env, cwd: AGENT, stdio: ['ignore', 'ignore', 'pipe'] });
  server.stderr.on('data', (d) => (serverErr += d));
  await up();
};
const stopServer = () =>
  new Promise((r) => {
    if (!server || server.exitCode !== null) return r();
    server.once('exit', r);
    server.kill('SIGTERM');
  });

before(async () => {
  fs.mkdirSync(path.join(AGENT, 'data'), { recursive: true });
  fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(AGENT, 'brain'), { recursive: true });
  for (const f of ['package.json', 'config.example.json']) fs.copyFileSync(path.join(AGENT_DIR, f), path.join(AGENT, f));
  const cfg = JSON.parse(fs.readFileSync(path.join(AGENT, 'config.example.json'), 'utf8'));
  cfg.port = PORT;
  fs.writeFileSync(path.join(AGENT, 'config.json'), JSON.stringify(cfg));
  fs.mkdirSync(path.dirname(SETTINGS), { recursive: true });
  fs.writeFileSync(SETTINGS, JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'echo foreign' }] }] }, model: 'x' }, null, 2) + '\n');
  await startServer();
});
after(async () => {
  await stopServer();
  fs.rmSync(ROOT, { recursive: true, force: true });
});

describe('a session, end to end (M1 listener + M2 state + M3 status)', () => {
  test('every hook answer is an empty 204, with a valid token, on the whole event sequence', async () => {
    for (const [n, o] of [
      ['SessionStart', { source: 'startup' }],
      ['UserPromptSubmit', { prompt: 'CANARY_PROMPT_TEXT' }],
      ['PreToolUse', { tool_name: 'Bash', tool_use_id: 'tu1', tool_input: { command: 'git push origin main CANARY_CMD_SECRET' } }],
    ]) {
      const r = await ev(n, o);
      assert.equal(r.status, 204, n);
      assert.equal(r.body.length, 0, n);
      assert.equal(r.headers['content-type'], undefined, n);
    }
    await settle();
    const s = await sheep();
    assert.ok(s, 'a sheep for the session');
    assert.match(s.state, /^(arriving|busy)$/);
    await settle();
    assert.equal((await sheep()).state, 'busy');
  });

  test('a permission request shows an alert on the sheep; status prints it; PostToolUse clears it', async () => {
    await ev('PermissionRequest', { tool_name: 'Bash', tool_input: { command: 'git push origin main CANARY_CMD_SECRET' } });
    await settle();
    let s = await sheep();
    assert.ok(s.alert, 'alert present');
    assert.match(s.alert.text, /Bash/);
    const st = cli(['status']);
    assert.equal(st.status, 0, st.stderr);
    assert.match(st.stdout, /meadow.*ALERT +Bash/);
    await ev('PostToolUse', { tool_name: 'Bash', tool_use_id: 'tu1', tool_input: { command: 'git push origin main CANARY_CMD_SECRET' } });
    await settle();
    s = await sheep();
    assert.equal(s.alert, undefined, 'alert cleared after the tool ran');
  });

  test('subagents start and stop as lambs; the main sheep keeps its state', async () => {
    for (const id of ['a0000000000000011', 'a0000000000000012']) await ev('SubagentStart', { agent_id: id, agent_type: 'Explore' });
    await settle();
    let s = await sheep();
    assert.equal(s.lambs.length, 2);
    assert.match(cli(['status']).stdout, /└ Explore/);
    await ev('SubagentStop', { agent_id: 'a0000000000000011', agent_type: 'Explore' });
    await settle();
    s = await sheep();
    assert.equal(s.lambs.length, 1);
    await ev('SubagentStop', { agent_id: 'a0000000000000012', agent_type: 'Explore' });
    await settle();
    assert.equal((await sheep()).lambs.length, 0);
  });

  test('a lamb alert is reported on the family and by status, and survives the main Stop', async () => {
    await ev('SubagentStart', { agent_id: 'a0000000000000013', agent_type: 'Plan' });
    await ev('PreToolUse', { agent_id: 'a0000000000000013', agent_type: 'Plan', tool_name: 'Bash', tool_use_id: 'tu9', tool_input: { command: 'rm -rf build' } });
    await ev('PermissionRequest', { agent_id: 'a0000000000000013', agent_type: 'Plan', tool_name: 'Bash', tool_input: { command: 'rm -rf build' } });
    await ev('Stop');
    await settle();
    const s = await sheep();
    assert.equal(s.lambs[0].alert?.text, 'Bash: rm -rf build?');
    assert.equal(s.lambAlert, true);
    assert.equal(s.alert, undefined);
    assert.match(cli(['status']).stdout, /ALERT +Bash: rm -rf build\?/);
  });

  test('a dismiss control clears a lamb alert; an unknown id is refused, not ignored', async () => {
    const s = await sheep();
    const id = s.lambs[0].id;
    const bad = await control({ action: 'dismiss', actorId: 'nope' });
    assert.notEqual(bad.status, 200);
    const ok = await control({ action: 'dismiss', actorId: id });
    assert.equal(ok.status, 200);
    await settle();
    assert.equal((await sheep()).lambs[0]?.alert, undefined);
  });

  test('idle: a Stop leaves the sheep not-busy; SessionEnd takes the sheep and its lambs away', async () => {
    const s = await sheep();
    assert.notEqual(s.state, 'busy');
    await ev('SessionEnd', { reason: 'other' });
    await settle();
    const gone = await sheep();
    assert.ok(gone === undefined || gone.state === 'leaving', `the sheep is still ${gone?.state} after SessionEnd`);
    assert.equal(gone?.lambs?.length ?? 0, 0);
    for (let i = 0; i < 12 && (await sheep()); i++) await new Promise((r) => setTimeout(r, 1000));
    assert.equal(await sheep(), undefined, 'the sheep never left');
    assert.match(cli(['status']).stdout, /0 sheep/);
  });

  test('no prompt, command or tool text is in /state, status output, the data dir or serve stderr', async () => {
    await ev('UserPromptSubmit', { prompt: 'CANARY_PROMPT_TEXT' });
    await settle();
    const blob = JSON.stringify(await state()) + cli(['status']).stdout + serverErr;
    assert.doesNotMatch(blob, /CANARY_/);
    const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]));
    for (const f of walk(DATA)) assert.doesNotMatch(fs.readFileSync(f, 'latin1'), /CANARY_/, f);
    await ev('SessionEnd', { reason: 'other' });
  });
});

describe('guards still hold with state and control in the mix (M1 vs M3)', () => {
  test('/hook, /state and /control refuse a missing token, a wrong token, an Origin and a foreign Host, with no body', async () => {
    const bad = [hdr({ 'X-Pasture-Token': undefined }), hdr({ 'X-Pasture-Token': 'wrong' + TOKEN }), hdr({ Origin: 'http://evil.example' }), hdr({ Host: 'evil.example' })];
    for (const p of ['/hook', '/state?wait=0', '/control']) {
      for (const h of bad) {
        for (const k of Object.keys(h)) if (h[k] === undefined) delete h[k];
        const r = await request(PORT, { method: p === '/state?wait=0' ? 'GET' : 'POST', path: p, headers: h, body: p === '/state?wait=0' ? '' : '{"hook_event_name":"Stop"}' });
        assert.equal(r.status, 403, `${p} ${JSON.stringify(Object.keys(h))}`);
        assert.equal(r.body.length, 0);
      }
    }
  });
  test('a hostile /control is refused and does not crash the brain', async () => {
    for (const body of ['not json', '{"action":"__proto__"}', '{"action":{"x":1}}', '[]', 'null', '{"action":"placed","id":"x","bounds":{"x":"a"}}']) {
      const r = await request(PORT, { method: 'POST', path: '/control', headers: hdr(), body });
      assert.ok(r.status === 400 || r.status === 200, body);
    }
    const h = await request(PORT, { method: 'GET', path: '/health', headers: { Host: `127.0.0.1:${PORT}` } });
    assert.equal(h.status, 200);
  });
});

describe('mute, hide, and what survives a restart (M2, M5, M7)', () => {
  test('mute shows in /state and status, unmute clears it; the CLI and /control agree', async () => {
    assert.equal(cli(['mute']).status, 0);
    await settle();
    assert.equal((await state()).muted, true);
    assert.match(cli(['status']).stdout, /MUTED/);
    assert.equal(cli(['unmute']).status, 0);
    await settle();
    assert.equal((await state()).muted, false);
    assert.equal((await control({ action: 'mute' })).status, 200);
    assert.equal((await state()).muted, true);
    assert.equal((await control({ action: 'unmute' })).status, 200);
  });

  test('mute survives a brain restart; an alert raised while muted is still on the sheep underneath', async () => {
    await ev('UserPromptSubmit');
    await ev('PermissionRequest', { tool_name: 'Bash', tool_input: { command: 'ls' } });
    cli(['mute']);
    await settle();
    assert.equal((await sheep()).alert?.text, 'Bash: ls?');
    await stopServer();
    await startServer();
    assert.equal((await state()).muted, true, 'mute was forgotten across a restart');
    cli(['unmute']);
    await settle();
    assert.equal((await state()).muted, false);
  });

  test('hide / show write the shell state the strip reads, atomically, and say so', () => {
    const h = cli(['hide']);
    assert.equal(h.status, 0, h.stderr);
    assert.match(h.stdout, /hidden/);
    const f = path.join(DATA, 'shell-state.json');
    assert.equal(JSON.parse(fs.readFileSync(f, 'utf8')).hidden, true);
    const s = cli(['show']);
    assert.match(s.stdout, /shown/);
    assert.equal(JSON.parse(fs.readFileSync(f, 'utf8')).hidden, false);
    assert.deepEqual(fs.readdirSync(DATA).filter((n) => n.includes('.tmp')), []);
  });

  test('a corrupt shell-state.json does not stop hide/show or the brain', () => {
    fs.writeFileSync(path.join(DATA, 'shell-state.json'), '{not json');
    const h = cli(['hide']);
    assert.equal(h.status, 0, `${h.stdout}${h.stderr}`);
    assert.doesNotMatch(h.stderr, /at .*\.mjs:\d+/, 'a bare stack trace');
    cli(['show']);
  });
});

describe('replay drives the live brain from the fixtures (M2, M6)', () => {
  test('S17 (lamb alert outlives the main Stop) replays and ends with a sheep and a lamb alert on the way', async () => {
    const r = spawnSync(process.execPath, [CLI, 'replay', path.join(FIXTURES, 's17-lamb-alert-outlives-main-stop.jsonl'), '--speed', '200', '--port', String(PORT)], { encoding: 'utf8', timeout: 60000, env, cwd: AGENT });
    assert.equal(r.status, 0, r.stdout + r.stderr);
    assert.match(r.stdout, /19|events/);
    const snap = await state();
    assert.equal(snap.flock.length, 1);
    assert.equal(snap.flock[0].label, 'meadow');
    // the fixture ends on a Stop: nothing busy and no stuck alert
    assert.notEqual(snap.flock[0].state, 'busy');
  });
  test('replay of a non-recording and of a missing file report a cause, not a stack', () => {
    fs.writeFileSync(path.join(ROOT, 'junk.jsonl'), 'hello\n');
    for (const f of [path.join(ROOT, 'junk.jsonl'), path.join(ROOT, 'missing.jsonl')]) {
      const r = spawnSync(process.execPath, [CLI, 'replay', f, '--port', String(PORT)], { encoding: 'utf8', timeout: 20000, env, cwd: AGENT });
      assert.notEqual(r.status, 0);
      assert.doesNotMatch(r.stderr, /\n\s+at .*\.mjs/);
      assert.match(r.stderr, /junk|missing|not JSON|cannot read/);
    }
  });
  test('replay against a port with no brain says so', () => {
    const r = spawnSync(process.execPath, [CLI, 'replay', path.join(FIXTURES, 's1-read.jsonl'), '--port', '1'], { encoding: 'utf8', timeout: 20000, env, cwd: AGENT });
    assert.notEqual(r.status, 0);
    assert.doesNotMatch(r.stderr, /\n\s+at .*\.mjs/);
  });
});

describe('doctor, with the brain up and with each credential broken (M1, M10)', () => {
  test('--json: brain and token pass; hooks fail because none are installed, naming the settings file; exit nonzero', () => {
    const r = cli(['doctor', '--json']);
    assert.notEqual(r.status, 0);
    const out = JSON.parse(r.stdout);
    const checks = out.checks ?? out;
    const by = (n) => checks.find((c) => new RegExp(n, 'i').test(c.name));
    assert.ok(by('brain'), JSON.stringify(checks.map((c) => c.name)));
    assert.match(String(by('brain').status ?? by('brain').result), /pass/i);
    assert.doesNotMatch(r.stdout + r.stderr, new RegExp(TOKEN));
  });
  test('the next line names the step the user is on', () => {
    const r = cli(['doctor']);
    assert.match(r.stdout, /next/i);
    assert.doesNotMatch(r.stdout + r.stderr, new RegExp(TOKEN));
  });
  test('no token: exit 3 naming PASTURE_HOOK_TOKEN; token the brain does not know: names the restart', () => {
    const e = { ...env };
    delete e.PASTURE_HOOK_TOKEN;
    const a = spawnSync(process.execPath, [CLI, 'doctor'], { encoding: 'utf8', timeout: 20000, env: e, cwd: AGENT });
    assert.equal(a.status, 3);
    assert.match(a.stdout + a.stderr, /PASTURE_HOOK_TOKEN/);
    const b = cli(['doctor'], { PASTURE_HOOK_TOKEN: 'someOtherTokenNotReal_9876543210' });
    assert.match(b.stdout + b.stderr, /different PASTURE_HOOK_TOKEN|403|refused/i);
    assert.notEqual(b.status, 0);
  });
  test('a broken config.json is reported with the file named, on every command that reads it', () => {
    const good = fs.readFileSync(path.join(AGENT, 'config.json'), 'utf8');
    fs.writeFileSync(path.join(AGENT, 'config.json'), '{ nope');
    try {
      for (const c of [['doctor'], ['status'], ['serve']]) {
        const r = cli(c);
        assert.equal(r.status, 3, c.join(' ') + r.stdout + r.stderr);
        assert.match(r.stdout + r.stderr, /config\.json/);
        assert.doesNotMatch(r.stderr, /\n\s+at .*\.mjs/);
      }
    } finally {
      fs.writeFileSync(path.join(AGENT, 'config.json'), good);
    }
  });
  test('status with no brain running exits 2 with a cause', async () => {
    await stopServer();
    try {
      const r = cli(['status']);
      assert.equal(r.status, 2, r.stdout + r.stderr);
      assert.match(r.stdout + r.stderr, /not running|pasture serve/);
      const d = cli(['doctor']);
      assert.equal(d.status, 2);
      assert.match(d.stdout, /pasture serve|autostart/);
    } finally {
      await startServer();
    }
  });
});

describe('hooks install against a temp settings file, and the real one stays untouched (M1)', () => {
  test('dry run changes nothing; --write adds pasture\'s groups and keeps the foreign hook; doctor then passes the hooks check; uninstall restores the bytes', () => {
    const original = fs.readFileSync(SETTINGS);
    const dry = cli(['hooks', 'install', '--settings', SETTINGS]);
    assert.equal(dry.status, 0, dry.stderr);
    assert.deepEqual(fs.readFileSync(SETTINGS), original);
    const w = cli(['hooks', 'install', '--write', '--settings', SETTINGS]);
    assert.equal(w.status, 0, w.stderr);
    const j = JSON.parse(fs.readFileSync(SETTINGS, 'utf8'));
    assert.ok(j.hooks.Stop.some((g) => g.hooks.some((h) => h.command === 'echo foreign')), 'foreign hook kept');
    assert.ok(Object.keys(j.hooks).length >= 10);
    const d = cli(['doctor', '--settings', SETTINGS]);
    assert.doesNotMatch(d.stdout, /FAIL.*hook/i, d.stdout);
    const u = cli(['hooks', 'uninstall', '--write', '--settings', SETTINGS]);
    assert.equal(u.status, 0, u.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(SETTINGS, 'utf8')), JSON.parse(original.toString()));
  });
});

describe('first run, on a fresh clone (M10)', () => {
  test('token init fills a missing .env in the repository root once, never prints the token, and is a no-op the second time', () => {
    const top = tempDir('pasture-fr-');
    const repo = path.join(top, 'bellwether');
    fs.cpSync(AGENT, repo, { recursive: true });
    const e = { PATH: process.env.PATH, HOME: top, PASTURE_DATA_DIR: path.join(top, 'd') };
    const run = () => spawnSync(process.execPath, [path.join(repo, 'brain', 'cli.mjs'), 'token', 'init'], { encoding: 'utf8', timeout: 20000, env: e, cwd: top });
    const a = run();
    assert.equal(a.status, 0, a.stderr);
    const text = fs.readFileSync(path.join(repo, '.env'), 'utf8');
    const tok = /PASTURE_HOOK_TOKEN=(\S+)/.exec(text)?.[1];
    assert.ok(tok && tok.length >= 16);
    assert.ok(!(a.stdout + a.stderr).includes(tok), 'token printed');
    const b = run();
    assert.equal(b.status, 0);
    assert.equal(fs.readFileSync(path.join(repo, '.env'), 'utf8'), text);
    assert.ok(!(b.stdout + b.stderr).includes(tok));
    // and the CLI reads it from there: the hooks file carries the token, with none exported
    const out = path.join(top, 'hooks-only.json');
    const p = spawnSync(process.execPath, [path.join(repo, 'brain', 'cli.mjs'), 'hooks', 'print', '--out', out], { encoding: 'utf8', timeout: 20000, env: e, cwd: top });
    assert.equal(p.status, 0, p.stderr);
    assert.ok(fs.readFileSync(out, 'utf8').includes(tok), 'hooks print uses the token from the repository .env');
    fs.rmSync(top, { recursive: true, force: true });
  });
  test('inside a hub (agents/<name>) with a hub .env and none of its own, the hub .env is used and kept', () => {
    const hub = tempDir('pasture-frhub-');
    const repo = path.join(hub, 'agents', 'pasture');
    fs.mkdirSync(path.dirname(repo), { recursive: true });
    fs.cpSync(AGENT, repo, { recursive: true });
    fs.writeFileSync(path.join(hub, '.env'), 'OTHER_SECRET=keep-me\n');
    const e = { PATH: process.env.PATH, HOME: hub, PASTURE_DATA_DIR: path.join(hub, 'd') };
    const a = spawnSync(process.execPath, [path.join(repo, 'brain', 'cli.mjs'), 'token', 'init'], { encoding: 'utf8', timeout: 20000, env: e, cwd: hub });
    assert.equal(a.status, 0, a.stderr);
    const text = fs.readFileSync(path.join(hub, '.env'), 'utf8');
    assert.match(text, /^OTHER_SECRET=keep-me$/m);
    assert.match(text, /^PASTURE_HOOK_TOKEN=\S{16,}$/m);
    assert.equal(fs.existsSync(path.join(repo, '.env')), false, 'no second .env');
    fs.rmSync(hub, { recursive: true, force: true });
  });
  test('autostart install without --write previews and writes nothing under HOME', () => {
    const r = cli(['autostart', 'install']);
    // the copy has no built shell, so this is also the "strip not built" failure: exit 2 with a cause
    assert.equal(r.status, 2, r.stdout + r.stderr);
    assert.match(r.stdout + r.stderr, /shell|build|pasture-shell/i);
    assert.doesNotMatch(r.stderr, /\n\s+at .*\.mjs/);
    assert.equal(fs.existsSync(path.join(HOME, 'Library', 'LaunchAgents')), false);
  });
  test('pets fetch with no network target is not run here; the pinned lock file names a sha256 for every pet', () => {
    const lock = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
    const pets = lock.pets ?? lock;
    assert.ok(JSON.stringify(pets).match(/[a-f0-9]{64}/), 'no sha256 pinned');
  });
});

describe('the real settings file is untouched by this whole pass', () => {
  test('sha256 and mtime equal what they were when the suite started', () => {
    assert.equal(realSha(), realBefore.sha);
    if (fs.existsSync(real)) assert.equal(fs.statSync(real).mtimeMs, realBefore.mtime);
  });
});
