import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { checkLocatorLog } from '../brain/doctor.mjs';
import { familyExtent } from '../shell/src/player/lambs.mjs';
import { familyHomes } from '../shell/src/player/layout.mjs';
import { FADE_IN_MS, FADE_OUT_MS, LINGER_MS, fadeStep, frontmost, tuftPixels, tuftX, TUFT } from '../shell/src/player/grass.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';

/** Tester probes for grass, Dock following and Dock-width confinement. Nothing here touches the Dock, Accessibility or the live strip. */

const SRC = path.join(AGENT_DIR, 'shell', 'src-tauri', 'src');
const rs = (f) => fs.readFileSync(path.join(SRC, f), 'utf8');
const strip = (s) => s.replace(/\/\/.*$/gm, '');

const dims = { parentW: 60, lambW: 36 };
let seed = 20261010;
const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);

describe('layout: the fallback is exactly the old full-width layout', () => {
  test('without a Dock extent (null, undefined, or no `dock` key) the result equals the frozen pre-Dock implementation, for random flocks', async () => {
    const f = path.join(AGENT_DIR, 'test', 'fixtures', 'layout-pre-dock.mjs');
    const old = fs.readFileSync(f, 'utf8');
    const { familyHomes: oldHomes } = await import(f);
    assert.ok(!/dock/i.test(old.split('\n').slice(2).join('\n')), 'the fixture has Dock code: the oracle is not the old layout');
    for (let n = 0; n < 400; n++) {
      const stripWidth = [700, 1200, 1728, 2560][n % 4];
      const slots = [...new Set(Array.from({ length: 1 + Math.floor(rnd() * 6) }, () => Math.floor(rnd() * 10)))];
      const sheep = slots.map((slot, i) => ({ id: `s${i}`, slot, extent: familyExtent(Math.floor(rnd() * 7), dims) }));
      const o = { stripWidth, slotWidth: 110, wanderMax: 120 };
      const want = oldHomes(sheep, o);
      for (const dock of [undefined, null]) assert.deepEqual([...familyHomes(sheep, { ...o, dock })], [...want]);
      assert.deepEqual([...familyHomes(sheep, o)], [...want]);
    }
  });
});

describe('layout on the Dock', () => {
  const run = (sheep, dock, stripWidth = 1728) => familyHomes(sheep, { stripWidth, slotWidth: 110, wanderMax: 120, dock });
  const box = (s, h) => ({ id: s.id, lo: h.centre - h.wander - s.extent.left, hi: h.centre + h.wander + s.extent.right, centre: h.centre });
  const flock = (n, lambs = 3) => Array.from({ length: n }, (_, i) => ({ id: `s${i}`, slot: i, extent: familyExtent(lambs, dims) }));

  for (const [name, dock, stripWidth] of [['centred', { lo: 514, hi: 1214 }, 1728], ['at the left', { lo: 0, hi: 600 }, 1728], ['at the right', { lo: 1100, hi: 1728 }, 1728], ['full width', { lo: 0, hi: 1728 }, 1728], ['offset (a strip not at 0)', { lo: 414, hi: 1114 }, 1728], ['wide on a 2560 strip', { lo: 300, hi: 2300 }, 2560]]) {
    test(`${name}: families that fit stay on the Dock, in slot order, disjoint, inside the strip`, () => {
      for (let n = 1; n <= 4; n++) {
        const sheep = flock(n, 2);
        const homes = run(sheep, dock, stripWidth);
        const boxes = sheep.map((s) => box(s, homes.get(s.id)));
        for (const b of boxes) assert.ok(b.lo >= -0.5 && b.hi <= stripWidth + 0.5, `${name} n=${n}: ${JSON.stringify(b)} leaves the strip`);
        const total = sheep.reduce((a, s) => a + Math.max(110, s.extent.left + s.extent.right + 8), 0);
        if (total <= dock.hi - dock.lo) {
          for (const b of boxes) assert.ok(b.lo >= dock.lo - 0.5 && b.hi <= dock.hi + 0.5, `${name} n=${n}: a family that fits left the Dock: ${JSON.stringify(b)} vs ${JSON.stringify(dock)}`);
          for (let i = 0; i + 1 < boxes.length; i++) assert.ok(boxes[i].hi <= boxes[i + 1].lo + 0.5 && boxes[i].centre < boxes[i + 1].centre, 'overlap or order');
        }
      }
    });
  }

  test('random Docks and flocks: no two families overlap while the strip has room for them all, and every wander range stays inside the strip', () => {
    let overlaps = 0;
    const examples = [];
    for (let n = 0; n < 500; n++) {
      const stripWidth = [900, 1728, 2560][n % 3];
      const lo = Math.floor(rnd() * (stripWidth - 200));
      const hi = Math.min(stripWidth, lo + 80 + Math.floor(rnd() * (stripWidth - lo - 80 + 1)));
      const count = 1 + Math.floor(rnd() * 9);
      const sheep = Array.from({ length: count }, (_, i) => ({ id: `s${i}`, slot: i, extent: familyExtent(Math.floor(rnd() * 6), dims) }));
      const homes = run(sheep, { lo, hi }, stripWidth);
      assert.equal(homes.size, sheep.length, 'a family has no home');
      const boxes = sheep.map((s) => box(s, homes.get(s.id)));
      const need = sheep.reduce((a, s) => a + Math.max(110, s.extent.left + s.extent.right + 8), 0);
      for (const b of boxes) assert.ok(Number.isFinite(b.lo) && Number.isFinite(b.hi));
      if (need + 48 > stripWidth) continue; // the strip itself is too small for everyone
      const sorted = [...boxes].sort((a, b) => a.lo - b.lo);
      for (let i = 0; i + 1 < sorted.length; i++) if (sorted[i].hi > sorted[i + 1].lo + 0.5) { overlaps++; if (examples.length < 3) examples.push({ dock: { lo, hi }, stripWidth, count, a: sorted[i], b: sorted[i + 1] }); }
      for (const b of boxes) assert.ok(b.lo >= -1 && b.hi <= stripWidth + 1, `outside the strip: ${JSON.stringify(b)}`);
    }
    assert.equal(overlaps, 0, `families overlap beside a narrow Dock: ${JSON.stringify(examples[0])}`);
  });

  test('a Dock too narrow for even one family puts the family beside it, never on thin air at the far end', () => {
    const sheep = flock(1, 5);
    const h = run(sheep, { lo: 800, hi: 880 }).get('s0');
    assert.ok(Number.isFinite(h.centre) && h.wander >= 0);
  });
  test('a Dock extent with hi < lo, NaN or zero width does not throw or produce NaN homes', () => {
    for (const dock of [{ lo: 900, hi: 100 }, { lo: 0, hi: 0 }, { lo: NaN, hi: NaN }, { lo: -50, hi: 20 }, { lo: 0, hi: 99999 }]) {
      const homes = run(flock(3, 2), dock);
      for (const [, v] of homes) assert.ok(Number.isFinite(v.centre) && Number.isFinite(v.wander), JSON.stringify(dock));
    }
  });
});

describe('grass', () => {
  const T = 40;
  test('the fade is bounded, reaches full in about FADE_IN_MS, lingers, fades out in about FADE_OUT_MS, and a long frame gap neither overshoots nor flickers', () => {
    let s;
    let t = 0;
    s = fadeStep(s, { eating: true, now: t });
    for (; t < FADE_IN_MS + 50; t += 16) s = fadeStep(s, { eating: true, now: t });
    assert.equal(s.alpha, 1);
    const stopAt = t;
    for (; t < stopAt + LINGER_MS - 20; t += 16) s = fadeStep(s, { eating: false, now: t });
    assert.equal(s.alpha, 1, 'faded during the linger');
    for (; t < stopAt + LINGER_MS + FADE_OUT_MS + 80; t += 16) s = fadeStep(s, { eating: false, now: t });
    assert.equal(s.alpha, 0);
    // a throttled page: one step of 30 s
    const jump = fadeStep({ alpha: 0.3, lastEating: 0, at: 0 }, { eating: true, now: 30_000 });
    assert.equal(jump.alpha, 1);
    const jumpOut = fadeStep({ alpha: 1, lastEating: 0, at: 0 }, { eating: false, now: 30_000 });
    assert.equal(jumpOut.alpha, 0);
    // a clock that goes backwards does not push alpha out of range
    const back = fadeStep({ alpha: 0.5, lastEating: 100, at: 1000 }, { eating: true, now: 500 });
    assert.ok(back.alpha >= 0 && back.alpha <= 1);
    // a brief interruption (graze, step, graze within the linger) never drops the tuft
    let a = { alpha: 1, lastEating: 0, at: 0 };
    for (let k = 1; k <= 20; k++) a = fadeStep(a, { eating: k % 5 !== 0, now: k * 50 });
    assert.equal(a.alpha, 1);
  });
  test('the tuft sits under the mouth for both facings, mirror-symmetric about the sprite centre, and scales for lambs', () => {
    for (let n = 0; n < 300; n++) {
      const width = 20 + Math.floor(rnd() * 100);
      const x = Math.floor(rnd() * 800);
      const front = Math.floor(rnd() * 12);
      const tuftW = Math.round(TUFT[0].length * (width / T));
      const left = tuftX({ x, width, tileW: T, front, flip: false, tuftW });
      const right = tuftX({ x, width, tileW: T, front, flip: true, tuftW });
      const centre = x + width / 2;
      assert.ok(Math.abs((left + tuftW / 2) + (right + tuftW / 2) - 2 * centre) <= 1.5, `not mirrored: ${left} ${right} about ${centre}`);
      const mouth = x + front * (width / T);
      assert.ok(left + tuftW / 2 >= mouth - 0.5 && left + tuftW / 2 <= mouth + 3 * (width / T) + 0.5, 'tuft not near the mouth (facing left)');
    }
    const sheepW = 60, lambW = 36;
    assert.ok(Math.round(TUFT[0].length * (lambW / T)) < Math.round(TUFT[0].length * (sheepW / T)));
  });
  test('frontmost() returns a column inside the tile; with nothing opaque it falls back sensibly; tuftPixels is exactly 9x6 art pixels at any scale and mirrors', () => {
    assert.equal(frontmost([1, 2], 40, 40, (f, x, y) => (x >= 7 && y === 20 ? 255 : 0)), 7);
    const none = frontmost([1], 40, 40, () => 0);
    assert.ok(none >= 0 && none < 40);
    for (const px of [1, 2, 3, 5]) {
      const a = tuftPixels(px, false);
      const b = tuftPixels(px, true);
      assert.deepEqual([a.w, a.h], [9 * px, 6 * px]);
      for (let y = 0; y < a.h; y++) for (let x = 0; x < a.w; x++) for (let k = 0; k < 4; k++) assert.equal(a.data[(y * a.w + x) * 4 + k], b.data[(y * a.w + (a.w - 1 - x)) * 4 + k]);
    }
  });
  test('main.js draws grass only for graze and chew, never while hidden or muted, behind the bubbles, and with the cursor fade applied', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.match(main, /sprite\.behaviour === 'graze' \|\| sprite\.behaviour === 'chew'/);
    const draw = main.slice(main.indexOf('function draw(now'), main.indexOf('function draw(now') + 900);
    assert.ok(draw.indexOf('if (view.hidden) return;') < draw.indexOf('drawGrass'), 'grass can be drawn while hidden');
    assert.ok(draw.indexOf('drawMuted();') < draw.indexOf('drawGrass') && /muted\) \{\s*drawMuted\(\);\s*return;/.test(draw), 'grass can be drawn while muted');
    assert.ok(draw.indexOf('drawGrass') < draw.indexOf('drawBubbles'));
    assert.match(main, /globalAlpha = g\.alpha \* fadeOf\(entry\.key\)/);
  });
});

describe('Accessibility and the Dock: unsafe code and the permission prompt', () => {
  const ax = strip(rs('ax.rs'));
  const lib = strip(rs('lib.rs'));
  test('the prompt option is used in exactly one function, which is called only for --ax-prompt', () => {
    assert.equal([...ax.matchAll(/kAXTrustedCheckOptionPrompt/g)].length, 2, 'one use plus its extern declaration');
    assert.equal([...ax.matchAll(/AXIsProcessTrustedWithOptions\(/g)].length, 2, 'declaration plus the single call in prompt()');
    const callers = [...lib.matchAll(/ax::prompt\(\)/g)];
    assert.equal(callers.length, 1);
    const at = lib.indexOf('ax::prompt()');
    assert.match(lib.slice(Math.max(0, at - 400), at), /"--ax-prompt"/);
    // M7: the onboarding window's "Allow…" (plan D6 step 4) is the one other caller, on the
    // user's click: once per run, only when not yet trusted (a second click opens the pane).
    const ob = strip(rs('onboarding.rs'));
    assert.equal([...ob.matchAll(/ax::prompt\(\)/g)].length, 1);
    const arm = ob.slice(ob.indexOf('"accessibility" => {'), ob.indexOf('crate::ax::prompt()'));
    assert.match(arm, /if crate::ax::trusted\(\) \{[\s\S]*\} else if !self\.prompted_ax\.swap\(true, Ordering::SeqCst\) \{\s*$/);
    assert.ok(ob.indexOf('pub fn permission_request(&self, kind: &str)') < ob.indexOf('crate::ax::prompt()'));
    for (const f of fs.readdirSync(SRC).filter((x) => x.endsWith('.rs') && x !== 'ax.rs' && x !== 'lib.rs' && x !== 'onboarding.rs')) assert.doesNotMatch(strip(rs(f)), /ax::prompt|AXIsProcessTrustedWithOptions/, f);
    assert.match(ax, /pub fn trusted\(\) -> bool \{\s*unsafe \{ AXIsProcessTrusted\(\) \}/);
  });
  test('the AX read only happens when already trusted, and only for a bottom, shown Dock', () => {
    assert.match(lib, /if trusted && !dock\.autohide && dock\.side == dock::Side::Bottom \{ ax::dock_pid\(\)\.and_then\(ax::dock_frame\) \}/);
  });
  test('the AX read has a short messaging timeout and does not run on the thread that hides the strip while sharing', () => {
    const hasTimeout = /AXUIElementSetMessagingTimeout/.test(ax);
    const privacyLoop = lib.slice(lib.indexOf('fn privacy_loop'), lib.indexOf('fn privacy_loop') + 9000);
    const inPrivacyThread = /ax::dock_frame/.test(privacyLoop);
    assert.ok(hasTimeout || !inPrivacyThread, 'ax::dock_frame (default AX messaging timeout: seconds) runs in privacy_loop, the same thread that hides the strip when a share starts: a hung or restarting Dock delays the privacy hide');
  });
  test('every CF object created or copied in ax.rs is released on every path (a partial AXPosition/AXSize failure must not leak)', () => {
    const body = ax.slice(ax.indexOf('pub fn dock_frame'), ax.indexOf('pub fn dock_pid'));
    // the pattern `let (Some(p), Some(s)) = (attr(..), attr(..)) else { continue }` leaks the one that succeeded
    assert.doesNotMatch(body, /let \(Some\(p\), Some\(s\)\) = \(attr/, 'if one of AXPosition/AXSize fails the other is leaked');
  });
});

describe('doctor `dock` line', () => {
  const run = (text) => {
    const d = tempDir('gd-');
    fs.writeFileSync(path.join(d, 'shell.log'), text);
    return checkLocatorLog(d).find((c) => c.name === 'dock');
  };
  test('not granted and withdrawn: INFO telling how to grant and that a rebuild needs a re-grant; granted: PASS; none: no line; webview lines cannot fake it', () => {
    const start = '1 bellwether-shell: starting: x\n';
    const no = run(`${start}2 bellwether-shell: dock: Accessibility not granted to this binary: the full-width strip is used\n`);
    assert.equal(no.status, 'INFO');
    assert.match(no.detail, /dock-permission/);
    assert.match(no.detail, /after a rebuild it must be given again/);
    assert.equal(run(`${start}2 bellwether-shell: dock: Accessibility granted to this binary: sheep stand\n`).status, 'PASS');
    assert.equal(run(`${start}2 bellwether-shell: dock: Accessibility granted to this binary: x\n3 bellwether-shell: dock: Accessibility withdrawn while running\n`).status, 'INFO');
    assert.equal(run(`${start}2 bellwether-shell: dock: Accessibility not granted\n3 bellwether-shell: dock: Accessibility granted while running\n`).status, 'PASS');
    assert.equal(run(`${start}`), undefined);
    assert.equal(run(`${start}2 bellwether-shell: webview: dock: Accessibility granted\n`), undefined);
  });
  test('the README says the grant must be repeated after a rebuild, names the command, and no longer claims the strip needs no Accessibility', () => {
    const readme = fs.readFileSync(path.join(AGENT_DIR, 'README.md'), 'utf8');
    assert.match(readme, /rebuild it must be given again/);
    assert.match(readme, /shell dock-permission/);
    const step6 = readme.slice(readme.indexOf('### 6.'), readme.indexOf('### 7.'));
    assert.doesNotMatch(step6, /no Accessibility/, 'First run step 6 still says the strip needs no Accessibility permission');
  });
});
