import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR } from './helpers.mjs';
import { decodePng } from './png.mjs';
import { tintHorns } from '../shell/src/player/horns.mjs';

/**
 * BR-3K's independent check of milestone 10b (the goose "brah"): D8 (a)-(e) and (h)
 * recomputed from the committed animations.xml with a separate hue test, a separate
 * CIEDE2000 and a separate adjacency scan, so they don't share code with
 * scripts/build-pet.mjs. Fails if the developer's gate code and the plain definition disagree.
 */

const GOOSE = path.join(AGENT_DIR, 'pets', 'brah');
const xml = fs.readFileSync(path.join(GOOSE, 'animations.xml'), 'utf8');
const T = 40;
const PALETTE8 = ['#FF8C00', '#1F5BFF', '#202020', '#E8202A', '#00C2D4', '#8B4513', '#12A84A', '#FF4FB0'];
const MASK3 = ['#730073', '#b300b3', '#ff7aff'];

const hexRgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const toHex = (r, g, b) => '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');

function sheet(x) {
  const tx = +/<tilesx>(\d+)/.exec(x)[1];
  const ty = +/<tilesy>(\d+)/.exec(x)[1];
  const b64 = /<png>([\s\S]*?)<\/png>/.exec(x)[1].replace('<![CDATA[', '').replace(']]>', '').replace(/\s+/g, '');
  return { tx, ty, png: decodePng(Buffer.from(b64, 'base64')) };
}
const S = sheet(xml);
const px = (tile, X, Y) => {
  const gx = (tile % S.tx) * T + X, gy = Math.floor(tile / S.tx) * T + Y;
  const i = (gy * S.png.width + gx) * 4, d = S.png.data;
  return [d[i], d[i + 1], d[i + 2], d[i + 3]];
};

// The frames each animation really plays, from the XML itself.
const anims = [...xml.matchAll(/<animation id="\d+">\s*<name>([^<]+)<\/name>([\s\S]*?)<\/animation>/g)].map((m) => ({
  name: m[1],
  frames: [...m[2].matchAll(/<frame>(\d+)<\/frame>/g)].map((f) => +f[1]),
}));
const mapped = [...new Set(anims.flatMap((a) => a.frames))].sort((a, b) => a - b);

// Own hue test (plain RGB->HSV), D8: hue 285-315, sat >= 0.35, alpha >= 128.
function inWindow(r, g, b, a) {
  if (a < 128) return false;
  const mx = Math.max(r, g, b), mn = Math.min(r, g, b), c = mx - mn;
  if (!c) return false;
  let h = mx === r ? ((g - b) / c) % 6 : mx === g ? (b - r) / c + 2 : (r - g) / c + 4;
  h = (h * 60 + 360) % 360;
  return c / mx >= 0.35 && h >= 285 && h <= 315;
}

// Own CIEDE2000 (Sharma et al.), checked against two published pairs below.
function lab([R, G, B]) {
  const f = (v) => ((v /= 255) <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  const [r, g, b] = [R, G, B].map(f);
  const X = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
  const Y = 0.2126729 * r + 0.7151522 * g + 0.072175 * b;
  const Z = (0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883;
  const q = (t) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 * t + 16) / 116);
  return [116 * q(Y) - 16, 500 * (q(X) - q(Y)), 200 * (q(Y) - q(Z))];
}
function de00(l1, l2) {
  const rad = Math.PI / 180, deg = 180 / Math.PI;
  const [L1, a1, b1] = l1, [L2, a2, b2] = l2;
  const C1 = Math.hypot(a1, b1), C2 = Math.hypot(a2, b2), Cb = (C1 + C2) / 2;
  const G = 0.5 * (1 - Math.sqrt(Cb ** 7 / (Cb ** 7 + 25 ** 7)));
  const ap1 = (1 + G) * a1, ap2 = (1 + G) * a2;
  const Cp1 = Math.hypot(ap1, b1), Cp2 = Math.hypot(ap2, b2);
  const hp = (b, a) => (b === 0 && a === 0 ? 0 : (Math.atan2(b, a) * deg + 360) % 360);
  const h1 = hp(b1, ap1), h2 = hp(b2, ap2);
  const dL = L2 - L1, dC = Cp2 - Cp1;
  let dh = 0;
  if (Cp1 * Cp2) { dh = h2 - h1; if (dh > 180) dh -= 360; else if (dh < -180) dh += 360; }
  const dH = 2 * Math.sqrt(Cp1 * Cp2) * Math.sin((dh * rad) / 2);
  const Lb = (L1 + L2) / 2, Cpb = (Cp1 + Cp2) / 2;
  let hb = h1 + h2;
  if (Cp1 * Cp2) { hb = Math.abs(h1 - h2) <= 180 ? (h1 + h2) / 2 : h1 + h2 < 360 ? (h1 + h2 + 360) / 2 : (h1 + h2 - 360) / 2; }
  const T_ = 1 - 0.17 * Math.cos((hb - 30) * rad) + 0.24 * Math.cos(2 * hb * rad) + 0.32 * Math.cos((3 * hb + 6) * rad) - 0.2 * Math.cos((4 * hb - 63) * rad);
  const dTh = 30 * Math.exp(-(((hb - 275) / 25) ** 2));
  const Rc = 2 * Math.sqrt(Cpb ** 7 / (Cpb ** 7 + 25 ** 7));
  const Sl = 1 + (0.015 * (Lb - 50) ** 2) / Math.sqrt(20 + (Lb - 50) ** 2);
  const Sc = 1 + 0.045 * Cpb, Sh = 1 + 0.015 * Cpb * T_;
  const Rt = -Math.sin(2 * dTh * rad) * Rc;
  return Math.sqrt((dL / Sl) ** 2 + (dC / Sc) ** 2 + (dH / Sh) ** 2 + Rt * (dC / Sc) * (dH / Sh));
}

test('my CIEDE2000 reproduces two published reference pairs (so the gates below mean something)', () => {
  assert.ok(Math.abs(de00([50, 2.6772, -79.7751], [50, 0, -82.7485]) - 2.0425) < 1e-3);
  assert.ok(Math.abs(de00([50, 2.5, 0], [73, 25, -18]) - 27.1492) < 1e-3);
});

describe('the goose, checked from the committed sheet alone', () => {
  const palette = JSON.parse(fs.readFileSync(path.join(GOOSE, 'src', 'palette.json'), 'utf8')).colours;
  const role = new Map(palette.map((c) => [c.hex.toLowerCase(), c.role]));

  test('(c) only the three mask tones are in the hue window, in every tile; no opaque pixel is outside the palette', () => {
    for (let t = 0; t < S.tx * S.ty; t++) for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
      const [r, g, b, a] = px(t, x, y);
      if (a === 0) continue;
      assert.equal(a, 255, `tile ${t} (${x},${y}) has partial alpha`);
      const h = toHex(r, g, b);
      assert.ok(role.has(h), `tile ${t} (${x},${y}) ${h} isn't in palette.json`);
      assert.equal(inWindow(r, g, b, a), MASK3.includes(h), `tile ${t} (${x},${y}) ${h}`);
    }
  });

  test('(c) the helmet (dark, mid and light tones) shows in every mapped frame, and every frame has pixels', () => {
    assert.equal(anims.length, 17);
    for (const t of mapped) {
      const n = { '#730073': 0, '#b300b3': 0, '#ff7aff': 0 };
      let opaque = 0;
      for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
        const p = px(t, x, y);
        if (p[3]) opaque++;
        const h = toHex(...p.slice(0, 3));
        if (p[3] && h in n) n[h]++;
      }
      assert.ok(opaque > 100, `frame ${t}: ${opaque} opaque pixels`);
      assert.ok(n['#b300b3'] >= 10 && n['#ff7aff'] >= 3 && n['#730073'] + n['#b300b3'] + n['#ff7aff'] >= 30, `frame ${t}: ${JSON.stringify(n)}`);
    }
  });

  test('(c) the ground line holds: walk, graze and chew frames reach row 38 or 39 (a 1-px bob is an observation, see the report)', () => {
    for (const name of ['walk', 'graze', 'chew']) {
      for (const t of anims.find((a) => a.name === name).frames) {
        let feet = 0;
        for (let x = 0; x < T; x++) if (px(t, x, 39)[3] || px(t, x, 38)[3]) feet++;
        assert.ok(feet >= 1, `${name} frame ${t}: nothing on row 38 or 39`);
      }
    }
  });

  test('(c) the bill never touches the helmet: no bill pixel within 2 pixels (Chebyshev) of a mask pixel, in any frame', () => {
    const bill = palette.filter((c) => c.role === 'bill').map((c) => c.hex.toLowerCase());
    assert.equal(bill.length, 1);
    let seen = 0;
    for (let t = 0; t < S.tx * S.ty; t++) {
      const bills = [], mask = [];
      for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
        const p = px(t, x, y); if (!p[3]) continue;
        const h = toHex(...p.slice(0, 3));
        if (h === bill[0]) bills.push([x, y]); else if (MASK3.includes(h)) mask.push([x, y]);
      }
      seen += bills.length;
      for (const [bx, by] of bills) for (const [mx, my] of mask) {
        assert.ok(Math.max(Math.abs(bx - mx), Math.abs(by - my)) >= 2, `tile ${t}: bill (${bx},${by}) next to helmet (${mx},${my})`);
      }
    }
    assert.ok(seen > 50, 'the bill colour was actually found');
  });

  test('(d) all three contrast rules, 8 palette colours, from the tinted tones, incl. black head/neck against #202020', () => {
    // Which palette colours touch the mask (4-neighbour), in any tile.
    const touches = new Set();
    const used = new Set();
    for (let t = 0; t < S.tx * S.ty; t++) for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
      const p = px(t, x, y); if (!p[3]) continue;
      const h = toHex(...p.slice(0, 3)); used.add(h);
      if (MASK3.includes(h)) continue;
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
        const X = x + dx, Y = y + dy;
        if (X < 0 || Y < 0 || X >= T || Y >= T) continue;
        const q = px(t, X, Y);
        if (q[3] && MASK3.includes(toHex(...q.slice(0, 3)))) touches.add(h);
      }
    }
    assert.ok(touches.has('#34343c'), 'the black head/neck touches the helmet (so rule 1 matters)');
    const worst = { 1: 99, 2: 99, 3: 99 };
    for (const sess of PALETTE8) {
      const buf = new Uint8ClampedArray(12);
      MASK3.forEach((m, i) => buf.set([...hexRgb(m), 255], i * 4));
      tintHorns(buf, sess);
      const tones = [0, 1, 2].map((i) => lab([buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]]));
      assert.deepEqual([...buf.slice(4, 7)], hexRgb(sess), 'the mid tone lands on the session colour exactly');
      for (const h of used) {
        if (MASK3.includes(h)) continue;
        const L = lab(hexRgb(h));
        const dark = role.get(h) === 'outline' || L[0] <= 35;
        const dMid = de00(L, tones[1]), dAny = Math.max(...tones.map((t) => de00(L, t)));
        if (touches.has(h)) {
          if (dark) { worst[1] = Math.min(worst[1], dAny); assert.ok(dAny >= 20, `rule 1: ${h} ${role.get(h)} vs ${sess}: ${dAny.toFixed(1)}`); }
          else { worst[2] = Math.min(worst[2], dMid); assert.ok(dMid >= 20, `rule 2: ${h} ${role.get(h)} vs ${sess}: ${dMid.toFixed(1)}`); }
        } else { worst[3] = Math.min(worst[3], dMid); assert.ok(dMid >= 10, `rule 3: ${h} ${role.get(h)} vs ${sess}: ${dMid.toFixed(1)}`); }
      }
    }
    console.log('M10b independent (d) worst margins by rule:', JSON.stringify(worst));
  });

  test('no stray pixels: every frame is one connected figure (8-neighbour) apart from the named extras (stars, z)', () => {
    const odd = [];
    for (const t of mapped) {
      const seen = new Set(); const comps = [];
      for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
        if (!px(t, x, y)[3] || seen.has(y * T + x)) continue;
        const stack = [[x, y]]; seen.add(y * T + x); let n = 0, star = 0;
        while (stack.length) {
          const [cx, cy] = stack.pop(); n++;
          if (toHex(...px(t, cx, cy).slice(0, 3)) === '#ffd75e') star++;
          for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
            const X = cx + dx, Y = cy + dy, k = Y * T + X;
            if (X < 0 || Y < 0 || X >= T || Y >= T || seen.has(k) || !px(t, X, Y)[3]) continue;
            seen.add(k); stack.push([X, Y]);
          }
        }
        comps.push({ n, star });
      }
      const loose = comps.filter((c) => c.n < 20 && c.star === 0);
      if (loose.length) odd.push(`frame ${t}: ${JSON.stringify(loose)}`);
    }
    assert.deepEqual(odd, [], 'small loose components (tiny stray pixels) not stars');
  });

  test('(e) the pin: sha256 and bytes match the file; the lock lists it; the dist config bundles it', () => {
    const bytes = fs.readFileSync(path.join(GOOSE, 'animations.xml'));
    const lock = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
    assert.equal(lock.bundled.brah.sha256, createHash('sha256').update(bytes).digest('hex'));
    assert.equal(lock.bundled.brah.bytes, bytes.length);
    assert.equal(Object.keys(lock.bundled.brah.animationMap).length, 16, '14 behaviour keys + carried and land');
    const names = new Set(anims.map((a) => a.name));
    for (const v of Object.values(lock.bundled.brah.animationMap)) for (const n of [].concat(v)) assert.ok(names.has(n), `map names animation ${n}`);
    const dist = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'tauri.dist.conf.json'), 'utf8'));
    assert.equal(dist.bundle.resources['../../pets/brah/animations.xml'], 'pets/brah/animations.xml');
  });

  test('(e) the sheet has real alpha, no <transparency> element, and no #FF00FF', () => {
    assert.ok(!/<transparency/.test(xml));
    for (let i = 0; i < S.png.data.length; i += 4) if (S.png.data[i + 3]) assert.ok(!(S.png.data[i] === 255 && S.png.data[i + 1] === 0 && S.png.data[i + 2] === 255));
  });

  const ESHEEP = path.join(AGENT_DIR, 'data', 'pets', 'esheep64', 'animations.xml');
  test('(h) tripwire: no goose frame is a copy of an eSheep frame (silhouette IoU < 0.95)', { skip: !fs.existsSync(ESHEEP) && 'data/pets/esheep64 not fetched' }, () => {
    const E = sheet(fs.readFileSync(ESHEEP, 'utf8'));
    const sil = (s, t) => { const o = new Uint8Array(T * T); for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) { const gx = (t % s.tx) * T + x, gy = Math.floor(t / s.tx) * T + y; o[y * T + x] = s.png.data[(gy * s.png.width + gx) * 4 + 3] >= 128 ? 1 : 0; } return o; };
    const eS = []; for (let t = 0; t < E.tx * E.ty; t++) eS.push(sil(E, t));
    let best = 0;
    for (const t of mapped) {
      const g = sil(S, t);
      for (const e of eS) { let i = 0, u = 0; for (let k = 0; k < T * T; k++) { i += g[k] & e[k]; u += g[k] | e[k]; } if (u) best = Math.max(best, i / u); }
    }
    console.log('M10b independent tripwire: max IoU', best.toFixed(3));
    assert.ok(best < 0.95, `max IoU ${best}`);
  });
});
