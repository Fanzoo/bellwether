import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { familyExtent, freePlace, lambOffset, lambX } from '../shell/src/player/lambs.mjs';
import { familyHomes } from '../shell/src/player/layout.mjs';
import { Sprite } from '../shell/src/player/sprite.mjs';
import { resolveAnimationMap, validatePet } from '../shell/src/player/pet.mjs';
import { FIXTURES } from './helpers.mjs';
import { layoutBubbles } from '../shell/src/player/bubbles.mjs';
import { withDefaults } from '../brain/defaults.mjs';
import { Flock } from '../brain/flock.mjs';
import { normalise } from '../brain/normalise.mjs';
import { makeTextCleaner } from '../brain/redact.mjs';

/** Tester probes for Milestone 6: lamb attribution, family layout, bubbles with lambs, agent_type safety. */

const config = withDefaults({ port: 0 });
const term = (p) => ({ herdrPane: `w1:p${p}`, herdrSocket: '/s' });
const mkFlock = () => {
  const f = new Flock({ config, t: 0 });
  const e = (t, event, p, o = {}) => f.apply({ receivedAt: t, event, sessionId: `s${p}`, cwd: `/w/p${p}`, projectLabel: `p${p}`, terminal: term(p), ...o });
  return { f, e };
};

describe('lamb attribution when agent ids collide across sessions', () => {
  test('a lamb\'s permission alert lands on its own session\'s lamb, clears there, and dismiss/stop affect only that session', () => {
    const { f, e } = mkFlock();
    for (const p of [1, 2]) {
      e(1000 + p, 'UserPromptSubmit', p);
      e(1100 + p, 'PreToolUse', p, { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolUseId: `tu${p}`, toolFingerprint: `fp${p}`, toolSummary: `cmd${p}` });
    }
    e(2000, 'PermissionRequest', 2, { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolFingerprint: 'fp2', toolSummary: 'cmd2' });
    f.tick(2100);
    const [a, b] = f.snapshot().flock.sort((x, y) => x.label.localeCompare(y.label));
    assert.equal(a.lambs[0].alert, undefined, 'session 1\'s lamb was given session 2\'s alert');
    assert.equal(a.lambAlert, undefined);
    assert.equal(b.lambs[0].alert?.text, 'Bash: cmd2?');
    assert.equal(b.lambAlert, true);
  });
  test('the dismiss control with an ambiguous lamb id cannot dismiss another session\'s alert', () => {
    const { f, e } = mkFlock();
    for (const p of [1, 2]) {
      e(1000 + p, 'UserPromptSubmit', p);
      e(1100 + p, 'PreToolUse', p, { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolUseId: `tu${p}`, toolFingerprint: `fp${p}`, toolSummary: `c${p}` });
      e(1200 + p, 'PermissionRequest', p, { agentId: 'ag1', agentType: 'Explore', toolName: 'Bash', toolFingerprint: `fp${p}`, toolSummary: `c${p}` });
    }
    const ids = f.snapshot().flock.flatMap((s) => s.lambs.map((l) => l.id));
    assert.equal(new Set(ids).size, 2, `lamb ids in the snapshot collide: ${ids}`);
  });
});

describe('hidden helper, cap, timeout and parent leaving (through the snapshot)', () => {
  test('a helper with agent_id and no type creates no lamb even across Pre/Permission/Post/Stop', () => {
    const { f, e } = mkFlock();
    e(1000, 'UserPromptSubmit', 1);
    for (const [t, ev] of [[1100, 'PreToolUse'], [1110, 'PermissionRequest'], [1200, 'PostToolUse'], [1300, 'SubagentStart'], [1400, 'SubagentStop']]) e(t, ev, 1, { agentId: 'helper', toolName: 'Read', toolUseId: 'h', toolFingerprint: 'h' });
    assert.equal(f.snapshot().flock[0].lambs.length, 0);
  });
  test('six lambs show five and "+1" (hiddenLambCount), the alerting one is never the hidden one, and 64 is the most that are held', () => {
    const { f, e } = mkFlock();
    e(1000, 'UserPromptSubmit', 1);
    for (let i = 0; i < 6; i++) e(1100 + i, 'PreToolUse', 1, { agentId: `a${i}`, agentType: `T${i}`, toolName: 'Read', toolUseId: `t${i}` });
    e(1500, 'PreToolUse', 1, { agentId: 'a5', agentType: 'T5', toolName: 'Bash', toolUseId: 'bx', toolFingerprint: 'bx' });
    e(1510, 'PermissionRequest', 1, { agentId: 'a5', agentType: 'T5', toolName: 'Bash', toolFingerprint: 'bx', toolSummary: 'ls' });
    const s = f.snapshot().flock[0];
    assert.equal(s.lambs.length, 5);
    assert.equal(s.hiddenLambCount, 1);
    assert.ok(s.lambs.some((l) => l.alert), 'the alerting lamb is hidden behind +N');
    for (let i = 0; i < 100; i++) e(2000 + i, 'PreToolUse', 1, { agentId: `m${i}`, agentType: 'X', toolName: 'Read', toolUseId: `m${i}` });
    const t = f.snapshot().flock[0];
    assert.ok(t.lambs.length + t.hiddenLambCount <= 64, `${t.lambs.length + t.hiddenLambCount} lambs held`);
  });
  test('parent SessionEnd takes its lambs (and their alerts) with it', () => {
    const { f, e } = mkFlock();
    e(1000, 'UserPromptSubmit', 1);
    e(1100, 'PreToolUse', 1, { agentId: 'a', agentType: 'E', toolName: 'Bash', toolUseId: 't', toolFingerprint: 'f' });
    e(1110, 'PermissionRequest', 1, { agentId: 'a', agentType: 'E', toolName: 'Bash', toolFingerprint: 'f', toolSummary: 'ls' });
    e(2000, 'SessionEnd', 1, { reason: 'other' });
    f.tick(2100); f.tick(9000);
    assert.equal(f.snapshot().flock.length, 0);
  });
});

describe('agent_type is untrusted', () => {
  const clean = makeTextCleaner({ maxChars: 60 });
  const lambOf = (agentType) => {
    const f = new Flock({ config, t: 0 });
    const n = (event, o) => f.apply(normalise({ hook_event_name: event, session_id: 's', cwd: '/w/p', ...o }, { 'x-pasture-herdr-pane': 'w1:p1', 'x-pasture-herdr-socket': '/s' }, 1000, { clean }));
    n('UserPromptSubmit', {});
    n('PreToolUse', { agent_id: 'a', agent_type: agentType, tool_name: 'Bash', tool_use_id: 't', tool_input: { command: 'ls' } });
    return f.snapshot().flock[0].lambs[0];
  };
  test('HTML-ish, bidi, control, newline, space and empty types never become a lamb label', () => {
    for (const t of ['<img src=x onerror=alert(1)>', 'a‮b', 'a\u0007b', 'a\nb', 'two words', '', 'x"y', "x'y", '`rm`', 'a;b', '%00', '​zw']) assert.equal(lambOf(t), undefined, JSON.stringify(t));
  });
  test('an identifier-shaped type is length-limited and a credential-shaped one is not shown as a label', () => {
    const long = lambOf('a'.repeat(200));
    assert.ok(!long || long.label.length <= 40, `label is ${long?.label.length} chars`);
    const key = lambOf('sk-abcdefghijklmnopqrstuvwxyz0123456789');
    assert.ok(!key || !/sk-abcdefghij/.test(key.label), `credential-shaped agent_type shown: ${key?.label}`);
  });
});

describe('lamb placement (lambs.mjs)', () => {
  const dims = { parentW: 60, lambW: 36 };
  test('places alternate right/left, never overlap each other or the parent, and stay inside the strip at either edge', () => {
    for (const stripWidth of [200, 400, 1728]) for (const parentX of [0, 5, 60, 150, stripWidth - 60, stripWidth - 80]) for (const n of [1, 3, 5]) {
      const xs = Array.from({ length: n }, (_, k) => lambX(k, { parentX: Math.max(0, parentX), ...dims, stripWidth }));
      for (const x of xs) assert.ok(x >= 0 && x + dims.lambW <= stripWidth + 0.001, `lamb at ${x} outside ${stripWidth}`);
      const iv = [...xs.map((x) => [x, x + dims.lambW]), [Math.max(0, parentX), Math.max(0, parentX) + dims.parentW]];
      if (stripWidth - dims.parentW > 0 && n * dims.lambW + dims.parentW < stripWidth)
        for (let a = 0; a < xs.length; a++) for (let b = a + 1; b < xs.length; b++) assert.ok(iv[a][1] <= iv[b][0] + 0.5 || iv[b][1] <= iv[a][0] + 0.5, `lambs ${a},${b} overlap at parent ${parentX}, strip ${stripWidth}`);
    }
  });
  test('freePlace reuses the lowest freed place', () => {
    assert.equal(freePlace(new Set([0, 1, 3])), 2);
    assert.equal(freePlace(new Set()), 0);
  });
  test('families never overlap: each family (sheep + lambs, at the sheep\'s furthest wander) stays inside its own stretch, and slot order is kept', () => {
    let seed = 7;
    const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
    for (let n = 0; n < 300; n++) {
      const stripWidth = [900, 1728][n % 2];
      const slots = [...new Set(Array.from({ length: 1 + Math.floor(rnd() * 5) }, () => Math.floor(rnd() * 8)))].sort((a, b) => a - b);
      const sheep = slots.map((slot, i) => ({ id: `s${i}`, slot, extent: familyExtent(Math.floor(rnd() * 7), dims) }));
      const homes = familyHomes(sheep, { stripWidth, slotWidth: 110, wanderMax: 120 });
      const top = Math.max(...slots);
      const need = Array.from({ length: top + 1 }, (_, k) => { const f = sheep.find((x) => x.slot === k); return f ? Math.max(110, f.extent.left + f.extent.right + 8) : 110; }).reduce((a, b) => a + b, 0);
      if (need > stripWidth - 48) continue; // bands don't fit: the plan scales them down
      const boxes = sheep.map((s) => {
        const h = homes.get(s.id);
        assert.ok(h && Number.isFinite(h.centre) && h.wander >= 0 && h.wander <= 120, `home ${JSON.stringify(h)}`);
        return { id: s.id, lo: h.centre - h.wander - s.extent.left, hi: h.centre + h.wander + s.extent.right, centre: h.centre };
      });
      for (const b of boxes) assert.ok(b.lo >= -0.5 && b.hi <= stripWidth + 0.5, `family ${b.id} leaves the strip: ${b.lo}..${b.hi} in ${stripWidth}`);
      for (let a = 0; a + 1 < boxes.length; a++) {
        assert.ok(boxes[a].centre < boxes[a + 1].centre, 'slot order was not kept');
        assert.ok(boxes[a].hi <= boxes[a + 1].lo + 0.5, `families overlap: ${JSON.stringify(boxes[a])} ${JSON.stringify(boxes[a + 1])}`);
      }
    }
  });
  test('slot stability: adding a lamb moves other families by at most about one lamb width, and never reorders them', () => {
    const base = [{ id: 'a', slot: 0, extent: familyExtent(2, dims) }, { id: 'b', slot: 1, extent: familyExtent(3, dims) }, { id: 'c', slot: 2, extent: familyExtent(1, dims) }];
    const before = familyHomes(base, { stripWidth: 1728, slotWidth: 110, wanderMax: 120 });
    const grown = base.map((s) => (s.id === 'a' ? { ...s, extent: familyExtent(3, dims) } : s));
    const after = familyHomes(grown, { stripWidth: 1728, slotWidth: 110, wanderMax: 120 });
    for (const id of ['a', 'b', 'c']) assert.ok(Math.abs(after.get(id).centre - before.get(id).centre) <= 80, `${id} jumped ${Math.abs(after.get(id).centre - before.get(id).centre)} px`);
    assert.ok(after.get('a').centre < after.get('b').centre && after.get('b').centre < after.get('c').centre);
  });
});

describe('bubbles with lamb items on the sheep rows (as main.js builds them)', () => {
  let seed = 99;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  const m = (t, bold) => [...t].length * (bold ? 6.6 : 6);
  test('mixed sheep and lamb bubbles (lamb: lower anchor, same rows) never overlap, stay in the strip, and keep the tail inside', () => {
    for (let n = 0; n < 400; n++) {
      const width = [500, 1728][n % 2];
      const items = Array.from({ length: 1 + Math.floor(rnd() * 12) }, (_, i) => {
        const lamb = rnd() < 0.5;
        return { id: `${lamb ? 'l' : 's'}${i}`, cx: 30 + rnd() * (width - 60), top: 80, anchorTop: lamb ? 100 : undefined, text: lamb ? 'Explore: Bash: npm test --coverage?' : 'Bash: kubectl rollout restart deployment/api', sub: 'meadow', priority: [2, 3][Math.floor(rnd() * 2)] };
      });
      const { placed, dropped } = layoutBubbles(items, { stripWidth: width, measure: m, measureSub: (t) => m(t, false) });
      assert.equal(placed.length + dropped.length, items.length);
      for (const p of placed) {
        assert.ok(p.x >= 0 && p.x + p.w <= width && p.y >= 0);
        assert.ok(p.tailX >= p.x && p.tailX <= p.x + p.w);
      }
      for (let a = 0; a < placed.length; a++) for (let b = a + 1; b < placed.length; b++) if (placed[a].row === placed[b].row) assert.ok(placed[a].x + placed[a].w <= placed[b].x || placed[b].x + placed[b].w <= placed[a].x);
    }
  });
});

describe('lambs stay next to their parent (the "far lambs" in the screenshots)', () => {
  const m = validatePet(JSON.parse(fs.readFileSync(path.join(FIXTURES, 'pet-model.json'), 'utf8')));
  const map = resolveAnimationMap(m, { walk: 'walk', run: 'run', graze: 'eat', sleep: ['sleep1a', 'sleep1b'], hop: 'jump', arrive: 'walk', leave: 'run', sit: 'eat' });
  const seeded = (seed) => () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
  // main.js's frame(): each lamb's home is lambX(place, parent's current x), every frame.
  const simulate = (lambBehaviour, parentBehaviour = 'walk', seconds = 90) => {
    const stripWidth = 1728;
    const parent = new Sprite({ model: m, map, width: 60, height: 60, stripWidth, stripHeight: 140, home: 500, wander: 100, rand: seeded(11) });
    parent.setBehaviour(parentBehaviour, 0);
    const lambs = [0, 1, 2].map((place) => {
      const l = new Sprite({ model: m, map, width: 36, height: 36, stripWidth, stripHeight: 140, home: 0, wander: 6, rand: seeded(20 + place) });
      l.x = parent.x + 12;
      l.setBehaviour(lambBehaviour, 0);
      return { l, place };
    });
    let worst = 0, sumLate = 0, nLate = 0;
    for (let t = 0; t < seconds * 1000; t += 50) {
      for (const { l, place } of lambs) l.setHome(lambX(place, { parentX: parent.x, parentW: 60, lambW: 36, stripWidth }));
      parent.tick(t);
      for (const { l, place } of lambs) {
        l.tick(t);
        const gap = Math.abs(l.x - lambX(place, { parentX: parent.x, parentW: 60, lambW: 36, stripWidth }));
        if (t > 10_000) { worst = Math.max(worst, gap); sumLate += gap; nLate++; }
      }
    }
    return { worst, mean: sumLate / nLate };
  };
  for (const b of ['walk', 'run', 'graze', 'sit', 'sleep']) {
    test(`a ${b} lamb stays within 25 px of its place while the parent walks about (max, after 10 s)`, () => {
      const r = simulate(b);
      assert.ok(r.worst <= 25, `a ${b} lamb strayed ${r.worst.toFixed(0)} px from its place beside the parent (mean ${r.mean.toFixed(0)} px)`);
    });
  }
});

describe('maxLambsShown above 5, and the 24-character agent type', () => {
  const dims = { parentW: 60, lambW: 36 };
  test('up to 12 lambs on strips of 300 to 1728 px, parent anywhere: inside the strip, none on another lamb', () => {
    for (const stripWidth of [300, 500, 900, 1728]) for (const parentX of [0, 40, 200, stripWidth / 2, stripWidth - 60]) for (const n of [6, 8, 10, 12]) {
      const xs = Array.from({ length: n }, (_, k) => lambX(k, { parentX: Math.min(parentX, stripWidth - 60), ...dims, stripWidth }));
      for (const x of xs) assert.ok(x >= 0 && x + 36 <= stripWidth + 0.01, `x ${x} outside ${stripWidth}`);
      if (n * 36 + 60 + 100 < stripWidth) for (let a = 0; a < n; a++) for (let b = a + 1; b < n; b++) assert.ok(Math.abs(xs[a] - xs[b]) >= 35.5, `lambs ${a},${b} overlap (${xs[a]}, ${xs[b]}) n=${n} parent ${parentX} strip ${stripWidth}`);
    }
  });
  test('with maxLambsShown 10, ten lambs are shown and +N counts the rest', () => {
    const cfg = withDefaults({ port: 0, behaviour: { maxLambsShown: 10 } });
    const f = new Flock({ config: cfg, t: 0 });
    f.apply({ receivedAt: 1000, event: 'UserPromptSubmit', sessionId: 's', terminal: { herdrPane: 'w1:p1', herdrSocket: '/s' } });
    for (let i = 0; i < 13; i++) f.apply({ receivedAt: 1100 + i, event: 'PreToolUse', sessionId: 's', agentId: `a${i}`, agentType: 'X', toolName: 'Read', toolUseId: `t${i}`, terminal: { herdrPane: 'w1:p1', herdrSocket: '/s' } });
    const s = f.snapshot().flock[0];
    assert.equal(s.lambs.length, 10);
    assert.equal(s.hiddenLambCount, 3);
  });
  test('agent_type longer than 24 characters is cut with an ellipsis, in normalise and in the reducer', () => {
    const clean = makeTextCleaner({ maxChars: 60 });
    const ev = normalise({ hook_event_name: 'PreToolUse', session_id: 's', agent_id: 'a', agent_type: 'abcdefghijklmnopqrstuvwxyz0123456789', tool_name: 'Read' }, {}, 1, { clean });
    assert.ok([...ev.agentType].length <= 24, ev.agentType);
    const f = new Flock({ config, t: 0 });
    f.apply({ receivedAt: 1, event: 'PreToolUse', sessionId: 's', agentId: 'a', agentType: 'x'.repeat(100), toolName: 'Read', toolUseId: 't', terminal: {} });
    assert.ok([...f.snapshot().flock[0].lambs[0].label].length <= 24);
  });
});
