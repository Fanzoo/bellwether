import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { SHELL_LABEL, plistPath } from '../brain/autostart.mjs';
import { runDoctor } from '../brain/doctor.mjs';
import { shellControl } from '../brain/shellctl.mjs';
import { displayLabels, terminalTag, MAX_LABEL } from '../shell/src/player/labels.mjs';
import { AGENT_DIR, notUnderRust, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { DEFAULT_PALETTE, OLD_DEFAULT_PALETTE, withDefaults } from './impl.mjs';

// What the app replaces: the repository's strip control and its LaunchAgent lines in doctor.
const REPO_SHELL = notUnderRust("the repository's `shell start|stop|restart` (its LaunchAgent, or a detached copy of target/release/bellwether-shell); the app's `shell start|stop|restart` controls Bellwether.app's own strip (test/DIVERGENCES.md 19)");
const REPO_AGENTS = notUnderRust("doctor's lines for the repository's LaunchAgents (`autostart`, `strip running`); the app's doctor reports its login items (shell/src-tauri cli.rs `login_verdict`) and leftover LaunchAgents (`legacy agents`, rename.test.mjs)");

/**
 * Tester probes for `bellwether shell start|stop|restart`, the palette and the labels.
 * Fakes and temp HOMEs only: no real process is signalled, launchctl is never reachable,
 * and the user's live strip and LaunchAgents are not touched.
 */

const BIN = '/hub/agents/bellwether/shell/src-tauri/target/release/bellwether-shell';
const mkEnv = () => {
  const home = tempDir('sc-home-');
  fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
  const dataDir = tempDir('sc-data-');
  const calls = { kill: [], spawn: [], run: [] };
  const live = new Map();
  const o = {
    home,
    dataDir,
    uid: 501,
    shellBin: BIN,
    realHome: () => false,
    run: (c, a) => (calls.run.push([c, ...a]), { ok: true, out: '' }),
    processes: () => new Map(live),
    kill: (pid, sig) => {
      calls.kill.push([pid, sig]);
      live.delete(pid);
    },
    spawnDetached: (bin, args) => (calls.spawn.push([bin, ...args]), 7001),
    brainUp: async () => true,
    platform: 'darwin',
  };
  const addAgent = () => fs.writeFileSync(plistPath(home, SHELL_LABEL), '<plist/>');
  return { o, calls, live, addAgent, dataDir, home };
};
const withBinary = (e) => {
  const real = fs.existsSync(BIN);
  void real;
  return e;
};

describe('shell stop signals only the exact release binary (no LaunchAgent, fakes)', REPO_SHELL, () => {
  test('a stale pid file (no such process) and a pid file pointing at an unrelated process signal nothing', async () => {
    const e = withBinary(mkEnv());
    e.live.set(555, '/usr/bin/vim');
    for (const pid of ['999999', '555', 'garbage', '', '-1', '0', '1', '4294967296', '555abc']) {
      fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), pid);
      const r = await shellControl('stop', e.o);
      assert.match(r.lines.join('\n'), /not running/, pid);
    }
    assert.deepEqual(e.calls.kill, [], 'something was signalled');
  });
  test('pid reuse: the pid in this data dir\'s file now belongs to another program, so nothing is signalled; a strip held by another pid (not in the file) is never signalled either', async () => {
    const e = mkEnv();
    fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), '300');
    e.live.set(300, '/Applications/Safari.app/Contents/MacOS/Safari');
    e.live.set(777, BIN);
    const r = await shellControl('stop', e.o);
    assert.match(r.lines.join('\n'), /not running/);
    assert.deepEqual(e.calls.kill, [], 'a process that is not in this data dir\'s pid file was signalled');
  });
  test('a symlinked or look-alike binary path is never matched: only the exact path string', async () => {
    const e = mkEnv();
    for (const exe of ['/tmp/link-to-bellwether-shell', `${BIN}-evil`, `${BIN}/../bellwether-shell`, '/other/target/release/bellwether-shell', 'bellwether-shell', `${BIN} `]) e.live.set(100 + e.live.size, exe);
    fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), '100');
    await shellControl('stop', e.o);
    assert.deepEqual(e.calls.kill, []);
  });
  test('the pid file is removed after a stop, SIGTERM is the only signal, and a second stop says it was not running', async () => {
    const e = mkEnv();
    e.live.set(42, BIN);
    fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), '42');
    const r = await shellControl('stop', e.o);
    assert.match(r.lines[0], /stopped the strip/);
    assert.deepEqual(e.calls.kill, [[42, 'SIGTERM']]);
    assert.equal(fs.existsSync(path.join(e.dataDir, 'shell.pid')), false);
    assert.match((await shellControl('stop', e.o)).lines[0], /not running/);
  });
  test('under a temp HOME with no pid file of its own, `stop` must not signal the real strip that happens to run the same binary (the real-HOME guard covers launchctl only)', async () => {
    const e = mkEnv(); // realHome() === false: this is a temp HOME
    e.live.set(8123, BIN); // "the user's live strip": launched by their LaunchAgent, not by this data dir
    await shellControl('stop', e.o);
    assert.deepEqual(e.calls.kill, [], `a temp-HOME run signalled pid 8123, the user's live strip`);
  });
});

describe('start and restart', REPO_SHELL, () => {
  test('start passes --no-spawn-brain only when a brain answers; otherwise the strip may start its own', async () => {
    for (const [up, args] of [[true, ['--no-spawn-brain']], [false, []]]) {
      const e = mkEnv();
      const bin = path.join(tempDir('sc-bin-'), 'bellwether-shell');
      fs.writeFileSync(bin, '');
      e.o.shellBin = bin;
      e.o.brainUp = async () => up;
      await shellControl('start', e.o);
      assert.deepEqual(e.calls.spawn, [[bin, ...args]], `brain up: ${up}`);
      assert.equal(fs.readFileSync(path.join(e.dataDir, 'shell.pid'), 'utf8'), '7001');
    }
  });
  test('start refuses without a built binary (exit 2 message) and does not start twice', async () => {
    const e = mkEnv();
    e.o.shellBin = '/nonexistent/bellwether-shell';
    await assert.rejects(shellControl('start', e.o), /isn't built yet/);
    const bin = path.join(tempDir('sc-bin-'), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    e.o.shellBin = bin;
    e.live.set(61, bin);
    fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), '61');
    const r = await shellControl('start', e.o);
    assert.match(r.lines[0], /already running/);
    assert.deepEqual(e.calls.spawn, []);
    // a strip this data dir did not start (no pid file of its own) is not claimed, and not signalled
    const e2 = mkEnv();
    e2.o.shellBin = bin;
    e2.live.set(62, bin);
    await shellControl('start', e2.o);
    assert.deepEqual(e2.calls.kill, []);
  });
  test('restart with one running: SIGTERM then a new start, in that order', async () => {
    const e = mkEnv();
    const bin = path.join(tempDir('sc-bin-'), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    e.o.shellBin = bin;
    e.live.set(61, bin);
    fs.writeFileSync(path.join(e.dataDir, 'shell.pid'), '61');
    const r = await shellControl('restart', e.o);
    assert.deepEqual(e.calls.kill, [[61, 'SIGTERM']]);
    assert.equal(e.calls.spawn.length, 1);
    assert.match(r.lines[0], /restarted the strip/);
  });
});

describe('the launchctl guard', REPO_SHELL, () => {
  test('with the strip agent installed, a temp HOME, a symlinked temp HOME and a missing HOME only print what would run; launchctl is never called', async () => {
    for (const action of ['start', 'stop', 'restart']) {
      const e = mkEnv();
      e.addAgent();
      const link = path.join(tempDir('sc-link-'), 'h');
      fs.symlinkSync(e.home, link);
      for (const home of [e.home, link]) {
        const r = await shellControl(action, { ...e.o, home });
        assert.match(r.lines.join('\n'), /would run: launchctl/);
        assert.match(r.lines.join('\n'), /launchctl not called/);
      }
      assert.deepEqual(e.calls.run, [], `${action} called launchctl`);
      assert.deepEqual(e.calls.kill, []);
      assert.deepEqual(e.calls.spawn, []);
    }
    // the real default guard (not injected): temp HOMEs are not real
    const e = mkEnv();
    e.addAgent();
    const { realHome: _drop, ...rest } = e.o;
    void _drop;
    const r = await shellControl('stop', { ...rest, run: () => { throw new Error('launchctl was reached'); } });
    assert.match(r.lines.join('\n'), /not called/);
  });
  test('with the guard forced on, the exact commands and label are used, and nothing else', async () => {
    const e = mkEnv();
    e.addAgent();
    const o = { ...e.o, realHome: () => true };
    await shellControl('start', o);
    await shellControl('restart', o);
    await shellControl('stop', o);
    const target = `gui/501/${SHELL_LABEL}`;
    assert.deepEqual(e.calls.run, [['launchctl', 'kickstart', target], ['launchctl', 'kickstart', '-k', target], ['launchctl', 'kill', 'TERM', target]]);
    assert.deepEqual(e.calls.kill, []);
  });
  test('an unknown action is refused', async () => {
    await assert.rejects(shellControl('pause', mkEnv().o), /usage/);
  });
});

describe('doctor: strip running', REPO_AGENTS, () => {
  const run = (o) => {
    const home = tempDir('sc-doc-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    fs.writeFileSync(path.join(home, 'Library', 'LaunchAgents', 'local.bellwether.brain.plist'), '<plist/>');
    if (o.strip) fs.writeFileSync(plistPath(home, SHELL_LABEL), '<plist/>');
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    cfg.port = 47999;
    const settings = path.join(home, 's.json');
    fs.writeFileSync(settings, '{}\n');
    return runDoctor({ settingsFile: settings, env: { BELLWETHER_HOOK_TOKEN: 'doctorTokenNotReal_0123456789' }, configLoader: () => ({ config: cfg, usingExample: true }), dataDir: tempDir('sc-dd-'), home, isStripRunning: () => o.running, probeTimeoutMs: 300 });
  };
  test('the agent installed and no strip running: WARN naming `shell start`; running: no such WARN', async () => {
    if (process.platform !== 'darwin') return;
    const down = await run({ strip: true, running: false });
    assert.equal(down.checks.find((c) => c.name === 'strip running')?.status, 'WARN');
    assert.match(down.checks.find((c) => c.name === 'strip running').detail, /shell start/);
    const up = await run({ strip: true, running: true });
    assert.equal(up.checks.find((c) => c.name === 'strip running'), undefined);
  });
  test('with the brain agent AND the strip agent installed and the strip running, doctor does not also claim the brain does not start at login', async () => {
    if (process.platform !== 'darwin') return;
    const up = await run({ strip: true, running: true });
    const bad = up.checks.filter((c) => c.name === 'autostart' && /does not start at login/.test(c.detail));
    assert.deepEqual(bad.map((c) => c.detail), [], 'a contradictory autostart line is printed beside the PASS');
  });
});

describe('palette', () => {
  test('exactly the old default (any letter case) is replaced by the new one; anything else is kept', () => {
    const upgraded = withDefaults({ port: 0, pets: { palette: [...OLD_DEFAULT_PALETTE] } });
    assert.deepEqual(upgraded.pets.palette, DEFAULT_PALETTE);
    assert.deepEqual(withDefaults({ port: 0, pets: { palette: OLD_DEFAULT_PALETTE.map((c) => c.toUpperCase()) } }).pets.palette, DEFAULT_PALETTE);
    const mine = [
      OLD_DEFAULT_PALETTE.slice(0, 7),
      [...OLD_DEFAULT_PALETTE].reverse(),
      [...OLD_DEFAULT_PALETTE.slice(0, 7), '#ff7b7c'],
      [...OLD_DEFAULT_PALETTE, '#000000'],
      OLD_DEFAULT_PALETTE.map((c) => ` ${c}`),
      OLD_DEFAULT_PALETTE.map((c) => c.slice(1)),
      [...OLD_DEFAULT_PALETTE.slice(0, 7), 5],
      ['#ffffff'],
      DEFAULT_PALETTE.map((c) => c.toLowerCase()),
    ];
    for (const p of mine) assert.deepEqual(withDefaults({ port: 0, pets: { palette: p } }).pets.palette, p, JSON.stringify(p).slice(0, 60));
  });
  test('the new default has eight distinct, saturated entries, none of them white, and the same length as the old one (saved colour indexes stay valid)', () => {
    assert.equal(DEFAULT_PALETTE.length, OLD_DEFAULT_PALETTE.length);
    assert.equal(new Set(DEFAULT_PALETTE.map((c) => c.toLowerCase())).size, 8);
    assert.ok(!DEFAULT_PALETTE.some((c) => /^#f{6}$/i.test(c)));
    for (const c of DEFAULT_PALETTE) assert.match(c, /^#[0-9A-Fa-f]{6}$/);
  });
  test('the committed config.example.json carries the new palette and the old default is only an upgrade path', () => {
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    assert.deepEqual(cfg.pets.palette, DEFAULT_PALETTE);
  });
});

describe('same-project labels', () => {
  const sheep = (id, label, terminal, slot = 0) => ({ id, label, terminal, slot });
  test('a sheep alone in its project keeps the plain label; two get their pane tags; collisions fall back to numbers for the whole group', () => {
    assert.equal(displayLabels([sheep('a', 'hub', { herdrPane: 'w1:p4' })]).get('a'), 'hub');
    const two = displayLabels([sheep('a', 'hub', { herdrPane: 'w1:p4' }, 0), sheep('b', 'hub', { herdrPane: 'w1:p5' }, 1)]);
    assert.equal(two.get('a'), 'hub · p4');
    assert.equal(two.get('b'), 'hub · p5');
    const same = displayLabels([sheep('a', 'hub', { herdrPane: 'w1:p4' }, 0), sheep('b', 'hub', { herdrPane: 'w9:p4' }, 1), sheep('c', 'hub', { herdrPane: 'w1:p7' }, 2)]);
    assert.deepEqual([...same.values()], ['hub · #1', 'hub · #2', 'hub · #3']);
    const missing = displayLabels([sheep('a', 'hub', {}, 0), sheep('b', 'hub', { herdrPane: 'w1:p5' }, 1)]);
    assert.deepEqual([...missing.values()], ['hub · #1', 'hub · #2']);
  });
  test('tags are cut to [A-Za-z0-9#%], 8 characters; hostile and empty tags fall back; iTerm and tmux tags', () => {
    assert.equal(terminalTag({ herdrPane: 'w1:p4; rm -rf /<script>' }), 'p4rmrfsc');
    assert.equal(terminalTag({ herdrPane: 'w1:' + 'a'.repeat(100) }).length, 8);
    assert.equal(terminalTag({ herdrPane: 'w1:!!!!' }), null);
    assert.equal(terminalTag({ herdrPane: 'w1:ünï' }), 'n');
    assert.equal(terminalTag({ itermSession: 'w0t2p0:UUID' }), 'tab 3');
    assert.equal(terminalTag({ tmuxPane: '%3' }), '%3');
    assert.equal(terminalTag({ herdrPane: 5, itermSession: {}, tmuxPane: ['%1'] }), null);
    assert.equal(terminalTag(undefined), null);
    for (const tag of ['<b>', '"; alert(1)', '‮', '\n']) assert.doesNotMatch(terminalTag({ herdrPane: `w1:${tag}p9` }) ?? '', /[^A-Za-z0-9#% ]/);
  });
  test('the whole label is capped at 40 characters (code points), also with a long project name and a tag', () => {
    const long = 'p'.repeat(100);
    const out = displayLabels([sheep('a', long, { herdrPane: 'w1:p4' }), sheep('b', long, { herdrPane: 'w1:p5' }), sheep('c', 'x'.repeat(100))]);
    for (const v of out.values()) assert.ok([...v].length <= MAX_LABEL, `${[...v].length} chars`);
    const emoji = displayLabels([sheep('a', '😀'.repeat(60), {})]).get('a');
    assert.ok([...emoji].length <= MAX_LABEL);
    assert.doesNotMatch(emoji, /[\ud800-\udbff](?![\udc00-\udfff])/);
  });
  test('two projects whose names differ only after the 40th character are not shown as identical labels', () => {
    const a = 'x'.repeat(45) + 'A';
    const b = 'x'.repeat(45) + 'B';
    const out = displayLabels([sheep('a', a, { herdrPane: 'w1:p1' }), sheep('b', b, { herdrPane: 'w1:p2' })]);
    assert.notEqual(out.get('a'), out.get('b'), 'two different projects show the same capped label');
  });
  test('non-string and missing labels do not throw', () => {
    assert.doesNotThrow(() => displayLabels([{ id: 'a', label: null }, { id: 'b', label: 5 }, { id: 'c' }]));
  });
});

describe('the single-instance bypass (M10-14: only --locate bypasses it; --selftest opens a window and stays under it)', () => {
  const lib = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
  test('exactly one .plugin( call, skipped only for --locate', () => {
    // Milestone 9 adds the updater's, only in a build that updates itself (never with
    // --locate, --selftest or --demo: `updates_on` is false for those).
    assert.equal([...lib.matchAll(/\.plugin\(tauri_plugin_single_instance/g)].length, 1);
    assert.equal([...lib.matchAll(/\.plugin\(/g)].length, 2);
    assert.match(lib, /let updates_on = updater_off\.is_none\(\) && !checks_only;/);
    assert.match(lib, /if flags\.locate\.is_none\(\) \{\s*builder = builder\.plugin\(tauri_plugin_single_instance::init/);
    assert.doesNotMatch(lib, /if [^\n{]*selftest[^\n{]*\{\s*builder = builder\.plugin/, '--selftest must stay under single-instance');
  });
  test('--locate returns before any window, tray or brain is created; every other mode that opens a window is under the plugin', () => {
    const setup = lib.slice(lib.indexOf('.setup(move |app|'), lib.indexOf('.build(context)'));
    const locateAt = setup.indexOf('flags.locate');
    const exitAt = setup.indexOf('handle.exit(0);\n                return Ok(());');
    const openAt = setup.indexOf('strips::open(');
    const trayAt = setup.indexOf('tray::build(');
    assert.ok(locateAt > 0 && exitAt > locateAt && openAt > exitAt && trayAt > exitAt, '--locate must exit before strips::open and tray::build');
  });
});

describe('same-project labels grouped on the capped form (D3)', () => {
  test('two long names equal once capped get tags, the whole label stays within 40, and the tag survives the cap', () => {
    const a = 'x'.repeat(45) + 'A';
    const b = 'x'.repeat(45) + 'B';
    const out = displayLabels([{ id: 'a', label: a, terminal: { herdrPane: 'w1:p1' }, slot: 0 }, { id: 'b', label: b, terminal: { herdrPane: 'w1:p2' }, slot: 1 }]);
    assert.notEqual(out.get('a'), out.get('b'));
    for (const v of out.values()) assert.ok([...v].length <= MAX_LABEL, v);
    assert.match(out.get('a'), /p1$/);
    assert.match(out.get('b'), /p2$/);
    // names that differ before the cap and a sheep alone are unaffected
    const plain = displayLabels([{ id: 'a', label: 'alpha', terminal: {}, slot: 0 }, { id: 'b', label: 'beta', terminal: {}, slot: 1 }]);
    assert.deepEqual([...plain.values()], ['alpha', 'beta']);
    // three capped-equal, two with the same tag: numbers for the whole group
    const c = displayLabels([a + '1', a + '2', a + '3'].map((l, i) => ({ id: String(i), label: l, terminal: { herdrPane: i < 2 ? 'w1:p1' : 'w9:p1' }, slot: i })));
    assert.equal(new Set(c.values()).size, 3);
  });
});

describe('doctor autostart lines: the branch structure (D1), every combination', REPO_AGENTS, () => {
  const run = async ({ brainPlist, stripPlist, running, hooks }) => {
    const home = tempDir('sc-matrix-');
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    if (brainPlist) fs.writeFileSync(path.join(home, 'Library', 'LaunchAgents', 'local.bellwether.brain.plist'), '<plist/>');
    if (stripPlist) fs.writeFileSync(plistPath(home, SHELL_LABEL), '<plist/>');
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    cfg.port = 47998;
    const settings = path.join(home, 's.json');
    const { hookBlock } = await import('../brain/hooks.mjs');
    fs.writeFileSync(settings, hooks ? JSON.stringify(hookBlock(cfg.port, 'doctorTokenNotReal_0123456789')) : '{}\n');
    const r = await runDoctor({ settingsFile: settings, env: { BELLWETHER_HOOK_TOKEN: 'doctorTokenNotReal_0123456789' }, configLoader: () => ({ config: cfg, usingExample: true }), dataDir: tempDir('sc-mdd-'), home, isStripRunning: () => running, probeTimeoutMs: 300 });
    return r.checks.filter((c) => c.name === 'autostart' || c.name === 'strip running').map((c) => `${c.name} ${c.status}`);
  };
  test('each combination prints one consistent set of lines', async () => {
    if (process.platform !== 'darwin') return;
    const T = true, F = false;
    assert.deepEqual(await run({ brainPlist: T, stripPlist: T, running: T, hooks: T }), ['autostart PASS']);
    assert.deepEqual(await run({ brainPlist: T, stripPlist: T, running: T, hooks: F }), ['autostart PASS']);
    assert.deepEqual(await run({ brainPlist: T, stripPlist: T, running: F, hooks: T }), ['autostart PASS', 'strip running WARN']);
    assert.deepEqual(await run({ brainPlist: T, stripPlist: F, running: F, hooks: T }), ['autostart PASS']);
    assert.deepEqual(await run({ brainPlist: F, stripPlist: F, running: F, hooks: T }), ['autostart WARN']);
    assert.deepEqual(await run({ brainPlist: F, stripPlist: F, running: F, hooks: F }), ['autostart INFO']);
    // a strip agent without a brain agent: the brain still does not start at login
    assert.deepEqual((await run({ brainPlist: F, stripPlist: T, running: T, hooks: T })).filter((l) => l.startsWith('autostart')), ['autostart WARN']);
  });
});

describe('horn mask, independently from the real sheet (skipped when the art is not on this machine)', () => {
  const xmlFile = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
  test('exactly the three horn colours are caught on every opaque pixel, nothing else changes, shading survives', async () => {
    if (!fs.existsSync(xmlFile)) return;
    const zlib = await import('node:zlib');
    const { isHorn, tintHorns } = await import('../shell/src/player/horns.mjs');
    const xml = fs.readFileSync(xmlFile, 'utf8');
    const png = Buffer.from(/<png>\s*(?:<!\[CDATA\[)?\s*([A-Za-z0-9+/=\s]+?)\s*(?:\]\]>)?\s*<\/png>/.exec(xml)[1].replace(/\s+/g, ''), 'base64');
    let pos = 8; let ihdr; const idat = [];
    while (pos < png.length) { const len = png.readUInt32BE(pos); const type = png.toString('latin1', pos + 4, pos + 8); if (type === 'IHDR') ihdr = png.subarray(pos + 8, pos + 8 + len); if (type === 'IDAT') idat.push(png.subarray(pos + 8, pos + 8 + len)); pos += 12 + len; }
    const W = ihdr.readUInt32BE(0), H = ihdr.readUInt32BE(4);
    if (ihdr[9] !== 6 || ihdr[8] !== 8) return; // this probe decodes 8-bit RGBA only
    const raw = zlib.inflateSync(Buffer.concat(idat));
    const stride = W * 4; const px = new Uint8ClampedArray(H * stride); let p = 0;
    for (let y = 0; y < H; y++) { const f = raw[p++]; for (let x = 0; x < stride; x++) { const a = x >= 4 ? px[y * stride + x - 4] : 0, b = y ? px[(y - 1) * stride + x] : 0, c = x >= 4 && y ? px[(y - 1) * stride + x - 4] : 0; let v = raw[p++]; if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1; else if (f === 4) { const pa = Math.abs(b - c), pb = Math.abs(a - c), pc = Math.abs(a + b - 2 * c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; } px[y * stride + x] = v & 255; } }
    const hex = (r, g, b) => '#' + [r, g, b].map((x) => x.toString(16).padStart(2, '0')).join('');
    const caught = new Set();
    for (let i = 0; i < W * H; i++) if (isHorn(px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3])) caught.add(hex(px[i * 4], px[i * 4 + 1], px[i * 4 + 2]));
    assert.deepEqual([...caught].sort(), ['#730073', '#b300b3', '#ff7aff']);
    for (const colour of ['#FF8C00', '#1F5BFF', '#202020', '#E8202A', '#00C2D4', '#8B4513', '#12A84A', '#FF4FB0']) {
      const d = new Uint8ClampedArray(px);
      tintHorns(d, colour);
      const tones = new Set();
      for (let i = 0; i < W * H; i++) {
        const was = isHorn(px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]);
        const same = [0, 1, 2, 3].every((k) => d[i * 4 + k] === px[i * 4 + k]);
        if (!was) assert.ok(same, `a non-horn pixel changed under ${colour}`);
        else tones.add(hex(d[i * 4], d[i * 4 + 1], d[i * 4 + 2]));
      }
      assert.equal(tones.size, 3, `${colour}: dark, mid and highlight must stay three tones`);
    }
  });
});
