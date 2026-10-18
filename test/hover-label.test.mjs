import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { describe, test } from 'node:test';
import { buildPet, miniDom } from '../scripts/build-pet.mjs';
import { parsePetDocument } from '../shell/src/player/pet.mjs';
import { hoverTarget } from '../shell/src/player/hover.mjs';
import { AGENT_DIR } from './helpers.mjs';

/**
 * The hover label (deviation 136). A label stuck on screen with Show labels off: the
 * strip's window stops hearing the mouse as soon as the shell's cursor poll finds the
 * cursor off every sprite, so no pointerleave ever came, and the hover kept from the last
 * pointer event stayed on. These run the strip's own main.js, unchanged, in a stub
 * webview (no Tauri, no GUI) and look at what is on the canvas after each step.
 */

const SRC = path.join(AGENT_DIR, 'shell', 'src');
const NAME = 'day-trip-explorer-app';

/** main.js and the player modules it uses, as one classic script (like playerBundle). */
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
  const main = fs.readFileSync(path.join(SRC, 'main.js'), 'utf8');
  for (const [, , from] of imports(main)) load(from);
  // The test's handle on the strip's own state (top-level lets are visible to code in the same script).
  const handle = 'globalThis.__strip = { sprites, frame, apply, applyView, hovered: () => hovered };';
  return `${out.join('\n\n')}\n\n${asConst(main)}\n${handle}`;
}

/** A 2D context that remembers what is on the canvas since the last clear. */
function recordingContext(screen) {
  const target = {
    fillStyle: '',
    measureText: (t) => ({ width: [...String(t)].length * 6 }),
    clearRect: () => (screen.length = 0),
    roundRect: (x, y, w, h) => screen.push({ op: 'roundRect', x, y, w, h }),
    fillText: (text, x, y) => screen.push({ op: 'fillText', text, x, y }),
    getImageData: (x, y, w, h) => ({ data: new Uint8ClampedArray(w * h * 4), width: w, height: h }),
  };
  return new Proxy(target, { get: (t, k) => (k in t ? t[k] : () => {}), set: (t, k, v) => ((t[k] = v), true) });
}

async function bootStrip() {
  const built = buildPet('bellwether');
  const model = parsePetDocument(miniDom(built.xml));
  const screen = [];
  const listeners = {};
  const winListeners = {};
  const docListeners = {};
  const boxes = [];
  let clock = 1000;
  const display = { palette: ['#FF8C00', '#1F5BFF', '#202020'], scale: 1.5, slotWidthPx: 110, homeWanderPx: 0, tint: 'horns', lambScale: 0.6, fadeOpacity: 0.25 };
  const invoke = (cmd, args) => {
    if (cmd === 'get_pet') return Promise.resolve({ display, xml: built.xml, folder: 'bellwether', animationMap: built.pin.animationMap, overlays: built.pin.overlays ?? {} });
    if (cmd === 'get_snapshot') return new Promise(() => {}); // the test hands it snapshots itself
    if (cmd === 'report_hitboxes') boxes.push(args.boxes);
    return Promise.resolve(null);
  };
  const canvas = { width: 0, height: 0, addEventListener: (t, f) => (listeners[t] = f), getContext: () => recordingContext(screen) };
  const offscreen = () => {
    const c = { width: 0, height: 0 };
    c.getContext = () => recordingContext([]);
    return c;
  };
  const document = {
    visibilityState: 'visible',
    getElementById: () => canvas,
    createElement: offscreen,
    addEventListener: (t, f) => (docListeners[t] = f),
  };
  const sandbox = {
    window: { __TAURI__: { core: { invoke } }, innerWidth: 1200, innerHeight: 140, devicePixelRatio: 2, screenX: 0, screenY: 0, addEventListener: (t, f) => (winListeners[t] = f) },
    location: { search: '' },
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
  for (let i = 0; i < 20 && !sandbox.__strip; i++) await new Promise((r) => setTimeout(r, 5));
  const S = sandbox.__strip;
  // Wait for boot's loadPet.
  for (let i = 0; i < 200 && !S.sprites.size; i++) {
    await new Promise((r) => setTimeout(r, 5));
    try {
      S.apply({ rev: 1, muted: false, flock: [{ id: 's1', label: NAME, slot: 0, colourIndex: 0, state: 'sleeping' }] });
    } catch {
      // the pet isn't loaded yet
    }
  }
  assert.equal(S.sprites.size, 1, 'the strip took the sheep');
  let viewRev = 0;
  const strip = {
    S,
    screen,
    document,
    winListeners,
    docListeners,
    sheep: () => S.sprites.get('s1'),
    /** The middle of the sheep, where it is now. */
    centre() {
      const { sprite } = strip.sheep();
      const d = sprite.drawInfo();
      return { x: d.x + sprite.width / 2, y: d.y + sprite.height / 2 };
    },
    view(v = {}) {
      S.applyView({ rev: ++viewRev, hidden: false, showLabels: false, near: [], over: [], ...v });
      strip.step();
    },
    /** One frame, far enough on that it may redraw. */
    step(n = 1) {
      for (let i = 0; i < n; i++) {
        clock += 200;
        S.frame();
      }
    },
    pointer(type, x, y) {
      listeners[type]?.({ offsetX: x, offsetY: y });
      strip.step();
    },
    /** The labels on the canvas now: the dark pill and its text. */
    labels() {
      const out = [];
      screen.forEach((op, i) => {
        if (op.op === 'fillText' && op.text === NAME && screen[i - 1]?.op === 'roundRect') out.push(screen[i - 1]);
      });
      return out;
    },
  };
  strip.view();
  strip.step(3);
  return strip;
}

/** The cursor on the sheep: the pointer event and the shell's poll both say so. */
function hoverOn(strip) {
  strip.view({ over: ['s1'] });
  const c = strip.centre();
  strip.pointer('pointermove', c.x, c.y);
  assert.equal(strip.labels().length, 1, 'the hover label shows while the cursor is on the sheep');
  return c;
}

describe('the hover label (deviation 136)', () => {
  test('shows while the cursor is on its sheep, centred over it, and follows it', async () => {
    const strip = await bootStrip();
    hoverOn(strip);
    const { sprite } = strip.sheep();
    const centreOf = (l) => l.x + l.w / 2;
    assert.ok(Math.abs(centreOf(strip.labels()[0]) - (sprite.drawInfo().x + sprite.width / 2)) < 0.5, 'centred over the sheep');
    // The sheep shifts a little under a still cursor (still on it): the label goes with it.
    sprite.x += 10;
    strip.step();
    const [l] = strip.labels();
    assert.ok(l, 'still shown');
    assert.ok(Math.abs(centreOf(l) - (sprite.drawInfo().x + sprite.width / 2)) < 0.5, 'the label moved with its sheep');
  });

  test('goes when the shell finds the cursor off the sheep, with no pointer event (the window stopped hearing the mouse)', async () => {
    const strip = await bootStrip();
    hoverOn(strip);
    // The cursor left fast: the shell's poll sees it off every box and makes the window
    // click-through, so the webview gets no pointermove and no pointerleave.
    strip.view({ over: [] });
    strip.step(3);
    assert.deepEqual(strip.labels(), [], 'no label left behind');
    assert.equal(strip.S.hovered(), null);
  });

  test('goes when the sheep walks out from under a still cursor', async () => {
    const strip = await bootStrip();
    hoverOn(strip);
    // No pointer event (the cursor didn't move), and the shell's word is a poll behind.
    strip.sheep().sprite.x += 300;
    strip.step(3);
    assert.deepEqual(strip.labels(), [], 'the label went with the hover, not left where the sheep was');
  });

  test('goes on pointerleave, and on the page being hidden', async () => {
    const strip = await bootStrip();
    hoverOn(strip);
    strip.pointer('pointerleave', -1, -1);
    assert.deepEqual(strip.labels(), []);
    hoverOn(strip);
    strip.document.visibilityState = 'hidden';
    strip.docListeners.visibilitychange?.();
    strip.step(2);
    assert.deepEqual(strip.labels(), [], 'hidden page: the pointer is no longer known');
  });

  test('Hide everything, Quiet, and Show labels off or on clear it; it comes back only with the cursor', async () => {
    for (const toggle of [{ hidden: true }, { showLabels: true }]) {
      const strip = await bootStrip();
      hoverOn(strip);
      strip.view({ ...toggle, over: ['s1'] });
      // Back again, and the shell still (stale) says the cursor's over it: no pointer event yet.
      strip.view({ over: ['s1'] });
      strip.step(2);
      assert.deepEqual(strip.labels(), [], `after ${JSON.stringify(toggle)} and back: no label until the cursor moves`);
      const c = strip.centre();
      strip.pointer('pointermove', c.x, c.y);
      assert.equal(strip.labels().length, 1, 'the cursor moving on the sheep shows it again');
    }
    const strip = await bootStrip();
    hoverOn(strip);
    strip.S.apply({ rev: 2, muted: true, flock: [{ id: 's1', label: NAME, slot: 0, colourIndex: 0, state: 'sleeping' }] });
    strip.step(2);
    strip.S.apply({ rev: 3, muted: false, flock: [{ id: 's1', label: NAME, slot: 0, colourIndex: 0, state: 'sleeping' }] });
    strip.step(2);
    assert.deepEqual(strip.labels(), [], 'after Quiet and back: no label until the cursor moves');
  });

  test('goes when its session ends (the sheep runs off), not following it out', async () => {
    const strip = await bootStrip();
    hoverOn(strip);
    strip.S.apply({ rev: 2, muted: false, flock: [] });
    strip.step(3);
    assert.deepEqual(strip.labels(), []);
  });

  test('Show labels off: no other sheep gets a label, and nothing is left on the canvas after the hover', async () => {
    const strip = await bootStrip();
    assert.deepEqual(strip.labels(), [], 'no label without a hover');
    hoverOn(strip);
    strip.view({ over: [] });
    strip.step(2);
    assert.equal(strip.screen.filter((op) => op.op === 'fillText').length, 0, 'no text at all on the canvas');
  });
});

test('the shell\'s cursor poll publishes, per strip, the sprites the cursor is on', () => {
  const poll = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'clickthrough.rs'), 'utf8');
  assert.match(poll, /let ids = over\(b, x, y\);/);
  assert.match(poll, /if now\.near != close \|\| now\.over != under \{[\s\S]*?v\.over = under;/);
  const brain = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'brain.rs'), 'utf8');
  assert.match(brain, /v\["over"\] = /, 'json_for sends each strip its own list');
});

describe('hoverTarget', () => {
  const hitAt = (x) => (x >= 100 && x < 160 ? 'a' : null);
  test('needs the pointer on the sprite where it is now, and the shell\'s poll to agree', () => {
    assert.equal(hoverTarget({ pointer: { x: 120, y: 0 }, over: new Set(['a']), hitAt }), 'a');
    assert.equal(hoverTarget({ pointer: { x: 120, y: 0 }, over: new Set(), hitAt }), null, 'the shell finds the cursor elsewhere');
    assert.equal(hoverTarget({ pointer: { x: 120, y: 0 }, over: null, hitAt }), null, 'no word from the shell');
    assert.equal(hoverTarget({ pointer: { x: 200, y: 0 }, over: new Set(['a']), hitAt }), null, 'the sprite has moved away');
    assert.equal(hoverTarget({ pointer: null, over: new Set(['a']), hitAt }), null, 'no pointer known');
    assert.equal(hoverTarget({ pointer: { x: NaN, y: 0 }, over: new Set(['a']), hitAt }), null);
  });
});
