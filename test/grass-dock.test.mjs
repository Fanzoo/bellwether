import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { FADE_IN_MS, FADE_OUT_MS, LINGER_MS, TUFT, fadeStep, frontmost, tuftPixels, tuftX } from '../shell/src/player/grass.mjs';
import { familyExtent } from '../shell/src/player/lambs.mjs';
import { familyHomes } from '../shell/src/player/layout.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';
import { checkLocatorLog } from './impl.mjs';

/** Milestone 10: grass while eating; sheep on the Dock's top edge. */

const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');

describe('grass while eating', () => {
  test('the tuft is pixel art in three greens, scaled by whole pixels, and mirrors exactly', () => {
    const a = tuftPixels(2);
    const b = tuftPixels(2, true);
    assert.equal(a.w, TUFT[0].length * 2);
    assert.equal(a.h, TUFT.length * 2);
    for (let y = 0; y < a.h; y++) for (let x = 0; x < a.w; x++) {
      const i = (y * a.w + x) * 4;
      const j = (y * a.w + (a.w - 1 - x)) * 4;
      assert.deepEqual([...a.data.slice(i, i + 4)], [...b.data.slice(j, j + 4)]);
    }
    assert.ok([...a.data].some((v, i) => i % 4 === 3 && v === 0), 'transparent between blades');
  });
  test('it goes under the head\'s frontmost reach, mirrored with the facing', () => {
    // A 10×4 "frame" whose art starts at column 3.
    assert.equal(frontmost([0], 10, 4, (f, x) => (x >= 3 ? 255 : 0)), 3);
    const left = tuftX({ x: 100, width: 60, tileW: 40, front: 3, flip: false, tuftW: 13.5 });
    const right = tuftX({ x: 100, width: 60, tileW: 40, front: 3, flip: true, tuftW: 13.5 });
    // Mirror images about the sprite's centre (130).
    assert.ok(Math.abs(130 - (left + 13.5 / 2) - ((right + 13.5 / 2) - 130)) <= 1, `${left} ${right}`);
    assert.ok(left < 130 && right > 130);
  });
  test('fades in while eating, lingers a moment after, then fades out', () => {
    let s = fadeStep(undefined, { eating: true, now: 0 });
    s = fadeStep(s, { eating: true, now: FADE_IN_MS / 2 });
    assert.ok(s.alpha > 0.4 && s.alpha < 0.6);
    s = fadeStep(s, { eating: true, now: FADE_IN_MS * 2 });
    assert.equal(s.alpha, 1);
    const stopped = FADE_IN_MS * 2;
    s = fadeStep(s, { eating: false, now: stopped + LINGER_MS / 2 });
    assert.equal(s.alpha, 1, 'still there just after it moves on');
    s = fadeStep(s, { eating: false, now: stopped + LINGER_MS + FADE_OUT_MS / 2 });
    s = fadeStep(s, { eating: false, now: stopped + LINGER_MS + FADE_OUT_MS * 3 });
    assert.equal(s.alpha, 0);
  });
  test('the webview: tufts made once at load, stepped per sprite, drawn after the sprites, never while muted or hidden', () => {
    assert.match(main, /pet = \{ model, map, sheets, tileW, tileH, (overlays, )?\.\.\.grassFor\(model, map, base, tileW, tileH\) \}/); // overlays: deviation 125
    assert.equal([...main.matchAll(/tuftPixels\(/g)].length, 1, 'one place makes tufts (at load)');
    assert.match(main, /if \(stepGrass\(entry, now\)\) dirty = true;/);
    assert.match(main, /drawSprite\(e, now\);\s*\/\/ The grass[^\n]*\n\s*for \(const e of all\) drawGrass\(e\);/);
    // draw() returns before any of this while hidden or muted.
    assert.ok(main.indexOf('if (view.hidden) return;') < main.indexOf('drawGrass(e)'));
    assert.match(main, /sprite\.behaviour === 'graze' \|\| sprite\.behaviour === 'chew'/);
  });
});

describe('sheep on the Dock (Accessibility)', () => {
  const geo = { parentW: 60, lambW: 36 };
  const opts = { stripWidth: 1728, slotWidth: 110, wanderMax: 120 };
  const sheep = (n, lambs = 0) => Array.from({ length: n }, (_, i) => ({ id: `s${i}`, slot: i, extent: familyExtent(lambs, geo) }));
  const reach = (homes, s) => {
    const h = homes.get(s.id);
    return [h.centre - h.wander - s.extent.left, h.centre + h.wander + s.extent.right];
  };
  for (const [name, dock] of [['centred', { lo: 514, hi: 1214 }], ['at the left', { lo: 0, hi: 420 }], ['wide', { lo: 40, hi: 1690 }]]) {
    test(`a Dock ${name}: every family stays on it, wandering only within it, in slot order`, () => {
      const flock = sheep(3);
      const homes = familyHomes(flock, { ...opts, dock });
      const spans = flock.map((s) => reach(homes, s));
      for (const [a, b] of spans) assert.ok(a >= dock.lo - 0.01 && b <= dock.hi + 0.01, `${a}..${b} off a Dock at ${dock.lo}..${dock.hi}`);
      for (let i = 1; i < spans.length; i++) assert.ok(spans[i - 1][1] <= spans[i][0] + 0.01, 'families overlap or are out of order');
    });
  }
  test('a narrow Dock, or more families than fit: those that fit stay on it, the rest stand beside it, never overlapping', () => {
    const dock = { lo: 800, hi: 1010 };
    const flock = sheep(5, 1);
    const homes = familyHomes(flock, { ...opts, dock });
    const spans = flock.map((s) => reach(homes, s));
    const on = spans.filter(([a, b]) => a >= dock.lo - 0.01 && b <= dock.hi + 0.01);
    assert.ok(on.length >= 1, 'at least the first family stands on the Dock');
    const sorted = [...spans].sort((x, y) => x[0] - y[0]);
    for (let i = 1; i < sorted.length; i++) assert.ok(sorted[i - 1][1] <= sorted[i][0] + 0.01, `overlap: ${JSON.stringify(sorted)}`);
    for (const [a, b] of spans) assert.ok(a >= 0 && b <= 1728);
  });
  test('no Dock extent (no permission, auto-hide, a side Dock): exactly the full-width layout as before', () => {
    const flock = sheep(3);
    assert.deepEqual([...familyHomes(flock, opts)], [...familyHomes(flock, { ...opts, dock: null })]);
  });
  test('the webview takes the extent from the shell view and lays families out again when it changes', () => {
    assert.match(main, /dock: view\.dock \}\);/);
    assert.match(main, /if \(dockChanged && pet\) placeFamilies\(\);/);
  });
  test('the permission is asked only on request: the binary run with --ax-prompt, or the setup window', () => {
    const lib = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.equal([...lib.matchAll(/ax::prompt\(\)/g)].length, 1, 'one prompt, behind --ax-prompt');
    assert.match(lib, /if std::env::args\(\)\.any\(\|a\| a == "--ax-prompt"\) \{\s*let granted = ax::prompt\(\);/);
    const onboarding = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'onboarding.rs'), 'utf8');
    assert.equal([...onboarding.matchAll(/ax::prompt\(\)/g)].length, 1, 'the setup window asks once, when the user chooses to');
  });
});

describe('M10-18: when the Dock can\'t hold them, the full-width layout; bad extents count as none', () => {
  const geo = { parentW: 60, lambW: 36 };
  const opts = { stripWidth: 1728, slotWidth: 110, wanderMax: 120 };
  const sheep = (n, lambs = 0) => Array.from({ length: n }, (_, i) => ({ id: `s${i}`, slot: i, extent: familyExtent(lambs, geo) }));
  const boxes = (flock, homes) => flock.map((s) => { const h = homes.get(s.id); return [h.centre - h.wander - s.extent.left, h.centre + h.wander + s.extent.right]; });
  test('random Docks and flocks: always disjoint and inside the strip when the strip can hold everyone', () => {
    let seed = 11;
    const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
    for (let n = 0; n < 500; n++) {
      const stripWidth = [900, 1728, 2560][n % 3];
      const lo = Math.floor(rnd() * (stripWidth - 100));
      const dock = { lo, hi: lo + 60 + Math.floor(rnd() * (stripWidth - lo - 60)) };
      const flock = sheep(1 + Math.floor(rnd() * 9), Math.floor(rnd() * 6));
      const need = flock.reduce((a, s) => a + Math.max(110, s.extent.left + s.extent.right + 8), 0);
      if (need + 48 > stripWidth) continue;
      const b = boxes(flock, familyHomes(flock, { ...opts, stripWidth, dock })).sort((x, y) => x[0] - y[0]);
      for (let i = 1; i < b.length; i++) assert.ok(b[i - 1][1] <= b[i][0] + 0.5, JSON.stringify({ dock, stripWidth, b }));
      for (const [x, y] of b) assert.ok(x >= -0.5 && y <= stripWidth + 0.5);
    }
  });
  test('NaN, null, reversed or empty extents are no Dock: the same homes as without one', () => {
    const flock = sheep(3, 2);
    const plain = [...familyHomes(flock, opts)];
    for (const dock of [{ lo: NaN, hi: NaN }, { lo: 900, hi: 100 }, { lo: 0, hi: 0 }, { lo: null, hi: 5 }, { lo: 3000, hi: 4000 }]) assert.deepEqual([...familyHomes(flock, { ...opts, dock })], plain, JSON.stringify(dock));
  });
  test('the AX read runs on its own thread with a short timeout; CF objects are released by a guard', () => {
    const ax = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'ax.rs'), 'utf8');
    assert.match(ax, /const AX_TIMEOUT_S: f32 = 0\.25;/);
    assert.match(ax, /AXUIElementSetMessagingTimeout\(app\.0, AX_TIMEOUT_S\)/);
    assert.match(ax, /impl Drop for Cf \{/);
    const lib = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.match(lib, /std::thread::spawn\(move \|\| loop \{\s*let \(trusted, dock\) = \(ax::trusted\(\), dock::read_prefs\(\)\);/);
    assert.match(lib, /\*dock_seen\.lock\(\)\.unwrap\(\)/, 'the privacy loop reads the last frame, never AX itself');
  });
});

describe('doctor: a stale Accessibility entry after a rebuild', () => {
  const run = (text) => {
    const d = tempDir('gd-stale-');
    fs.writeFileSync(path.join(d, 'shell.log'), text);
    return checkLocatorLog(d).find((c) => c.name === 'dock');
  };
  const start = '1 bellwether-shell: starting: x\n';
  test('granted in an earlier run, not granted now: WARN, remove the old entry, then grant again', () => {
    const stale = run(`${start}2 bellwether-shell: dock: Accessibility granted to this binary: x\n${start}4 bellwether-shell: dock: Accessibility not granted to this binary: the full-width strip is used\n`);
    assert.equal(stale.status, 'WARN');
    assert.match(stale.detail, /Privacy & Security > Accessibility still lists the old entry, which belongs to the previous build/);
    assert.match(stale.detail, /remove it with −, then grant Bellwether again/);
  });
  test('granted before and now: PASS; never granted: INFO naming where to grant it', () => {
    assert.equal(run(`${start}2 bellwether-shell: dock: Accessibility granted to this binary: x\n${start}4 bellwether-shell: dock: Accessibility granted to this binary: x\n`).status, 'PASS');
    const never = run(`${start}2 bellwether-shell: dock: Accessibility not granted to this binary: x\n`);
    assert.equal(never.status, 'INFO');
    assert.match(never.detail, /turn Bellwether on in System Settings > Privacy & Security > Accessibility/);
  });
  test('docs/DEVELOPMENT.md says what to do when it was granted before and now says not granted', () => {
    const readme = fs.readFileSync(path.join(AGENT_DIR, 'docs', 'DEVELOPMENT.md'), 'utf8');
    assert.match(readme, /Remove the old `bellwether-shell`\s+entry \(select it, then −\), run the new binary with `--ax-prompt`/);
  });
});
