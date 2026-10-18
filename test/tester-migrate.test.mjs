import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, FIXTURES, spawnBrainSync, tempDir } from './helpers.mjs';
import { hookBlock, install, uninstall } from './impl.mjs';

/**
 * BR-3K's probes for the pasture -> bellwether rename and `migrate`. Temp directories
 * only: launchctl is never run, the real HOME, .env and settings are never read. The
 * repository-to-repository migration's in-process probes (the Node brain's, launchd
 * injected), the X-Pasture-* header mapping and the PASTURE_* variables went with the
 * Node brain in milestone 11 (test/DIVERGENCES.md 4, 6, 15, 17 and 25); the app's
 * `migrate --from` runs here through the binary.
 */

// pasture's LaunchAgents, as an old install has them.
const OLD_LABELS = ['local.pasture.brain', 'local.pasture.shell'];
const plistPath = (home, label) => path.join(home, 'Library', 'LaunchAgents', `${label}.plist`);

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

function world({ shellAgent = true, oldEnv, settings, port = PORT } = {}) {
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
  return { root, hub, old, home, repo, settingsFile };
}

const noTokens = (text, ...tokens) => tokens.every((t) => !String(text).includes(t));

describe('migrate preview: changes nothing and never shows a token', () => {
  test('the CLI preview names the old hub-labelled LaunchAgents (local.<hub>.pasture.*) and changes nothing anywhere', () => {
    const w = world({ shellAgent: false });
    for (const l of OLD_LABELS) fs.rmSync(plistPath(w.home, l), { force: true });
    const hubLabels = ['local.example-hub.pasture.brain', 'local.example-hub.pasture.shell'];
    for (const l of hubLabels) fs.writeFileSync(plistPath(w.home, l), '<plist/>');
    const before = snapshot(w.root);
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'd') };
    const r = spawnBrainSync(['migrate', '--from', w.old, '--settings', w.settingsFile], { root: w.repo, cwd: w.repo, encoding: 'utf8', env });
    assert.equal(r.status, 0, r.stderr);
    for (const l of hubLabels) assert.match(r.stdout, new RegExp(l.replace(/\./g, '\\.')));
    assert.match(r.stdout, /Preview only; nothing was changed/);
    assert.ok(noTokens(r.stdout + r.stderr, OLD, NEW));
    assert.deepEqual(snapshot(w.root), before);
  });
  test('the CLI under temp directories: preview prints no token on stdout or stderr, even for an unknown flag or a missing --from', () => {
    const w = world();
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'd'), BELLWETHER_HOOK_TOKEN: NEW };
    for (const args of [['migrate', '--from', w.old, '--settings', w.settingsFile], ['migrate', '--from', path.join(w.root, 'nope')], ['migrate', '--bogus'], ['migrate', '--env', path.join(w.root, 'nope.env')]]) {
      const r = spawnBrainSync([...args], { encoding: 'utf8', env });
      assert.ok(noTokens(r.stdout + r.stderr, OLD, NEW), args.join(' '));
    }
    assert.equal(fs.existsSync(path.join(w.repo, '.env')), false);
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
