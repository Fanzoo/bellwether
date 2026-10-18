import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, TOKEN, spawnBrainSync, tempDir } from './helpers.mjs';
import { checkSettings, countGroups, hookBlock, install, isLegacyHandler, maskToken, runDoctor, uninstall } from './impl.mjs';

// What the app doesn't carry over from the rename (plan, Configuration; test/DIVERGENCES.md
// 4, 6 and 15): PASTURE_* variables, X-Pasture-* headers (the Rust brain refuses them:
// crates/brain hook.rs `the_guards_in_order`), the repository's LaunchAgents and the
// `pasture` shim. Their Node tests went with the Node brain in milestone 11.

/**
 * The rename from pasture to bellwether: hook entries pasture wrote are recognised and
 * cleaned up, its LaunchAgents are pointed out, its old names are no longer read. Temp
 * directories and HOMEs only; launchctl is never run.
 */

/** A hook block exactly as pasture wrote it: the same URL, X-Pasture-* headers. */
const pastureBlock = (port, token) => JSON.parse(JSON.stringify(hookBlock(port, token)).replaceAll('X-Bellwether-', 'X-Pasture-'));
const sum = (c) => Object.values(c).reduce((a, b) => a + b, 0);
const freePort = async () => {
  const s = http.createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const port = s.address().port;
  await new Promise((r) => s.close(r));
  return port;
};

describe('old PASTURE_* variables', () => {
  test('an exported PASTURE_HOOK_TOKEN alone is ignored: no token, exit 3', () => {
    const dir = tempDir('rn-only-old-');
    const r = spawnBrainSync(['hooks', 'print'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: dir, BELLWETHER_DATA_DIR: path.join(dir, 'd'), PASTURE_HOOK_TOKEN: 'exportedOldName_0123456789ab' } });
    assert.equal(r.status, 3, r.stderr);
    assert.match(r.stderr, /BELLWETHER_HOOK_TOKEN is not set/);
  });
  test('the token mask also hides values under the old header name', () => {
    assert.equal(maskToken('"X-Pasture-Token": "someOldToken_123456"', 'other-token-value', new Set()), '"X-Pasture-Token": "some…"');
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
  // pasture's two labels (its brain and its strip), as the repository wrote them.
  const withOld = (labels = ['local.pasture.brain', 'local.pasture.shell']) => {
    const home = tempDir('rn-la-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    for (const l of labels) fs.writeFileSync(path.join(home, 'Library', 'LaunchAgents', `${l}.plist`), '<plist/>');
    return home;
  };
  test('status and doctor point them out', async () => {
    const home = withOld(['local.pasture.brain']);
    if (process.platform !== 'darwin') return;
    const port = await freePort();
    const settings = path.join(home, 'settings.json');
    fs.writeFileSync(settings, '{}');
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home, dataDir: tempDir('rn-data-'), isStripRunning: () => false });
    // The app's doctor calls them "legacy agents" (the four labels of plan D5, the
    // repository's local.bellwether.* included).
    const name = 'legacy agents';
    assert.equal(r.checks.find((c) => c.name === name)?.status, 'WARN');
    assert.match(r.checks.find((c) => c.name === name)?.detail ?? '', /local\.pasture\.brain[\s\S]*bellwether migrate/);
  });
  test("the app's doctor names every legacy agent, of both names, and never asks launchd under a temp HOME", { skip: process.platform !== 'darwin' && 'the app doctor on macOS' }, async () => {
    const home = withOld(['local.bellwether.brain', 'local.bellwether.shell', 'local.pasture.shell']);
    const settings = path.join(home, 'settings.json');
    fs.writeFileSync(settings, '{}');
    const port = await freePort();
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home, dataDir: tempDir('rn-data-'), isStripRunning: () => false });
    const line = r.checks.find((c) => c.name === 'legacy agents');
    assert.equal(line?.status, 'WARN');
    // Plists only: a temp HOME isn't the real home, so launchd (with this Mac's real agents) isn't asked.
    assert.match(line.detail, /^local\.bellwether\.brain \(plist present\), local\.bellwether\.shell \(plist present\), local\.pasture\.shell \(plist present\): /);
    assert.doesNotMatch(line.detail, /pid \d/);
    fs.rmSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    const none = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: settings, configLoader: () => ({ config: { port }, usingExample: false }), home, dataDir: tempDir('rn-data-'), isStripRunning: () => false });
    assert.equal(none.checks.find((c) => c.name === 'legacy agents'), undefined);
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
