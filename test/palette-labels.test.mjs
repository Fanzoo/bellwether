import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { MAX_LABEL, displayLabels, terminalTag } from '../shell/src/player/labels.mjs';
import { AGENT_DIR, TOKEN, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { DEFAULT_PALETTE, OLD_DEFAULT_PALETTE, PALETTE_UPGRADED, runDoctor, withDefaults } from './impl.mjs';

/** Milestone 10 additions: a palette you can tell apart, and same-project labels told apart. */

const rgb = (hex) => [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16));

describe('the sheep palette', () => {
  test('eight saturated colours, no white or near-white, all different; the same eight everywhere', () => {
    assert.equal(DEFAULT_PALETTE.length, 8);
    assert.equal(new Set(DEFAULT_PALETTE.map((c) => c.toLowerCase())).size, 8);
    for (const c of DEFAULT_PALETTE) assert.ok(Math.min(...rgb(c)) < 0x90, `${c} is too pale to tint the wool`);
    const example = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    assert.deepEqual(example.pets.palette, DEFAULT_PALETTE);
    const rs = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.ok(rs.includes(`pub const DEFAULT_PALETTE: [&str; 8] = [${DEFAULT_PALETTE.map((c) => `"${c}"`).join(', ')}];`), 'lib.rs has the same list');
  });
  test('the first two colours two sessions get are far apart (not white and pale blue any more)', () => {
    const [a, b] = DEFAULT_PALETTE.map(rgb);
    const dist = Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    assert.ok(dist > 250, `distance ${dist}`);
  });
  test('a config.json with the old default palette verbatim gets the new one (same length, so saved colour indexes stay valid); a palette of your own is kept', () => {
    const up = withDefaults({ pets: { palette: OLD_DEFAULT_PALETTE.map((c) => c.toUpperCase()) } });
    assert.deepEqual(up.pets.palette, DEFAULT_PALETTE);
    assert.equal(up[PALETTE_UPGRADED], true);
    assert.equal(OLD_DEFAULT_PALETTE.length, DEFAULT_PALETTE.length);
    const mine = withDefaults({ pets: { palette: ['#123456', '#654321'] } });
    assert.deepEqual(mine.pets.palette, ['#123456', '#654321']);
    assert.equal(mine[PALETTE_UPGRADED], undefined);
  });
  test('doctor says so (INFO), without touching config.json', async () => {
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: path.join(tempDir(), 'none.json'), configLoader: () => ({ config: withDefaults({ port: 1, pets: { palette: [...OLD_DEFAULT_PALETTE] } }), usingExample: false }), dataDir: tempDir(), home: tempDir(), probeTimeoutMs: 100 });
    const p = r.checks.find((c) => c.name === 'palette');
    assert.equal(p?.status, 'INFO');
    assert.match(p.detail, /new default palette is used\. Your config\.json is unchanged/);
  });
  test('the webview tints every colour, the first included', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.match(main, /display\.palette\.map\(\(hex\) => tinted\(base, hex, display\.tint\)\)/);
  });
});

describe('labels for same-project sheep', () => {
  test('alone in its project: the plain label; two or more: a distinguisher from the terminal', () => {
    const m = displayLabels([
      { id: 'a', label: 'my-project', terminal: { herdrPane: 'w4:p2' }, slot: 0 },
      { id: 'b', label: 'my-project', terminal: { herdrPane: 'w4:p4' }, slot: 1 },
      { id: 'c', label: 'meadow', terminal: { herdrPane: 'w1:p1' }, slot: 2 },
    ]);
    assert.deepEqual([...m], [['a', 'my-project · p2'], ['b', 'my-project · p4'], ['c', 'meadow']]);
  });
  test('iTerm tab, tmux pane, else numbered by slot; tags that collide or are missing fall back to numbers', () => {
    assert.equal(terminalTag({ itermSession: 'w0t1p0:0F0E-AA' }), 'tab 2');
    assert.equal(terminalTag({ tmuxPane: '%3' }), '%3');
    assert.equal(terminalTag({}), null);
    const m = displayLabels([
      { id: 'b', label: 'x', terminal: { herdrPane: 'w1:p1' }, slot: 5 },
      { id: 'a', label: 'x', terminal: {}, slot: 1 },
    ]);
    assert.deepEqual([m.get('a'), m.get('b')], ['x · #1', 'x · #2']);
    const same = displayLabels([{ id: 'a', label: 'x', terminal: { herdrPane: 'w1:p1' } }, { id: 'b', label: 'x', terminal: { herdrPane: 'w2:p1' } }]);
    assert.deepEqual([...same.values()].sort(), ['x · #1', 'x · #2'], 'p1 and p1 would not tell them apart');
  });
  test('hostile terminal values become harmless short tags, and the label is capped', () => {
    const m = displayLabels([
      { id: 'a', label: 'p', terminal: { herdrPane: 'w1:<img src=x>‮evil' } },
      { id: 'b', label: 'p', terminal: { herdrPane: 'w1:p2' } },
    ]);
    assert.match(m.get('a'), /^p · [A-Za-z0-9#%]{1,8}$/);
    const long = 'a'.repeat(60);
    const n = displayLabels([{ id: 'a', label: long }, { id: 'b', label: long }]);
    for (const v of n.values()) assert.ok([...v].length <= MAX_LABEL && /… · #\d$/.test(v), `${v}: the name gives way, the tag stays`);
  });
  test('the webview uses them in bubbles, lamb bubbles, hover and Show labels', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.match(main, /const labels = displayLabels\(snapshot\.flock/);
    assert.match(main, /sub: entry\.shownLabel \?\? actor\.label/);
    assert.match(main, /projectLabel: entry\.shownLabel/);
    assert.match(main, /label\(e\.shownLabel \?\? e\.actor\.label/);
  });
});
