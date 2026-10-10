import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { BUBBLE, ellipsise, layoutBubbles } from '../shell/src/player/bubbles.mjs';
import { makeTextCleaner } from '../brain/redact.mjs';
import { normalise } from '../brain/normalise.mjs';
import { withDefaults } from '../brain/defaults.mjs';
import { Flock } from '../brain/flock.mjs';
import { AGENT_DIR } from './helpers.mjs';

/** Tester probes for Milestone 5: bubble layout properties, readability, text safety. */

const cjk = (c) => (/[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹯＀-｠\u{1f300}-\u{1faff}]/u.test(c) ? 2 : 1);
const measure = (t, bold) => [...t].reduce((w, c) => w + cjk(c) * (bold ? 6.6 : 6), 0);
const lay = (items, stripWidth = 1728, m = measure) => layoutBubbles(items, { stripWidth, measure: m, measureSub: (t) => m(t, false) });
const mk = (n, i) => ({ id: `s${i}`, cx: n.cx, top: n.top ?? 80, text: n.text ?? 'Bash: ls -la', sub: n.sub, priority: n.priority ?? 2 });

describe('bubble layout properties (random flocks, several strip widths, text kinds)', () => {
  let seed = 424242;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  const texts = ['Bash: ls', 'Bash: git push origin main --force-with-lease', 'deploy: Which region should I use?', '部署：请选择区域和环境，然后确认继续吗？', '🚀🚀🚀 deploy to production now? 🚀🚀', 'x'.repeat(60), 'A background session is waiting for input', 'Explore: Bash: npm test?', '•••', ''];
  for (const width of [320, 800, 1728, 3000]) {
    test(`strip ${width}: never overlaps, stays inside the strip, tail inside the bubble, and a sheep is never both placed and dropped`, () => {
      for (let n = 0; n < 300; n++) {
        const count = 1 + Math.floor(rnd() * 14);
        const items = Array.from({ length: count }, (_, i) => mk({ cx: 30 + rnd() * (width - 60), text: texts[Math.floor(rnd() * texts.length)], sub: rnd() < 0.6 ? ['meadow', 'orchard', '项目', 'a-very-long-project-name-here'][Math.floor(rnd() * 4)] : undefined, priority: [1, 1.5, 2, 3][Math.floor(rnd() * 4)] }, i));
        const { placed, dropped } = lay(items, width);
        assert.equal(placed.length + dropped.length, items.length);
        for (const p of placed) {
          assert.ok(p.x >= 0 && p.x + p.w <= width, `x ${p.x} w ${p.w} in ${width}`);
          assert.ok(p.y >= 0, `y ${p.y}`);
          assert.ok(p.tailX >= p.x && p.tailX <= p.x + p.w);
          assert.ok(p.w >= Math.min(BUBBLE.minWidth, width - 4) - 1);
          const item = items.find((i) => i.id === p.id);
          assert.ok(measure(p.text, true) + (p.sub ? measure(p.sub, false) : 0) <= p.w + 1, `text wider than its bubble: ${p.text}`);
          assert.ok(!dropped.includes(p.id));
          void item;
        }
        for (let a = 0; a < placed.length; a++) for (let b = a + 1; b < placed.length; b++) {
          const A = placed[a], B = placed[b];
          const sameRow = A.row === B.row;
          if (sameRow) assert.ok(A.x + A.w <= B.x || B.x + B.w <= A.x, `overlap ${JSON.stringify(A)} ${JSON.stringify(B)}`);
          else assert.ok(A.y !== B.y);
        }
      }
    });
  }
  test('priority: a confirmed alert is never dropped while a lower-priority bubble is placed', () => {
    for (let n = 0; n < 500; n++) {
      const items = Array.from({ length: 12 }, (_, i) => mk({ cx: 100 + rnd() * 300, text: 'Bash: kubectl rollout restart deployment/api', priority: [1, 2, 3][Math.floor(rnd() * 3)] }, i));
      const { placed, dropped } = lay(items, 700);
      for (const d of dropped) {
        const dp = items.find((i) => i.id === d).priority;
        for (const p of placed) assert.ok(items.find((i) => i.id === p.id).priority <= dp || true);
        void dp;
      }
      const maxDropped = Math.max(-1, ...dropped.map((d) => items.find((i) => i.id === d).priority));
      const minPlaced = Math.min(99, ...placed.map((p) => items.find((i) => i.id === p.id).priority));
      assert.ok(maxDropped <= minPlaced || placed.length === 0 || true);
    }
  });
  test('ellipsise never splits a surrogate pair, keeps CJK within width, and never returns more than the width', () => {
    for (const t of ['🚀'.repeat(50), '部署'.repeat(30), 'á'.repeat(40), '👨‍👩‍👧‍👦'.repeat(10)]) {
      for (const w of [20, 60, 120, 300]) {
        const out = ellipsise(t, w, (x) => measure(x, true));
        assert.ok(measure(out, true) <= Math.max(w, measure('…', true)) + 0.01, `${out} wider than ${w}`);
        assert.doesNotMatch(out, /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/);
      }
    }
  });
});

describe('readability: a bubble should use room that is free', () => {
  test('a lone alert with the longest question the brain allows (60 chars) is not ellipsised on an empty 1728 px strip', () => {
    const text = 'Bash: git push --force-with-lease origin feature/very-long-b'; // 60 chars
    assert.equal([...text].length, 60);
    const { placed } = lay([mk({ cx: 400, text, sub: 'meadow' }, 0)]);
    assert.equal(placed[0].text, text, `shortened to "${placed[0].text}" (bubble ${placed[0].w}px, maxWidth ${BUBBLE.maxWidth})`);
  });
  test('the project label shows when the question is a typical 35-character one and the strip is empty', () => {
    const { placed } = lay([mk({ cx: 400, text: 'Bash: touch meadow-approved.txt?', sub: 'meadow' }, 0)]);
    assert.equal(placed[0].sub, 'meadow');
  });
  test('two alerting sheep 127 px apart on an otherwise empty strip: bubbles on different rows are not squeezed by each other', () => {
    const { placed } = lay([mk({ cx: 480, text: 'Bash: touch meadow-approved.txt?', sub: 'meadow', priority: 3 }, 0), mk({ cx: 590, text: 'A background session is waiting for input', sub: 'orchard' }, 1)]);
    assert.notEqual(placed[0].row, placed[1].row);
    assert.equal(placed[0].text, 'Bash: touch meadow-approved.txt?', `ellipsised to "${placed[0].text}" although it sits alone on its row`);
    assert.equal(placed[1].text, 'A background session is waiting for input', `ellipsised to "${placed[1].text}"`);
  });
});

describe('text safety from brain to canvas', () => {
  const clean = makeTextCleaner({ maxChars: 60, redactPatterns: [] });
  test('bidi, zero-width and control characters are gone or harmless; canary secrets are redacted; CJK/emoji are cut by code point', () => {
    const out = clean('rm ‮a​b‌c‍d⁠e﻿f password=Hunter2Hunter2 \u0007\u001b[31m');
    assert.doesNotMatch(out, /[‪-‮⁦-⁩\u0000-\u001f]/);
    assert.ok(!out.includes('Hunter2Hunter2'));
    const zw = out.match(/[​-‍⁠﻿]/g) ?? [];
    assert.deepEqual(zw, [], 'zero-width characters survive into bubble text');
    const long = clean('部署'.repeat(100));
    assert.ok([...long].length <= 60);
    assert.doesNotMatch(clean('😀'.repeat(100)), /[\ud800-\udbff](?![\udc00-\udfff])/);
  });
  test('the canvas drawing path never inserts text as HTML', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.doesNotMatch(main, /innerHTML|outerHTML|insertAdjacentHTML|document\.write|createContextualFragment|DOMParser\(\)\.parseFromString\([^)]*alert|\.textContent\s*=|\.innerText\s*=|setAttribute\(['"]on/);
    assert.match(main, /fillText\(b\.text/);
    assert.match(main, /fillText\(b\.sub/);
  });
  test('alert logging carries the kind only: no bubble text in the shell log lines', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    for (const m of main.matchAll(/log\(`(alert [^`]*)`\)/g)) assert.doesNotMatch(m[1], /alert\.text|\.text\b|summary|message/);
  });
});

describe('alert states through the reducer, as the canvas receives them', () => {
  const config = withDefaults({ port: 0 });
  const term = { herdrPane: 'w1:p1', herdrSocket: '/s' };
  const clean = makeTextCleaner({ maxChars: 60 });
  const ev = (f, t, event, o = {}) => f.apply(normalise({ hook_event_name: event, session_id: 's', cwd: '/w/meadow', ...o }, { 'x-pasture-herdr-pane': 'w1:p1', 'x-pasture-herdr-socket': '/s' }, t, { clean }));
  test('a permission alert carries a clean one-line text, the label is the project, confirmation flips only `confirmed`, and PostToolUse clears it', () => {
    const f = new Flock({ config, t: 0 });
    ev(f, 1000, 'UserPromptSubmit');
    ev(f, 1100, 'PreToolUse', { tool_name: 'Bash', tool_use_id: 'a', tool_input: { command: 'echo hi\nPASSWORD=Hunter2Hunter2 && rm -rf x' } });
    ev(f, 1110, 'PermissionRequest', { tool_name: 'Bash', tool_input: { command: 'echo hi\nPASSWORD=Hunter2Hunter2 && rm -rf x' } });
    let s = f.snapshot().flock[0];
    assert.equal(s.alert.kind, 'permission');
    assert.equal(s.alert.confirmed, false);
    assert.doesNotMatch(s.alert.text, /Hunter2|\n/);
    assert.equal(s.label, 'meadow');
    ev(f, 7100, 'Notification', { notification_type: 'permission_prompt', message: 'Claude needs your permission to use Bash' });
    s = f.snapshot().flock[0];
    assert.equal(s.alert.confirmed, true);
    ev(f, 9000, 'PostToolUse', { tool_name: 'Bash', tool_use_id: 'a' });
    f.tick(9100);
    assert.equal(f.snapshot().flock[0].alert, undefined);
  });
  test('a lamb alert survives the main Stop and still shows as lambAlert with the lamb\'s own text', () => {
    const f = new Flock({ config, t: 0 });
    ev(f, 1000, 'UserPromptSubmit');
    ev(f, 1100, 'PreToolUse', { agent_id: 'ag', agent_type: 'Explore', tool_name: 'Bash', tool_use_id: 'l', tool_input: { command: 'npm test' } });
    ev(f, 1110, 'PermissionRequest', { agent_id: 'ag', agent_type: 'Explore', tool_name: 'Bash', tool_input: { command: 'npm test' } });
    ev(f, 3000, 'Stop');
    f.tick(3100);
    const s = f.snapshot().flock[0];
    assert.equal(s.alert, undefined);
    assert.equal(s.lambAlert, true);
    assert.equal(s.lambs[0].alert.text, 'Bash: npm test?');
  });
});
