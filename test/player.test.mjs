import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { compile, evaluate } from '../shell/src/player/expr.mjs';
import { familyExtent, freePlace, lambOffset, lambX } from '../shell/src/player/lambs.mjs';
import { familyHomes, homeX, stripRect } from '../shell/src/player/layout.mjs';
import { assertSafePetXml, resolveAnimationMap, validatePet } from '../shell/src/player/pet.mjs';
import { Sprite, behaviourOf } from '../shell/src/player/sprite.mjs';
import { frameAt, motionAt, pickNext, tileOf, totalSteps } from '../shell/src/player/stepper.mjs';
import { AGENT_DIR, FIXTURES } from './helpers.mjs';

/**
 * The sprite player's logic, without a DOM, against a hand-written pet model
 * (test/fixtures/pet-model.json; no art).
 */

const model = () => validatePet(JSON.parse(fs.readFileSync(path.join(FIXTURES, 'pet-model.json'), 'utf8')));
const MAP = { walk: 'walk', run: 'run', graze: 'eat', sleep: ['sleep1a', 'sleep1b'], hop: 'jump', arrive: 'walk', leave: 'run', sit: 'eat' };
const ENV = { screenW: 1000, screenH: 140, areaW: 1000, areaH: 140, imageW: 60, imageH: 60, imageX: 10, imageY: 80, random: 50, randS: 7 };
const seeded = (seed = 1) => () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;

describe('the expression evaluator', () => {
  test('numbers, + - * / ( ), unary minus, and the keywords', () => {
    assert.equal(evaluate('1+2*3', ENV), 7);
    assert.equal(evaluate('(1+2)*3', ENV), 9);
    assert.equal(evaluate('-imageW*0.7', ENV), -42);
    assert.equal(evaluate('random*(screenW-imageW-50)/100+25', ENV), 470);
    assert.equal(evaluate('areaH/2-(randS*areaH/2)/120-imageH', ENV), 70 - (7 * 70) / 120 - 60);
    assert.equal(evaluate('  2.5 ', ENV), 2.5);
    assert.equal(evaluate('screenW/0', ENV), 0, 'division by zero is 0, not Infinity');
  });

  test('hostile or malformed expressions evaluate to 0 with one warning each, and nothing runs', () => {
    const warnings = [];
    globalThis.__bellwetherExprPwned = false;
    const hostile = [
      'alert(1)',
      "constructor.constructor('globalThis.__bellwetherExprPwned=true')()",
      'screenW.constructor',
      'imageW;imageH',
      '"text"',
      'this',
      'process.exit(1)',
      '1 +',
      '((1)',
      '2**3',
      'x'.repeat(500),
      '[]',
      '`${1}`',
    ];
    for (const e of hostile) assert.equal(evaluate(e, ENV, (w) => warnings.push(w)), 0, e);
    assert.equal(globalThis.__bellwetherExprPwned, false);
    assert.equal(warnings.length, hostile.length);
    evaluate('alert(1)', ENV, (w) => warnings.push(w));
    assert.equal(warnings.length, hostile.length, 'warned once per expression');
    assert.throws(() => compile('Math.random()'));
  });

  test('the evaluator source uses no eval or Function', () => {
    for (const f of fs.readdirSync(path.join(AGENT_DIR, 'shell', 'src', 'player'))) {
      const code = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'player', f), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
      assert.doesNotMatch(code, /\beval\s*\(|\bnew\s+Function\b|\bFunction\s*\(|setTimeout\s*\(\s*['"`]/, f);
    }
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.doesNotMatch(main, /\beval\s*\(|\bnew\s+Function\b|innerHTML|outerHTML|insertAdjacentHTML|document\.write/);
  });
});

describe('frame stepping', () => {
  test('frames play once, then the tail from repeatfrom repeats', () => {
    const m = model();
    const walk = m.animations[1];
    assert.equal(totalSteps(walk, ENV), 2 + 2 * 3);
    assert.deepEqual([...Array(8).keys()].map((i) => frameAt(walk, i)), [0, 1, 0, 1, 0, 1, 0, 1]);
    const eat = m.animations[2];
    assert.equal(totalSteps(eat, { ...ENV, random: 100 }), 2 + 1 * 2);
    assert.deepEqual([0, 1, 2, 3].map((i) => frameAt(eat, i)), [2, 3, 3, 3]);
  });

  test('movement and interval are interpolated from start to end', () => {
    const walk = model().animations[1];
    const total = totalSteps(walk, ENV);
    assert.equal(motionAt(walk, 0, total, ENV).interval, 100);
    assert.equal(motionAt(walk, total - 1, total, ENV).interval, 200);
    assert.equal(motionAt(walk, 3, total, ENV).dx, -2);
    const jump = model().animations[5];
    const jt = totalSteps(jump, ENV);
    assert.equal(motionAt(jump, 0, jt, ENV).dy, -15);
    assert.equal(motionAt(jump, jt - 1, jt, ENV).dy, 15);
  });

  test('next: by probability, only "none"/"taskbar", and within an allowed set', () => {
    const walk = model().animations[1];
    assert.equal(pickNext(walk, 0), 1);
    assert.equal(pickNext(walk, 0.95), 2);
    assert.equal(pickNext(walk, 0.999), 2, 'the window-only entry (id 5) never applies on the strip');
    assert.equal(pickNext(walk, 0.5, new Set([2])), 2);
    assert.equal(pickNext(walk, 0.5, new Set([4])), null);
  });

  test('tiles: frame n is column n % tilesx, row n / tilesx', () => {
    assert.deepEqual(tileOf(model(), 6), { col: 2, row: 1 });
  });

  test('validatePet drops frames outside the sheet and links to missing animations', () => {
    const m = JSON.parse(fs.readFileSync(path.join(FIXTURES, 'pet-model.json'), 'utf8'));
    m.animations[1].frames.push(99, -1, 1.5);
    m.animations[1].next.push({ id: 42, probability: 10, only: 'none' });
    validatePet(m);
    assert.deepEqual(m.animations[1].frames, [0, 1]);
    assert.ok(!m.animations[1].next.some((n) => n.id === 42));
    assert.throws(() => validatePet({ ...m, tilesx: 0 }));
  });

  test('animationMap: a name the pet lacks is reported and dropped', () => {
    const warnings = [];
    const map = resolveAnimationMap(model(), { ...MAP, dizzy: 'sleep2a' }, (w) => warnings.push(w));
    assert.deepEqual(map.sleep, [3, 4]);
    assert.equal(map.dizzy, undefined);
    assert.match(warnings.join(), /no animation "sleep2a"/);
  });
});

describe('a sprite on the strip', () => {
  const make = (o = {}) => {
    const m = model();
    return new Sprite({ model: m, map: resolveAnimationMap(m, MAP), width: 60, height: 60, stripWidth: 1000, stripHeight: 140, home: 400, wander: 120, rand: seeded(3), ...o });
  };

  test('a walking sheep stays within home ± wander and inside the strip, and turns at the bounds', () => {
    const s = make();
    s.setBehaviour('run', 0);
    const xs = [];
    let turns = 0;
    let facing = s.facing;
    for (let t = 0; t < 120_000; t += 20) {
      s.tick(t);
      xs.push(s.x);
      if (s.facing !== facing) turns++;
      facing = s.facing;
    }
    assert.ok(Math.min(...xs) >= 280 && Math.max(...xs) <= 520, `${Math.min(...xs)}..${Math.max(...xs)}`);
    assert.ok(turns > 5, `turns ${turns}`);
  });

  test('near the strip edge, the wander bound is clipped to the strip', () => {
    const s = make({ home: 10 });
    s.setBehaviour('run', 0);
    for (let t = 0; t < 60_000; t += 20) {
      s.tick(t);
      assert.ok(s.x >= 0 && s.x <= 1000 - 60);
    }
  });

  test('grazing and sleeping sheep stay put; sleep goes sleep1a then sleep1b', () => {
    const s = make();
    s.setBehaviour('sleep', 0);
    const x = s.x;
    const anims = new Set();
    for (let t = 0; t < 10_000; t += 50) {
      s.tick(t);
      anims.add(s.anim.id);
    }
    assert.equal(s.x, x);
    assert.deepEqual([...anims], [3, 4]);
  });

  test('a hop plays once and the behaviour resumes', () => {
    const s = make();
    s.setBehaviour('graze', 0);
    s.playOnce('hop', 0);
    assert.equal(s.anim.id, 5);
    let t = 0;
    while (s.oneShot && t < 10_000) s.tick((t += 10));
    assert.equal(s.oneShot, null);
    assert.equal(s.anim.id, 2, 'back to eat');
    s.setBehaviour('walk', t);
    assert.equal(s.anim.id, 1);
  });

  test('a leaving sheep runs off the nearer edge and is gone', () => {
    const s = make({ home: 900 });
    s.setBehaviour('leave', 0);
    assert.equal(s.facing, 1);
    for (let t = 0; t < 30_000 && !s.gone; t += 20) s.tick(t);
    assert.ok(s.gone);
  });

  test('ticks only when a frame is due', () => {
    const s = make();
    s.setBehaviour('walk', 0);
    assert.equal(s.tick(0), true);
    assert.equal(s.tick(50), false);
    assert.equal(s.tick(100), true);
  });

  test('the art faces left: drawn mirrored when facing right', () => {
    const s = make();
    s.facing = 1;
    assert.equal(s.drawInfo().flip, true);
    s.facing = -1;
    assert.equal(s.drawInfo().flip, false);
    assert.equal(s.drawInfo().y, 140 - 60);
  });
});

describe('behaviour from the snapshot', () => {
  test('calm states map to behaviours; an alert or leaving wins', () => {
    const cases = [
      [{ state: 'busy', flavour: 'running' }, 'run'],
      [{ state: 'busy', flavour: 'reading' }, 'walk'],
      [{ state: 'busy', flavour: 'thinking' }, 'walk'],
      [{ state: 'idle' }, 'graze'],
      [{ state: 'sleeping' }, 'sleep'],
      [{ state: 'done' }, 'sit'],
      [{ state: 'waiting' }, 'lie'],
      [{ state: 'compacting' }, 'chew'],
      [{ state: 'error', flavour: 'rate_limit' }, 'dizzy'],
      [{ state: 'arriving' }, 'arrive'],
      [{ state: 'idle', alert: { kind: 'permission' } }, 'alert'],
      [{ state: 'leaving', alert: { kind: 'permission' } }, 'leave'],
    ];
    for (const [actor, want] of cases) assert.equal(behaviourOf(actor), want, JSON.stringify(actor));
  });
});

describe('slots', () => {
  test('each slot has its own home, stable and inside the strip; slots wrap rather than leave the screen', () => {
    const o = { stripWidth: 1728, slotWidth: 110, spriteWidth: 60 };
    const xs = [0, 1, 2, 3].map((s) => homeX(s, o));
    assert.deepEqual(xs, [49, 159, 269, 379]);
    assert.equal(homeX(1, o), homeX(1, o));
    const capacity = Math.floor((1728 - 48) / 110);
    assert.equal(homeX(capacity, o), homeX(0, o));
    assert.ok(homeX(-3, o) >= 0);
  });

  test('the strip is the bottom heightPx of the work area', () => {
    assert.deepEqual(stripRect({ x: 0, y: 37, width: 1728, height: 1014 }, 140), { x: 0, y: 911, width: 1728, height: 140 });
    assert.deepEqual(stripRect({ x: 0, y: 0, width: 800, height: 100 }, 140), { x: 0, y: 0, width: 800, height: 100 });
  });
});

test('pet XML with a DOCTYPE or ENTITY is refused before parsing', () => {
  assert.throws(() => assertSafePetXml('<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol">]><animations/>'), /DOCTYPE or ENTITY/);
  assert.throws(() => assertSafePetXml('<animations><!entity x></animations>'), /DOCTYPE or ENTITY/);
  assert.equal(assertSafePetXml('<animations/>'), '<animations/>');
  const real = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
  if (fs.existsSync(real)) assertSafePetXml(fs.readFileSync(real, 'utf8'));
});

describe('alert bubbles (layout)', async () => {
  const { BUBBLE, ellipsise, layoutBubbles } = await import('../shell/src/player/bubbles.mjs');
  const measure = (t) => [...t].length * 7;
  const W = 1728;
  const item = (id, cx, text = 'Bash: git push origin main?', priority = 2, sub = 'meadow') => ({ id, cx, top: 80, text, sub, priority });
  const overlaps = (a, b) => a.row === b.row && a.x < b.x + b.w && b.x < a.x + a.w;

  test('ellipsise shortens to fit, keeping as much as possible', () => {
    assert.equal(ellipsise('short', 100, measure), 'short');
    const e = ellipsise('Bash: git push origin main --force-with-lease', 140, measure);
    assert.ok(e.endsWith('…') && measure(e) <= 140, e);
    assert.equal(ellipsise('abc', 1, measure), '…');
  });

  test('one bubble sits above its sheep, label on the same line, tail pointing at it', () => {
    const { placed, dropped } = layoutBubbles([item('a', 600)], { stripWidth: W, measure });
    assert.deepEqual(dropped, []);
    const [b] = placed;
    assert.equal(b.row, 0);
    assert.equal(b.tailX, 600);
    assert.ok(b.x < 600 && b.x + b.w > 600);
    assert.equal(b.sub, 'meadow');
    assert.ok(b.y >= 0 && b.y + b.h + BUBBLE.tail <= 80 - BUBBLE.gap + 0.5);
  });

  test('near the edges the bubble shifts inwards and stays inside the strip; the tail still points at the sheep', () => {
    for (const cx of [3, W - 3]) {
      const [b] = layoutBubbles([item('a', cx, 'A long question that is wider than the gap to the edge?')], { stripWidth: W, measure }).placed;
      assert.ok(b.x >= BUBBLE.margin && b.x + b.w <= W - BUBBLE.margin, JSON.stringify(b));
      assert.ok(Math.abs(b.tailX - cx) <= 10);
    }
  });

  test('random flocks: placed bubbles never overlap, stay inside the strip, and are readable', () => {
    const rand = seeded(11);
    for (let trial = 0; trial < 300; trial++) {
      const n = 1 + Math.floor(rand() * 9);
      const items = [...Array(n).keys()].map((i) => item(`s${i}`, 30 + rand() * (W - 60), 'x'.repeat(5 + Math.floor(rand() * 80)), Math.floor(rand() * 4)));
      const { placed, dropped } = layoutBubbles(items, { stripWidth: W, measure });
      assert.equal(placed.length + dropped.length, n);
      for (const b of placed) {
        assert.ok(b.x >= 0 && b.x + b.w <= W && b.y >= 0, JSON.stringify(b));
        assert.ok(b.w >= BUBBLE.minWidth);
        for (const o of placed) if (o !== b) assert.ok(!overlaps(b, o), `${JSON.stringify(b)} overlaps ${JSON.stringify(o)}`);
      }
    }
  });

  test('bubbles on different rows do not narrow each other; the m5-t04 scene keeps whole questions', () => {
    const items = [
      item('bash', 79, 'Bash: touch meadow-approved.txt?', 3),
      item('deploy', 330, 'deploy: Which region should the release go to?', 2),
      item('lamb', 380, 'Explore: Bash: npm test?', 1),
      item('input', 600, 'A background session is waiting for you', 2),
    ];
    const { placed, dropped } = layoutBubbles(items, { stripWidth: W, measure });
    assert.deepEqual(dropped, []);
    for (const p of placed) assert.ok(!p.text.endsWith('…'), `${p.id}: ${p.text}`);
  });

  test('nine neighbouring sheep (slots 110 px apart) all get bubbles, on alternating rows', () => {
    const items = [...Array(9).keys()].map((i) => item(`s${i}`, 79 + 110 * i, 'Bash: touch meadow-approved.txt and more?'));
    const { placed, dropped } = layoutBubbles(items, { stripWidth: W, measure });
    assert.deepEqual(dropped, []);
    assert.deepEqual(new Set(placed.map((p) => p.row)), new Set([0, 1]));
  });

  test('two sheep that wander close together keep readable bubbles (no global squeeze)', () => {
    const items = [item('a', 79), item('b', 330, 'deploy: Which region should the release go to?'), item('c', 380, 'Explore: Bash: npm test?', 1)];
    const { placed } = layoutBubbles(items, { stripWidth: W, measure });
    const a = placed.find((p) => p.id === 'a');
    assert.equal(a.text, 'Bash: git push origin main?', 'a bubble far from the crowd is untouched');
  });

  test('when there is no room, the highest priority (a confirmed alert) wins and the rest are reported', () => {
    const items = [item('low', 100, 'x'.repeat(40), 1), item('confirmed', 102, 'y'.repeat(40), 3), item('mid', 104, 'z'.repeat(40), 2)];
    const { placed, dropped } = layoutBubbles(items, { stripWidth: 120, measure });
    assert.equal(placed[0].id, 'confirmed');
    assert.deepEqual(dropped, ['low'], 'only the lowest priority gives way');
    // Sheep 90 px apart on a narrow strip: all three stay, narrowed, rather than one being dropped.
    const spaced = [item('low', 60, 'x'.repeat(40), 1), item('confirmed', 150, 'y'.repeat(40), 3), item('mid', 240, 'z'.repeat(40), 2)];
    const r = layoutBubbles(spaced, { stripWidth: 300, measure });
    assert.deepEqual(r.dropped, []);
    assert.ok(r.placed.some((p) => p.text.endsWith('…')));
  });

  test('the label shrinks before it goes, and the question always wins', () => {
    const one = (text, sub, width = W) => layoutBubbles([{ id: 'a', cx: 600, top: 80, text, sub, priority: 2 }], { stripWidth: width, measure }).placed[0];
    assert.equal(one('Bash: git push origin main?', 'meadow').sub, 'meadow', 'room for both');
    const long = one('q'.repeat(45), 'a-long-project-name');
    assert.equal(long.text, 'q'.repeat(45), 'the question is whole');
    assert.ok(long.sub && long.sub.endsWith('…'), `label shortened: ${long.sub}`);
    const full = one('q'.repeat(60), 'meadow');
    assert.equal(full.text, 'q'.repeat(60), 'a 60-character question fits whole in maxWidth');
    const tight = one('q'.repeat(60), 'meadow', 300);
    assert.ok(tight.text.endsWith('…') && tight.sub === undefined, 'a question that does not fit takes all the room');
  });
});

describe('the alert, on the sprite', () => {
  const m = model();
  const map = resolveAnimationMap(m, { ...MAP, turn: 'sleep1a', alert: 'jump' });
  const sprite = () => new Sprite({ model: m, map, width: 60, height: 60, stripWidth: 1000, stripHeight: 140, home: 400, wander: 120, rand: seeded(5) });

  test('it stops, turns (the turn animation), then holds and bounces; the bounce grows once confirmed', () => {
    const s = sprite();
    s.setBehaviour('walk', 0);
    s.setBehaviour('alert', 100);
    assert.equal(s.anim.id, map.turn[0]);
    let t = 100;
    while (!s.holding && t < 20_000) s.tick((t += 20));
    assert.ok(s.holding, 'holds after the turn');
    const x = s.x;
    for (let i = 0; i < 50; i++) s.tick((t += 20));
    assert.equal(s.x, x, 'does not walk while alerting');
    assert.equal(s.drawInfo(t).flip, false, 'faces forward, unmirrored');
    const amp = (s0, from) => Math.max(...Array.from({ length: 60 }, (_, i) => s0.bounce(from + i * 20)));
    const calm = amp(s, t);
    assert.ok(calm >= 3 && calm <= 5, `bounce ${calm}`);
    s.setConfirmed(true, t);
    assert.equal(s.oneShot, 'alert', 'the escalation animation plays once');
    while (!s.holding && t < 40_000) s.tick((t += 20));
    assert.ok(s.holding);
    assert.ok(amp(s, t) >= 8, 'bigger bounce once confirmed');
    s.setConfirmed(true, t);
    assert.equal(s.oneShot, null, 'escalates only once');
  });

  test('a click during an alert hops, then goes back to facing forward', () => {
    const s = sprite();
    s.setBehaviour('alert', 0);
    let t = 0;
    while (!s.holding && t < 20_000) s.tick((t += 20));
    s.playOnce('hop', t);
    assert.equal(s.holding, false);
    while (!s.holding && t < 40_000) s.tick((t += 20));
    assert.ok(s.holding);
  });

  test('waiting lies down; no bounce when calm', () => {
    assert.equal(behaviourOf({ state: 'waiting' }), 'lie');
    const s = sprite();
    s.setBehaviour('graze', 0);
    assert.equal(s.bounce(1000), 0);
    assert.equal(s.bouncing, false);
  });
});

describe('lambs beside their parent (Milestone 6)', () => {
  const m = model();
  const map = resolveAnimationMap(m, MAP);
  const geo = { parentW: 60, lambW: 36 };

  test('places alternate right and left, move outwards, and never overlap each other or sit on the parent\'s centre', () => {
    const xs = Array.from({ length: 6 }, (_, k) => lambOffset(k, geo));
    assert.ok(xs[0] > 0 && xs[1] < 0 && xs[2] > xs[0] && xs[3] < xs[1] && xs[4] > xs[2]);
    for (let a = 0; a < xs.length; a++) {
      assert.ok(Math.abs(xs[a]) - geo.lambW / 2 >= geo.parentW / 2, `place ${a} at ${xs[a]} is clear of the parent`);
      for (let b = a + 1; b < xs.length; b++) if (Math.sign(xs[a]) === Math.sign(xs[b])) assert.ok(Math.abs(xs[a] - xs[b]) >= geo.lambW + 4);
    }
    // Five lambs fit within about two and a half slots (110 px), parent included.
    assert.ok(Math.max(...xs.slice(0, 5)) - Math.min(...xs.slice(0, 5)) + geo.lambW <= 275);
  });

  test('a place is reused only once its lamb has gone; places stay on the strip at either edge', () => {
    assert.equal(freePlace(new Set()), 0);
    assert.equal(freePlace(new Set([0, 1, 3])), 2);
    // Beside a parent at either edge, five places are still five different spots.
    for (const parentX of [0, 1728 - 60]) {
      const xs = Array.from({ length: 5 }, (_, k) => lambX(k, { parentX, ...geo, stripWidth: 1728 })).sort((a, b) => a - b);
      for (let i = 1; i < xs.length; i++) assert.ok(xs[i] - xs[i - 1] >= geo.lambW + 4, `parent at ${parentX}: ${xs.join(', ')}`);
    }
    for (const parentX of [-20, 0, 500, 1700]) {
      for (let k = 0; k < 6; k++) {
        const x = lambX(k, { parentX, ...geo, stripWidth: 1728 });
        assert.ok(x >= 0 && x + geo.lambW <= 1728, `place ${k} beside ${parentX}: ${x}`);
      }
    }
  });

  test('a lamb whose place moved walks there (faster when far behind) rather than jumping', () => {
    const lamb = new Sprite({ model: m, map, width: 36, height: 36, stripWidth: 1000, stripHeight: 140, home: 500, wander: 10, rand: seeded(7) });
    lamb.x = 400;
    lamb.setBehaviour('walk', 0);
    let t = 0;
    const seen = [];
    for (let i = 0; i < 400 && lamb.x < 490; i++) {
      t += 50;
      lamb.tick(t);
      seen.push(lamb.x);
    }
    assert.ok(lamb.x >= 490, `reached its place: ${lamb.x}`);
    for (let i = 1; i < seen.length; i++) assert.ok(seen[i] - seen[i - 1] <= 20, `no jump: ${seen[i - 1]} -> ${seen[i]}`);
  });

  test('a leaving lamb trots back into its parent and is gone there; with no parent it runs off the strip', () => {
    const lamb = new Sprite({ model: m, map, width: 36, height: 36, stripWidth: 1000, stripHeight: 140, home: 500, wander: 10, rand: seeded(8) });
    lamb.x = 520;
    lamb.returnTo = () => 450;
    lamb.setBehaviour('leave', 0);
    for (let t = 50; t < 20_000 && !lamb.gone; t += 50) lamb.tick(t);
    assert.ok(lamb.gone);
    assert.equal(lamb.x, 450);
    const orphan = new Sprite({ model: m, map, width: 36, height: 36, stripWidth: 1000, stripHeight: 140, home: 500, wander: 10, rand: seeded(9) });
    orphan.returnTo = () => null;
    orphan.setBehaviour('leave', 0);
    for (let t = 50; t < 60_000 && !orphan.gone; t += 50) orphan.tick(t);
    assert.ok(orphan.gone && (orphan.x < 0 || orphan.x > 1000 - 36));
  });

  test('the webview draws lambs at lambScale, in the parent\'s tint, and a lamb\'s alert gets its own bubble with the parent wearing a tag', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.match(main, /display\.lambScale/);
    assert.match(main, /colourIndex: actor\.colourIndex/);
    assert.match(main, /lambTag\(/);
    // The dashed bubble on the parent is only the fallback for a lamb that isn't on the strip.
    assert.match(main, /if \(onStrip\) tags\.push[\s\S]*?else if \(!actor\.alert\)/);
  });
});

describe('families on the strip (Milestone 6, D2)', () => {
  const geo = { parentW: 60, lambW: 36 };
  const opts = { stripWidth: 1728, slotWidth: 110, wanderMax: 120 };
  let seed = 7;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);

  test('families never overlap, wherever each sheep wanders within its range, and keep slot order', () => {
    for (let n = 0; n < 500; n++) {
      const count = 1 + Math.floor(rnd() * 8);
      const slots = [...new Set(Array.from({ length: count }, () => Math.floor(rnd() * 10)))];
      const sheep = slots.map((slot, i) => ({ id: `s${i}`, slot, extent: familyExtent(Math.floor(rnd() * 7), geo) }));
      const homes = familyHomes(sheep, opts);
      const spans = sheep
        .map((s) => ({ s, h: homes.get(s.id) }))
        .sort((a, b) => a.s.slot - b.s.slot)
        .map(({ s, h }) => [h.centre - h.wander - s.extent.left, h.centre + h.wander + s.extent.right]);
      const total = sheep.reduce((a, s) => a + Math.max(110, s.extent.left + s.extent.right + 8), 0) + 110 * (Math.max(...slots) + 1 - slots.length);
      if (total > 1728 - 48) continue; // too many to fit: scaled down, overlaps allowed
      for (let i = 1; i < spans.length; i++) assert.ok(spans[i - 1][1] <= spans[i][0] + 0.01, `families ${i - 1} and ${i} overlap: ${JSON.stringify(spans)}`);
      for (const [lo, hi] of spans) assert.ok(lo >= -0.01 && hi <= 1728.01);
    }
  });

  test('three sheep with three lambs each, on an empty strip: at least one family width between families', () => {
    const ext = familyExtent(3, geo);
    const sheep = [0, 1, 2].map((slot) => ({ id: `s${slot}`, slot, extent: ext }));
    const h = sheep.map((s) => familyHomes(sheep, opts).get(s.id));
    const width = ext.left + ext.right;
    for (let i = 1; i < 3; i++) assert.ok(h[i].centre - h[i - 1].centre >= 2 * width, `centres ${h[i - 1].centre} and ${h[i].centre}`);
  });

  test('a lamb arriving moves any family by little more than its own width (the gaps adjust a little)', () => {
    const crowded = { ...opts, stripWidth: 700 };
    const sheep = [0, 1, 2, 3].map((slot) => ({ id: `s${slot}`, slot, extent: familyExtent(1, geo) }));
    const before = familyHomes(sheep, crowded);
    sheep[1] = { ...sheep[1], extent: familyExtent(2, geo) };
    const after = familyHomes(sheep, crowded);
    assert.ok(Math.abs(after.get('s0').centre - before.get('s0').centre) <= 10, 'the family before it barely moves');
    for (const id of ['s1', 's2', 's3']) assert.ok(Math.abs(after.get(id).centre - before.get(id).centre) <= geo.lambW + 4 + 10, `${id} moved ${after.get(id).centre - before.get(id).centre}`);
  });

  test('any number of lambs (maxLambsShown above 5) never stand on each other while the strip has room', () => {
    for (const stripWidth of [200, 400, 800, 1728]) {
      for (let parentX = 0; parentX <= stripWidth - 60; parentX += 5) {
        for (let n = 1; n <= 12; n++) {
          if (n * 40 + 60 > stripWidth) continue;
          const xs = Array.from({ length: n }, (_, k) => lambX(k, { parentX, ...geo, stripWidth })).sort((a, b) => a - b);
          for (const x of xs) assert.ok(x >= 0 && x + geo.lambW <= stripWidth);
          for (let i = 1; i < n; i++) assert.ok(xs[i] - xs[i - 1] >= geo.lambW - 0.01, `strip ${stripWidth}, parent ${parentX}, ${n} lambs: ${xs.join(', ')}`);
        }
      }
    }
  });

  test('familyExtent covers every place lambX uses away from the edges', () => {
    for (let n = 0; n <= 8; n++) {
      const e = familyExtent(n, geo);
      for (let k = 0; k < n; k++) {
        const x = lambX(k, { parentX: 800, ...geo, stripWidth: 1728 });
        assert.ok(x >= 830 - e.left - 0.01 && x + geo.lambW <= 830 + e.right + 0.01);
      }
    }
  });
});


describe('resting sprites follow a moving home (Milestone 6, R1)', () => {
  const m = model();
  const map = resolveAnimationMap(m, MAP);
  const lamb = (behaviour) => {
    const l = new Sprite({ model: m, map, width: 36, height: 36, stripWidth: 1000, stripHeight: 140, home: 300, wander: 6, rand: seeded(4) });
    l.setBehaviour(behaviour, 0);
    l.setHome(450);
    return l;
  };
  test('a grazing lamb walks back to its place with the walk animation, then grazes again', () => {
    const l = lamb('graze');
    let walked = false;
    let t = 0;
    for (; t < 30_000 && (l.catchingUp || Math.abs(l.stray()) > 3 || !walked); t += 50) {
      l.tick(t);
      if (l.catchingUp) {
        walked = true;
        assert.ok(map.walk.includes(l.anim.id), 'walking, not sliding');
      }
    }
    assert.ok(walked && !l.catchingUp, `back in its range by ${t} ms`);
    assert.ok(map.graze.includes(l.anim.id), 'grazing again');
  });
  test('a sleeping lamb slides back without waking (its sleep animation keeps playing)', () => {
    const l = lamb('sleep');
    for (let t = 0; t < 10_000; t += 50) {
      l.tick(t);
      assert.ok(map.sleep.includes(l.anim.id));
    }
    assert.ok(Math.abs(l.stray()) <= 3, `stray ${l.stray()}`);
  });
});

describe('unobtrusiveness in the webview (Milestone 7)', () => {
  const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
  test('one long poll carries the shell view too; still only the six commands (and the selftest\'s refused probe)', () => {
    assert.match(main, /invoke\('get_snapshot', \{ after: rev, shellAfter: view\.rev \}\)/);
    const commands = new Set([...main.matchAll(/invoke\('([a-z_]+)'/g)].map((m) => m[1]));
    assert.deepEqual([...commands].sort(), ['debug_snapshot', 'get_pet', 'get_snapshot', 'report_hitboxes', 'setup_status', 'shell_log', 'sprite_clicked']);
    // Milestone 7: setup_status (an onboarding command) is called only by --selftest, which
    // passes only when the strip's capability refuses it.
    assert.equal([...main.matchAll(/invoke\('setup_status'/g)].length, 1);
    const selftest = main.slice(main.indexOf('if (display.selftest) {'), main.indexOf("log(`selftest: done ${missing.length ? 'FAIL' : 'PASS'}`);"));
    assert.match(selftest, /const refused = await invoke\('setup_status'\)\.then\(\(\) => null, /);
    assert.match(selftest, /if \(!refused\) missing\.push\(/);
  });
  test('muted or hidden: no ticking, nothing clickable, only badges (muted) or nothing (hidden)', () => {
    assert.match(main, /const quiet = \(\) => muted \|\| view\.hidden;/);
    assert.match(main, /\.filter\(\(\[, e\]\) => !e\.sprite\.gone && !quiet\(\)\)/);
    assert.match(main, /if \(quiet\(\)\) \{[\s\S]*?return;\s*\}/);
    assert.match(main, /if \(view\.hidden\) return;\s*if \(muted\) \{\s*drawMuted\(\);/);
  });
  test('the fade applies to the sprite and its bubble, and is drawn at once', () => {
    assert.match(main, /d\.opacity \*= fadeOf\(key\)/);
    assert.match(main, /ctx\.globalAlpha = fadeOf\(item\.id\)/);
    assert.match(main, /if \(nearChanged && !quiet\(\)\) \{[\s\S]*?draw\(now\)/);
  });
});
