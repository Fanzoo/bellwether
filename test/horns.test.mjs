import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { HORN_HUE, isHorn, tintHorns, tintWhole } from '../shell/src/player/horns.mjs';
import { AGENT_DIR, FIXTURES } from './helpers.mjs';
import { petSheet } from './png.mjs';

/** M10-13: the session colour on the horns only. */

const HORNS = ['#730073', '#b300b3', '#ff7aff'];
const rgb = (hex) => [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16));

describe('the horn mask, on every colour esheep64 uses', () => {
  const { colours } = JSON.parse(fs.readFileSync(path.join(FIXTURES, 'esheep64-colours.json'), 'utf8'));
  test('it takes exactly the three horn shades: no wool, face, eye, outline or key colour', () => {
    const caught = Object.keys(colours).filter((c) => isHorn(...rgb(c), 255));
    assert.deepEqual(caught.sort(), HORNS);
    assert.ok(Object.keys(colours).length > 30);
    assert.equal(isHorn(255, 0, 255, 255), false, 'the magenta transparency key');
    assert.equal(isHorn(0xb3, 0, 0xb3, 0), false, 'transparent');
  });
  test('recolouring keeps three distinct tones: the mid tone becomes the colour exactly, darker below, lighter above', () => {
    const px = new Uint8ClampedArray(HORNS.flatMap((c) => [...rgb(c), 255]).concat([0xf0, 0xf0, 0x90, 255]));
    assert.equal(tintHorns(px, '#1F5BFF'), 3, 'only the three horn pixels');
    const [dark, mid, light] = [0, 4, 8].map((i) => [...px.slice(i, i + 3)]);
    assert.deepEqual(mid, [0x1f, 0x5b, 0xff]);
    const sum = (a) => a[0] + a[1] + a[2];
    assert.ok(sum(dark) < sum(mid) && sum(mid) < sum(light));
    assert.deepEqual([...px.slice(12, 15)], [0xf0, 0xf0, 0x90], 'the wool pixel is untouched');
  });
  test('whole mode multiplies every pixel, as before', () => {
    const px = new Uint8ClampedArray([255, 255, 255, 255, 0xb3, 0, 0xb3, 255]);
    tintWhole(px, '#808080');
    assert.deepEqual([...px.slice(0, 3)], [128, 128, 128]);
    assert.ok(HORN_HUE[0] < 300 && HORN_HUE[1] > 300);
  });
});

const xml = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
describe('the horn mask, frame by frame on the real sheet', { skip: !fs.existsSync(xml) && 'no pet art here (node scripts/fetch-pets.mjs)' }, () => {
  test('every frame of every animation bellwether uses shows horns; the frames without are only bath, flower and black-sheep ones', () => {
    const text = fs.readFileSync(xml, 'utf8');
    const sheet = petSheet(text);
    const tilesx = Number(/<tilesx>(\d+)/.exec(text)[1]);
    const T = sheet.width / tilesx;
    const horny = new Set();
    for (let i = 0; i < sheet.data.length; i += 4) {
      if (isHorn(sheet.data[i], sheet.data[i + 1], sheet.data[i + 2], sheet.data[i + 3])) {
        const p = i / 4;
        horny.add(Math.floor((p % sheet.width) / T) + tilesx * Math.floor(Math.floor(p / sheet.width) / T));
      }
    }
    const anims = Object.fromEntries(text.split('<animation ').slice(1).map((a) => [/<name>([^<]*)<\/name>/.exec(a)?.[1], [...a.matchAll(/<frame>(\d+)<\/frame>/g)].map((m) => Number(m[1]))]));
    const used = ['walk', 'run', 'eat', 'sleep1a', 'sleep1b', 'boing', 'rotate1a', 'fall soft', 'sync', 'sleep2a', 'jump', 'sleep3a'];
    for (const name of used) for (const f of anims[name] ?? []) assert.ok(horny.has(f), `${name} frame ${f} has no horns`);
    const without = Object.entries(anims).filter(([, fs2]) => fs2.some((f) => !horny.has(f))).map(([n]) => n).sort();
    // Horns are hidden only while the sheep is in the bath, behind a flower, or swapped
    // for the black sheep; none of these is mapped to a bellwether behaviour.
    assert.ok(without.length > 0 && without.every((n) => /^(bath|flower|blacksheep)/.test(n)), without.join(', '));
  });
});

describe('the tint mode', () => {
  test('pets.tint is "horns" by default, "whole" switches back; the shell passes it and the webview honours it', async () => {
    const { withDefaults } = await import('./impl.mjs');
    assert.throws(() => withDefaults({ pets: { tint: 'stripes' } }), /pets\.tint must be "horns".*or "whole"/);
    withDefaults({ pets: { tint: 'whole' } });
    assert.equal(JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8')).pets.tint, 'horns');
    const rs = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    // (M7: get_pet reads a copy of the config the onboarding window's pet choice can change.)
    assert.match(rs, /let config = shared\.config\.read\(\)\.unwrap\(\)\.clone\(\);/);
    assert.match(rs, /"tint": match config\.pointer\("\/pets\/tint"\)\.and_then\(Value::as_str\) \{ Some\("whole"\) => "whole", _ => "horns" \}/);
    const main = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src', 'main.js'), 'utf8');
    assert.match(main, /if \(mode === 'whole'\) tintWhole\(img\.data, hex\);\s*else tintHorns\(img\.data, hex\);/);
    // Once per palette entry at load, never per frame.
    assert.equal([...main.matchAll(/tinted\(/g)].length, 2, 'defined once, called once (in the palette map)');
    assert.match(main, /display\.palette\.map\(\(hex\) => tinted\(base, hex, display\.tint\)\)/);
  });
});
