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
 * Bellwether's own pets (distribution plan D8, milestone M10a): acceptance (b) to (e) and
 * the silhouette tripwire (h), run on every bundled pet that has sources: each from a file
 * of its own (pets-bundled-<name>.test.mjs, from pets-bundled-suite.mjs). (a), the
 * webview's real parser, is `--selftest` on a build; here the same pet.mjs parser reads
 * the file through a small DOM.
 */

const PETS = bundledPets();
const ESHEEP = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
const lock = () => JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
const MASK_HEX = Object.values(MASK);
const hex = (d, i) => `#${[d[i], d[i + 1], d[i + 2]].map((v) => v.toString(16).padStart(2, '0')).join('')}`;

test('there is at least the sheep, and the bundled names are fixed', () => {
  assert.deepEqual(BUNDLED, ['bellwether', 'brah', 'bryan']);
  assert.ok(PETS.includes('bellwether'), 'M10a ships the sheep');
});

test('every bundled pet has its own checks file (test/pets-bundled-<name>.test.mjs, which runs pets-bundled-suite.mjs)', () => {
  for (const name of PETS) {
    const f = path.join(AGENT_DIR, 'test', `pets-bundled-${name}.test.mjs`);
    assert.ok(fs.existsSync(f), `${f} is missing: copy pets-bundled-bryan.test.mjs and change the name`);
    assert.match(fs.readFileSync(f, 'utf8'), new RegExp(`bundledPetSuite\\('${name}'\\)`));
  }
});

test('build-pet --check passes: every bundled pet and its pin are what the sources build (e)', () => {
  const text = fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8');
  const pins = {};
  for (const name of PETS) {
    const b = buildPet(name);
    assert.equal(fs.readFileSync(path.join(AGENT_DIR, 'pets', name, 'animations.xml'), 'utf8'), b.xml, `${name}: run node scripts/build-pet.mjs build ${name}`);
    pins[name] = b.pin;
  }
  assert.equal(lockWith(text, pins), text, 'pets.lock.json\'s "bundled" pins');
  assert.deepEqual(Object.keys(lock().bundled), PETS, 'no pin without sources, in the fixed order');
});

test('the fetch pins still list only esheep64: bundled pets are never downloaded', async () => {
  const { PINNED_PETS } = await import('../scripts/fetch-pets.mjs');
  assert.deepEqual(Object.keys(lock().pets), ['esheep64']);
  assert.deepEqual(Object.keys(PINNED_PETS), ['esheep64'], 'what scripts/fetch-pets.mjs downloads');
  assert.deepEqual(Object.keys(lock().bundled), PETS);
  for (const n of BUNDLED) assert.ok(!Object.hasOwn(lock().pets, n), `${n} isn't under "pets"`);
});

describe('the pipeline refuses bad sources, with the reason', () => {
  const mkPet = (palette, frame, extra = {}) => {
    const dir = tempDir('pet-src-');
    const root = path.join(dir, 'bellwether');
    fs.mkdirSync(path.join(root, 'src', 'frames'), { recursive: true });
    fs.mkdirSync(path.join(root, 'src', 'parts'), { recursive: true });
    const colours = [
      { char: '1', hex: '#730073', role: 'mask-dark' },
      { char: '2', hex: '#b300b3', role: 'mask-mid' },
      { char: '3', hex: '#ff7aff', role: 'mask-light' },
      { char: 'o', hex: '#2a2522', role: 'outline' },
      ...palette,
    ];
    fs.writeFileSync(path.join(root, 'src', 'palette.json'), JSON.stringify({ colours }));
    fs.writeFileSync(path.join(root, 'src', 'parts', 'dot.txt'), '22\no2\n');
    fs.writeFileSync(path.join(root, 'src', 'frames', 'walk_01.txt'), frame);
    for (const [f, t] of Object.entries(extra)) fs.writeFileSync(path.join(root, 'src', 'frames', f), t);
    const behaviours = Object.fromEntries(BEHAVIOURS.map((b) => [b, 'walk']));
    fs.writeFileSync(path.join(root, 'pet.json'), JSON.stringify({ title: 'T', animations: { walk: { frames: ['walk_01'], x: -2 } }, behaviours }));
    return dir;
  };
  test('a good composition resolves to 40x40, with flip, overlay, erase and shift', () => {
    const dir = mkPet([], 'part dot 0 38\npart dot 10 38 flip\ngrid 0 0\n3~\nend\nshift 1 0\n');
    const src = loadPet('bellwether', dir);
    const g = src.frames.get('walk_01');
    assert.equal(g.length, 40);
    assert.ok(g.every((r) => r.length === 40));
    assert.equal(g[39].slice(0, 3).join(''), '.o2');
    assert.equal(g[39].slice(11, 13).join(''), '2o', 'flipped');
    assert.equal(g[0].slice(0, 2).join(''), '.3', 'the overlay, shifted');
  });
  for (const [why, palette, frame, extra, want] of [
    ['an unknown character', [], 'grid 0 0\nx\nend\n', {}, /isn't in palette\.json/],
    ['a mask role in another colour', [{ char: '4', hex: '#b300b4', role: 'mask-mid' }], 'part dot 0 0\n', {}, /listed twice|exactly|used twice/],
    ['a non-mask colour in the hue window', [{ char: 'm', hex: '#a020a0', role: 'scarf' }], 'part dot 0 0\n', {}, /hue window/],
    ['#FF00FF', [{ char: 'k', hex: '#ff00ff', role: 'key' }], 'part dot 0 0\n', {}, /FF00FF/],
    ['a part off the tile', [], 'part dot 39 39\n', {}, /outside the 40x40 tile/],
    ['a short grid frame', [], 'o\n', {}, /40 rows of 40/],
    ['a frame file no animation uses', [], 'part dot 0 0\n', { 'stray_01.txt': 'part dot 0 0\n' }, /stray_01 isn't used/],
    ['an unknown directive', [], 'paint dot 0 0\n', {}, /40 rows|unknown line/],
  ]) {
    test(why, () => {
      assert.throws(() => loadPet('bellwether', mkPet(palette, frame, extra)), want);
    });
  }
  test('the colour maths: CIEDE2000 matches the values D8 quotes', () => {
    assert.equal(deltaE2000(rgbOf('#2e2a2a'), rgbOf('#202020')).toFixed(1), '4.3');
    assert.equal(deltaE2000(rgbOf('#ff9a1f'), rgbOf('#ff8c00')).toFixed(1), '3.9');
    assert.equal(deltaE2000(rgbOf('#dde8f0'), rgbOf('#00c2d4')).toFixed(1), '23.8');
  });
});

test('review.html runs the player\'s own modules: the bundle exports what the page uses, and they work', () => {
  const ctx = {};
  vm.runInNewContext(`${playerBundle()}\nthis.M = { M_expr, M_stepper, M_sprite, M_pet, M_horns };`, ctx);
  const { M_pet, M_horns, M_sprite, M_stepper } = ctx.M;
  for (const f of ['parsePetDocument', 'assertSafePetXml', 'resolveAnimationMap']) assert.equal(typeof M_pet[f], 'function', f);
  for (const f of ['totalSteps', 'frameAt', 'motionAt', 'pickNext', 'tileOf']) assert.equal(typeof M_stepper[f], 'function', f);
  assert.equal(typeof M_sprite.Sprite, 'function');
  const px = new Uint8ClampedArray([0xb3, 0, 0xb3, 255]);
  M_horns.tintHorns(px, '#1F5BFF');
  assert.deepEqual([...px], [0x1f, 0x5b, 0xff, 255]);
  // The bundled Sprite walks a bundled pet across a strip.
  for (const name of PETS) {
    const b = buildPet(name);
    const model = M_pet.parsePetDocument(miniDom(b.xml));
    const map = M_pet.resolveAnimationMap(model, b.src.pet.behaviours);
    const sp = new M_sprite.Sprite({ model, map, width: 60, height: 60, stripWidth: 900, stripHeight: 110, home: 400, wander: 200, rand: () => 0.3 });
    sp.setBehaviour('walk', 0);
    const frames = new Set();
    const xs = new Set();
    for (let t = 0; t < 3000; t += 16) {
      sp.tick(t);
      frames.add(sp.drawInfo(t).frame);
      xs.add(sp.drawInfo(t).x);
    }
    assert.ok(frames.size >= 4 && xs.size > 20, `${name}: ${frames.size} frames, ${xs.size} positions`);
  }
});

test('a pet without carried or land (an imported one) falls back to walk in the player; a bundled pet plays both when dragged (milestone 12)', async () => {
  const { Sprite } = await import('../shell/src/player/sprite.mjs');
  const { parsePetDocument: parse } = await import('../shell/src/player/pet.mjs');
  for (const name of PETS) {
    const b = buildPet(name);
    const model = parse(miniDom(b.xml));
    const old = Object.fromEntries(Object.entries(b.src.pet.behaviours).filter(([k]) => k !== 'carried' && k !== 'land'));
    const warnings = [];
    const map = resolveAnimationMap(model, old, (w) => warnings.push(w));
    assert.deepEqual(warnings, []);
    const sp = new Sprite({ model, map, width: 60, height: 60, stripWidth: 900, stripHeight: 110, home: 400, wander: 100 });
    assert.deepEqual(sp.idsFor('carried'), map.walk);
    assert.deepEqual(sp.idsFor('land'), map.walk);
    const full = resolveAnimationMap(model, b.src.pet.behaviours);
    assert.equal(model.animations[full.carried[0]].name, 'carried');
    assert.equal(model.animations[full.land[0]].name, 'land');
    // Milestone 12 plays them: carried while held, land once it is set down.
    const dragged = new Sprite({ model, map: full, width: 60, height: 60, stripWidth: 900, stripHeight: 110, home: 400, wander: 100 });
    dragged.setBehaviour('graze', 0);
    dragged.hold(10);
    assert.equal(model.animations[dragged.anim.id].name, 'carried', `${name}: carried while held`);
    dragged.setDown(20);
    const played = new Set();
    for (let t = 20; t < 3000; t += 20) {
      dragged.tick(t);
      played.add(model.animations[dragged.anim.id].name);
    }
    assert.ok(played.has('land'), `${name}: land after the drop (${[...played]})`);
    // Without them: walk frames while held, and no landing.
    sp.setBehaviour('graze', 0);
    sp.hold(10);
    assert.deepEqual(map.walk.includes(sp.anim.id), true);
  }
  // The strip's own default map (esheep64's names) asks for neither, so nothing is missing.
  const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
  const defaults = main.slice(main.indexOf('const DEFAULT_ANIMATION_MAP'), main.indexOf('};', main.indexOf('const DEFAULT_ANIMATION_MAP')));
  assert.doesNotMatch(defaults, /carried|land/);
});

describe('overlays: the sleeping "z", drawn separately and never mirrored (deviation 125)', async () => {
  const { cleanOverlays, overlayPlacements } = await import('../shell/src/player/overlay.mjs');
  test('placement mirrors the position for a sheep facing right, never the image', () => {
    const ov = { 7: [{ tile: 53, x: 10, y: 11, w: 7, h: 7 }] };
    const [left] = overlayPlacements(ov, 7, { x: 100, y: 50, width: 60, tileW: 40, flip: false });
    const [right] = overlayPlacements(ov, 7, { x: 100, y: 50, width: 60, tileW: 40, flip: true });
    assert.deepEqual(left, { tile: 53, sw: 7, sh: 7, dx: 115, dy: 67, dw: 10.5, dh: 10.5 });
    assert.equal(right.dx, Math.round(100 + (40 - 10 - 7) * 1.5), 'beside the head on the right');
    assert.ok(right.dw > 0 && right.sw > 0, 'a positive width: not flipped');
    assert.deepEqual(overlayPlacements(ov, 8, { x: 0, y: 0, width: 60, tileW: 40, flip: true }), []);
    assert.deepEqual(overlayPlacements(undefined, 7, { x: 0, y: 0, width: 60, tileW: 40, flip: false }), [], 'an imported pet has none');
  });
  test('only well-formed overlays get through', () => {
    assert.deepEqual(cleanOverlays({ 1: [{ tile: 3, x: 1, y: 2, w: 3, h: 4, extra: 1 }], x: [], 2: [{ tile: 99, x: 0, y: 0, w: 1, h: 1 }], 3: 'no' }, 64), { 1: [{ tile: 3, x: 1, y: 2, w: 3, h: 4 }] });
    assert.deepEqual(cleanOverlays(null, 64), {});
  });
  test('the strip draws overlays after its mirrored transform is restored, unscaled', () => {
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    const body = /function drawSprite\([\s\S]*?\n\}\n/.exec(main)[0];
    const restore = body.indexOf('ctx.restore();');
    const over = body.indexOf('overlayPlacements(');
    assert.ok(restore > 0 && over > restore, 'after the restore');
    assert.doesNotMatch(body.slice(over), /scale\(/, 'no scale after it: never mirrored');
    assert.match(main, /cleanOverlays\(p\.overlays, model\.tilesx \* model\.tilesy\)/);
    const rs = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.match(rs, /let overlays = bundled_overlays\(&folder\);/);
  });
  for (const name of PETS) {
    test(`${name}: the overlays come from pet.json, outside the frames, and (c) and (d) cover their pixels`, () => {
      const b = buildPet(name);
      const pinned = lock().bundled[name].overlays ?? {};
      assert.deepEqual(pinned, b.pin.overlays ?? {});
      for (const [stem, list] of b.src.overlays) {
        const i = b.src.order.indexOf(stem);
        assert.ok(pinned[i], `${stem} is pinned`);
        const frame = b.src.frames.get(stem).flat().join('');
        const shown = composite(b.src, stem).flat().join('');
        assert.notEqual(frame, shown, `${stem}: the overlay isn't in the frame itself`);
        for (const o of list) for (const row of b.src.overlayGrids.get(o.part)) for (const ch of row) assert.ok(ch === '.' || !b.src.palette.byChar.get(ch).mask, 'no mask tone in an overlay');
      }
      // (d) is measured on the frames as shown (with their overlays): every colour an overlay draws (the z's white and outline) is in it.
      const reported = new Set(contrastReport(b.src).map((r) => r.char));
      for (const part of b.src.overlayParts) for (const row of b.src.overlayGrids.get(part)) for (const ch of row) if (ch !== '.') assert.ok(reported.has(ch), `${part}: ${ch} is measured by (d)`);
      // The overlay tiles are part of the sheet, which (c) checks pixel by pixel.
      assert.ok(b.sheet.tilesx * b.sheet.tilesy >= b.src.order.length + b.src.overlayParts.length);
    });
  }
  if (PETS.includes('bellwether'))
    test('bellwether: the sleeping "z" is an overlay on sleep 4 and every sleep-loop frame', () => {
      const b = buildPet('bellwether');
      assert.deepEqual([...b.src.overlays.keys()], ['sleep_04', 'sleep_loop_01', 'sleep_loop_02', 'sleep_loop_03']);
      assert.deepEqual(b.src.overlayParts, ['z_small', 'z_big']);
    });
});

test('review.html runs to the end: its script, run with a stub DOM, draws every scene without an error', async () => {
  const out = tempDir('pet-review-run-');
  review(PETS[0], out);
  const html = fs.readFileSync(path.join(out, 'review.html'), 'utf8');
  const script = /<script>([\s\S]*)<\/script>/.exec(html)[1];
  const els = new Map();
  const el = (id) => {
    if (!els.has(id)) els.set(id, { id, textContent: '', style: {}, dataset: {}, checked: false, children: [], append(...c) { this.children.push(...c); }, addEventListener() {} });
    return els.get(id);
  };
  const ctx2d = (w, h) => ({ setTransform() {}, clearRect() {}, save() {}, restore() {}, translate() {}, scale() {}, drawImage() { draws++; }, getImageData: () => ({ data: new Uint8ClampedArray(w * h * 4) }), putImageData() {}, set imageSmoothingEnabled(_) {} });
  let draws = 0;
  const document = {
    body: { dataset: {} },
    getElementById: el,
    querySelectorAll: () => [],
    createElement: (tag) => {
      const e = { tag, style: {}, dataset: {}, children: [], append(...c) { this.children.push(...c); }, width: 640, height: 160, textContent: '', className: '' };
      e.getContext = () => ctx2d(e.width, e.height);
      return e;
    },
  };
  const frames = [];
  const sandbox = {
    document,
    window: { devicePixelRatio: 2 },
    location: { hash: '#t=4000', reload() {} },
    performance: { now: () => 0 },
    requestAnimationFrame: (f) => frames.push(f),
    DOMParser: class { parseFromString(xml) { return miniDom(xml); } },
    Image: class { set src(_) {} get width() { return 640; } get height() { return 160; } decode() { return Promise.resolve(); } },
    console,
  };
  vm.runInNewContext(script, sandbox);
  await new Promise((r) => setTimeout(r, 50));
  assert.equal(el('info').textContent.startsWith('FAILED'), false, el('info').textContent);
  assert.equal(document.body.dataset.ready, '1');
  assert.doesNotMatch(el('info').textContent, /WARNINGS/);
  assert.ok(draws > 100, `drew ${draws} images while fast-forwarding`);
});

describe('bundling and choosing (plan D8)', () => {
  test('every bundled pet\'s animations.xml (only) is a resource of the app, at Resources/pets/<name>/', () => {
    const conf = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'tauri.dist.conf.json'), 'utf8'));
    assert.deepEqual(conf.bundle.resources, Object.fromEntries(PETS.map((n) => [`../../pets/${n}/animations.xml`, `pets/${n}/animations.xml`])));
    const app = fs.readFileSync(path.join(AGENT_DIR, 'scripts', 'build-app.mjs'), 'utf8');
    assert.match(app, /overlay\.bundle\.resources = Object\.fromEntries/, 'build-app makes the paths absolute for its overlay');
  });
  test('the setup window tints each still\'s helmet with the first palette colour, exactly as the strip\'s tinter does', () => {
    const js = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'onboarding.js'), 'utf8');
    assert.match(js, /still\(p\.still, pets\.tint\)/);
    assert.match(js, /if \(tint\) tintMask\(d\.data, tint\);/);
    const src = /\nfunction tintMask\(data, hex\) \{[\s\S]*?\n\}\n/.exec(js)[0];
    const copy = vm.runInNewContext(`${src}; tintMask`);
    // Every mask tone, the magenta key, transparent and near-window colours, and a spread of others.
    const colours = [...MASK_HEX, '#ff00ff', '#a020f0', '#c000c0', '#800080', '#ff80ff', '#2a2522', '#f3ead6'];
    for (let v = 0; v < 4096; v += 7) colours.push(`#${((v * 2654435761) >>> 8 & 0xffffff).toString(16).padStart(6, '0')}`);
    const px = colours.flatMap((c, i) => [...rgbOf(c), i % 9 === 0 ? 100 : 255]);
    for (const tint of [...DEFAULT_PALETTE, '#123456', 'nope']) {
      const a = new Uint8ClampedArray(px);
      const b = new Uint8ClampedArray(px);
      assert.equal(copy(a, tint), tintHorns(b, tint), tint);
      assert.deepEqual([...a], [...b], tint);
    }
  });
  test('config.example.json\'s pet is the sheep', () => {
    assert.equal(JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8')).pets.default, 'bellwether');
  });
});

describe('(g) provenance', () => {
  test('pets/LICENSE is the CC0 1.0 Universal legal code, with its public-licence fallback', () => {
    const l = fs.readFileSync(path.join(AGENT_DIR, 'pets', 'LICENSE'), 'utf8');
    for (const s of ['CC0 1.0 Universal', 'Statement of Purpose', '1. Copyright and Related Rights.', '2. Waiver.', '3. Public License Fallback.', '4. Limitations and Disclaimers.']) assert.ok(l.includes(s), s);
    assert.equal(createHash('sha256').update(l).digest('hex'), 'a2010f343487d3f7618affe54f789f5487602331c0a8d03f49e9a7c547cf0499', 'the legal code as Creative Commons publishes it');
  });
  test('NOTICE says how the bundled pets were made and dedicates them under CC0, naming each bundled pet', () => {
    const n = fs.readFileSync(path.join(AGENT_DIR, 'NOTICE'), 'utf8').replace(/\s+/g, ' ');
    assert.match(n, /Bundled pets \(the sheep "bellwether", the aviator goose "brah" and the zombie aviator "bryan"\), CC0-1\.0\./);
    assert.match(n, /made by an AI coding agent under the direction of the project's maintainer, from a written specification \(pets\/SPEC\.md\)/);
    assert.match(n, /No third-party art was used/);
    assert.match(n, /dedicated to the public domain under CC0 1\.0 Universal \(pets\/LICENSE\)/);
    assert.doesNotMatch(n, /Clearly licensed replacement art is planned/);
    for (const name of PETS) assert.ok(n.includes(`"${name}"`), `NOTICE names ${name}`);
  });
});
