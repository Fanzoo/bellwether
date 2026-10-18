import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';
import { countGroups, hookBlock } from './impl.mjs';

// The app's `migrate --from` moves a pasture install into its own files and never writes or
// boots out a LaunchAgent (plan D5; the agent swap is `migrate --to-app`, milestone 8). The
// repository-to-repository migration (the Node brain's, launchd injected) was retired with
// the Node brain in milestone 11 (test/DIVERGENCES.md 17 and 25).

// pasture's LaunchAgents, as an old install has them.
const OLD_LABELS = ['local.pasture.brain', 'local.pasture.shell'];
const plistPath = (home, label) => path.join(home, 'Library', 'LaunchAgents', `${label}.plist`);

/**
 * `bellwether migrate`: a pasture install laid out in temp directories (a hub with
 * agents/pasture, its .env, data/, a settings file and LaunchAgents under a temp HOME)
 * moved over to a bellwether checkout that is also a temp directory. launchctl is never
 * run; the real HOME, .env and settings are never read.
 */

const OLD_TOKEN = 'oldPastureTokenNotReal_0123456789';
const pastureBlock = (port, token) => JSON.parse(JSON.stringify(hookBlock(port, token)).replaceAll('X-Bellwether-', 'X-Pasture-'));
const sum = (c) => Object.values(c).reduce((a, b) => a + b, 0);

/** Every file under `dirs` with its sha256, to prove a preview changed nothing. */
function snapshot(...dirs) {
  const out = {};
  const walk = (d) => {
    if (!fs.existsSync(d)) return;
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else out[p] = crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
    }
  };
  for (const d of dirs) walk(d);
  return out;
}

function world({ shellAgent = true, config = true } = {}) {
  const root = tempDir('mig-');
  const hub = path.join(root, 'hub');
  const old = path.join(hub, 'agents', 'pasture');
  fs.mkdirSync(path.join(old, 'data'), { recursive: true });
  fs.mkdirSync(path.join(old, 'brain'));
  fs.writeFileSync(path.join(hub, '.env'), `OTHER_SECRET=keep-me\nPASTURE_HOOK_TOKEN=${OLD_TOKEN}\n`);
  if (config) fs.writeFileSync(path.join(old, 'config.json'), JSON.stringify({ port: 47417, pets: { tint: 'whole' } }));
  const now = Date.now();
  fs.writeFileSync(
    path.join(old, 'data', 'state.json'),
    JSON.stringify({ version: 1, muted: true, identities: { 'herdr:w1:p1': { colourIndex: 2, slots: { main: 3 }, label: 'meadow', lastSeen: now }, 'herdr:w1:p2': { colourIndex: 5, slots: { main: 0 }, label: 'barn', lastSeen: now } } }),
  );
  // As the strips write it (privacy.rs).
  fs.writeFileSync(path.join(old, 'data', 'shell-state.json'), JSON.stringify({ version: 1, hidden: false, showLabels: true }, null, 2));
  const home = path.join(root, 'home');
  fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
  fs.mkdirSync(path.join(home, '.claude'));
  for (const l of shellAgent ? OLD_LABELS : OLD_LABELS.slice(0, 1)) fs.writeFileSync(plistPath(home, l), '<plist/>');
  const settingsFile = path.join(home, '.claude', 'settings.json');
  const settings = pastureBlock(47417, OLD_TOKEN);
  settings.hooks.Stop.unshift({ hooks: [{ type: 'command', command: 'afplay /Users/example/.claude/sounds/ready.wav' }] });
  settings.statusLine = { type: 'command', command: 'echo hi' };
  fs.writeFileSync(settingsFile, JSON.stringify(settings, null, 2) + '\n');
  // the new checkout
  const repo = path.join(root, 'bellwether');
  fs.mkdirSync(repo);
  fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), path.join(repo, 'config.example.json'));
  return { root, hub, old, home, repo, settingsFile };
}

describe('bellwether migrate', () => {
  test('the CLI: a preview by default, exit 0, the usage lists it, and unknown flags are refused', () => {
    const w = world();
    const before = snapshot(w.root);
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'cli-data') };
    const cli = (...a) => spawnBrainSync(['migrate', ...a], { encoding: 'utf8', env, timeout: 20000 });
    const r = cli('--from', w.old, '--settings', w.settingsFile);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Preview only; nothing was changed/);
    assert.ok(!r.stdout.includes(OLD_TOKEN));
    assert.deepEqual(snapshot(w.root), before);
    assert.equal(cli('--from', w.old, '--writ').status, 3);
    assert.equal(cli('--from').status, 3, '--from needs a value');
  });
});

/** The CLI for a world: the binary with BELLWETHER_HOME = its data dir. */
function cliFor(w) {
  const data = path.join(w.root, 'cli-data');
  const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: data };
  const cli = (...a) => spawnBrainSync(['migrate', ...a], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
  // Where the app keeps the token it migrated.
  const token = () => (fs.existsSync(path.join(data, 'hook-token')) ? fs.readFileSync(path.join(data, 'hook-token'), 'utf8').trim() : null);
  return { cli, data, token };
}

describe('the CLI moves an install, step by step', () => {
  test('the preview names every step and changes nothing anywhere', () => {
    const w = world();
    const { cli } = cliFor(w);
    const before = snapshot(w.root);
    const r = cli('--from', w.old, '--settings', w.settingsFile);
    assert.equal(r.status, 0, r.stderr);
    for (const s of ['1. the hook token', '2. config.json', '3. colours and slots', '4. start at login (LaunchAgents)', '5. hook entries in', 'would copy', '2 session colour(s) and slot(s), and mute', 'would replace 16 pasture entries', 'local.pasture.brain', 'Preview only; nothing was changed']) assert.ok(r.stdout.includes(s), s);
    assert.ok(!(r.stdout + r.stderr).includes(OLD_TOKEN));
    assert.deepEqual(snapshot(w.root), before);
  });

  test('--from can be the old data directory; existing files in the target are kept unless --force', () => {
    const w = world({ config: false });
    const { cli, data } = cliFor(w);
    fs.mkdirSync(data, { recursive: true });
    fs.writeFileSync(path.join(data, 'state.json'), JSON.stringify({ version: 1, muted: false, identities: {} }));
    const r = cli('--from', path.join(w.old, 'data'), '--settings', w.settingsFile, '--write');
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /state\.json: .* exists; kept \(--force replaces it\)/);
    assert.equal(JSON.parse(fs.readFileSync(path.join(data, 'state.json'), 'utf8')).muted, false);
    assert.equal(cli('--from', path.join(w.old, 'data'), '--settings', w.settingsFile, '--write', '--force').status, 0);
    assert.equal(JSON.parse(fs.readFileSync(path.join(data, 'state.json'), 'utf8')).muted, true);
  });

  test('--env names the old .env; a corrupt state.json is a warning, not a stop; a --from that isn\'t an install is exit 3', () => {
    const w = world();
    const { cli, token } = cliFor(w);
    const f = path.join(w.root, 'elsewhere.env');
    fs.writeFileSync(f, 'PASTURE_HOOK_TOKEN=explicitTokenNotReal_012345678\n');
    fs.writeFileSync(path.join(w.old, 'data', 'state.json'), '{ not json');
    const r = cli('--from', w.old, '--env', f, '--settings', w.settingsFile, '--write');
    assert.equal(r.status, 0, r.stderr);
    assert.equal(token(), 'explicitTokenNotReal_012345678');
    assert.match(r.stdout, /warning: .*state\.json is not a valid state file/);
    assert.ok(!(r.stdout + r.stderr).includes('explicitTokenNotReal_012345678'), 'the token is never shown');
    assert.equal(cli('--from', w.home).status, 3, 'a HOME is not an install');
  });

  test('no token in any .env: the app takes the token the entries carry (plan D9)', () => {
    const w = world();
    const { cli, token } = cliFor(w);
    fs.writeFileSync(path.join(w.hub, '.env'), 'OTHER_SECRET=keep-me\n');
    const r = cli('--from', w.old, '--settings', w.settingsFile, '--write');
    assert.equal(r.status, 0, r.stderr);
    assert.ok(!(r.stdout + r.stderr).includes(OLD_TOKEN));
    assert.match(r.stdout, /copied the token of the hook entries/);
    assert.equal(token(), OLD_TOKEN);
    const s = JSON.parse(fs.readFileSync(w.settingsFile, 'utf8'));
    assert.equal(s.hooks.Stop.at(-1).hooks[0].headers['X-Bellwether-Token'], OLD_TOKEN, 'the same token, under the new header');
  });
});

describe('the CLI moves an install, end to end (temp directories only)', () => {
  // The maintainer's decision on the distribution plan's deviation 82: plain `migrate` used to
  // keep only `hidden` and `labels`, while the strips write `showLabels`, so Show labels was lost.
  test('Show labels and Hide everything survive, written the way the strip writes them (and an old `labels` key is read too)', () => {
    for (const [given, want] of [
      [{ version: 1, hidden: true, showLabels: true }, { hidden: true, showLabels: true }],
      [{ version: 1, hidden: false, showLabels: false }, { hidden: false, showLabels: false }],
      [{ hidden: false, labels: true }, { hidden: false, showLabels: true }],
    ]) {
      const w = world();
      fs.writeFileSync(path.join(w.old, 'data', 'shell-state.json'), JSON.stringify(given, null, 2));
      const data = path.join(w.root, 'cli-data');
      const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: data };
      const cli = (...a) => spawnBrainSync(['migrate', '--from', w.old, '--settings', w.settingsFile, ...a], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
      const p = cli();
      assert.equal(p.status, 0, p.stderr);
      assert.match(p.stdout, new RegExp(`shell-state\\.json: would copy ${JSON.stringify(want).replace(/[{}]/g, '\\$&')}`));
      const r = cli('--write');
      assert.equal(r.status, 0, r.stderr);
      // Byte for byte what `bellwether show|hide` and the strip's tray write (DIVERGENCES 24: no `onboarded` added).
      assert.equal(fs.readFileSync(path.join(data, 'shell-state.json'), 'utf8'), JSON.stringify({ version: 1, ...want }, null, 2), JSON.stringify(given));
    }
  });

  test('--write: the token, config, colours and slots, tray settings and hook entries move over, the token is never printed, and a second run changes nothing', () => {
    const w = world();
    // The app's target is its own files, BELLWETHER_HOME (here the data dir).
    const data = path.join(w.root, 'cli-data');
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: data };
    const cli = (...a) => spawnBrainSync(['migrate', '--from', w.old, '--settings', w.settingsFile, ...a], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
    const r = cli('--write');
    assert.equal(r.status, 0, r.stderr);
    assert.ok(!(r.stdout + r.stderr).includes(OLD_TOKEN), 'the token is never shown');
    // 1. the same token, in the app's token file
    assert.equal(fs.readFileSync(path.join(data, 'hook-token'), 'utf8'), `${OLD_TOKEN}\n`);
    assert.equal(fs.statSync(path.join(data, 'hook-token')).mode & 0o777, 0o600);
    // 2. config.json
    const config = path.join(data, 'config.json');
    assert.deepEqual(JSON.parse(fs.readFileSync(config, 'utf8')).pets, { tint: 'whole' });
    // 3. colours, slots, mute and the tray settings
    const st = JSON.parse(fs.readFileSync(path.join(data, 'state.json'), 'utf8'));
    assert.equal(st.muted, true);
    assert.equal(st.identities['herdr:w1:p1'].colourIndex, 2);
    assert.deepEqual(st.identities['herdr:w1:p2'].slots, { main: 0 });
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(data, 'shell-state.json'), 'utf8')), { version: 1, hidden: false, showLabels: true });
    // 4. hook entries: bellwether's, with the same token; the other tool's and statusLine kept; a backup
    const s = JSON.parse(fs.readFileSync(w.settingsFile, 'utf8'));
    assert.doesNotMatch(JSON.stringify(s), /X-Pasture-/);
    assert.equal(sum(countGroups(s, 47417)), 16);
    assert.equal(s.hooks.Stop[0].hooks[0].command, 'afplay /Users/example/.claude/sounds/ready.wav');
    assert.deepEqual(s.statusLine, { type: 'command', command: 'echo hi' });
    assert.equal(s.hooks.Stop.at(-1).hooks[0].headers['X-Bellwether-Token'], OLD_TOKEN);
    assert.equal(fs.readdirSync(path.join(data, 'backups')).filter((f) => f.endsWith('.json')).length, 1);
    // 5. LaunchAgents: the app names them and leaves them (plan D5; the swap is
    //    `migrate --to-app`, milestone 8)
    for (const l of OLD_LABELS) assert.ok(fs.existsSync(plistPath(w.home, l)), `${l} was touched`);
    assert.match(r.stdout, /local\.pasture\.brain[\s\S]*left as it is[\s\S]*migrate --to-app/);
    // A second run has nothing left to do.
    const before = snapshot(w.root);
    const again = cli('--write');
    assert.equal(again.status, 0, again.stderr);
    assert.match(again.stdout, /already set/);
    assert.match(again.stdout, /no pasture entries/);
    assert.deepEqual(snapshot(w.root), before);
  });

  test('a --from that is not an install stops it with exit 3; a settings file that is not JSON is left as it is; neither shows the token', () => {
    const w = world();
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'cli-data') };
    const cli = (...a) => spawnBrainSync(['migrate', ...a], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
    const nope = cli('--from', path.join(w.root, 'nope'), '--write');
    assert.equal(nope.status, 3);
    assert.match(nope.stderr, /no such directory/);
    fs.writeFileSync(w.settingsFile, `{"hooks": {"Stop": [{"hooks": [{"type": "http", "url": "http://127.0.0.1:47417/hook", "headers": {"X-Pasture-Token": "${OLD_TOKEN}" "oops": 1}}]}]}}`);
    const text = fs.readFileSync(w.settingsFile, 'utf8');
    const bad = cli('--from', w.old, '--settings', w.settingsFile, '--write');
    assert.ok(!(bad.stdout + bad.stderr).includes(OLD_TOKEN), 'the token is never shown');
    assert.equal(fs.readFileSync(w.settingsFile, 'utf8'), text, 'a settings file that is not JSON is never rewritten');
  });
});
