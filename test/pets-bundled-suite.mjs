import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import zlib from 'node:zlib';
import { describe, test } from 'node:test';
import {
  BEHAVIOURS,
  BUNDLED,
  DEFAULT_PALETTE,
  MASK,
  TILE,
  buildPet,
  bundledPets,
  contrastReport,
  deltaE2000,
  loadPet,
  composite,
  miniDom,
  playerBundle,
  lockWith,
  playerView,
  review,
  rgbOf,
} from '../scripts/build-pet.mjs';
import { isHorn, tintHorns } from '../shell/src/player/horns.mjs';
import { resolveAnimationMap } from '../shell/src/player/pet.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';
import { decodePng, petSheet } from './png.mjs';

/**
 * One bundled pet's checks (distribution plan D8: acceptance (b) to (e) and the silhouette
 * tripwire (h)). Each bundled pet runs them from a test file of its own,
 * `test/pets-bundled-<name>.test.mjs`, so a file stays well inside the runner's 120 s
 * limit on a slow CI machine (with the third pet, one file for all of them didn't).
 */

const ESHEEP = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
const lock = () => JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
const MASK_HEX = Object.values(MASK);
const hex = (d, i) => `#${[d[i], d[i + 1], d[i + 2]].map((v) => v.toString(16).padStart(2, '0')).join('')}`;

/** Registers the checks of the bundled pet `name`. */
export function bundledPetSuite(name) {
  describe(`the bundled pet ${name}`, () => {
    const built = buildPet(name);
    const view = playerView(built.xml, DEFAULT_PALETTE, built.pin.overlays ?? {});
    const file = path.join(AGENT_DIR, 'pets', name, 'animations.xml');

    test('(e) the pin matches the committed file; a rebuild is byte-identical', () => {
      const bytes = fs.readFileSync(file);
      const pin = lock().bundled[name];
      assert.equal(pin.sha256, sha256(bytes));
      assert.equal(pin.bytes, bytes.length);
      assert.equal(buildPet(name).xml, built.xml);
    });

    test('the sheet is a plain RGBA PNG with real alpha: no gamma, time or text chunks, no magenta key', () => {
      const types = [];
      for (let p = 8; p < built.png.length; ) {
        const len = built.png.readUInt32BE(p);
        types.push(built.png.toString('latin1', p + 4, p + 8));
        p += 12 + len;
      }
      assert.deepEqual(types, ['IHDR', 'IDAT', 'IEND']);
      assert.equal(built.png[25], 6, 'colour type 6 (RGBA)');
      assert.doesNotMatch(built.xml, /<transparency>/);
      // node:zlib and the tests' own decoder read the same pixels the script wrote.
      const decoded = decodePng(built.png);
      assert.deepEqual(Buffer.from(decoded.data), Buffer.from(built.sheet.data));
      assert.ok(zlib.inflateSync(built.png.subarray(41, 41 + built.png.readUInt32BE(33))).length > 0);
      const pixels = petSheet(built.xml);
      for (let i = 0; i < pixels.data.length; i += 4) {
        assert.ok(pixels.data[i + 3] === 0 || pixels.data[i + 3] === 255, 'alpha is 0 or 255');
        if (pixels.data[i + 3]) assert.notEqual(hex(pixels.data, i), '#ff00ff');
      }
      assert.equal(pixels.width, 16 * TILE);
      assert.equal(view.model.tilesx, 16);
    });

    test('(a) the player\'s parser reads it: the title, the tiles and every animation', () => {
      assert.equal(view.model.title, built.src.pet.title);
      assert.equal(view.base.width / view.model.tilesx, TILE);
      assert.equal(view.base.height / view.model.tilesy, TILE);
      assert.deepEqual(Object.keys(view.model.byName), Object.keys(built.src.pet.animations));
    });

    test('(b) all 14 behaviours are mapped, each resolves, and every frame is on the sheet', () => {
      const map = lock().bundled[name].animationMap;
      assert.deepEqual(Object.keys(map).sort(), [...BEHAVIOURS].sort());
      const warnings = [];
      const resolved = resolveAnimationMap(view.model, map, (w) => warnings.push(w));
      assert.deepEqual(warnings, []);
      assert.deepEqual(Object.keys(resolved).sort(), [...BEHAVIOURS].sort());
      const max = view.model.tilesx * view.model.tilesy;
      for (const a of Object.values(view.model.animations)) {
        assert.ok(a.frames.length > 0, a.name);
        for (const f of a.frames) assert.ok(Number.isInteger(f) && f >= 0 && f < max, `${a.name} frame ${f}`);
        for (const n of a.next) assert.ok(view.model.animations[n.id], `${a.name} next ${n.id}`);
      }
      // The player's alert holds turn's last frame, which must face front: a frame of its own.
      const turn = view.model.animations[resolved.turn[0]];
      assert.ok(turn.frames.length >= 2 && new Set(turn.frames).size === turn.frames.length);
      assert.equal(view.model.animations[resolved.walk[0]].start.x.startsWith('-'), true, 'walk moves left: the art faces left');
    });

    test('(b) each behaviour plays the animation of its own name (sleep: sleep, then sleep loop)', { todo: built.src.pet.draft ? `draft: ${built.src.pet.draft}` : false }, () => {
      const map = lock().bundled[name].animationMap;
      for (const b of BEHAVIOURS) assert.deepEqual(map[b], b === 'sleep' ? ['sleep', 'sleep loop'] : b, b);
    });

    test('(c) the mask: every opaque pixel is a mask tone or outside the hue window, and the helmet shows in every mapped frame', () => {
      const d = view.base.data;
      const W = view.base.width;
      const masked = new Map();
      for (let i = 0; i < d.length; i += 4) {
        if (d[i + 3] < 128) continue;
        const h = hex(d, i);
        const mask = MASK_HEX.includes(h);
        assert.ok(mask || !isHorn(d[i], d[i + 1], d[i + 2], d[i + 3]), `${h} is in the tinter's window but isn't a mask tone`);
        if (!mask) continue;
        const p = i / 4;
        const tile = Math.floor((p % W) / TILE) + view.model.tilesx * Math.floor(Math.floor(p / W) / TILE);
        const t = masked.get(tile) ?? { all: 0, mid: 0 };
        t.all++;
        if (h === MASK['mask-mid']) t.mid++;
        masked.set(tile, t);
      }
      const map = lock().bundled[name].animationMap;
      for (const [b, ids] of Object.entries(resolveAnimationMap(view.model, map)))
        for (const id of ids)
          for (const f of view.model.animations[id].frames) {
            const t = masked.get(f) ?? { all: 0, mid: 0 };
            assert.ok(t.all >= 8 && t.mid >= 4, `${b}: ${view.model.animations[id].name} frame ${f} shows too little helmet (${t.all} mask pixels, ${t.mid} mid tone)`);
          }
    });

    test('(d) contrast with the session colour, CIEDE2000, for all 8 default palette colours', () => {
      const report = contrastReport(built.src, DEFAULT_PALETTE);
      assert.ok(report.length > 0);
      for (const r of report) assert.ok(r.ok, `${r.char} ${r.hex} (${r.role}), rule ${r.rule}: ${r.worst.toFixed(1)} against ${r.against}, needs ${r.need}`);
      // A dark colour beside the helmet (the face) falls under rule 1, not rule 2 (D8).
      const face = report.find((r) => r.role === 'face');
      if (face) assert.equal(face.rule, 1);
    });

    test('(h) not a copy: no frame\'s silhouette matches an esheep64 frame at IoU >= 0.95, mirrored or not', { skip: !fs.existsSync(ESHEEP) && 'no local esheep64 (node scripts/fetch-pets.mjs); the tripwire is skipped' }, (t) => {
      // Read programmatically only: the developer agent never looks at these pixels (D8).
      const theirs = silhouettes(petSheet(fs.readFileSync(ESHEEP, 'utf8')), true);
      const ours = silhouettes(view.base, false);
      let worst = { iou: 0 };
      for (const [i, a] of ours.entries())
        for (const [j, b] of theirs.entries()) {
          if (!a.n || !b.n) continue;
          for (const mirror of [false, true]) {
            const v = iou(a, b, mirror);
            if (v > worst.iou) worst = { iou: v, ours: i, theirs: j, mirror };
          }
        }
      t.diagnostic(`highest silhouette IoU against esheep64: ${worst.iou.toFixed(3)}`);
      assert.ok(worst.iou < 0.95, `our tile ${worst.ours} matches esheep64 tile ${worst.theirs}${worst.mirror ? ' (mirrored)' : ''} at IoU ${worst.iou.toFixed(3)}`);
    });

    test('review writes the sheets of (f): 2 scales x 2 densities x 2 backgrounds, an overview and review.html', () => {
      const out = tempDir('pet-review-');
      const { files } = review(name, out);
      assert.equal(files.length, 10);
      for (const f of files.filter((f) => f.endsWith('.png'))) assert.ok(decodePng(fs.readFileSync(f)).width > 0, f);
      assert.ok(files.some((f) => f.endsWith(`${name}-lamb-1x-dark.png`)));
      // review.html: self-contained (no src or href to load), its script parses, and it carries this build of the pet.
      const html = fs.readFileSync(path.join(out, 'review.html'), 'utf8');
      assert.doesNotMatch(html, /<script[^>]+src=|<link[^>]+href=|https?:\/\//);
      const script = /<script>([\s\S]*)<\/script>/.exec(html)[1];
      assert.doesNotThrow(() => new vm.Script(script));
      assert.ok(script.includes(JSON.stringify(built.xml).replace(/</g, '\\u003c').slice(0, 200)));
    });
  });
}


/** Each 40x40 tile's opaque silhouette as a bit set (esheep64's magenta key counts as clear). */
function silhouettes(sheet, keyed) {
  const cols = sheet.width / TILE;
  const rows = sheet.height / TILE;
  const out = [];
  for (let t = 0; t < cols * rows; t++) {
    const bits = new Uint8Array(TILE * TILE);
    let n = 0;
    for (let y = 0; y < TILE; y++)
      for (let x = 0; x < TILE; x++) {
        const i = (((Math.floor(t / cols) * TILE + y) * sheet.width) + (t % cols) * TILE + x) * 4;
        const d = sheet.data;
        const clear = d[i + 3] < 128 || (keyed && d[i] > 240 && d[i + 1] < 16 && d[i + 2] > 240);
        if (!clear) {
          bits[y * TILE + x] = 1;
          n++;
        }
      }
    out.push({ bits, n });
  }
  return out;
}

function iou(a, b, mirror) {
  let both = 0;
  let either = 0;
  for (let y = 0; y < TILE; y++)
    for (let x = 0; x < TILE; x++) {
      const p = a.bits[y * TILE + x];
      const q = b.bits[y * TILE + (mirror ? TILE - 1 - x : x)];
      if (p && q) both++;
      if (p || q) either++;
    }
  return either ? both / either : 0;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
