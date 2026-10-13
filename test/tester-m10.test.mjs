import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { SHELL_LABEL, LABEL, autostart, plistPath, shellPlistText, plistText } from '../brain/autostart.mjs';
import { AGENT_DIR, spawnBrain, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Tester probes for Milestone 10. Autostart runs only under temp HOMEs with a `run` that
 * fails the test if launchctl is ever reached; the doctor walk uses a temp copy of the
 * brain with its own port, data dir and HOME. Nothing touches ~/Library/LaunchAgents.
 */

const never = () => {
  throw new Error('launchctl was reached');
};
const lint = (text) => {
  const f = path.join(tempDir('plist-'), 'x.plist');
  fs.writeFileSync(f, text);
  return spawnSync('plutil', ['-lint', f], { encoding: 'utf8' });
};

describe('autostart: both LaunchAgents, temp HOME only', () => {
  const mkHome = () => {
    const h = tempDir('m10-home-');
    fs.mkdirSync(path.join(h, 'Library', 'LaunchAgents'), { recursive: true });
    return h;
  };
  const fakeBin = () => {
    const f = path.join(tempDir('m10-bin-'), 'bellwether-shell');
    fs.writeFileSync(f, '#!/bin/sh\n', { mode: 0o755 });
    return f;
  };
  test('install --write writes both plists that lint, carry no token, restart only on a crash, and never call launchctl', () => {
    process.env.BELLWETHER_HOOK_TOKEN = 'plistCanaryToken_0123456789abcdef';
    try {
      const home = mkHome();
      const bin = fakeBin();
      const prev = autostart('install', { home, shell: true, shellBin: bin, run: never, uid: 501 });
      assert.equal(prev.wrote, false);
      assert.deepEqual(fs.readdirSync(path.join(home, 'Library', 'LaunchAgents')), [], 'the preview wrote something');
      const w = autostart('install', { home, write: true, shell: true, shellBin: bin, run: never, uid: 501 });
      assert.equal(w.wrote, true);
      const files = [plistPath(home, LABEL), plistPath(home, SHELL_LABEL)];
      for (const f of files) {
        const t = fs.readFileSync(f, 'utf8');
        assert.equal(lint(t).status, 0, f);
        assert.doesNotMatch(t, /plistCanaryToken|BELLWETHER_HOOK_TOKEN/);
        assert.match(t, /<key>KeepAlive<\/key>\s*<dict>\s*<key>SuccessfulExit<\/key>\s*<false\/>/, 'KeepAlive must mean restart on failure only');
        assert.doesNotMatch(t, /<key>KeepAlive<\/key>\s*<true\/>/);
        assert.match(t, /<key>RunAtLoad<\/key>\s*<true\/>/);
      }
      const shellText = fs.readFileSync(files[1], 'utf8');
      assert.match(shellText, new RegExp(`<string>${bin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}</string>\\s*<string>--no-spawn-brain</string>`), 'the strip must not start a second brain');
      assert.match(shellText, /<key>ProcessType<\/key>\s*<string>Interactive<\/string>/);
      assert.match(w.lines.join('\n'), /launchctl not called/);
      const un = autostart('uninstall', { home, write: true, shell: true, shellBin: bin, run: never, uid: 501 });
      assert.equal(un.wrote, true);
      for (const f of files) assert.equal(fs.existsSync(f), false, `${f} survived uninstall`);
      // uninstall needs no built strip
      autostart('install', { home, write: true, shell: true, shellBin: bin, run: never, uid: 501 });
      autostart('uninstall', { home, write: true, shell: true, shellBin: '/nonexistent/bellwether-shell', run: never, uid: 501 });
      assert.deepEqual(fs.readdirSync(path.join(home, 'Library', 'LaunchAgents')), []);
    } finally {
      delete process.env.BELLWETHER_HOOK_TOKEN;
    }
  });
  test('install refuses, writing nothing, when the strip is not built; --brain-only (shell:false) still installs the brain', () => {
    const home = mkHome();
    assert.throws(() => autostart('install', { home, write: true, shell: true, shellBin: '/nonexistent/bellwether-shell', run: never, uid: 501 }), /isn't built yet/);
    assert.deepEqual(fs.readdirSync(path.join(home, 'Library', 'LaunchAgents')), [], 'a refused install wrote a plist');
    autostart('install', { home, write: true, run: never, uid: 501 });
    assert.deepEqual(fs.readdirSync(path.join(home, 'Library', 'LaunchAgents')), [`${LABEL}.plist`]);
  });
  test('a temp HOME, a symlinked temp HOME and a nonexistent HOME never reach launchctl (default guard, `run` throws if called)', () => {
    const home = mkHome();
    const link = path.join(tempDir('m10-link-'), 'h');
    fs.symlinkSync(home, link);
    for (const h of [home, link]) for (const action of ['install', 'status', 'uninstall']) autostart(action, { home: h, write: true, shell: true, shellBin: fakeBin(), run: never, uid: 501 });
    assert.doesNotThrow(() => autostart('status', { home: path.join(home, 'does', 'not', 'exist'), shell: true, shellBin: fakeBin(), run: never, uid: 501 }));
  });
  test('the real-home path calls exactly bootout then bootstrap for each agent, in that order (guard forced on, run injected)', () => {
    const home = mkHome();
    const calls = [];
    autostart('install', { home, write: true, shell: true, shellBin: fakeBin(), uid: 501, realHome: () => true, run: (c, a) => (calls.push([c, ...a].join(' ')), { ok: true, out: '' }) });
    assert.deepEqual(calls.map((c) => c.split(' ')[1]), ['bootout', 'bootstrap', 'bootout', 'bootstrap']);
    assert.ok(calls.every((c) => c.startsWith('launchctl ')));
    assert.match(calls[0], new RegExp(LABEL));
    assert.match(calls[2], new RegExp(SHELL_LABEL));
  });
  test('paths with XML metacharacters stay valid in the strip plist', () => {
    const t = shellPlistText({ shellBin: '/h/a&b <c>/bellwether-shell', agentDir: '/h/a&b <c>', dataDir: '/h/a&b <c>/data' });
    assert.equal(lint(t).status, 0);
    assert.doesNotMatch(t, /a&b <c>/);
    void plistText;
  });
  test('the CLI under a temp HOME: preview writes nothing, --write writes both, --brain-only writes one, uninstall removes them, bad flags exit 3', () => {
    const home = mkHome();
    // A temp copy of the checkout whose strip "build" is a stub file: installing the
    // strip's LaunchAgent only needs the binary to exist (it is never run from a temp
    // HOME), so this runs the same with or without a release build of the strip here.
    const agent = path.join(home, 'checkout');
    fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(agent, 'brain'), { recursive: true });
    fs.copyFileSync(path.join(AGENT_DIR, 'package.json'), path.join(agent, 'package.json'));
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    cfg.port = JSON.parse(fs.readFileSync(process.env.BELLWETHER_CONFIG, 'utf8')).port; // the suite's test port, never 47417
    fs.writeFileSync(path.join(agent, 'config.example.json'), JSON.stringify(cfg));
    const strip = path.join(agent, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
    fs.mkdirSync(path.dirname(strip), { recursive: true });
    fs.writeFileSync(strip, '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    const env = { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'data'), BELLWETHER_HOOK_TOKEN: 'cliCanaryToken_0123456789abcdef' };
    const run = (...a) => spawnBrainSync(['autostart', ...a], { root: agent, encoding: 'utf8', env, cwd: agent });
    const dir = path.join(home, 'Library', 'LaunchAgents');
    assert.equal(run('install').status, 0);
    assert.deepEqual(fs.readdirSync(dir), []);
    const w = run('install', '--write');
    assert.equal(w.status, 0, w.stderr);
    assert.equal(fs.readdirSync(dir).length, 2);
    assert.doesNotMatch(w.stdout + w.stderr, /cliCanaryToken/);
    assert.equal(run('uninstall', '--write').status, 0);
    assert.deepEqual(fs.readdirSync(dir), []);
    assert.equal(run('install', '--write', '--brain-only').status, 0);
    assert.equal(fs.readdirSync(dir).length, 1);
    run('uninstall', '--write', '--brain-only');
    for (const bad of [['install', '--writ'], ['install', '--brain'], ['bogus'], ['status', '--write'], ['install', 'extra']]) assert.equal(run(...bad).status, 3, bad.join(' '));
  });
});

describe('doctor `next`: a walk from nothing to "all set"', () => {
  let root, agent, home, dataDir, port, env, settings, brain;
  const freePort = () => new Promise((r) => { const s = net.createServer().listen(0, '127.0.0.1', () => { const { port: p } = s.address(); s.close(() => r(p)); }); });
  const cli = (args, extra = {}) => spawnBrainSync([...args], { root: agent, encoding: 'utf8', env: { ...env, ...extra }, cwd: agent, timeout: 30000 });
  const next = (extra) => {
    const r = cli(['doctor', '--json', '--settings', settings], extra);
    const j = JSON.parse(r.stdout);
    return { text: j.checks.find((c) => c.name === 'next')?.detail ?? '', checks: j.checks };
  };
  before(async () => {
    root = tempDir('m10-walk-');
    agent = path.join(root, 'agents', 'bellwether');
    home = path.join(root, 'home');
    dataDir = path.join(root, 'data');
    fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    fs.cpSync(path.join(AGENT_DIR, 'brain'), path.join(agent, 'brain'), { recursive: true });
    fs.copyFileSync(path.join(AGENT_DIR, 'package.json'), path.join(agent, 'package.json'));
    port = await freePort();
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    cfg.port = port;
    fs.writeFileSync(path.join(agent, 'config.example.json'), JSON.stringify(cfg));
    settings = path.join(home, '.claude', 'settings.json');
    env = { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: dataDir, BELLWETHER_HOOK_TOKEN: 'walkTokenNotReal_0123456789abc' };
  });
  after(() => {
    brain?.kill('SIGTERM');
    fs.rmSync(root, { recursive: true, force: true });
  });

  test('each state names exactly the right single step, in order, and hooks come last', async (t) => {
    const e = (v) => ({ BELLWETHER_HOOK_TOKEN: v });
    // 0 nothing: no token
    assert.match(next({ BELLWETHER_HOOK_TOKEN: '' }).text, /token/);
    // 1 token, no pet art
    assert.match(next().text, /pets fetch/);
    // 2 pet art present (the real, hash-pinned file when this machine has it), strip not built
    const realPet = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
    if (!fs.existsSync(realPet)) return t.skip('steps 2-7 need the pet art (bellwether pets fetch); steps 0-1 passed'); // the rest of the walk needs it
    fs.mkdirSync(path.join(dataDir, 'pets', 'esheep64'), { recursive: true });
    fs.copyFileSync(realPet, path.join(dataDir, 'pets', 'esheep64', 'animations.xml'));
    assert.match(next().text, /shell --dev --release/);
    // 3 strip built, no login items
    const bin = path.join(agent, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
    fs.mkdirSync(path.dirname(bin), { recursive: true });
    fs.writeFileSync(bin, '#!/bin/sh\n', { mode: 0o755 });
    assert.match(next().text, /autostart install/);
    assert.doesNotMatch(next().text, /hooks install/);
    // 4 only the brain's agent (--brain-only): the strip's is still missing
    assert.equal(cli(['autostart', 'install', '--write', '--brain-only']).status, 0);
    assert.match(next().text, /autostart install/);
    // 5 both agents, but no brain running: never "hooks install", even if hooks are already in the settings file
    assert.equal(cli(['autostart', 'install', '--write']).status, 0);
    let n = next().text;
    assert.match(n, /brain should be running from its LaunchAgent|start the brain/);
    assert.doesNotMatch(n, /hooks install/);
    assert.equal(cli(['hooks', 'install', '--write', '--settings', settings]).status, 0);
    n = next().text;
    assert.doesNotMatch(n, /all set|hooks install/, `with hooks installed but no brain, next said: ${n}`);
    // 6 hooks removed again, brain running: now the hooks step
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', settings]).status, 0);
    brain = spawnBrain(['serve'], { root: agent, env, cwd: agent, stdio: ['ignore', 'pipe', 'pipe'] });
    await new Promise((r) => { let s = ''; brain.stderr.on('data', (d) => { s += d; if (s.includes('listening on')) r(); }); });
    assert.match(next().text, /hooks install/);
    // 7 hooks installed: all set
    assert.equal(cli(['hooks', 'install', '--write', '--settings', settings]).status, 0);
    assert.match(next().text, /all set/);
    void e;
  });

  test('"all set" is not said while a doctor check is failing (a token mismatch, disableAllHooks, a bad timeout)', async (t) => {
    if (!brain) return t.skip('needs the brain the walk above starts, which needs the pet art');
    const s = JSON.parse(fs.readFileSync(settings, 'utf8'));
    s.disableAllHooks = true;
    fs.writeFileSync(settings, JSON.stringify(s, null, 2));
    let r = next();
    assert.ok(r.checks.some((c) => c.status === 'FAIL'), 'precondition: a FAIL line');
    assert.doesNotMatch(r.text, /all set/, `next said "${r.text}" with a FAIL line present`);
    delete s.disableAllHooks;
    s.hooks.Stop[0].hooks[0].headers['X-Bellwether-Token'] = 'someOtherTokenValue1234567890';
    fs.writeFileSync(settings, JSON.stringify(s, null, 2));
    r = next();
    assert.ok(r.checks.some((c) => c.status === 'FAIL'));
    assert.doesNotMatch(r.text, /all set/, `next said "${r.text}" with a hook-token FAIL`);
    const out = cli(['hooks', 'install', '--write', '--settings', settings]);
    assert.equal(out.status, 0);
  });

  test('another brain token (brain started with a different token) is not "all set"', async (t) => {
    if (!brain) return t.skip('needs the brain the walk above starts, which needs the pet art');
    const r = next({ BELLWETHER_HOOK_TOKEN: 'aDifferentTokenNotReal_0123456789' });
    assert.ok(r.checks.some((c) => c.status === 'FAIL'));
    assert.doesNotMatch(r.text, /all set/, `next said "${r.text}"`);
  });
});

describe('README "First run": every command exists, with the flags shown', () => {
  const readme = fs.readFileSync(path.join(AGENT_DIR, 'README.md'), 'utf8');
  const section = readme.slice(readme.indexOf('## First run'), readme.indexOf('\n## Setup'));
  const commands = [...section.matchAll(/\.\/bellwether ([a-z]+(?: [a-z]+)?(?: --[a-z-]+)*)/g)].map((m) => m[1]);
  test('the section names the expected commands', () => {
    for (const c of ['doctor', 'pets fetch', 'shell --dev --release', 'autostart install', 'autostart install --write', 'hooks install', 'hooks install --write', 'hooks uninstall --write', 'autostart uninstall --write']) assert.ok(commands.includes(c), `README does not mention ${c}`);
  });
  test('every one is accepted by the CLI (none is an unknown command or flag), run with a temp HOME and nothing that builds, serves or downloads', () => {
    const home = tempDir('m10-readme-');
    fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
    // BELLWETHER_ENV keeps `token init` off the repository's real .env.
    const env = { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'data'), BELLWETHER_HOOK_TOKEN: 'readmeTokenNotReal_0123456789ab', BELLWETHER_ENV: path.join(home, 'test.env') };
    for (const c of new Set(commands)) {
      if (/^(pets|shell)/.test(c)) continue; // downloads / builds
      const r = spawnBrainSync([...c.split(' ')], { encoding: 'utf8', env, timeout: 30000 });
      assert.doesNotMatch(r.stderr + r.stdout, /unknown (command|option)|unexpected argument|usage:/i, `${c}: ${r.stderr.slice(0, 160)}`);
    }
    // the two the sandbox skips: their flags are in the CLI's own usage text
    const usage = spawnBrainSync(['help'], { encoding: 'utf8' }).stdout;
    assert.match(usage, /pets fetch/);
    assert.match(usage, /shell \[--dev\] \[--release\]/);
  });
  test('step 1 is `bellwether token init` (no shell append), in First run and in Setup', () => {
    assert.match(section, /\.\/bellwether token init/);
    assert.doesNotMatch(section, />> \.env/);
    assert.doesNotMatch(readme.slice(readme.indexOf('## Setup'), readme.indexOf('### 2.')), />> \.env/);
  });
});

describe('previews', () => {
  test('the previews it tells the model to show do not contain the token', () => {
    const T = 'previewCanaryToken_0123456789ab';
    const home = tempDir('m10-prev-');
    fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
    fs.writeFileSync(path.join(home, '.claude', 'settings.json'), '{}\n');
    const env = { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'data'), BELLWETHER_HOOK_TOKEN: T };
    const r = spawnBrainSync(['hooks', 'install'], { encoding: 'utf8', env });
    assert.equal(r.status, 0);
    assert.ok(!(r.stdout + r.stderr).includes(T), 'the `hooks install` preview prints the token (16 times); previews are meant to be shown and pasted');
  });
});

describe('licensing documents', () => {
  const scripts = path.join(AGENT_DIR, 'scripts', 'third-party.mjs');
  test('--check passes here, and also on a machine with no cargo registry (a fresh clone before the first build)', () => {
    assert.equal(spawnSync(process.execPath, [scripts, '--check'], { encoding: 'utf8' }).status, 0);
    const r = spawnSync(process.execPath, [scripts, '--check'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: tempDir('m10-nocargo-'), CARGO_HOME: '/nonexistent' } });
    assert.equal(r.status, 0, `--check failed with no cargo registry: ${r.stdout}${r.stderr}`);
  });
  test('NOTICE names the pinned commit and says the art is not included; THIRD-PARTY.md lists the five MPL-2.0 crates', () => {
    const n = fs.readFileSync(path.join(AGENT_DIR, 'NOTICE'), 'utf8');
    assert.match(n, /1ba33bebed993439f03e502b37e9bd2daf061773/);
    assert.match(n, /NOT included|not included/i);
    const t = fs.readFileSync(path.join(AGENT_DIR, 'THIRD-PARTY.md'), 'utf8');
    for (const c of ['cssparser', 'selectors', 'option-ext']) assert.match(t, new RegExp(c));
    const pins = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
    assert.ok(JSON.stringify(pins).includes('0e0cc153ec5795dd3c37a7094d2eb912b95544a58fcfb1b9a203707ea96329cf'));
  });
});

describe('bellwether token init (temp files only; the real .env is never touched)', () => {
  const tokenInit = async (...a) => (await import('../brain/token.mjs')).tokenInit(...a);
  const mk = (text, mode) => {
    const d = tempDir('m10-tok-');
    const f = path.join(d, '.env');
    if (text !== null) fs.writeFileSync(f, text, mode ? { mode } : undefined);
    return { d, f };
  };
  const tokenOf = (f) => /^BELLWETHER_HOOK_TOKEN=(.*)$/m.exec(fs.readFileSync(f, 'utf8'))?.[1];
  const FIXED = () => 'initTokenFixedValue_0123456789abcdef';

  test('no file: created 0600 with the token; a second run changes nothing and prints no token', async () => {
    const { f } = mk(null);
    const r = await tokenInit(f, { generate: FIXED });
    assert.equal(r.changed, true);
    assert.ok(!r.message.includes(FIXED()));
    assert.equal(fs.statSync(f).mode & 0o777, 0o600);
    const again = await tokenInit(f, { generate: () => 'DIFFERENT_value_0123456789abcdefghij' });
    assert.equal(again.changed, false);
    assert.equal(tokenOf(f), FIXED());
  });
  test('an existing last line without a newline is kept apart; an existing mode is kept; other lines are untouched', async () => {
    const { f } = mk('A=1\nOTHER_KEY=abc', 0o640);
    fs.chmodSync(f, 0o640);
    await tokenInit(f, { generate: FIXED });
    assert.equal(fs.readFileSync(f, 'utf8'), `A=1\nOTHER_KEY=abc\nBELLWETHER_HOOK_TOKEN=${FIXED()}\n`);
    assert.equal(fs.statSync(f).mode & 0o777, 0o640);
  });
  test('already set in any form Node reads: bare, spaces, export, double and single quotes: left alone', async () => {
    for (const line of ['BELLWETHER_HOOK_TOKEN=abcdef0123456789abcdef', 'BELLWETHER_HOOK_TOKEN = abcdef0123456789abcdef', 'export BELLWETHER_HOOK_TOKEN=abcdef0123456789abcdef', 'BELLWETHER_HOOK_TOKEN="abcdef0123456789abcdef"', "BELLWETHER_HOOK_TOKEN='abcdef0123456789abcdef'", '  BELLWETHER_HOOK_TOKEN=abcdef0123456789abcdef']) {
      const { f } = mk(`X=1\n${line}\nY=2\n`);
      const before = fs.readFileSync(f, 'utf8');
      assert.equal((await tokenInit(f, { generate: FIXED })).changed, false, line);
      assert.equal(fs.readFileSync(f, 'utf8'), before);
    }
  });
  test('empty forms are filled in place (bare, quotes, export, spaces); a commented line is not mistaken for the key', async () => {
    for (const line of ['BELLWETHER_HOOK_TOKEN=', 'BELLWETHER_HOOK_TOKEN=""', "BELLWETHER_HOOK_TOKEN=''", 'BELLWETHER_HOOK_TOKEN=   ', 'export BELLWETHER_HOOK_TOKEN=']) {
      const { f } = mk(`X=1\n${line}\nY=2\n`);
      assert.equal((await tokenInit(f, { generate: FIXED })).changed, true, line);
      assert.equal(tokenOf(f), FIXED(), line);
      assert.match(fs.readFileSync(f, 'utf8'), /^X=1\n.*\nY=2\n$/s);
    }
    const { f } = mk('# BELLWETHER_HOOK_TOKEN=\nX=1\n');
    await tokenInit(f, { generate: FIXED });
    assert.equal(tokenOf(f), FIXED());
    assert.match(fs.readFileSync(f, 'utf8'), /^# BELLWETHER_HOOK_TOKEN=\n/);
  });
  test('an empty value followed by a comment (`BELLWETHER_HOOK_TOKEN= # fill me in`), which Node reads as empty, is filled', async () => {
    const { f } = mk('BELLWETHER_HOOK_TOKEN= # fill me in\n');
    const r = await tokenInit(f, { generate: FIXED });
    assert.equal(r.changed, true, 'Node reads this value as empty, but token init says it is already set');
  });
  test('CRLF files stay CRLF: a filled or appended line gets the same line ending', async () => {
    for (const text of ['A=1\r\nBELLWETHER_HOOK_TOKEN=\r\nB=2\r\n', 'A=1\r\nB=2\r\n']) {
      const { f } = mk(text);
      await tokenInit(f, { generate: FIXED });
      const out = fs.readFileSync(f, 'utf8');
      assert.ok(!/(?<!\r)\n/.test(out), `mixed line endings: ${JSON.stringify(out)}`);
    }
  });
  test('two BELLWETHER_HOOK_TOKEN lines: Node uses the LAST one, so after token init Node must read a non-empty token in both orders', async () => {
    for (const text of ['BELLWETHER_HOOK_TOKEN=\nBELLWETHER_HOOK_TOKEN=abcdef0123456789abcdef\n', 'BELLWETHER_HOOK_TOKEN=abcdef0123456789abcdef\nBELLWETHER_HOOK_TOKEN=\n']) {
      const { f } = mk(text);
      await tokenInit(f, { generate: FIXED });
      const r = spawnSync(process.execPath, [`--env-file=${f}`, '-e', 'process.stdout.write(process.env.BELLWETHER_HOOK_TOKEN ?? "")'], { encoding: 'utf8', env: { PATH: process.env.PATH } });
      assert.notEqual(r.stdout, '', `Node reads an empty token from ${JSON.stringify(text)} after token init said it was set`);
    }
  });
  test('a symlinked .env is written through and stays a symlink; a dangling one is created at its target', async () => {
    const { d } = mk(null);
    const target = path.join(d, 'real.env');
    fs.writeFileSync(target, 'A=1\n');
    const link = path.join(d, 'link.env');
    fs.symlinkSync(target, link);
    await tokenInit(link, { generate: FIXED });
    assert.ok(fs.lstatSync(link).isSymbolicLink());
    assert.match(fs.readFileSync(target, 'utf8'), /BELLWETHER_HOOK_TOKEN=initTokenFixed/);
    const dangling = path.join(d, 'dangling.env');
    fs.symlinkSync(path.join(d, 'new-target.env'), dangling);
    await tokenInit(dangling, { generate: FIXED });
    assert.ok(fs.existsSync(path.join(d, 'new-target.env')));
  });
  test('a read-only file or directory fails with a message that names the file, not a stack trace, and leaves the content', async () => {
    const { f } = mk('A=1\n');
    fs.chmodSync(f, 0o444);
    let err;
    try { await tokenInit(f, { generate: FIXED }); } catch (e) { err = e; }
    assert.equal(fs.readFileSync(f, 'utf8'), 'A=1\n');
    assert.ok(err, 'a read-only .env was reported as success');
    const d = tempDir('m10-tok-ro-');
    fs.chmodSync(d, 0o500);
    try {
      let e2;
      try { await tokenInit(path.join(d, '.env'), { generate: FIXED }); } catch (e) { e2 = e; }
      assert.ok(e2);
    } finally { fs.chmodSync(d, 0o700); }
    // and through the CLI wrapper: never a bare stack trace
    const home = tempDir('m10-tok-cli-');
    const r = spawnBrainSync(['token', 'bogus'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: home } });
    assert.equal(r.status, 3);
  });
  test('the write is not a truncate-then-write of the only copy of the .env\'s other secrets (a temp file and rename, or a backup)', () => {
    const src = fs.readFileSync(path.join(AGENT_DIR, 'brain', 'token.mjs'), 'utf8').replace(/\/\/.*$|\/\*[\s\S]*?\*\//gm, '');
    assert.match(src, /renameSync|copyFileSync|\.bak/, 'tokenInit rewrites the whole .env in place with writeFileSync: a crash or full disk mid-write loses every other account\'s tokens');
  });
});

describe('the token mask (stdout, stderr, serve log, doctor, status, replay, crafted settings)', () => {
  const OLD = 'oldTokenCanary_ZZZZZZZZZZ9876';
  const CUR = 'currentTokenCanary_0123456789AB';
  const run = (file, args) => {
    const home = tempDir('m10-mask-');
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    return spawnBrainSync([...args, '--settings', file], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'd'), BELLWETHER_HOOK_TOKEN: CUR } });
    void cfg;
  };
  const stale = (headerKey, extra = '') => {
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    const f = path.join(tempDir('m10-set-'), 's.json');
    fs.writeFileSync(f, JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'http', url: `http://127.0.0.1:${cfg.port}/hook`, timeout: 1, headers: { [headerKey]: OLD, ...(extra ? { [extra]: OLD } : {}) } }] }] } }, null, 2) + '\n');
    return f;
  };
  test('the current token is never in a preview, a doctor run or an error, and neither is an old token under the canonical header name', () => {
    for (const args of [['hooks', 'install'], ['hooks', 'uninstall'], ['doctor'], ['doctor', '--json']]) {
      const r = run(stale('X-Bellwether-Token'), args);
      assert.ok(!(r.stdout + r.stderr).includes(OLD) && !(r.stdout + r.stderr).includes(CUR), args.join(' '));
    }
    const bad = path.join(tempDir('m10-bad-'), 's.json');
    fs.writeFileSync(bad, `{"hooks":{"Stop":[{"hooks":[{"headers":{"X-Bellwether-Token":"${OLD}" "oops":1}}]}]}}`);
    const r = run(bad, ['hooks', 'install']);
    assert.ok(!(r.stdout + r.stderr).includes(OLD), 'a JSON error message carried the old token');
  });
  test('an old token under a differently-cased header name, or repeated in another header, is masked too', () => {
    for (const key of ['x-bellwether-token', 'X-BELLWETHER-TOKEN']) {
      const r = run(stale(key), ['hooks', 'install']);
      assert.ok(!(r.stdout + r.stderr).includes(OLD), `old token printed in the diff under ${key}`);
    }
    const r = run(stale('X-Bellwether-Token', 'X-Other'), ['hooks', 'install']);
    assert.ok(!(r.stdout + r.stderr).includes(OLD), 'an old token repeated under another header name was printed');
  });
});
