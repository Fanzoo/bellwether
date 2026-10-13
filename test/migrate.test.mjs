import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { LABEL, OLD_LABELS, SHELL_LABEL, plistPath } from '../brain/autostart.mjs';
import { countGroups, hookBlock } from '../brain/hooks.mjs';
import { migrate, sourceOf } from '../brain/migrate.mjs';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * `bellwether migrate`: a pasture install laid out in temp directories (a hub with
 * agents/pasture, its .env, data/, a settings file and LaunchAgents under a temp HOME)
 * moved over to a bellwether checkout that is also a temp directory. launchctl is never
 * run; the real HOME, .env and settings are never read.
 */

const OLD_TOKEN = 'oldPastureTokenNotReal_0123456789';
const pastureBlock = (port, token) => JSON.parse(JSON.stringify(hookBlock(port, token)).replaceAll('X-Bellwether-', 'X-Pasture-'));
const never = () => assert.fail('launchctl must not run under a temp HOME');
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
  fs.writeFileSync(path.join(old, 'data', 'shell-state.json'), JSON.stringify({ hidden: false, labels: true }));
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
  const shellBin = path.join(repo, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
  fs.mkdirSync(path.dirname(shellBin), { recursive: true });
  fs.writeFileSync(shellBin, '');
  const opts = { from: old, home, settingsFile, agentDir: repo, dataDir: path.join(repo, 'data'), targetEnv: path.join(repo, '.env'), run: never, uid: 501, realHome: () => false, platform: 'darwin', shellBin };
  return { root, hub, old, home, repo, settingsFile, opts };
}

describe('bellwether migrate', () => {
  test('the preview names every step and changes nothing anywhere (old install, new checkout, HOME)', () => {
    const w = world();
    const before = snapshot(w.root);
    const r = migrate(w.opts);
    assert.deepEqual(snapshot(w.root), before);
    assert.equal(r.wrote, false);
    const text = r.lines.join('\n');
    for (const s of ['would copy PASTURE_HOOK_TOKEN', 'would copy', '2 session colour(s) and slot(s), and mute', 'would replace 16 pasture entries', "pasture's old LaunchAgent local.pasture.brain", 'would remove', 'Preview only']) assert.ok(text.includes(s), s);
    assert.ok(!text.includes(OLD_TOKEN), 'the token is never shown');
  });

  test('--write moves everything over: token, config, colours and slots, tray settings, hooks, LaunchAgents', () => {
    const w = world();
    const hubEnv = fs.readFileSync(path.join(w.hub, '.env'), 'utf8');
    const r = migrate({ ...w.opts, write: true });
    assert.ok(!r.lines.join('\n').includes(OLD_TOKEN));
    // 1. the same token under the new name, in the new checkout's .env; the old .env untouched
    assert.equal(fs.readFileSync(path.join(w.repo, '.env'), 'utf8'), `BELLWETHER_HOOK_TOKEN=${OLD_TOKEN}\n`);
    assert.equal(fs.readFileSync(path.join(w.hub, '.env'), 'utf8'), hubEnv);
    // 2. config.json
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(w.repo, 'config.json'), 'utf8')).pets, { tint: 'whole' });
    // 3. state: the same colours and slots, and mute
    const st = JSON.parse(fs.readFileSync(path.join(w.repo, 'data', 'state.json'), 'utf8'));
    assert.equal(st.muted, true);
    assert.equal(st.identities['herdr:w1:p1'].colourIndex, 2);
    assert.deepEqual(st.identities['herdr:w1:p1'].slots, { main: 3 });
    assert.equal(st.identities['herdr:w1:p2'].colourIndex, 5);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(w.repo, 'data', 'shell-state.json'), 'utf8')), { hidden: false, labels: true });
    // 4. hooks: bellwether's entries with the same token; the other tool's hook and statusLine kept; a backup
    const s = JSON.parse(fs.readFileSync(w.settingsFile, 'utf8'));
    assert.doesNotMatch(JSON.stringify(s), /X-Pasture-/);
    assert.equal(sum(countGroups(s, 47417)), 16);
    assert.equal(s.hooks.Stop[0].hooks[0].command, 'afplay /Users/example/.claude/sounds/ready.wav');
    assert.deepEqual(s.statusLine, { type: 'command', command: 'echo hi' });
    assert.equal(s.hooks.Stop.at(-1).hooks[0].headers['X-Bellwether-Token'], OLD_TOKEN);
    assert.equal(fs.readdirSync(path.join(w.repo, 'data', 'backups')).filter((f) => f.endsWith('.json')).length, 1, 'one settings backup');
    // the old plists are backed up before they are removed
    assert.deepEqual(fs.readdirSync(path.join(w.home, 'Library', 'Application Support', 'bellwether', 'launchagent-backups')).sort(), OLD_LABELS.map((l) => `${l}.plist`));
    // 5. LaunchAgents: pasture's gone, bellwether's written (not loaded: temp HOME)
    for (const l of OLD_LABELS) assert.ok(!fs.existsSync(plistPath(w.home, l)), l);
    assert.ok(fs.existsSync(plistPath(w.home, LABEL)) && fs.existsSync(plistPath(w.home, SHELL_LABEL)));
  });

  test('a second run has nothing left to do and changes nothing', () => {
    const w = world();
    migrate({ ...w.opts, write: true });
    const before = snapshot(w.root);
    const r = migrate({ ...w.opts, write: true });
    const text = r.lines.join('\n');
    assert.match(text, /already set/);
    assert.match(text, /no pasture entries/);
    assert.match(text, /no pasture LaunchAgents/);
    const after = snapshot(w.root);
    // only the brain plist may be rewritten (same bytes); nothing else changes
    assert.deepEqual(after, before);
  });

  test('--from can be the old data directory; existing files in the new checkout are kept unless --force', () => {
    const w = world({ config: false });
    fs.mkdirSync(path.join(w.repo, 'data'));
    fs.writeFileSync(path.join(w.repo, 'data', 'state.json'), JSON.stringify({ version: 1, muted: false, identities: {} }));
    assert.deepEqual(sourceOf(path.join(w.old, 'data')), { root: w.old, dataDir: path.join(w.old, 'data') });
    const r = migrate({ ...w.opts, from: path.join(w.old, 'data'), write: true });
    assert.match(r.lines.join('\n'), /state\.json: .* exists; kept \(--force replaces it\)/);
    assert.equal(JSON.parse(fs.readFileSync(path.join(w.repo, 'data', 'state.json'), 'utf8')).muted, false);
    migrate({ ...w.opts, from: path.join(w.old, 'data'), write: true, force: true });
    assert.equal(JSON.parse(fs.readFileSync(path.join(w.repo, 'data', 'state.json'), 'utf8')).muted, true);
  });

  test('a strip that pasture started at login but bellwether hasn\'t built: only the brain moves, pasture\'s strip agent stays, and it says how to finish', () => {
    const w = world();
    const r = migrate({ ...w.opts, shellBin: path.join(w.root, 'not-built'), write: true });
    assert.match(r.lines.join('\n'), /stays as it is: bellwether's strip isn't built yet, so only the brain moves over now/);
    assert.ok(fs.existsSync(plistPath(w.home, LABEL)));
    assert.ok(!fs.existsSync(plistPath(w.home, SHELL_LABEL)));
    assert.ok(!fs.existsSync(plistPath(w.home, OLD_LABELS[0])));
    assert.ok(fs.existsSync(plistPath(w.home, OLD_LABELS[1])), 'the old strip agent stays until there is a replacement');
  });

  test('no old token anywhere: hooks are left alone and it says what to do', () => {
    const w = world();
    fs.writeFileSync(path.join(w.hub, '.env'), 'OTHER_SECRET=keep-me\n');
    const before = fs.readFileSync(w.settingsFile, 'utf8');
    const r = migrate({ ...w.opts, write: true });
    const text = r.lines.join('\n');
    assert.match(text, /no PASTURE_HOOK_TOKEN found/);
    assert.match(text, /no token to write into the new ones/);
    assert.equal(fs.readFileSync(w.settingsFile, 'utf8'), before);
  });

  test('--env names the old .env explicitly; a --from that isn\'t an install is refused', () => {
    const w = world();
    const f = path.join(w.root, 'elsewhere.env');
    fs.writeFileSync(f, 'PASTURE_HOOK_TOKEN=explicitTokenNotReal_012345678\n');
    migrate({ ...w.opts, envFrom: f, write: true });
    assert.match(fs.readFileSync(path.join(w.repo, '.env'), 'utf8'), /^BELLWETHER_HOOK_TOKEN=explicitTokenNotReal_012345678$/m);
    assert.throws(() => sourceOf(path.join(w.root, 'nope')), /no such directory/);
    assert.throws(() => sourceOf(w.home), /doesn't look like a pasture install/);
  });

  test('the CLI: a preview by default, exit 0, the usage lists it, and unknown flags are refused', () => {
    const w = world();
    const before = snapshot(w.root);
    const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'cli-data'), BELLWETHER_ENV: path.join(w.root, 'cli.env') };
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

/** A plist the way an old pasture install wrote it: its label and what it runs. */
const plist = (label, args) => `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${label}</string>
  <key>ProgramArguments</key>
  <array>
${args.map((a) => `    <string>${a.replace(/&/g, '&amp;').replace(/</g, '&lt;')}</string>`).join('\n')}
  </array>
</dict>
</plist>
`;

describe('migrate with launchd (injected; the real HOME and launchctl are never used)', () => {
  // realHome is forced true and every launchctl / probe call goes to `run`; the port
  // holder is injected, so nothing here opens a socket to a real brain.
  // The process table: by default the login item's strip (parent launchd, the agent's
  // own command line) appears once launchd has loaded the strip agent.
  const real = (w, { run, holder = { state: 'free' }, procs = new Map(), starts = true, parents = new Map([[99, 1]]), commands = new Map([[99, `${w.opts.shellBin} --no-spawn-brain`]]), kill } = {}) => {
    const calls = [];
    const fake = run ?? ((c, a) => ({ ok: true, out: '' }));
    const loaded = () => calls.some((c) => c.startsWith('launchctl bootstrap') && c.includes('local.bellwether.shell'));
    const table = () => new Map([...procs, ...(starts && loaded() ? [[99, w.opts.shellBin]] : [])]);
    return { calls, opts: { ...w.opts, realHome: () => true, portHolder: () => holder, processes: table, parents: () => parents, commands: () => commands, kill, stripWaitMs: 0, run: (c, a) => (calls.push(c === 'launchctl' ? `launchctl ${a.join(' ')}` : `probe ${a.slice(1).join(' ')}`), fake(c, a)) } };
  };
  const hubAgents = (w, prefix = 'local.example-hub') => {
    for (const l of OLD_LABELS) fs.rmSync(plistPath(w.home, l), { force: true });
    const brain = path.join(w.home, 'Library', 'LaunchAgents', `${prefix}.pasture.brain.plist`);
    const shell = path.join(w.home, 'Library', 'LaunchAgents', `${prefix}.pasture.shell.plist`);
    fs.writeFileSync(brain, plist(`${prefix}.pasture.brain`, ['/opt/node/bin/node', path.join(w.old, 'brain', 'cli.mjs'), 'serve']));
    fs.writeFileSync(shell, plist(`${prefix}.pasture.shell`, [path.join(w.old, 'shell', 'src-tauri', 'target', 'release', 'pasture-shell'), '--no-spawn-brain']));
    return { brain, shell };
  };

  test('old agents are found by what they run: a hub-prefixed label, or any label that runs the --from install; a .pasture.brain name that runs something else is left alone', async () => {
    const { findOldAgents } = await import('../brain/autostart.mjs');
    const w = world({ shellAgent: false });
    fs.rmSync(plistPath(w.home, OLD_LABELS[0]));
    const { brain, shell } = hubAgents(w);
    const odd = path.join(w.home, 'Library', 'LaunchAgents', 'com.other.thing.plist');
    fs.writeFileSync(odd, plist('com.other.thing', ['/usr/bin/node', path.join(w.old, 'brain', 'cli.mjs'), 'serve']));
    const impostor = path.join(w.home, 'Library', 'LaunchAgents', 'com.someone.pasture.brain.plist');
    fs.writeFileSync(impostor, plist('com.someone.pasture.brain', ['/usr/local/bin/some-other-daemon']));
    const found = findOldAgents(w.home, { from: w.old });
    assert.deepEqual(found.map((a) => [a.label, a.kind]).sort(), [['com.other.thing', 'brain'], ['local.example-hub.pasture.brain', 'brain'], ['local.example-hub.pasture.shell', 'shell']]);
    assert.ok(!found.some((a) => a.file === impostor), 'a matching name alone, running something else, is not pasture');
    void brain;
    void shell;
  });

  test('the order: port check, old agents out, new ones in, the new brain answers, and only then the hooks', () => {
    const w = world();
    hubAgents(w);
    let settingsAtProbe = null;
    const r = real(w, { run: (c, a) => (c !== 'launchctl' && settingsAtProbe === null && (settingsAtProbe = fs.readFileSync(w.settingsFile, 'utf8')), { ok: true, out: 'the bellwether brain answers' }) });
    migrate({ ...r.opts, write: true });
    const i = (re) => r.calls.findIndex((c) => re.test(c));
    assert.ok(i(/bootout .*example-hub\.pasture\.brain/) < i(/bootstrap .*local\.bellwether\.brain/));
    assert.ok(i(/bootout .*example-hub\.pasture\.shell/) < i(/bootstrap .*local\.bellwether\.shell/));
    assert.ok(i(/bootstrap .*local\.bellwether\.shell/) < i(/^probe /), r.calls.join('\n'));
    assert.match(settingsAtProbe, /X-Pasture-Token/, 'the hooks were rewritten before the new brain answered');
    assert.doesNotMatch(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
    assert.ok(!r.calls.some((c) => c.includes(OLD_TOKEN)), 'the token is never passed on a command line');
  });

  test('the new brain not answering: the new agents go, the old ones come back from their backups, the hooks are untouched, and the message says what changed; a re-run then finishes', () => {
    const w = world();
    const { brain, shell } = hubAgents(w);
    const before = { brain: fs.readFileSync(brain, 'utf8'), shell: fs.readFileSync(shell, 'utf8'), settings: fs.readFileSync(w.settingsFile, 'utf8') };
    const r = real(w, { run: (c) => (c === 'launchctl' ? { ok: true, out: '' } : { ok: false, out: 'a pasture brain (pid 4321) still holds port 47417 after waiting' }) });
    let err;
    try {
      migrate({ ...r.opts, write: true });
    } catch (e) {
      err = e;
    }
    assert.ok(err);
    assert.match(err.message, /the new brain didn't come up: a pasture brain \(pid 4321\) still holds port 47417/);
    assert.match(err.message, /booted out and removed .*example-hub\.pasture\.brain\.plist \(backup: /);
    assert.match(err.message, /restored .*example-hub\.pasture\.brain\.plist and loaded it/);
    assert.match(err.message, /The hooks were not touched/);
    assert.ok(!err.message.includes(OLD_TOKEN));
    assert.equal(fs.readFileSync(brain, 'utf8'), before.brain);
    assert.equal(fs.readFileSync(shell, 'utf8'), before.shell);
    assert.equal(fs.readFileSync(w.settingsFile, 'utf8'), before.settings);
    assert.ok(!fs.existsSync(plistPath(w.home, LABEL)) && !fs.existsSync(plistPath(w.home, SHELL_LABEL)));
    assert.ok(r.calls.includes(`launchctl bootstrap gui/501 ${brain}`), 'the old brain agent was loaded again');
    // fixed: the same command finishes
    const again = real(w);
    assert.match(migrate({ ...again.opts, write: true }).lines.join('\n'), /Done/);
    assert.ok(!fs.existsSync(brain) && fs.existsSync(plistPath(w.home, LABEL)));
    assert.doesNotMatch(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
  });

  test('a port held by something else stops before any LaunchAgent or hook changes, naming the pid and executable (never its arguments), and signals nothing', () => {
    const w = world();
    const { brain } = hubAgents(w);
    const r = real(w, { holder: { state: 'other', pid: 777, command: 'python3 -m http.server 47417' } });
    assert.throws(() => migrate({ ...r.opts, write: true }), /port 47417 is held by pid 777 \(python3\), which isn't a bellwether or pasture brain[\s\S]*nothing was stopped/);
    assert.ok(fs.existsSync(brain));
    assert.match(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
    assert.ok(!r.calls.some((c) => /bootout|bootstrap|kill/.test(c)), r.calls.join('\n'));
  });

  test('a pasture brain on the port is accepted only if it is the old agent\'s (launchd\'s pid); one started by hand stops the run', () => {
    const w = world();
    hubAgents(w);
    const launchd = (pid) => (c, a) => (c === 'launchctl' && a[0] === 'print' && /pasture\.brain$/.test(a[1]) ? { ok: true, out: `state = running\n\tpid = ${pid}\n` } : { ok: true, out: '' });
    const byHand = real(w, { holder: { state: 'brain', name: 'pasture', pid: 999 }, run: launchd(555) });
    assert.throws(() => migrate({ ...byHand.opts, write: true }), /held by a pasture brain, pid 999, not started by these LaunchAgents/);
    const ours = real(w, { holder: { state: 'brain', name: 'pasture', pid: 555 }, run: launchd(555) });
    assert.match(migrate({ ...ours.opts, write: true }).lines.join('\n'), /Done/);
  });

  test('a strip started by hand stops the run before anything changes, says how, and is not signalled; the preview warns about it', () => {
    const w = world();
    const { brain } = hubAgents(w);
    const killed = [];
    const r = real(w, { procs: new Map([[4242, w.opts.shellBin.replace('/release/', '/debug/')]]), parents: new Map([[4242, 777]]), kill: (pid) => killed.push(pid) });
    assert.match(migrate({ ...r.opts }).lines.join('\n'), /a strip is running by hand \(pid 4242, .*\): quit it from the menu bar \(Quit Bellwether\) before --write/);
    assert.throws(() => migrate({ ...r.opts, write: true }), /a strip is already running \(pid 4242, .*\), not the login item's[\s\S]*Quit it from the menu bar \(Quit Bellwether\), then run this again/);
    assert.deepEqual(killed, []);
    assert.ok(fs.existsSync(brain), 'nothing was booted out');
    assert.ok(!r.calls.some((c) => /bootout|bootstrap/.test(c)), r.calls.join('\n'));
    assert.match(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
  });

  test('the strip this data dir started (shell.pid) is stopped first; after loading, the login item\'s strip is reported running, or not', () => {
    const w = world();
    hubAgents(w);
    fs.mkdirSync(w.opts.dataDir, { recursive: true });
    fs.writeFileSync(path.join(w.opts.dataDir, 'shell.pid'), '5150');
    const procs = new Map([[5150, w.opts.shellBin]]);
    const killed = [];
    const r = real(w, { procs, parents: new Map([[5150, 1], [99, 1]]), kill: (pid) => (killed.push(pid), procs.delete(pid)) });
    const text = migrate({ ...r.opts, write: true }).lines.join('\n');
    assert.deepEqual(killed, [5150]);
    assert.match(text, /stopped the strip started by `bellwether shell start` \(pid 5150\)/);
    assert.match(text, /the strip is running \(pid 99, started by its LaunchAgent\)/);
    const w2 = world();
    hubAgents(w2);
    const none = real(w2, { starts: false });
    assert.match(migrate({ ...none.opts, write: true }).lines.join('\n'), /the strip's LaunchAgent is loaded, but its strip is not running/);
  });

  test('a re-run with the agents already moved still waits for the brain before the hooks, and leaves them if none answers', () => {
    const w = world();
    fs.rmSync(plistPath(w.home, OLD_LABELS[0]));
    fs.rmSync(plistPath(w.home, OLD_LABELS[1]));
    const down = real(w, { run: () => ({ ok: false, out: 'nothing answered after waiting' }) });
    const text = migrate({ ...down.opts, write: true }).lines.join('\n');
    assert.match(text, /left as they are: nothing answered after waiting/);
    assert.match(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
    const up = real(w);
    migrate({ ...up.opts, write: true });
    assert.doesNotMatch(fs.readFileSync(w.settingsFile, 'utf8'), /X-Pasture-Token/);
  });
});

describe('which token wins', () => {
  test('an exported BELLWETHER_*, then BELLWETHER_* in the .env, then an exported PASTURE_*, then PASTURE_* in the .env', () => {
    const dir = tempDir('mig-prec-');
    const load = (envText, exported) => {
      const f = path.join(dir, `${Math.random().toString(36).slice(2)}.env`);
      fs.writeFileSync(f, envText);
      const r = spawnSync(process.execPath, ['--input-type=module', '-e', `await import(${JSON.stringify(path.join(AGENT_DIR, 'brain', 'envload.mjs'))}); process.stdout.write(process.env.BELLWETHER_HOOK_TOKEN ?? '')`], { encoding: 'utf8', env: { PATH: process.env.PATH, BELLWETHER_ENV: f, ...exported } });
      return r.stdout;
    };
    const both = 'PASTURE_HOOK_TOKEN=file_old\nBELLWETHER_HOOK_TOKEN=file_new\n';
    assert.equal(load(both, { BELLWETHER_HOOK_TOKEN: 'env_new', PASTURE_HOOK_TOKEN: 'env_old' }), 'env_new');
    assert.equal(load(both, { PASTURE_HOOK_TOKEN: 'env_old' }), 'file_new');
    assert.equal(load('PASTURE_HOOK_TOKEN=file_old\n', { PASTURE_HOOK_TOKEN: 'env_old' }), 'env_old');
    assert.equal(load('PASTURE_HOOK_TOKEN=file_old\n', {}), 'file_old');
  });
});

describe('cargo, for building the strip', () => {
  test('found on PATH, then in ~/.cargo/bin, then in $CARGO_HOME/bin; none: null', async () => {
    const { findCargo } = await import('../brain/cargo.mjs');
    const root = tempDir('cargo-');
    const mk = (dir) => {
      fs.mkdirSync(dir, { recursive: true });
      const f = path.join(dir, 'cargo');
      fs.writeFileSync(f, '#!/bin/sh\n', { mode: 0o755 });
      return f;
    };
    const home = path.join(root, 'home');
    const env = { PATH: path.join(root, 'empty'), CARGO_HOME: path.join(root, 'ch') };
    assert.equal(findCargo({ env, home, platform: 'darwin' }), null);
    const ch = mk(path.join(root, 'ch', 'bin'));
    assert.equal(findCargo({ env, home, platform: 'darwin' }), ch);
    const hb = mk(path.join(home, '.cargo', 'bin'));
    assert.equal(findCargo({ env, home, platform: 'darwin' }), hb);
    const onPath = mk(path.join(root, 'onpath'));
    assert.equal(findCargo({ env: { ...env, PATH: [path.join(root, 'empty'), path.join(root, 'onpath')].join(path.delimiter) }, home, platform: 'darwin' }), onPath);
  });
  test('`shell --dev` without cargo: one line with the fix, exit 1, no stack', { skip: fs.existsSync(path.join(path.dirname(process.execPath), 'cargo')) && 'cargo sits next to node here' }, () => {
    const home = tempDir('cargo-home-');
    const r = spawnBrainSync(['shell', '--dev'], { encoding: 'utf8', timeout: 20000, env: { PATH: path.dirname(process.execPath), HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'd'), BELLWETHER_ENV: path.join(home, 'none.env') } });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /^bellwether: cargo \(Rust\) wasn't found on PATH, in ~\/\.cargo\/bin or in \$CARGO_HOME\/bin: run `source ~\/\.cargo\/env`, or install rustup from https:\/\/rustup\.rs\n$/);
  });
  test('doctor shows cargo as INFO either way', async () => {
    const { runDoctor } = await import('../brain/doctor.mjs');
    const settings = path.join(tempDir('cargo-doc-'), 'settings.json');
    fs.writeFileSync(settings, '{}');
    const base = { env: { BELLWETHER_HOOK_TOKEN: 'cargoDoctorTokenNotReal_012345' }, settingsFile: settings, configLoader: () => ({ config: null, usingExample: false }) };
    const missing = (await runDoctor({ ...base, findCargoFor: () => null })).checks.find((c) => c.name === 'cargo');
    assert.equal(missing.status, 'INFO');
    assert.match(missing.detail, /not found; only needed to build the strip\. .*source ~\/\.cargo\/env/);
  });
});
