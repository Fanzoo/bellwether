import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { displayLabels, terminalTag, MAX_LABEL } from '../shell/src/player/labels.mjs';
import { AGENT_DIR } from './helpers.mjs';
import { DEFAULT_PALETTE, OLD_DEFAULT_PALETTE, withDefaults } from './impl.mjs';

/**
 * Tester probes for the palette and the labels. The repository's `shell start|stop|restart`
 * and its doctor lines for its LaunchAgents went with the Node brain in milestone 11 (the
 * app's strip control: test/DIVERGENCES.md 19; its login items: shell/src-tauri cli.rs
 * `login_verdict`; leftover LaunchAgents: `legacy agents`, rename.test.mjs).
 */

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
