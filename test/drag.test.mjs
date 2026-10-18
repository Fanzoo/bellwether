import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { describe, test } from 'node:test';
import { buildPet, miniDom } from '../scripts/build-pet.mjs';
import { DRAG_THRESHOLD_PX, dragEvent, idleDrag } from '../shell/src/player/drag.mjs';
import { dropHome, familyHomes, homesWithDrops } from '../shell/src/player/layout.mjs';
import { parsePetDocument, resolveAnimationMap, validatePet } from '../shell/src/player/pet.mjs';
import { Sprite } from '../shell/src/player/sprite.mjs';
import { AGENT_DIR, FIXTURES } from './helpers.mjs';

/**
 * Dragging a sheep along the strip (D10, milestone 12): the press, click or drag state
 * machine, where a dropped sheep lives (dropHome), the sprite's held state, and the
 * strip's own main.js run unchanged in a stub webview (no Tauri, no GUI).
 */

const SRC = path.join(AGENT_DIR, 'shell', 'src');
const MAIN = fs.readFileSync(path.join(SRC, 'main.js'), 'utf8');

/** Feeds a list of events through the machine; returns the final state and every action. */
function run(events, state = idleDrag()) {
  const actions = [];
  for (const ev of events) {
    const r = dragEvent(state, ev);
    state = r.state;
    actions.push(...r.actions);
  }
  return { state, actions, did: actions.map((a) => a.do) };
}
const down = (o = {}) => ({ type: 'down', id: 's1', button: 0, x: 100, y: 50, left: 80, ...o });
const move = (x, y = 50) => ({ type: 'move', x, y });
const up = (x = 100, y = 50) => ({ type: 'up', x, y });

describe('press, click or drag (drag.mjs)', () => {
  test('the threshold: 3.9 px is still a click, 4.0 px is a drag', () => {
    assert.equal(DRAG_THRESHOLD_PX, 4);
    const click = run([down(), move(103.9), up(103.9)]);
    assert.deepEqual(click.did, ['press', 'click', 'release'], 'released within the threshold: a click, on release');
    const drag = run([down(), move(104)]);
    assert.deepEqual(drag.did, ['press', 'lift', 'carry']);
    assert.equal(drag.actions[1].grabDx, 20, 'the grab offset: cursor x − sprite left');
    assert.equal(drag.state.phase, 'carrying');
    // Diagonally: the distance, not either axis.
    assert.deepEqual(run([down(), move(102.8, 52.8)]).did, ['press'], '3.96 px');
    assert.deepEqual(run([down(), move(102.9, 52.9)]).did, ['press', 'lift', 'carry'], '4.1 px');
  });

  test('a long press without moving is a click, and nothing happens on the press itself', () => {
    const pressed = run([down()]);
    assert.deepEqual(pressed.did, ['press'], 'no click on the press: a press cannot know yet whether it will be a drag');
    const r = run([down(), move(101), move(99, 51), move(100), up()]);
    assert.deepEqual(r.did, ['press', 'click', 'release']);
    assert.equal(r.state.phase, 'idle');
  });

  test('a release after the lift is a drop, never a click', () => {
    const r = run([down(), move(110), move(300, 20), up(300, 20)]);
    assert.deepEqual(r.did, ['press', 'lift', 'carry', 'carry', 'drop']);
    assert.ok(!r.did.includes('click'), 'a drag never focuses anything');
    assert.deepEqual(r.actions.at(-1), { do: 'drop', id: 's1', x: 300, y: 20 });
    assert.equal(r.state.phase, 'idle');
  });

  test('a lamb press: a click within the threshold, nothing at all beyond it', () => {
    assert.deepEqual(run([down({ lamb: true }), move(102), up(102)]).did, ['press', 'click', 'release']);
    const r = run([down({ lamb: true }), move(120), move(300), up(300)]);
    assert.deepEqual(r.did, ['press', 'log', 'release'], 'no lift, no carry, no click');
    assert.equal(r.actions[1].line, 'lamb drag ignored');
  });

  test('a right or middle press keeps its old meaning: a click at once, with no hold', () => {
    for (const button of [1, 2]) {
      const r = run([down({ button }), move(200), up(200)]);
      assert.deepEqual(r.did, ['click'], `button ${button}`);
      assert.equal(r.state.phase, 'idle');
    }
  });

  test('a primary press with a modifier key is a drag or a click like any other', () => {
    assert.deepEqual(run([down({ shiftKey: true, metaKey: true }), up()]).did, ['press', 'click', 'release']);
    assert.deepEqual(run([down({ altKey: true }), move(150)]).did, ['press', 'lift', 'carry']);
  });

  test('every cancel path ends the gesture with nothing recorded, and says whether the sheep had been lifted', () => {
    for (const reason of ['pointercancel', 'Quiet is on', 'the strip is hidden', 'its session ended', 'the shell ended it']) {
      const pressed = run([down(), { type: 'cancel', reason }]);
      assert.deepEqual(pressed.actions.at(-1), { do: 'cancel', id: 's1', reason, lifted: false });
      const carried = run([down(), move(150), { type: 'cancel', reason }]);
      assert.deepEqual(carried.actions.at(-1), { do: 'cancel', id: 's1', reason, lifted: true });
      assert.equal(carried.state.phase, 'idle');
      // After a cancel, the release that follows does nothing.
      assert.deepEqual(run([up()], carried.state).did, []);
    }
    assert.deepEqual(run([{ type: 'cancel', reason: 'x' }]).did, [], 'nothing under way: nothing to cancel');
    const lamb = run([down({ lamb: true }), move(150), { type: 'cancel', reason: 'pointercancel' }]);
    assert.equal(lamb.actions.at(-1).lifted, false);
  });

  test('only the pressing button ends the gesture: another button\'s down or up mid-gesture is ignored', () => {
    // By `button` (no bitmask): a right button coming up while the left is held.
    const right = run([down(), move(150), { type: 'down', id: 's2', button: 2, x: 150, y: 50 }, { type: 'up', x: 150, y: 50, button: 2 }, move(200)]);
    assert.deepEqual(right.did, ['press', 'lift', 'carry', 'carry'], 'still carrying: no drop, no click for s2');
    assert.equal(right.state.phase, 'carrying');
    assert.deepEqual(run([up(200)], right.state).did, ['drop'], 'the left button\'s release drops it');
    // Pressed but not yet lifted: a middle release doesn't click.
    const pressed = run([down(), { type: 'up', x: 100, y: 50, button: 1 }]);
    assert.deepEqual(pressed.did, ['press']);
    assert.equal(pressed.state.phase, 'pressed');
    // By `buttons` (pointer events' bitmask of what is still held): a pointerup that still
    // has the primary's bit is another button's, whatever `button` says.
    const chord = run([down(), move(150), { type: 'up', x: 150, y: 50, button: 2, buttons: 1 }]);
    assert.equal(chord.state.phase, 'carrying');
    // The left released first under chording comes as a move without the primary's bit: it drops.
    assert.deepEqual(run([{ type: 'move', x: 160, y: 50, buttons: 2 }], chord.state).did, ['drop']);
    // ... and the last release of a chord, reported as the right button's, with nothing held.
    assert.deepEqual(run([{ type: 'up', x: 160, y: 50, button: 2, buttons: 0 }], chord.state).did, ['drop']);
    // A lamb's ignored drag ends only on its own button too.
    const lamb = run([down({ lamb: true }), move(150), { type: 'up', x: 150, y: 50, button: 2 }]);
    assert.equal(lamb.state.phase, 'ignored');
    assert.deepEqual(run([up(150)], lamb.state).did, ['release']);
  });

  test('one gesture at a time, and nonsense events are ignored', () => {
    const r = run([down(), down({ id: 's2' }), move(150)]);
    assert.ok(r.actions.every((a) => a.id === 's1'));
    assert.deepEqual(run([move(150), up(), { type: 'what' }, null]).did, []);
    assert.deepEqual(run([down({ id: null })]).did, []);
  });
});

describe('where a dropped sheep lives (dropHome)', () => {
  const ext = { left: 30, right: 30 };
  const opts = { stripWidth: 1000, wanderMax: 120 };

  test('a clear spot: its home is the drop, wandering homeWanderPx cut to the room on the nearer side', () => {
    assert.deepEqual(dropHome({ centre: 500, extent: ext, dir: 1 }, [], opts), { centre: 500, wander: 120, noRoom: false, moved: false });
    // A neighbour's reach ends at 400: from 500 there is 500 − 30 − 400 = 70 px of room on that side.
    const h = dropHome({ centre: 500, extent: ext, dir: 1 }, [{ lo: 100, hi: 400 }], opts);
    assert.deepEqual(h, { centre: 500, wander: 70, noRoom: false, moved: false });
  });

  test('onto another sheep: the nearest centre where its family fits clear; the other sheep is not moved', () => {
    const others = [{ lo: 400, hi: 600 }];
    // Dropped at 520: clear at 370 (left, 150 away) or 630 (right, 110 away).
    const h = dropHome({ centre: 520, extent: ext, dir: -1 }, others, opts);
    assert.deepEqual(h, { centre: 630, wander: 0, noRoom: false, moved: true });
    assert.deepEqual(others, [{ lo: 400, hi: 600 }]);
  });

  test('a tie goes the way it was being carried', () => {
    const others = [{ lo: 400, hi: 600 }];
    assert.equal(dropHome({ centre: 500, extent: ext, dir: 1 }, others, opts).centre, 630);
    assert.equal(dropHome({ centre: 500, extent: ext, dir: -1 }, others, opts).centre, 370);
  });

  test('no room anywhere on the strip: it stays where it was dropped, with no wander', () => {
    const h = dropHome({ centre: 500, extent: ext, dir: 1 }, [{ lo: 0, hi: 520 }, { lo: 510, hi: 1000 }], opts);
    assert.deepEqual(h, { centre: 500, wander: 0, noRoom: true, moved: false });
  });

  test('the strip\'s edges: the drop is kept on the strip with its whole family, and the wander inside it', () => {
    assert.deepEqual(dropHome({ centre: -40, extent: ext, dir: -1 }, [], opts), { centre: 30, wander: 0, noRoom: false, moved: false });
    assert.deepEqual(dropHome({ centre: 990, extent: { left: 30, right: 80 }, dir: 1 }, [], opts), { centre: 920, wander: 0, noRoom: false, moved: false });
    assert.equal(dropHome({ centre: 100, extent: ext, dir: 1 }, [], opts).wander, 70, 'cut to the room before the left edge');
  });

  const fam = (id, slot, drop) => ({ id, slot, extent: { left: 30, right: 30 }, ...(drop ? { drop } : {}) });
  const layout = { stripWidth: 1200, slotWidth: 110, wanderMax: 120 };
  const reachOf = (h, f) => [h.centre - h.wander - f.extent.left, h.centre + h.wander + f.extent.right];
  const clear = (families, homes) => {
    const boxes = families.map((f) => reachOf(homes.get(f.id), f)).sort((a, b) => a[0] - b[0]);
    return boxes.every((b, i) => i === 0 || boxes[i - 1][1] <= b[0] + 0.5);
  };

  test('the ghost band: picked up and dropped on its own strip, it moves nobody else', () => {
    const before = familyHomes([fam('a', 0), fam('b', 1), fam('c', 2)], layout);
    for (const centre of [60, 400, 700, 1150]) {
      const after = homesWithDrops([fam('a', 0), fam('b', 1, { centre, seq: 1, ghost: true, dir: 1 }), fam('c', 2)], layout);
      assert.deepEqual(after.get('a'), before.get('a'), `a, b dropped at ${centre}`);
      assert.deepEqual(after.get('c'), before.get('c'), `c, b dropped at ${centre}`);
    }
    // Dropped back into its own (empty) band: it stays there.
    const own = before.get('b').centre;
    const back = homesWithDrops([fam('a', 0), fam('b', 1, { centre: own, seq: 1, ghost: true, dir: 1 }), fam('c', 2)], layout);
    assert.equal(back.get('b').centre, own);
    assert.equal(back.get('b').moved, false);
  });

  test('a guest from another monitor takes no band: the strip\'s own sheep spread as they would without it', () => {
    const own = familyHomes([fam('a', 0), fam('c', 2)], layout);
    const withGuest = homesWithDrops([fam('a', 0), fam('g', 1, { centre: 900, seq: 1, ghost: false, dir: 1 }), fam('c', 2)], layout);
    assert.deepEqual(withGuest.get('a'), own.get('a'));
    assert.deepEqual(withGuest.get('c'), own.get('c'));
    assert.ok(clear([fam('a', 0), fam('g', 1), fam('c', 2)], withGuest));
  });

  test('onto a sheep through homesWithDrops: it walks to the nearest clear spot and the other keeps its home', () => {
    const before = familyHomes([fam('a', 0), fam('c', 2)], layout);
    const c = before.get('c').centre;
    const homes = homesWithDrops([fam('a', 0), fam('b', 5, { centre: c + 5, seq: 1, ghost: false, dir: 1 }), fam('c', 2)], layout);
    assert.deepEqual(homes.get('c'), before.get('c'), 'the slotted sheep never moves for a dropped one');
    assert.equal(homes.get('b').moved, true);
    assert.ok(homes.get('b').centre > c, 'nearest clear spot, and on a near-tie the way it was carried');
    assert.ok(clear([fam('a', 0), fam('b', 5), fam('c', 2)], homes));
  });

  test('the Dock extent: the slotted sheep stand on the Dock; the drop stands where it was dropped, beside the Dock', () => {
    const dock = { lo: 400, hi: 800 };
    const before = familyHomes([fam('a', 0), fam('c', 1)], { ...layout, dock });
    const homes = homesWithDrops([fam('a', 0), fam('b', 2, { centre: 1000, seq: 1, ghost: false, dir: 1 }), fam('c', 1)], { ...layout, dock });
    assert.deepEqual(homes.get('a'), before.get('a'));
    assert.deepEqual(homes.get('c'), before.get('c'));
    assert.equal(homes.get('b').centre, 1000, 'off the Dock, like an overflow family');
    const onDock = homesWithDrops([fam('a', 0), fam('b', 2, { centre: 600, seq: 1, ghost: false, dir: 1 }), fam('c', 1)], { ...layout, dock });
    assert.ok(clear([fam('a', 0), fam('b', 2), fam('c', 1)], onDock), 'on the Dock, among the others: clear of them');
  });

  test('re-checked when the layout changes: a new family arriving pushes the dropped sheep aside, not the other way round', () => {
    const one = homesWithDrops([fam('a', 0), fam('b', 3, { centre: 400, seq: 1, ghost: false, dir: 1 })], layout);
    assert.equal(one.get('b').centre, 400);
    const arrived = [fam('a', 0), fam('b', 3, { centre: 400, seq: 1, ghost: false, dir: 1 }), fam('n', 1)];
    const two = homesWithDrops(arrived, layout);
    const n = familyHomes([fam('a', 0), fam('n', 1)], layout).get('n');
    assert.deepEqual(two.get('n'), n, 'the newcomer takes its slot as if no drop were there');
    assert.ok(clear(arrived, two));
  });

  test('two dropped sheep treat each other\'s reach like any other; the later drop gives way', () => {
    const fams = [fam('x', 0, { centre: 500, seq: 2, ghost: false, dir: 1 }), fam('y', 1, { centre: 505, seq: 1, ghost: false, dir: 1 })];
    const homes = homesWithDrops(fams, layout);
    assert.equal(homes.get('y').centre, 505, 'the earlier drop stays');
    assert.equal(homes.get('y').moved, false);
    assert.equal(homes.get('x').moved, true, 'the later one moves aside');
    assert.ok(clear(fams, homes));
  });
});

describe('the sprite while carried', () => {
  const m = validatePet(JSON.parse(fs.readFileSync(path.join(FIXTURES, 'pet-model.json'), 'utf8')));
  // Distinct animations for each, so the sequence can be read off the ids.
  const MAP = { walk: 'walk', graze: 'eat', sleep: 'sleep1a', carried: 'sleep1b', land: 'jump', turn: 'run', alert: 'eat', leave: 'walk' };
  const map = resolveAnimationMap(m, MAP);
  const id = (name) => m.byName[name];
  const sprite = (o = {}) => new Sprite({ model: m, map, width: 60, height: 60, stripWidth: 1000, stripHeight: 140, home: 400, wander: 120, rand: () => 0.5, ...o });
  /** Ticks to `until`, returning the distinct animation ids in the order they played. */
  const play = (s, from, until) => {
    const seen = [];
    for (let t = from; t <= until; t += 20) {
      s.tick(t);
      if (seen.at(-1) !== s.anim.id) seen.push(s.anim.id);
    }
    return seen;
  };

  test('held, it keeps the snapshot\'s behaviours but shows `carried`; set down, it falls, lands, then shows the kept one', () => {
    const s = sprite();
    s.setBehaviour('graze', 0);
    s.hold(100);
    assert.equal(s.anim.id, id('sleep1b'));
    s.setBehaviour('sleep', 150);
    assert.equal(s.behaviour, 'sleep', 'kept');
    assert.deepEqual(play(s, 200, 1000), [id('sleep1b')], 'still carried');
    s.carry(250, 20);
    assert.deepEqual([s.x, s.drawInfo().y], [250, 20], 'dangling with its top at the cursor');
    s.carry(-50, -400);
    assert.deepEqual([s.x, s.drawInfo().y], [0, 0], 'kept inside the strip window');
    s.carry(300, 500);
    assert.equal(s.drawInfo().y, 80, 'never below the ground');
    s.carry(300, 0);
    assert.ok(s.setDown(1000));
    assert.ok(s.fall && s.fall.ms > 0, 'a fall from where it was held');
    const ys = [];
    for (let t = 1000; t < 1000 + s.fall.ms; t += 20) ys.push(s.drawInfo(t).y);
    ys.push(s.drawInfo(1000 + s.fall.ms).y);
    assert.ok(ys.every((y, i) => i === 0 || y >= ys[i - 1]) && ys[0] === 0 && ys.at(-1) === 80, `falls to the ground: ${ys}`);
    assert.deepEqual(play(s, 1000, 5000), [id('sleep1b'), id('jump'), id('sleep1a')], 'carried, land, then its kept behaviour');
    assert.equal(s.x, 300, 'it landed where it was let go');
  });

  test('`carried` falls back to `walk`, and a pet without `land` just carries on, with no warning', () => {
    const warnings = [];
    const plain = resolveAnimationMap(m, { walk: 'walk', graze: 'eat' });
    const s = new Sprite({ model: m, map: plain, width: 60, height: 60, stripWidth: 1000, stripHeight: 140, home: 400, wander: 120, rand: () => 0.5, warn: (w) => warnings.push(w) });
    s.setBehaviour('graze', 0);
    s.hold(10);
    assert.equal(s.anim.id, id('walk'), 'its walk frames while carried');
    s.setDown(100);
    assert.deepEqual(play(s, 100, 3000).filter((a) => a !== id('walk')), [id('eat')], 'no land: straight back to grazing');
    assert.deepEqual(warnings, []);
  });

  test('`land` plays once per drop: the same seq again only sets it down', () => {
    const s = sprite();
    s.setBehaviour('graze', 0);
    s.hold(0);
    assert.equal(s.setDown(0, { seq: 7 }), true);
    assert.ok(play(s, 0, 3000).includes(id('jump')));
    s.hold(3000);
    assert.equal(s.setDown(3000, { seq: 7 }), false, 'seq 7 again');
    assert.ok(!play(s, 3000, 6000).includes(id('jump')), 'no second landing');
    s.hold(6000);
    assert.equal(s.setDown(6000, { seq: 8 }), true);
    assert.ok(play(s, 6000, 9000).includes(id('jump')), 'a new drop lands');
  });

  test('an alert that arrives while held starts after the landing: land, then the turn, hold and bounce', () => {
    const s = sprite();
    s.setBehaviour('walk', 0);
    s.hold(10);
    s.carry(400, 0);
    s.setBehaviour('alert', 50);
    assert.equal(s.anim.id, id('sleep1b'), 'no turn while carried');
    assert.equal(s.bounce(60), 0, 'no bounce while carried');
    s.setDown(100);
    assert.deepEqual(play(s, 100, 4000), [id('sleep1b'), id('jump'), id('run')], 'carried, land, turn');
    assert.ok(s.holding, 'then it holds and bounces');
    assert.ok(s.bounce(4200) >= 0);
  });

  test('a confirmed escalation that arrives mid-carry plays after the landing', () => {
    const s = sprite();
    s.setBehaviour('alert', 0);
    play(s, 0, 2000);
    s.hold(2000);
    s.carry(400, 0);
    s.setConfirmed(true, 2100);
    assert.equal(s.anim.id, id('sleep1b'), 'not now');
    s.setDown(2200);
    assert.deepEqual(play(s, 2200, 6000), [id('sleep1b'), id('jump'), id('eat'), id('run')], 'land, the alert animation, the turn');
    assert.ok(s.holding && s.confirmed);
  });

  test('`leave` while held: set down where it is, and it runs off', () => {
    const s = sprite();
    s.setBehaviour('graze', 0);
    s.hold(0);
    s.carry(700, 10);
    s.setBehaviour('leave', 100);
    assert.equal(s.held, false);
    assert.equal(s.behaviour, 'leave');
    assert.equal(s.drawInfo().y, 80, 'on the ground');
    for (let t = 100; t < 20000 && !s.gone; t += 50) s.tick(t);
    assert.ok(s.gone, 'ran off the strip');
  });
});

// ------------------------------------------------------------ main.js, in a stub webview

/** main.js and the player modules it uses, as one classic script (as hover-label.test does). */
function stripScript() {
  const done = new Set();
  const out = [];
  const imports = (src) => [...src.matchAll(/^import \{([^}]+)\} from '\.\/(?:player\/)?(\w+)\.mjs';$/gm)];
  const asConst = (src) => src.replace(/^import \{([^}]+)\} from '\.\/(?:player\/)?(\w+)\.mjs';$/gm, (_, list, from) => `const {${list.replace(/\bas\b/g, ':')}} = M_${from};`);
  const load = (m) => {
    if (done.has(m)) return;
    done.add(m);
    const src = fs.readFileSync(path.join(SRC, 'player', `${m}.mjs`), 'utf8');
    for (const [, , from] of imports(src)) load(from);
    const names = [...src.matchAll(/^export (?:async )?(?:function|const|class|let) (\w+)/gm)].map((x) => x[1]);
    out.push(`const M_${m} = (() => {\n${asConst(src).replace(/^export /gm, '')}\nreturn { ${names.join(', ')} };\n})();`);
  };
  for (const [, , from] of imports(MAIN)) load(from);
  const handle = 'globalThis.__strip = { sprites, frame, apply, applyView, draw, gesture: () => gesture, carried: () => carried, guest: () => guest };';
  return `${out.join('\n\n')}\n\n${asConst(MAIN)}\n${handle}`;
}

function recordingContext(screen) {
  const target = {
    fillStyle: '',
    globalAlpha: 1,
    measureText: (t) => ({ width: [...String(t)].length * 6 }),
    clearRect: () => (screen.length = 0),
    roundRect: (x, y, w, h) => screen.push({ op: 'roundRect', x, y, w, h }),
    fillText: (text, x, y) => screen.push({ op: 'fillText', text, x, y }),
    arc: (x, y) => screen.push({ op: 'arc', x, y }),
    drawImage: (...a) => screen.push({ op: 'drawImage', alpha: target.globalAlpha, args: a }),
    getImageData: (x, y, w, h) => ({ data: new Uint8ClampedArray(w * h * 4), width: w, height: h }),
  };
  return new Proxy(target, { get: (t, k) => (k in t ? t[k] : () => {}), set: (t, k, v) => ((t[k] = v), true) });
}

const sheep = (id, slot, o = {}) => ({ id, identityKey: `k:${id}`, label: `p${slot}`, slot, colourIndex: slot, state: 'idle', ...o });

async function bootStrip(flock = [sheep('s1', 0), sheep('s2', 1), sheep('s3', 2)]) {
  const built = buildPet('bellwether');
  const model = parsePetDocument(miniDom(built.xml));
  const screen = [];
  const listeners = {};
  const calls = [];
  let clock = 1000;
  let release = (args) => ({ monitor: 'primary', centreX: args.x, seq: 1, ghost: true, here: true });
  const display = { palette: ['#FF8C00', '#1F5BFF', '#202020', '#E8202A'], scale: 1.5, slotWidthPx: 110, homeWanderPx: 0, tint: 'horns', lambScale: 0.6, fadeOpacity: 0.25 };
  const invoke = (cmd, args) => {
    calls.push({ cmd, ...args });
    if (cmd === 'get_pet') return Promise.resolve({ display, xml: built.xml, folder: 'bellwether', animationMap: built.pin.animationMap, overlays: built.pin.overlays ?? {} });
    if (cmd === 'get_snapshot') return new Promise(() => {});
    if (cmd === 'sprite_drag' && args.phase === 'release' && Number.isFinite(args.x)) return Promise.resolve(release(args));
    return Promise.resolve(null);
  };
  const canvas = { width: 0, height: 0, addEventListener: (t, f) => (listeners[t] = f), getContext: () => recordingContext(screen) };
  const document = {
    visibilityState: 'visible',
    getElementById: () => canvas,
    createElement: () => ({ width: 0, height: 0, getContext: () => recordingContext([]) }),
    addEventListener: () => {},
  };
  const sandbox = {
    window: { __TAURI__: { core: { invoke } }, innerWidth: 1200, innerHeight: 140, devicePixelRatio: 2, screenX: 0, screenY: 0, addEventListener: () => {} },
    location: { search: '?monitor=Built-in' },
    screen: { width: 1200, height: 800 },
    document,
    performance: { now: () => clock },
    setInterval: () => 0,
    setTimeout,
    DOMParser: class {
      parseFromString(xml) {
        return { ...miniDom(xml), getElementsByTagName: () => [] };
      }
    },
    Image: class {
      set src(_) {}
      get width() {
        return model.tilesx * 40;
      }
      get height() {
        return model.tilesy * 40;
      }
      decode() {
        return Promise.resolve();
      }
    },
    ImageData: class {
      constructor(data, width, height) {
        Object.assign(this, { data, width, height });
      }
    },
    URLSearchParams,
    console,
  };
  sandbox.globalThis = sandbox;
  vm.runInNewContext(stripScript(), sandbox);
  const S = sandbox.__strip;
  let rev = 1;
  for (let i = 0; i < 200 && !S.sprites.size; i++) {
    await new Promise((r) => setTimeout(r, 5));
    try {
      S.apply({ rev, muted: false, flock });
    } catch {
      // the pet isn't loaded yet
    }
  }
  assert.ok(S.sprites.size >= flock.length, 'the strip took the flock');
  let viewRev = 0;
  const strip = {
    S,
    screen,
    calls,
    flock,
    setRelease: (f) => (release = f),
    entry: (id) => S.sprites.get(id),
    centre(id) {
      const { sprite } = S.sprites.get(id);
      const d = sprite.drawInfo();
      return { x: d.x + sprite.width / 2, y: d.y + sprite.height / 2 };
    },
    snapshot(f = strip.flock, o = {}) {
      strip.flock = f;
      S.apply({ rev: ++rev, muted: false, flock: f, ...o });
      strip.step();
    },
    view(v = {}) {
      S.applyView({ rev: ++viewRev, hidden: false, showLabels: false, near: [], over: [], ...v });
      strip.step();
    },
    step(n = 1) {
      for (let i = 0; i < n; i++) {
        clock += 50;
        S.frame();
      }
    },
    /** Lets promise callbacks (the shell's answers) run. */
    settle: () => new Promise((r) => setTimeout(r, 0)),
    pointer(type, x, y, button = 0, buttons) {
      listeners[type]?.({ offsetX: x, offsetY: y, button, buttons, pointerId: 1 });
      strip.step();
    },
    dragCalls: () => calls.filter((c) => c.cmd === 'sprite_drag').map((c) => c.phase),
    clicks: () => calls.filter((c) => c.cmd === 'sprite_clicked').map((c) => c.id),
    logs: () => calls.filter((c) => c.cmd === 'shell_log').map((c) => c.line),
    home: (id) => S.sprites.get(id).sprite.home,
  };
  strip.view();
  strip.step(3);
  return strip;
}

/** Press on a sheep and carry it `dx` px. */
function lift(strip, id, dx = 60) {
  const c = strip.centre(id);
  strip.pointer('pointerdown', c.x, c.y);
  strip.pointer('pointermove', c.x + dx, c.y - 20);
  return c;
}

describe('the strip: click on release, drag, drop (main.js)', () => {
  test('a press and release within 4 px is a click on release: the hop and sprite_clicked, after the press', async () => {
    const strip = await bootStrip();
    const c = strip.centre('s1');
    strip.pointer('pointerdown', c.x, c.y);
    assert.deepEqual(strip.clicks(), [], 'nothing on the press');
    assert.deepEqual(strip.dragCalls(), ['press'], 'the shell is told, so the window keeps the mouse');
    assert.equal(strip.entry('s1').sprite.oneShot, null, 'no hop yet');
    strip.pointer('pointermove', c.x + 3, c.y);
    strip.pointer('pointerup', c.x + 3, c.y);
    assert.deepEqual(strip.clicks(), ['s1']);
    assert.equal(strip.entry('s1').sprite.oneShot, 'hop');
    assert.deepEqual(strip.dragCalls(), ['press', 'release'], 'the hold ends');
    assert.ok(!strip.calls.some((c) => c.phase === 'lift'));
  });

  test('a right click while the left button carries a sheep neither drops it nor clicks anything', async () => {
    const strip = await bootStrip();
    const s = strip.entry('s1').sprite;
    const c = lift(strip, 's1', 100);
    const s2 = strip.centre('s2');
    strip.pointer('pointerdown', s2.x, s2.y, 2, 3);
    strip.pointer('pointerup', s2.x, s2.y, 2, 1);
    assert.ok(s.held, 'still carried');
    assert.deepEqual(strip.clicks(), [], 'no click on s2');
    assert.deepEqual(strip.dragCalls(), ['press', 'lift'], 'no release, no second press');
    strip.pointer('pointermove', c.x + 200, c.y, 0, 1);
    assert.ok(s.held);
    strip.pointer('pointerup', c.x + 200, c.y, 0, 0);
    assert.equal(s.held, false);
    assert.deepEqual(strip.dragCalls(), ['press', 'lift', 'release']);
    await strip.settle();
    assert.deepEqual(strip.clicks(), []);
  });

  test('a right press clicks at once, as before, with no hold', async () => {
    const strip = await bootStrip();
    const c = strip.centre('s2');
    strip.pointer('pointerdown', c.x, c.y, 2);
    assert.deepEqual(strip.clicks(), ['s2']);
    assert.deepEqual(strip.dragCalls(), []);
  });

  test('a drag: lifted past 4 px, carried under the cursor, dropped where it is let go; no terminal comes forward', async () => {
    const strip = await bootStrip();
    const s = strip.entry('s1').sprite;
    const startX = s.x;
    const c = lift(strip, 's1', 4);
    const liftCall = strip.calls.find((x) => x.phase === 'lift');
    assert.ok(liftCall, 'lifted at 4 px');
    assert.equal(liftCall.width, s.width);
    assert.ok(Math.abs(liftCall.grabDx - (c.x - startX)) < 1e-9, 'grab offset: cursor x − sprite left');
    assert.ok(s.held);
    strip.pointer('pointermove', c.x + 300, c.y - 30);
    assert.equal(s.x, startX + 300, 'follows the cursor along the strip');
    assert.equal(s.drawInfo().y, Math.max(0, c.y - 30), 'dangles with its top at the cursor');
    strip.pointer('pointerup', c.x + 300, c.y - 30);
    const rel = strip.calls.find((x) => x.phase === 'release');
    assert.equal(rel.x, s.x + s.width / 2, 'the drop centre, in this strip\'s px');
    assert.equal(s.held, false);
    await strip.settle();
    strip.step(60);
    assert.equal(s.landedSeq, 1);
    assert.equal(s.home, rel.x - s.width / 2, 'its new home is the drop centre');
    assert.deepEqual(strip.clicks(), [], 'a drag never focuses anything');
    // The shell's record arrives: no second landing, home unchanged.
    strip.snapshot([sheep('s1', 0, { dropped: { centreX: rel.x, seq: 1, ghost: true } }), sheep('s2', 1), sheep('s3', 2)]);
    assert.equal(s.landedSeq, 1);
    assert.equal(s.fall, null);
    assert.equal(s.home, rel.x - s.width / 2);
  });

  test('nobody else moves: dropping a sheep on its own strip moves no other sheep\'s home', async () => {
    const strip = await bootStrip();
    const before = { s1: strip.home('s1'), s3: strip.home('s3') };
    const c = lift(strip, 's2', 40);
    strip.pointer('pointermove', c.x + 250, c.y);
    strip.pointer('pointerup', c.x + 250, c.y);
    await strip.settle();
    strip.step(5);
    assert.deepEqual({ s1: strip.home('s1'), s3: strip.home('s3') }, before);
    strip.snapshot([sheep('s1', 0), sheep('s2', 1, { dropped: { centreX: c.x + 250, seq: 1, ghost: true } }), sheep('s3', 2)]);
    assert.deepEqual({ s1: strip.home('s1'), s3: strip.home('s3') }, before, 'nor after the shell\'s record');
  });

  test('the carried sheep is drawn last, above the bubbles, at full opacity, wearing a badge for its alert', async () => {
    const strip = await bootStrip([sheep('s1', 0, { state: 'busy', alert: { kind: 'permission', text: 'Bash: ls?', since: 0, confirmed: false } }), sheep('s2', 1, { alert: { kind: 'input', text: 'go on?', since: 0, confirmed: false } })]);
    strip.step(5);
    lift(strip, 's1', 30);
    // The cursor is near it: the fade would apply to any other sprite.
    strip.view({ near: ['s1', 's2'] });
    strip.step(2);
    strip.S.draw(99_999);
    const ops = strip.screen;
    const lastImage = ops.map((o, i) => [o, i]).filter(([o]) => o.op === 'drawImage').at(-1);
    const lastBubble = ops.map((o, i) => [o, i]).filter(([o]) => o.op === 'fillText' && o.text === 'go on?').at(-1);
    assert.ok(lastBubble, 's2 still has its bubble');
    assert.ok(lastImage[1] > lastBubble[1], 'drawn after the bubbles');
    assert.equal(lastImage[0].alpha, 1, 'exempt from the fade');
    assert.ok(!ops.some((o) => o.op === 'fillText' && o.text === 'Bash: ls?'), 'no bubble while carried');
    assert.ok(ops.slice(lastImage[1]).some((o) => o.op === 'arc'), 'the "!" badge, after it');
    assert.ok(strip.logs().some((l) => l === 'alert shown: s1 permission'), 'the badge is logged as the alert shown');
  });

  for (const [how, cause, reason] of [
    ['Quiet turned on', (strip) => strip.snapshot(strip.flock, { muted: true }), 'Quiet is on'],
    ['Hide everything', (strip) => strip.view({ hidden: true }), 'the strip is hidden'],
    ['pointercancel', (strip) => strip.pointer('pointercancel', -1, -1), 'pointercancel'],
  ]) {
    test(`${how} mid-drag cancels it: the sheep is back where it was picked up, and the shell hears why`, async () => {
      const strip = await bootStrip();
      const s = strip.entry('s1').sprite;
      const from = s.x;
      lift(strip, 's1', 200);
      assert.notEqual(s.x, from);
      cause(strip);
      assert.equal(strip.S.gesture().phase, 'idle');
      assert.equal(strip.S.carried(), null);
      assert.equal(s.held, false);
      assert.equal(s.x, from, 'set down where it was picked up');
      const cancel = strip.calls.find((c) => c.phase === 'cancel');
      assert.equal(cancel?.reason, reason);
      assert.ok(!strip.calls.some((c) => c.phase === 'release'), 'nothing recorded');
    });
  }

  test('Quiet and Hide: nothing can be picked up while either is on', async () => {
    for (const quiet of [(strip) => strip.snapshot(strip.flock, { muted: true }), (strip) => strip.view({ hidden: true })]) {
      const strip = await bootStrip();
      quiet(strip);
      const c = strip.centre('s1');
      strip.pointer('pointerdown', c.x, c.y);
      strip.pointer('pointermove', c.x + 100, c.y);
      assert.deepEqual(strip.dragCalls(), []);
      assert.equal(strip.entry('s1').sprite.held, false);
    }
  });

  test('the shell\'s own cancel (its fail-safe, a display change): set back down, no second word to the shell', async () => {
    const strip = await bootStrip();
    const s = strip.entry('s1').sprite;
    const from = s.x;
    lift(strip, 's1', 200);
    strip.view({ dragCancel: { id: 's1', n: 1 } });
    assert.equal(strip.S.gesture().phase, 'idle');
    assert.equal(s.x, from);
    assert.ok(!strip.calls.some((c) => c.phase === 'cancel'), 'the shell already let go');
    // The release that comes later lands on nothing.
    strip.pointer('pointerup', 0, 0);
    assert.ok(!strip.calls.some((c) => c.phase === 'release'));
  });

  test('the session ends mid-drag: the drag ends at once, and the sheep runs off from where it is', async () => {
    const strip = await bootStrip();
    const s = strip.entry('s1').sprite;
    lift(strip, 's1', 300);
    const at = s.x;
    strip.snapshot([sheep('s1', 0, { state: 'leaving' }), sheep('s2', 1), sheep('s3', 2)]);
    assert.equal(strip.S.gesture().phase, 'idle');
    assert.equal(strip.calls.find((c) => c.phase === 'cancel')?.reason, 'its session ended');
    assert.equal(s.held, false);
    assert.equal(s.behaviour, 'leave');
    assert.ok(Math.abs(s.x - at) < 20, 'set down where it was carried, not back at its slot');
  });

  test('lambs follow their parent along the strip, and can\'t be dragged themselves', async () => {
    const lambs = [{ id: 'l1', label: 'Explore', state: 'busy', flavour: 'reading' }];
    const strip = await bootStrip([sheep('s1', 0, { lambs }), sheep('s2', 1)]);
    strip.step(80);
    const lamb = strip.entry('s1/l1');
    const lc = { x: lamb.sprite.drawInfo().x + lamb.sprite.width / 2, y: lamb.sprite.drawInfo().y + lamb.sprite.height / 2 };
    strip.pointer('pointerdown', lc.x, lc.y);
    strip.pointer('pointermove', lc.x + 50, lc.y);
    strip.pointer('pointerup', lc.x + 50, lc.y);
    assert.ok(strip.logs().includes('lamb drag ignored'));
    assert.deepEqual(strip.clicks(), [], 'no click');
    assert.ok(!strip.calls.some((c) => c.phase === 'lift'), 'no drag');
    assert.deepEqual(strip.dragCalls(), ['press', 'release']);
    // A plain click on the lamb: its id, which the shell takes to its parent's terminal.
    const lc2 = { x: lamb.sprite.drawInfo().x + lamb.sprite.width / 2, y: lamb.sprite.drawInfo().y + lamb.sprite.height / 2 };
    strip.pointer('pointerdown', lc2.x, lc2.y);
    strip.pointer('pointerup', lc2.x, lc2.y);
    assert.deepEqual(strip.clicks(), ['l1']);
    // The parent carried 400 px: the lamb trots after it, on the ground.
    const before = lamb.sprite.x;
    const c = lift(strip, 's1', 40);
    strip.pointer('pointermove', c.x + 400, c.y);
    strip.step(60);
    assert.ok(lamb.sprite.x > before + 300, `the lamb followed: ${before} -> ${lamb.sprite.x}`);
    assert.equal(lamb.sprite.drawInfo().y, 140 - lamb.sprite.height, 'on the ground');
  });

  test('carried to another monitor\'s strip: drawn there, not here; dropped there, gone from here at once without running off', async () => {
    // Busy sheep: no grass tufts, so every image is a sheep.
    const strip = await bootStrip([sheep('s1', 0, { state: 'busy' }), sheep('s2', 1, { state: 'busy' }), sheep('s3', 2, { state: 'busy' })]);
    const s = strip.entry('s1').sprite;
    const c = lift(strip, 's1', 100);
    strip.view({ carry: { id: 's1', away: true } });
    strip.S.draw(5000);
    const images = strip.screen.filter((o) => o.op === 'drawImage').length;
    strip.view({ carry: null });
    strip.S.draw(5000);
    assert.equal(strip.screen.filter((o) => o.op === 'drawImage').length, images + 1, 'one sprite fewer while it is drawn elsewhere');
    strip.view({ carry: { id: 's1', away: true } });
    strip.step(10);
    const boxes = strip.calls.filter((x) => x.cmd === 'report_hitboxes').at(-1).boxes.map((b) => b.id);
    assert.ok(!boxes.includes('s1') && boxes.includes('s2'), `no box for a sheep drawn elsewhere: ${boxes}`);
    const frozen = s.x;
    strip.pointer('pointermove', c.x + 900, c.y);
    assert.equal(s.x, frozen, 'here it waits (and so do its lambs)');
    strip.setRelease(() => ({ monitor: 'DELL', centreX: 640, seq: 2, ghost: false, here: false }));
    strip.pointer('pointerup', c.x + 900, c.y);
    await strip.settle();
    strip.step();
    assert.equal(strip.entry('s1').carriedAway, true);
    strip.snapshot([sheep('s2', 1, { state: 'busy' }), sheep('s3', 2, { state: 'busy' })]);
    assert.equal(strip.entry('s1'), undefined, 'removed at once, no `leave`');
    assert.ok(strip.logs().includes('sheep s1 carried to another monitor'));
  });

  test('a sheep carried in from another strip is drawn here, then dropped here it falls and lands, starting where it was let go', async () => {
    const strip = await bootStrip([sheep('s2', 1)]);
    strip.view({ carry: { id: 'sX', x: 600, y: 30, grabDx: 20, colourIndex: 3, alert: null } });
    const g = strip.S.guest();
    assert.ok(g, 'the shell\'s carry is drawn here');
    assert.equal(g.sprite.x, 580);
    strip.S.draw(5000);
    assert.equal(strip.screen.filter((o) => o.op === 'drawImage').at(-1).alpha, 1);
    strip.view({ carry: { id: 'sX', x: 640, y: 30, grabDx: 20, colourIndex: 3, alert: null } });
    assert.equal(strip.S.guest().sprite.x, 620, 'follows the cursor');
    // The drop: the carry ends and the sheep comes in the flock with its drop.
    strip.view({ carry: null });
    assert.equal(strip.S.guest(), null);
    const w = strip.entry('s2').sprite.width;
    strip.snapshot([sheep('s2', 1), sheep('sX', 0, { state: 'arriving', dropped: { centreX: 650, seq: 9, ghost: false } })]);
    const s = strip.entry('sX').sprite;
    assert.equal(s.x, 650 - w / 2, 'at the drop, not at its home');
    assert.ok(s.fall, 'falling from where it was held');
    assert.notEqual(s.behaviour, 'arrive', 'no arrive: it was carried in');
    strip.step(80);
    assert.equal(s.landedSeq, 9);
    assert.ok(strip.logs().some((l) => l.startsWith('sheep sX (p0) dropped here at x 650')));
  });

  test('dropped where there is no room: it stays in front and the log says no room', async () => {
    const strip = await bootStrip();
    const s = strip.entry('s1').sprite;
    lift(strip, 's1', 10);
    // Every other sheep's reach covers the strip: a crowd on a narrow strip.
    const crowd = Array.from({ length: 12 }, (_, i) => sheep(`c${i}`, i));
    strip.pointer('pointerup', strip.centre('s1').x, 60);
    await strip.settle();
    // A guest from another monitor (no band here), onto a strip whose twelve slots fill it.
    strip.snapshot([sheep('s1', 12, { dropped: { centreX: 600, seq: 1, ghost: false } }), ...crowd]);
    strip.step(5);
    assert.ok(strip.entry('s1').inFront, 'drawn in front');
    assert.ok(strip.logs().some((l) => /^sheep s1 dropped on Built-in at x 600: no room/.test(l)), strip.logs().filter((l) => l.includes('room')).join('\n'));
    assert.equal(s.wander, 0);
  });

  test('a reload doesn\'t land a dropped sheep again: on the first snapshot it is simply there', async () => {
    const strip = await bootStrip([sheep('s1', 0, { dropped: { centreX: 700, seq: 4, ghost: true } }), sheep('s2', 1)]);
    const s = strip.entry('s1').sprite;
    assert.equal(s.fall, null);
    assert.equal(s.held, false);
    assert.equal(s.landedSeq, 4);
    assert.equal(s.x, 700 - s.width / 2);
    assert.ok(strip.logs().some((l) => /^first snapshot drawn: 2 sheep, \d+ ms after the page started$/.test(l)), 'M12 step 1 (e): the first-snapshot line');
  });
});

describe('main.js, by its source (milestone 12)', () => {
  test('the click handler is on pointerup, not pointerdown', () => {
    assert.match(MAIN, /canvas\.addEventListener\('pointerup', \(e\) => gestureEvent\(\{ type: 'up', x: e\.offsetX, y: e\.offsetY, button: e\.button \?\? 0, buttons: e\.buttons \}\)\);/, 'with the button and what is still held');
    const onDown = MAIN.slice(MAIN.indexOf('function pointerDown'), MAIN.indexOf('function gestureEvent'));
    assert.doesNotMatch(onDown.replace(/\/\/.*$/gm, ''), /invoke\('sprite_clicked'/, 'pointerdown never clicks by itself');
    assert.match(onDown, /\.removedAt !== undefined \|\| entry\.actor\.state === 'leaving'\) \{[\s\S]*?click\(entry\);/, 'except on a sheep that is going anyway, as before');
  });

  test('the carried sheep is drawn after the bubbles, and is exempt from the fade', () => {
    const draw = MAIN.slice(MAIN.indexOf('function draw('), MAIN.indexOf('function carriedEntry'));
    assert.ok(draw.indexOf('drawBubbles(now)') < draw.indexOf('drawCarried(now)'));
    assert.match(MAIN, /if \(!full\) d\.opacity \*= fadeOf\(key\);/);
    assert.match(MAIN, /drawSprite\(e, now, \{ full: true \}\)/);
  });

  test('the idle redraw cap is unchanged; only a carry redraws faster', () => {
    assert.match(MAIN, /const DRAW_INTERVAL_MS = 150;/);
    assert.match(MAIN, /const step = carrying\(\) \? CARRY_DRAW_MS : sprites\.size > CROWDED_SPRITES \? CROWDED_DRAW_INTERVAL_MS : DRAW_INTERVAL_MS;/);
  });

  test('a drop wakes the strips: the flock is applied again when the shell\'s placement changes', () => {
    assert.match(MAIN, /if \(snap\.rev !== rev \|\| moved\) apply\(snap\);/);
  });

  test('an imported esheep64 drags with its walk frames: the default map names no carried or land, so nothing is missing', () => {
    const map = MAIN.slice(MAIN.indexOf('const DEFAULT_ANIMATION_MAP'), MAIN.indexOf('};', MAIN.indexOf('const DEFAULT_ANIMATION_MAP')));
    assert.doesNotMatch(map, /carried|land/);
  });

  test('the log lines carry ids and positions, never bubble text', () => {
    const lib = fs.readFileSync(path.join(SRC, '..', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    for (const line of ['"sheep {id} picked up"', '"sheep {id} dropped on {monitor:?} at x {} ({})"', '"drop for {} ended: {reason}"']) assert.ok(lib.includes(line), line);
    const ct = fs.readFileSync(path.join(SRC, '..', 'src-tauri', 'src', 'clickthrough.rs'), 'utf8');
    assert.match(ct, /log\(&format!\("drag cancelled: \{reason\}/);
    const machine = fs.readFileSync(path.join(SRC, 'player', 'drag.mjs'), 'utf8');
    assert.match(machine, /\{ do: 'log', line: 'lamb drag ignored' \}/);
    assert.match(MAIN, /case 'log':\s*log\(a\.line\);/);
  });
});
