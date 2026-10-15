import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { describe, test } from 'node:test';
import { OLD_LABELS, LABEL, SHELL_LABEL, autostart, plistPath } from '../brain/autostart.mjs';
import { startBrain } from '../brain/brain.mjs';
import { withDefaults } from '../brain/defaults.mjs';
import { adoptLegacyEnv } from '../brain/envpath.mjs';
import { adoptLegacyHeaders } from '../brain/normalise.mjs';
import { tokenInit } from '../brain/token.mjs';
import { AGENT_DIR, IMPL, TOKEN, notUnderRust, request, spawnBrainSync, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { checkSettings, countGroups, hookBlock, install, isLegacyHandler, maskToken, runDoctor, uninstall } from './impl.mjs';

// What the app doesn't carry over from the rename (plan, Configuration; test/DIVERGENCES.md).
const PASTURE_NAMES = notUnderRust('the app reads no PASTURE_* names and no .env (test/DIVERGENCES.md 6 and 15); `migrate` imports a legacy token');
const PASTURE_HEADERS = notUnderRust('the Rust brain refuses X-Pasture-* headers (test/DIVERGENCES.md 4; crates/brain hook.rs `the_guards_in_order`)');
const REPO_AGENTS = notUnderRust("the repository's LaunchAgent autostart; the app's `autostart` drives SMAppService (milestone 6), and the harness never runs it on this Mac (plan deviation 22)");

/**
 * The rename from pasture to bellwether: for one release the old names (PASTURE_*
 * variables, X-Pasture-* headers, hook entries pasture wrote, its LaunchAgents and its
 * `pasture` command) still work or are cleaned up. Temp directories and HOMEs only;
 * launchctl is never run.
 */

/** A hook block exactly as pasture wrote it: the same URL, X-Pasture-* headers. */
const pastureBlock = (port, token) => JSON.parse(JSON.stringify(hookBlock(port, token)).replaceAll('X-Bellwether-', 'X-Pasture-'));
const never = () => assert.fail('launchctl must not run under a temp HOME');
const sum = (c) => Object.values(c).reduce((a, b) => a + b, 0);
const freePort = async () => {
  const s = http.createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const port = s.address().port;
  await new Promise((r) => s.close(r));
  return port;
};

describe('old PASTURE_* variables', () => {
  test('fill the BELLWETHER_* of the same name; a set new one (even "") wins; the old names used are remembered', PASTURE_NAMES, () => {
    const env = { PASTURE_HOOK_TOKEN: 'old', PASTURE_DATA_DIR: '/d', BELLWETHER_ENV: '', PASTURE_ENV: '/x.env', OTHER: '1' };
    const used = adoptLegacyEnv(env, new Set());
    assert.equal(env.BELLWETHER_HOOK_TOKEN, 'old');
    assert.equal(env.BELLWETHER_DATA_DIR, '/d');
    assert.equal(env.BELLWETHER_ENV, '', 'the new name was set, so the old one is ignored');
    assert.deepEqual([...used].sort(), ['PASTURE_DATA_DIR', 'PASTURE_HOOK_TOKEN']);
  });
  test('the CLI takes an exported PASTURE_HOOK_TOKEN, and one in a .env that only has the old name', PASTURE_NAMES, () => {
    const dir = tempDir('rn-env-');
    const run = (env) => {
      const out = path.join(dir, `h-${Math.random().toString(36).slice(2)}.json`);
      const e = { PATH: process.env.PATH, HOME: dir, BELLWETHER_DATA_DIR: path.join(dir, 'd'), BELLWETHER_ENV: path.join(dir, 'none.env'), ...env };
      for (const k of Object.keys(e)) if (e[k] === undefined) delete e[k];
      const r = spawnBrainSync(['hooks', 'print', '--out', out], { encoding: 'utf8', env: e });
      assert.equal(r.status, 0, r.stderr);
      return fs.readFileSync(out, 'utf8');
    };
    assert.ok(run({ PASTURE_HOOK_TOKEN: 'exportedOldName_0123456789ab' }).includes('exportedOldName_0123456789ab'));
    const f = path.join(dir, 'old.env');
    fs.writeFileSync(f, 'PASTURE_HOOK_TOKEN=fileOldName_0123456789abcd\n');
    assert.ok(run({ BELLWETHER_ENV: f }).includes('fileOldName_0123456789abcd'));
    assert.ok(run({ BELLWETHER_ENV: undefined, PASTURE_ENV: f }).includes('fileOldName_0123456789abcd'), 'PASTURE_ENV is read too');
  });
  test('doctor says which old names it read, as INFO', PASTURE_NAMES, async () => {
    const port = await freePort();
    const settings = path.join(tempDir('rn-doc-'), 'settings.json');
    fs.writeFileSync(settings, '{}');
    const r = await runDoctor({ env: { PASTURE_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home: tempDir('rn-home-'), dataDir: tempDir('rn-data-') });
    const c = r.checks.find((x) => x.name === 'old names');
    assert.equal(c?.status, 'INFO');
    assert.match(c.detail, /PASTURE_HOOK_TOKEN -> BELLWETHER_HOOK_TOKEN/);
    assert.equal(r.checks.find((x) => x.name === 'token').status, 'PASS');
    const fresh = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home: tempDir('rn-home-'), dataDir: tempDir('rn-data-') });
    assert.ok(!fresh.checks.some((x) => x.name === 'old names'));
  });
  test('token init on a .env with only PASTURE_HOOK_TOKEN adds the same value under the new name and keeps the old line', PASTURE_NAMES, () => {
    const f = path.join(tempDir('rn-tok-'), '.env');
    fs.writeFileSync(f, 'OTHER=1\nPASTURE_HOOK_TOKEN=keepThisValue_0123456789\n');
    const r = tokenInit(f, { generate: () => assert.fail('must reuse the old value') });
    assert.equal(r.changed, true);
    assert.match(r.message, /same value as PASTURE_HOOK_TOKEN/);
    assert.doesNotMatch(r.message, /keepThisValue/);
    assert.equal(fs.readFileSync(f, 'utf8'), 'OTHER=1\nPASTURE_HOOK_TOKEN=keepThisValue_0123456789\nBELLWETHER_HOOK_TOKEN=keepThisValue_0123456789\n');
  });
  test('an exported PASTURE_HOOK_TOKEN alone: the repository CLI adopts it for one release; the app ignores it (no token, exit 3)', () => {
    const dir = tempDir('rn-only-old-');
    const r = spawnBrainSync(['hooks', 'print'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: dir, BELLWETHER_DATA_DIR: path.join(dir, 'd'), BELLWETHER_ENV: path.join(dir, 'none.env'), PASTURE_HOOK_TOKEN: 'exportedOldName_0123456789ab' } });
    if (IMPL === 'rust') {
      assert.equal(r.status, 3, r.stderr);
      assert.match(r.stderr, /BELLWETHER_HOOK_TOKEN is not set/);
    } else {
      assert.equal(r.status, 0, r.stderr);
      assert.match(r.stdout, /"X-Bellwether-Token": "expo…"/);
    }
  });
  test('the token mask also hides values under the old header name', () => {
    assert.equal(maskToken('"X-Pasture-Token": "someOldToken_123456"', 'other-token-value', new Set()), '"X-Pasture-Token": "some…"');
  });
});

describe('old X-Pasture-* headers', () => {
  test('are read as the X-Bellwether-* of the same name, which wins when both are there', PASTURE_HEADERS, () => {
    const h = adoptLegacyHeaders({ 'x-pasture-token': 'a', 'x-pasture-herdr-pane': 'w1:p1', 'x-bellwether-herdr-pane': 'w2:p2', host: 'h' });
    assert.deepEqual(h, { 'x-bellwether-token': 'a', 'x-bellwether-herdr-pane': 'w2:p2', host: 'h' });
  });
  test('a running brain accepts a hook sent by an entry pasture wrote, and still refuses a wrong old token', PASTURE_HEADERS, async () => {
    const brain = await startBrain({ config: withDefaults({ port: 0 }), token: TOKEN, dataDir: tempDir('rn-brain-') });
    try {
      const body = JSON.stringify({ hook_event_name: 'UserPromptSubmit', session_id: 'aaaaaaaa-1111-4222-8333-000000000001', cwd: '/Users/example/projects/meadow' });
      const base = { Host: `127.0.0.1:${brain.port}`, 'Content-Type': 'application/json' };
      assert.equal((await request(brain.port, { headers: { ...base, 'X-Pasture-Token': TOKEN, 'X-Pasture-Herdr-Pane': 'w1:p1' }, body })).status, 204);
      assert.equal((await request(brain.port, { headers: { ...base, 'X-Pasture-Token': 'wrongTokenNotReal_0123456789' }, body })).status, 403);
    } finally {
      await brain.close();
    }
  });
});

describe('hook entries pasture wrote', () => {
  const port = 47417;
  const other = { type: 'command', command: 'afplay /Users/example/.claude/sounds/ready.wav' };
  const settings = () => {
    const s = pastureBlock(port, 'oldTokenNotReal_0123456789ab');
    s.hooks.Stop.unshift({ hooks: [other] });
    return s;
  };
  test('are recognised as bellwether\'s (exact URL plus the old header)', () => {
    const h = pastureBlock(port, 'x').hooks.Stop[0].hooks[0];
    assert.ok(isLegacyHandler(h, port));
    assert.ok(!isLegacyHandler(hookBlock(port, 'x').hooks.Stop[0].hooks[0], port), 'a current entry is not legacy');
    assert.ok(!isLegacyHandler(h, port + 1), 'another port is not ours');
  });
  test('`hooks install` replaces them with one bellwether entry per event, and leaves other tools alone', () => {
    const out = install(settings(), port, TOKEN);
    const text = JSON.stringify(out);
    assert.doesNotMatch(text, /X-Pasture-/);
    assert.equal(sum(countGroups(out, port)), 16);
    assert.deepEqual(out.hooks.Stop[0], { hooks: [other] });
  });
  test('`hooks uninstall` removes them', () => {
    const out = uninstall(settings(), port);
    assert.equal(sum(countGroups(out, port)), 0);
    assert.doesNotMatch(JSON.stringify(out), /X-Pasture-|127\.0\.0\.1/);
    assert.deepEqual(out.hooks, { Stop: [{ hooks: [other] }] });
  });
  test('doctor counts them as installed, with a WARN that names the fix, and checks their token', () => {
    const checks = checkSettings(pastureBlock(port, TOKEN), { port, token: TOKEN });
    assert.equal(checks.find((c) => c.name === 'hooks installed').status, 'PASS');
    assert.match(checks.find((c) => c.name === 'hooks from pasture').detail, /16 entries were written by pasture[\s\S]*hooks install --write/);
    assert.equal(checks.find((c) => c.name === 'hook token').status, 'PASS');
    assert.equal(checkSettings(pastureBlock(port, 'staleTokenNotReal_0123456789'), { port, token: TOKEN }).find((c) => c.name === 'hook token').status, 'FAIL');
    assert.ok(!checkSettings(hookBlock(port, TOKEN), { port, token: TOKEN }).some((c) => c.name === 'hooks from pasture'));
  });
});

describe('pasture\'s LaunchAgents', () => {
  const withOld = (labels = OLD_LABELS) => {
    const home = tempDir('rn-la-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    for (const l of labels) fs.writeFileSync(plistPath(home, l), '<plist/>');
    return home;
  };
  const bin = () => {
    const f = path.join(tempDir('rn-bin-'), 'bellwether-shell');
    fs.writeFileSync(f, '');
    return f;
  };
  test('the labels are local.bellwether.*', REPO_AGENTS, () => {
    assert.equal(LABEL, 'local.bellwether.brain');
    assert.equal(SHELL_LABEL, 'local.bellwether.shell');
    assert.deepEqual(OLD_LABELS, ['local.pasture.brain', 'local.pasture.shell']);
  });
  test('`autostart install` previews removing them and changes nothing', REPO_AGENTS, () => {
    const home = withOld();
    const r = autostart('install', { home, run: never, uid: 501, realHome: () => false, shell: true, shellBin: bin(), platform: 'darwin' });
    assert.equal(r.wrote, false);
    const text = r.lines.join('\n');
    assert.match(text, /pasture's old LaunchAgent local\.pasture\.brain[\s\S]*would remove/);
    assert.match(text, /local\.pasture\.shell/);
    for (const l of OLD_LABELS) assert.ok(fs.existsSync(plistPath(home, l)));
    assert.ok(!fs.existsSync(plistPath(home, LABEL)));
  });
  test('with --write it removes them and writes bellwether\'s; launchctl is not called under a temp HOME', REPO_AGENTS, () => {
    const home = withOld();
    const r = autostart('install', { home, write: true, run: never, uid: 501, realHome: () => false, shell: true, shellBin: bin(), platform: 'darwin' });
    assert.equal(r.wrote, true);
    for (const l of OLD_LABELS) assert.ok(!fs.existsSync(plistPath(home, l)), l);
    assert.ok(fs.existsSync(plistPath(home, LABEL)) && fs.existsSync(plistPath(home, SHELL_LABEL)));
  });
  test('with the real HOME it boots the old ones out before loading the new ones (checked by injection)', REPO_AGENTS, () => {
    const home = withOld(['local.pasture.brain']);
    const calls = [];
    autostart('install', { home, write: true, uid: 501, realHome: () => true, run: (c, a) => (calls.push(a.join(' ')), { ok: true, out: '' }), platform: 'darwin' });
    assert.deepEqual(calls, ['bootout gui/501/local.pasture.brain', 'bootout gui/501/local.bellwether.brain', `bootstrap gui/501 ${plistPath(home, LABEL)}`]);
  });
  test('an unbuilt strip is refused before anything old is removed', REPO_AGENTS, () => {
    const home = withOld();
    assert.throws(() => autostart('install', { home, write: true, run: never, uid: 501, realHome: () => false, shell: true, shellBin: '/nonexistent/bellwether-shell', platform: 'darwin' }), /isn't built yet/);
    for (const l of OLD_LABELS) assert.ok(fs.existsSync(plistPath(home, l)), `${l} must still be there`);
  });
  test('status and doctor point them out', async () => {
    const home = withOld(['local.pasture.brain']);
    if (IMPL === 'node') assert.match(autostart('status', { home, run: never, uid: 501, realHome: () => false, platform: 'darwin' }).lines.join('\n'), /pasture's old LaunchAgent is still installed/);
    if (process.platform !== 'darwin') return;
    const port = await freePort();
    const settings = path.join(home, 'settings.json');
    fs.writeFileSync(settings, '{}');
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home, dataDir: tempDir('rn-data-'), isStripRunning: () => false });
    // The repository's doctor calls them "old LaunchAgents"; the app's, "legacy agents" (the
    // four labels of plan D5, its own repository's local.bellwether.* included).
    const name = IMPL === 'rust' ? 'legacy agents' : 'old LaunchAgents';
    assert.equal(r.checks.find((c) => c.name === name)?.status, 'WARN');
    assert.match(r.checks.find((c) => c.name === name)?.detail ?? '', /local\.pasture\.brain[\s\S]*bellwether migrate/);
  });
});

describe('the `pasture` command', () => {
  test('prints a deprecation note on stderr and forwards arguments and the exit code to `bellwether`', { skip: process.platform === 'win32', ...notUnderRust('the /bin/sh shim and pasture.cmd run the Node CLI; the app has neither (test/DIVERGENCES.md 6)') }, () => {
    const env = { PATH: process.env.PATH, HOME: tempDir('rn-shim-'), BELLWETHER_HOOK_TOKEN: TOKEN };
    const ok = spawnBrainSync(['help'], { shim: path.join(AGENT_DIR, 'pasture'),  encoding: 'utf8', env });
    assert.equal(ok.status, 0);
    assert.match(ok.stderr, /now `bellwether`/);
    assert.match(ok.stdout, /bellwether migrate/);
    const bad = spawnBrainSync(['bogus-command'], { shim: path.join(AGENT_DIR, 'pasture'),  encoding: 'utf8', env });
    assert.equal(bad.status, 1);
    assert.match(bad.stderr, /unknown command/);
  });
  test('pasture.cmd says the same and forwards to bellwether.cmd (CRLF)', () => {
    const cmd = fs.readFileSync(path.join(AGENT_DIR, 'pasture.cmd'), 'latin1');
    assert.ok(cmd.split('\n').filter(Boolean).every((l) => l.endsWith('\r')));
    assert.match(cmd, /call "%~dp0bellwether\.cmd" %\*/);
    assert.match(cmd, /exit \/b %ERRORLEVEL%/);
  });
});

describe('one place for the app identifier', () => {
  test('local.bellwether is in tauri.conf.json and in no other source file', () => {
    const conf = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'tauri.conf.json'), 'utf8'));
    assert.equal(conf.identifier, 'local.bellwether');
    const hits = [];
    const walk = (d) => {
      for (const e of fs.readdirSync(d, { withFileTypes: true })) {
        if (['target', 'gen', 'node_modules', '.git', 'data', 'test', 'docs'].includes(e.name)) continue;
        const p = path.join(d, e.name);
        if (e.isDirectory()) walk(p);
        else if (/\.(rs|mjs|js|json|toml)$/.test(e.name) && /["']local\.bellwether["']/.test(fs.readFileSync(p, 'utf8'))) hits.push(path.relative(AGENT_DIR, p));
      }
    };
    walk(AGENT_DIR);
    assert.deepEqual(hits, [path.join('shell', 'src-tauri', 'tauri.conf.json')]);
  });
});
