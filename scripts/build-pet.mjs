#!/usr/bin/env node
/**
 * Builds Bellwether's own pets from their text sources (distribution plan D8, milestone
 * M10a). Everything under pets/ is CC0-1.0; this script is code, under the repository's
 * MIT licence.
 *
 *   node scripts/build-pet.mjs build <name>      pets/<name>/src + pet.json -> pets/<name>/animations.xml,
 *                                                and its pin under "bundled" in pets.lock.json
 *   node scripts/build-pet.mjs build --all       every bundled pet
 *   node scripts/build-pet.mjs --check           rebuild every bundled pet in memory; exit 1 if
 *                                                animations.xml or its pin differs from the committed one
 *   node scripts/build-pet.mjs review <name> [--out <dir>]
 *        the review sheets of acceptance (f): every animation's frames, and the alert sequence,
 *        at sheep scale (1.5) and lamb scale (0.9), 1x and 2x device pixels, on light and dark,
 *        one row per palette colour; plus overview.png. Default --out: data/pet-review/<name>/
 *        (gitignored). Drawn through the player's own parser (pet.mjs) and tinter (horns.mjs).
 *
 * The source form is described at the top of pets/SPEC.md. In short: src/palette.json (one
 * character per colour, with a role), src/parts/*.txt (rectangular grids), and
 * src/frames/<anim>_<nn>.txt (a 40x40 grid, or a composition of parts and overlay grids).
 *
 * The PNG is encoded here, not by node:zlib: a fixed-Huffman deflate with a simple, fixed
 * match search, so the bytes depend on nothing but the pixels (zlib builds differ between
 * machines in how they search for matches). No time, gamma or text chunks.
 */
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { isHorn, tintHorns } from '../shell/src/player/horns.mjs';
import { parsePetDocument, resolveAnimationMap } from '../shell/src/player/pet.mjs';
import { overlayPlacements } from '../shell/src/player/overlay.mjs';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const PETS_DIR = path.join(AGENT, 'pets');
export const LOCK_FILE = path.join(AGENT, 'pets.lock.json');

/** The bundled pets, in the order the setup window lists them (D8). Reserved as names. */
export const BUNDLED = ['bellwether', 'brah', 'bryan'];
/** The tile, the sheet's width in tiles, and the tinter's three mask tones (D8). */
export const TILE = 40;
export const SHEET_COLS = 16;
export const MASK = { 'mask-dark': '#730073', 'mask-mid': '#b300b3', 'mask-light': '#ff7aff' };
/**
 * bellwether's behaviours (crates/brain/src/pets.rs BEHAVIOURS), in that order: the 14 the
 * strip plays today, then `carried` and `land`, drawn now for the planned drag along the
 * strip (nothing plays them yet; a pet without them falls back to walk).
 */
export const BEHAVIOURS = ['walk', 'run', 'graze', 'sleep', 'alert', 'turn', 'trip', 'sit', 'dizzy', 'chew', 'arrive', 'leave', 'hop', 'lie', 'carried', 'land'];
/** The default palette (shell/src-tauri/src/lib.rs DEFAULT_PALETTE). */
export const DEFAULT_PALETTE = ['#FF8C00', '#1F5BFF', '#202020', '#E8202A', '#00C2D4', '#8B4513', '#12A84A', '#FF4FB0'];
/** Acceptance (d)'s minimums, CIEDE2000. */
export const DE = { darkEdge: 20, touching: 20, other: 10, darkL: 35 };

class PetError extends Error {}
const fail = (m) => {
  throw new PetError(m);
};

// ------------------------------------------------------------------ sources

/** The bundled pets that have sources here, in BUNDLED order. */
export function bundledPets(dir = PETS_DIR) {
  return BUNDLED.filter((n) => fs.existsSync(path.join(dir, n, 'pet.json')));
}

/** Reads palette.json: {colours: [{char, hex, role}]} -> {byChar, list}. */
export function readPalette(file) {
  const raw = readJson(file);
  const list = raw?.colours;
  if (!Array.isArray(list) || !list.length) fail(`${file}: "colours" must be a non-empty list`);
  const byChar = new Map();
  const hexes = new Set();
  for (const c of list) {
    const where = `${file}: ${JSON.stringify(c)}`;
    if (typeof c.char !== 'string' || c.char.length !== 1 || !/^[!-~]$/.test(c.char) || '.~#'.includes(c.char)) fail(`${where}: "char" must be one printable ASCII character other than . ~ #`);
    if (!/^#[0-9a-f]{6}$/i.test(c.hex ?? '')) fail(`${where}: "hex" must be #rrggbb`);
    if (typeof c.role !== 'string' || !/^[a-z][a-z0-9-]*$/.test(c.role)) fail(`${where}: "role" must be a lower-case name`);
    if (byChar.has(c.char)) fail(`${where}: the character ${c.char} is used twice`);
    const hex = c.hex.toLowerCase();
    if (hexes.has(hex)) fail(`${where}: the colour ${hex} is listed twice`);
    if (hex === '#ff00ff') fail(`${where}: #FF00FF is never used (the sheet has real alpha)`);
    const rgb = rgbOf(hex);
    if (c.role in MASK) {
      if (hex !== MASK[c.role]) fail(`${where}: the ${c.role} tone must be exactly ${MASK[c.role]}`);
    } else if (c.role.startsWith('mask')) fail(`${where}: the mask roles are ${Object.keys(MASK).join(', ')}`);
    else if (isHorn(...rgb, 255)) fail(`${where}: ${hex} is inside the tinter's hue window (285-315 degrees, saturation >= 0.35) but isn't a mask tone, so it would take the session colour`);
    hexes.add(hex);
    byChar.set(c.char, { char: c.char, hex, role: c.role, rgb, mask: c.role in MASK });
  }
  for (const role of Object.keys(MASK)) if (![...byChar.values()].some((c) => c.role === role)) fail(`${file}: no colour has the role ${role}`);
  return { byChar, list: [...byChar.values()] };
}

/** A JSON file, or a PetError that says which file and why. */
function readJson(file) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (e) {
    fail(`${file} can't be read (${e.code ?? e.message})`);
  }
  try {
    return JSON.parse(text);
  } catch (e) {
    fail(`${file} isn't valid JSON (${e.message})`);
  }
}

/** Lines of a source file without comments (# to the end of a line) and trailing space. */
function lines(text) {
  return text.split('\n').map((l) => l.replace(/#.*$/, '').replace(/\s+$/, ''));
}

/** A grid's rows: rectangular, of palette characters, '.' (clear) and, if allowed, '~' (erase). */
function checkRows(rows, palette, where, { erase }) {
  if (!rows.length) fail(`${where}: no rows`);
  const w = rows[0].length;
  rows.forEach((r, y) => {
    if (r.length !== w) fail(`${where}: row ${y + 1} is ${r.length} characters wide, not ${w}`);
    for (const ch of r) {
      if (ch === '.' || (erase && ch === '~')) continue;
      if (!palette.byChar.has(ch)) fail(`${where}: row ${y + 1} has ${JSON.stringify(ch)}, which isn't in palette.json`);
    }
  });
  return rows;
}

/** Reads every part: parts/<name>.txt -> rows. */
export function readParts(dir, palette) {
  const parts = new Map();
  if (!fs.existsSync(dir)) return parts;
  for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.txt')).sort()) {
    const rows = lines(fs.readFileSync(path.join(dir, f), 'utf8')).filter((l) => l !== '');
    parts.set(f.slice(0, -4), checkRows(rows, palette, `parts/${f}`, { erase: true }));
  }
  return parts;
}

function paint(grid, rows, x0, y0, flip, where) {
  rows.forEach((row, dy) => {
    for (let dx = 0; dx < row.length; dx++) {
      const ch = flip ? row[row.length - 1 - dx] : row[dx];
      if (ch === '.') continue;
      const x = x0 + dx;
      const y = y0 + dy;
      if (x < 0 || y < 0 || x >= TILE || y >= TILE) fail(`${where}: a pixel lands at (${x}, ${y}), outside the ${TILE}x${TILE} tile`);
      grid[y][x] = ch === '~' ? '.' : ch;
    }
  });
}

/**
 * One frame file -> a 40x40 grid of palette characters and '.'. Either the grid itself (40
 * rows of 40), or a composition, layered in order:
 *   part <name> <x> <y> [flip]    parts/<name>.txt with its top-left at (x, y); flip mirrors it
 *   grid <x> <y>                  an overlay grid, its rows following, closed by `end`
 *   shift <dx> <dy>               moves everything drawn so far (pixels moved off the tile are an error)
 * In parts and overlays '.' leaves what is below and '~' erases it.
 */
export function resolveFrame(text, parts, palette, where) {
  const ls = lines(text);
  const first = ls.find((l) => l !== '');
  if (first === undefined) fail(`${where}: empty`);
  if (!/^(part|grid|shift)\b/.test(first)) {
    const rows = ls.filter((l) => l !== '');
    if (rows.length !== TILE || rows.some((r) => r.length !== TILE)) fail(`${where}: a grid frame must be ${TILE} rows of ${TILE} characters (or start with part/grid/shift)`);
    return checkRows(rows, palette, where, { erase: false }).map((r) => [...r]);
  }
  let grid = Array.from({ length: TILE }, () => Array(TILE).fill('.'));
  for (let i = 0; i < ls.length; i++) {
    const l = ls[i].trim();
    if (!l) continue;
    const w = l.split(/\s+/);
    const at = `${where} line ${i + 1}`;
    const int = (s) => (/^-?\d+$/.test(s ?? '') ? Number(s) : fail(`${at}: ${JSON.stringify(s)} isn't an integer`));
    if (w[0] === 'part') {
      if (w.length < 4 || w.length > 5 || (w[4] && w[4] !== 'flip')) fail(`${at}: part <name> <x> <y> [flip]`);
      const rows = parts.get(w[1]) ?? fail(`${at}: no part ${w[1]} (parts/${w[1]}.txt)`);
      paint(grid, rows, int(w[2]), int(w[3]), w[4] === 'flip', `${at} (part ${w[1]})`);
    } else if (w[0] === 'grid') {
      if (w.length !== 3) fail(`${at}: grid <x> <y>, then rows, then end`);
      const rows = [];
      let j = i + 1;
      for (; j < ls.length && ls[j].trim() !== 'end'; j++) if (ls[j] !== '') rows.push(ls[j]);
      if (j >= ls.length) fail(`${at}: the grid has no end line`);
      paint(grid, checkRows(rows, palette, at, { erase: true }), int(w[1]), int(w[2]), false, at);
      i = j;
    } else if (w[0] === 'shift') {
      if (w.length !== 3) fail(`${at}: shift <dx> <dy>`);
      const [dx, dy] = [int(w[1]), int(w[2])];
      const next = Array.from({ length: TILE }, () => Array(TILE).fill('.'));
      grid.forEach((row, y) =>
        row.forEach((ch, x) => {
          if (ch === '.') return;
          const [nx, ny] = [x + dx, y + dy];
          if (nx < 0 || ny < 0 || nx >= TILE || ny >= TILE) fail(`${at}: shifting moves a pixel off the tile`);
          next[ny][nx] = ch;
        }),
      );
      grid = next;
    } else fail(`${at}: unknown line ${JSON.stringify(l)} (part, grid, shift)`);
  }
  return grid;
}

/** pet.json, checked. */
export function readPetJson(file) {
  if (!fs.existsSync(file)) fail(`${path.relative(AGENT, file)} doesn't exist: ${path.basename(path.dirname(file))} has no sources yet`);
  const pet = readJson(file);
  if (!pet || typeof pet !== 'object' || Array.isArray(pet)) fail(`${file}: not a JSON object`);
  if (typeof pet.title !== 'string' || !pet.title) fail(`${file}: "title" is required`);
  if (!pet.animations || typeof pet.animations !== 'object') fail(`${file}: "animations" is required`);
  for (const [name, a] of Object.entries(pet.animations)) {
    if (!/^[a-z][a-z ]*$/.test(name)) fail(`${file}: animation name ${JSON.stringify(name)}: lower-case letters and spaces`);
    if (!Array.isArray(a.frames) || !a.frames.length || !a.frames.every((f) => typeof f === 'string' && /^[a-z0-9_-]+$/.test(f))) fail(`${file}: ${name}.frames must list frame file names (without .txt)`);
    for (const n of Object.keys(a.next ?? {})) if (!pet.animations[n]) fail(`${file}: ${name}.next names ${JSON.stringify(n)}, which isn't an animation`);
  }
  for (const [stem, list] of Object.entries(pet.overlays ?? {})) {
    if (!Array.isArray(list) || !list.length) fail(`${file}: overlays.${stem} must be a non-empty list`);
    for (const o of list) if (typeof o?.part !== 'string' || !Number.isInteger(o.x) || !Number.isInteger(o.y)) fail(`${file}: overlays.${stem}: each entry is { "part": <name>, "x": <int>, "y": <int> }`);
  }
  const map = pet.behaviours ?? fail(`${file}: "behaviours" is required`);
  for (const b of BEHAVIOURS) if (!(b in map)) fail(`${file}: behaviours has no ${b} (all ${BEHAVIOURS.length} are required)`);
  for (const [b, names] of Object.entries(map)) {
    if (!BEHAVIOURS.includes(b)) fail(`${file}: ${b} isn't one of bellwether's behaviours`);
    for (const n of [names].flat()) if (!pet.animations[n]) fail(`${file}: behaviour ${b} names ${JSON.stringify(n)}, which isn't an animation`);
  }
  return pet;
}

/** Everything about one pet's sources, resolved: {name, pet, palette, frames: Map(stem -> grid), order: [stems]}. */
export function loadPet(name, dir = PETS_DIR) {
  const root = path.join(dir, name);
  const pet = readPetJson(path.join(root, 'pet.json'));
  const palette = readPalette(path.join(root, 'src', 'palette.json'));
  const parts = readParts(path.join(root, 'src', 'parts'), palette);
  const framesDir = path.join(root, 'src', 'frames');
  const order = [];
  for (const a of Object.values(pet.animations)) for (const f of a.frames) if (!order.includes(f)) order.push(f);
  const files = fs.existsSync(framesDir) ? fs.readdirSync(framesDir).filter((f) => f.endsWith('.txt')).map((f) => f.slice(0, -4)) : [];
  const unused = files.filter((f) => !order.includes(f));
  if (unused.length) fail(`pets/${name}/src/frames: ${unused.join(', ')} ${unused.length > 1 ? 'aren\'t' : 'isn\'t'} used by any animation in pet.json`);
  const frames = new Map();
  for (const stem of order) {
    const f = path.join(framesDir, `${stem}.txt`);
    if (!fs.existsSync(f)) fail(`pets/${name}/pet.json uses frame ${stem}, but src/frames/${stem}.txt doesn't exist`);
    frames.set(stem, resolveFrame(fs.readFileSync(f, 'utf8'), parts, palette, `frames/${stem}.txt`));
  }
  // Overlays (deviation 125): parts drawn over a frame by the player, never mirrored. Each
  // part becomes one extra tile after the frames; '~' in an overlay part is just clear.
  const overlays = new Map();
  const overlayParts = [];
  for (const [stem, list] of Object.entries(pet.overlays ?? {})) {
    if (!order.includes(stem)) fail(`pets/${name}/pet.json: overlays names frame ${stem}, which no animation uses`);
    for (const o of list) {
      const rows = parts.get(o.part) ?? fail(`pets/${name}/pet.json: overlays.${stem} names part ${o.part}, which doesn't exist`);
      if (o.x < 0 || o.y < 0 || o.x + rows[0].length > TILE || o.y + rows.length > TILE) fail(`pets/${name}/pet.json: overlays.${stem}: ${o.part} at (${o.x}, ${o.y}) leaves the ${TILE}x${TILE} tile`);
      if (!overlayParts.includes(o.part)) overlayParts.push(o.part);
    }
    overlays.set(stem, list.map(({ part, x, y }) => ({ part, x, y })));
  }
  const overlayGrids = new Map(overlayParts.map((n) => [n, parts.get(n).map((r) => [...r.replace(/~/g, '.')])]));
  return { name, pet, palette, frames, order, overlays, overlayParts, overlayGrids };
}

// ------------------------------------------------------------------ the sheet

/** RGBA pixels of the sheet: SHEET_COLS tiles wide, as many rows as the frames need. */
export function sheetPixels(src) {
  const tilesx = SHEET_COLS;
  const count = src.order.length + (src.overlayParts?.length ?? 0);
  const tilesy = Math.max(1, Math.ceil(count / tilesx));
  const width = tilesx * TILE;
  const height = tilesy * TILE;
  const data = new Uint8Array(width * height * 4);
  const grids = [...src.order.map((stem) => src.frames.get(stem)), ...(src.overlayParts ?? []).map((n) => src.overlayGrids.get(n))];
  grids.forEach((grid, i) => {
    const ox = (i % tilesx) * TILE;
    const oy = Math.floor(i / tilesx) * TILE;
    for (let y = 0; y < grid.length; y++)
      for (let x = 0; x < grid[y].length; x++) {
        const ch = grid[y][x];
        if (ch === '.') continue;
        const o = ((oy + y) * width + ox + x) * 4;
        data.set([...src.palette.byChar.get(ch).rgb, 255], o);
      }
  });
  return { width, height, tilesx, tilesy, data };
}

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bufs) {
  let c = 0xffffffff;
  for (const b of bufs) for (let i = 0; i < b.length; i++) c = CRC[(c ^ b[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

const LEN_BASE = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/**
 * A zlib stream of `data`: one fixed-Huffman deflate block. Matches are looked for only at
 * the given distances (the previous pixel, the pixel above), longest first, ties to the
 * earlier distance: deterministic by construction.
 */
export function zlibFixed(data, distances) {
  const out = [0x78, 0x01];
  let acc = 0;
  let n = 0;
  const put = (v, bits) => {
    acc |= v << n;
    n += bits;
    while (n >= 8) {
      out.push(acc & 0xff);
      acc >>>= 8;
      n -= 8;
    }
  };
  const huff = (code, len) => {
    let r = 0;
    for (let i = 0; i < len; i++) r = (r << 1) | ((code >> i) & 1);
    put(r, len);
  };
  const sym = (s) => {
    if (s < 144) huff(0x30 + s, 8);
    else if (s < 256) huff(0x190 + s - 144, 9);
    else if (s < 280) huff(s - 256, 7);
    else huff(0xc0 + s - 280, 8);
  };
  put(1, 1);
  put(1, 2);
  for (let i = 0; i < data.length; ) {
    let best = 0;
    let dist = 0;
    for (const d of distances) {
      if (d > i || d > 32768) continue;
      let l = 0;
      while (l < 258 && i + l < data.length && data[i + l] === data[i + l - d]) l++;
      if (l > best) [best, dist] = [l, d];
    }
    if (best >= 3) {
      let c = LEN_BASE.length - 1;
      while (LEN_BASE[c] > best) c--;
      sym(257 + c);
      if (LEN_EXTRA[c]) put(best - LEN_BASE[c], LEN_EXTRA[c]);
      let dc = DIST_BASE.length - 1;
      while (DIST_BASE[dc] > dist) dc--;
      huff(dc, 5);
      if (DIST_EXTRA[dc]) put(dist - DIST_BASE[dc], DIST_EXTRA[dc]);
      i += best;
    } else {
      sym(data[i]);
      i++;
    }
  }
  sym(256);
  if (n > 0) put(0, 8 - n);
  let a = 1;
  let b = 0;
  for (let i = 0; i < data.length; i++) {
    a = (a + data[i]) % 65521;
    b = (b + a) % 65521;
  }
  out.push((b >>> 8) & 0xff, b & 0xff, (a >>> 8) & 0xff, a & 0xff);
  return Buffer.from(out);
}

/** An 8-bit RGBA PNG, filter 0 on every row, IHDR + one IDAT + IEND. */
export function encodePng({ width, height, data }) {
  const stride = width * 4;
  const raw = new Uint8Array((stride + 1) * height);
  for (let y = 0; y < height; y++) raw.set(data.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
  const chunk = (type, body) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(body.length);
    const t = Buffer.from(type, 'latin1');
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32([t, body]));
    return Buffer.concat([len, t, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([8, 6, 0, 0, 0], 8);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr), chunk('IDAT', zlibFixed(raw, [4, stride + 1])), chunk('IEND', Buffer.alloc(0))]);
}

// ------------------------------------------------------------------ animations.xml

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const pair = (v, d) => (Array.isArray(v) ? v : [v ?? d, v ?? d]);

/** The desktopPet-format file, as text. Animation ids are 1.. in pet.json order. */
export function petXml(src, png) {
  const { pet, order } = src;
  const sheet = sheetPixels(src);
  const names = Object.keys(pet.animations);
  const id = (n) => names.indexOf(n) + 1;
  const b64 = png.toString('base64').replace(/.{1,100}/g, '$&\n      ').trimEnd();
  const out = [];
  out.push('<?xml version="1.0" encoding="utf-8"?>');
  out.push(`<!-- ${esc(pet.title)}: built by scripts/build-pet.mjs from pets/${src.name}/src and pets/${src.name}/pet.json. CC0-1.0 (pets/LICENSE). Edit the sources, not this file. -->`);
  out.push('<animations>');
  out.push('  <header>');
  out.push('    <author>Bellwether</author>');
  out.push(`    <title>${esc(pet.title)}</title>`);
  out.push(`    <petname>${esc(src.name)}</petname>`);
  out.push('    <version>1</version>');
  out.push('    <info>CC0-1.0. Made for Bellwether from a written specification (pets/SPEC.md).</info>');
  out.push('    <application>1</application>');
  out.push('  </header>');
  out.push('  <image>');
  out.push(`    <tilesx>${sheet.tilesx}</tilesx>`);
  out.push(`    <tilesy>${sheet.tilesy}</tilesy>`);
  out.push(`    <png>\n      ${b64}\n    </png>`);
  out.push('  </image>');
  out.push('  <spawns>');
  out.push(`    <spawn id="1" probability="100"><x>screenW/2</x><y>areaH-imageH</y><next probability="100">${id(names.includes('walk') ? 'walk' : names[0])}</next></spawn>`);
  out.push('  </spawns>');
  out.push('  <animations>');
  for (const name of names) {
    const a = pet.animations[name];
    const [x0, x1] = pair(a.x, 0);
    const [y0, y1] = pair(a.y, 0);
    const [i0, i1] = pair(a.interval, 200);
    const [o0, o1] = pair(a.offsety, 0);
    const pos = (tag, x, y, i, o) => `      <${tag}><x>${x}</x><y>${y}</y><interval>${i}</interval>${o ? `<offsety>${o}</offsety>` : ''}</${tag}>`;
    out.push(`    <animation id="${id(name)}">`);
    out.push(`      <name>${esc(name)}</name>`);
    out.push(pos('start', x0, y0, i0, o0));
    out.push(pos('end', x1, y1, i1, o1));
    out.push(`      <sequence repeat="${esc(a.repeat ?? '0')}" repeatfrom="${a.repeatfrom ?? 0}">`);
    for (const f of a.frames) out.push(`        <frame>${order.indexOf(f)}</frame>`);
    for (const [n, p] of Object.entries(a.next ?? {})) out.push(`        <next probability="${p}" only="none">${id(n)}</next>`);
    out.push('      </sequence>');
    out.push('    </animation>');
  }
  out.push('  </animations>');
  out.push('</animations>');
  return out.join('\n') + '\n';
}

/** Builds one pet in memory: {xml, png, sheet, pin}. */
export function buildPet(name, dir = PETS_DIR) {
  const src = loadPet(name, dir);
  const sheet = sheetPixels(src);
  const png = encodePng(sheet);
  const xml = petXml(src, png);
  const bytes = Buffer.byteLength(xml);
  const sha256 = sha256Hex(xml);
  const pin = { sha256, bytes, animationMap: src.pet.behaviours };
  const overlays = pinOverlays(src);
  if (Object.keys(overlays).length) pin.overlays = overlays;
  return { src, sheet, png, xml, pin };
}

/** The pin's overlays: { "<frame index>": [{ tile, x, y, w, h }] } (shell/src/player/overlay.mjs). */
export function pinOverlays(src) {
  const out = {};
  for (const [stem, list] of src.overlays ?? []) {
    out[src.order.indexOf(stem)] = list.map((o) => {
      const g = src.overlayGrids.get(o.part);
      return { tile: src.order.length + src.overlayParts.indexOf(o.part), x: o.x, y: o.y, w: g[0].length, h: g.length };
    });
  }
  return out;
}

/** A frame as the player shows it: the frame's grid with its overlays painted on. */
export function composite(src, stem) {
  const grid = src.frames.get(stem).map((r) => [...r]);
  for (const o of src.overlays?.get(stem) ?? []) {
    src.overlayGrids.get(o.part).forEach((row, dy) => row.forEach((ch, dx) => {
      if (ch !== '.') grid[o.y + dy][o.x + dx] = ch;
    }));
  }
  return grid;
}

function sha256Hex(text) {
  return createHash('sha256').update(text).digest('hex');
}

/** pets.lock.json's text with the bundled pins set (key order kept; "bundled" goes last). */
export function lockWith(lockText, pins) {
  const lock = JSON.parse(lockText);
  lock.bundled = { ...(lock.bundled ?? {}), ...pins };
  const ordered = {};
  for (const n of BUNDLED) if (lock.bundled[n]) ordered[n] = lock.bundled[n];
  lock.bundled = ordered;
  return JSON.stringify(lock, null, 2) + '\n';
}

// ------------------------------------------------------------------ colour checks

export function rgbOf(hex) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  return m.slice(1).map((x) => Number.parseInt(x, 16));
}
const hexOf = (rgb) => `#${rgb.map((v) => v.toString(16).padStart(2, '0')).join('')}`;

/** sRGB (D65) -> CIELAB. */
export function lab([r, g, b]) {
  const lin = (c) => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const [R, G, B] = [lin(r), lin(g), lin(b)];
  const X = (0.4124564 * R + 0.3575761 * G + 0.1804375 * B) / 0.95047;
  const Y = 0.2126729 * R + 0.7151522 * G + 0.072175 * B;
  const Z = (0.0193339 * R + 0.119192 * G + 0.9503041 * B) / 1.08883;
  const f = (t) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 * t + 16) / 116);
  return [116 * f(Y) - 16, 500 * (f(X) - f(Y)), 200 * (f(Y) - f(Z))];
}

/** CIEDE2000 (Sharma, Wu and Dalal's formulation), kL = kC = kH = 1. */
export function deltaE2000(rgb1, rgb2) {
  const [L1, a1, b1] = lab(rgb1);
  const [L2, a2, b2] = lab(rgb2);
  const rad = Math.PI / 180;
  const C1 = Math.hypot(a1, b1);
  const C2 = Math.hypot(a2, b2);
  const Cb = (C1 + C2) / 2;
  const G = 0.5 * (1 - Math.sqrt(Cb ** 7 / (Cb ** 7 + 25 ** 7)));
  const a1p = (1 + G) * a1;
  const a2p = (1 + G) * a2;
  const C1p = Math.hypot(a1p, b1);
  const C2p = Math.hypot(a2p, b2);
  const hp = (b, a) => {
    if (b === 0 && a === 0) return 0;
    const h = Math.atan2(b, a) / rad;
    return h < 0 ? h + 360 : h;
  };
  const h1p = hp(b1, a1p);
  const h2p = hp(b2, a2p);
  const dLp = L2 - L1;
  const dCp = C2p - C1p;
  let dhp = 0;
  if (C1p * C2p !== 0) {
    dhp = h2p - h1p;
    if (dhp > 180) dhp -= 360;
    else if (dhp < -180) dhp += 360;
  }
  const dHp = 2 * Math.sqrt(C1p * C2p) * Math.sin((dhp / 2) * rad);
  const Lbp = (L1 + L2) / 2;
  const Cbp = (C1p + C2p) / 2;
  let hbp = h1p + h2p;
  if (C1p * C2p !== 0) {
    if (Math.abs(h1p - h2p) <= 180) hbp = (h1p + h2p) / 2;
    else hbp = h1p + h2p < 360 ? (h1p + h2p + 360) / 2 : (h1p + h2p - 360) / 2;
  }
  const T = 1 - 0.17 * Math.cos((hbp - 30) * rad) + 0.24 * Math.cos(2 * hbp * rad) + 0.32 * Math.cos((3 * hbp + 6) * rad) - 0.2 * Math.cos((4 * hbp - 63) * rad);
  const dTheta = 30 * Math.exp(-(((hbp - 275) / 25) ** 2));
  const RC = 2 * Math.sqrt(Cbp ** 7 / (Cbp ** 7 + 25 ** 7));
  const SL = 1 + (0.015 * (Lbp - 50) ** 2) / Math.sqrt(20 + (Lbp - 50) ** 2);
  const SC = 1 + 0.045 * Cbp;
  const SH = 1 + 0.015 * Cbp * T;
  const RT = -Math.sin(2 * dTheta * rad) * RC;
  return Math.sqrt((dLp / SL) ** 2 + (dCp / SC) ** 2 + (dHp / SH) ** 2 + RT * (dCp / SC) * (dHp / SH));
}

/** The three mask tones tinted to `hex` through the player's tintHorns: [dark, mid, light] RGB. */
export function tintedTones(hex) {
  const px = new Uint8ClampedArray([MASK['mask-dark'], MASK['mask-mid'], MASK['mask-light']].flatMap((m) => [...rgbOf(m), 255]));
  tintHorns(px, hex);
  return [0, 4, 8].map((i) => [px[i], px[i + 1], px[i + 2]]);
}

/**
 * Acceptance (d): every non-mask colour used in the sheet, its rule and its worst margin
 * over the palette. Rule 1: dark edge colours (role outline, or L* <= 35) touching the mask
 * (4-neighbour, any frame): >= 20 from at least one tinted tone. Rule 2: other colours
 * touching the mask: >= 20 from the tinted mid tone. Rule 3: the rest: >= 10 from it.
 */
export function contrastReport(src, palette = DEFAULT_PALETTE) {
  const used = new Set();
  const touching = new Set();
  for (const grid of [...src.frames.keys()].map((stem) => composite(src, stem)))
    for (let y = 0; y < TILE; y++)
      for (let x = 0; x < TILE; x++) {
        const ch = grid[y][x];
        if (ch === '.') continue;
        used.add(ch);
        const c = src.palette.byChar.get(ch);
        if (c.mask) continue;
        for (const [nx, ny] of [[x - 1, y], [x + 1, y], [x, y - 1], [x, y + 1]]) {
          const n = grid[ny]?.[nx];
          if (n && n !== '.' && src.palette.byChar.get(n).mask) touching.add(ch);
        }
      }
  const tones = palette.map((p) => ({ p, tones: tintedTones(p) }));
  const rows = [];
  for (const c of src.palette.list) {
    if (c.mask || !used.has(c.char)) continue;
    const L = lab(c.rgb)[0];
    const dark = c.role === 'outline' || L <= DE.darkL;
    const touches = touching.has(c.char);
    const rule = touches ? (dark ? 1 : 2) : 3;
    const need = rule === 1 ? DE.darkEdge : rule === 2 ? DE.touching : DE.other;
    let worst = { de: Infinity };
    for (const { p, tones: t } of tones) {
      const de = rule === 1 ? Math.max(...t.map((tone) => deltaE2000(c.rgb, tone))) : deltaE2000(c.rgb, t[1]);
      if (de < worst.de) worst = { de, against: p, mid: hexOf(t[1]) };
    }
    rows.push({ char: c.char, hex: c.hex, role: c.role, L, rule, need, worst: worst.de, against: worst.against, ok: worst.de >= need });
  }
  return rows;
}

// ------------------------------------------------------------------ review sheets

/** A small XML reader for our own animations.xml, with the DOM surface pet.mjs uses. */
export function miniDom(xml) {
  const body = xml.replace(/<\?[\s\S]*?\?>/g, '').replace(/<!--[\s\S]*?-->/g, '');
  const re = /<(\/?)([A-Za-z_][\w.:-]*)((?:\s+[\w.:-]+\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*(\/?)>|([^<]+)/g;
  const unesc = (s) => s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');
  const mk = (name, attrs) => {
    const el = {
      localName: name.replace(/^.*:/, ''),
      tagName: name,
      children: [],
      text: '',
      getAttribute: (k) => (k in attrs ? attrs[k] : null),
      get textContent() {
        return this.text + this.children.map((c) => c.textContent).join('');
      },
    };
    return el;
  };
  const stack = [mk('#document', {})];
  for (const m of body.matchAll(re)) {
    if (m[5] !== undefined) {
      stack.at(-1).text += unesc(m[5]);
      continue;
    }
    if (m[1]) {
      const el = stack.pop();
      if (el.tagName !== m[2]) throw new Error(`</${m[2]}> closes <${el.tagName}>`);
      continue;
    }
    const attrs = {};
    for (const a of m[3].matchAll(/([\w.:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) attrs[a[1]] = unesc(a[2] ?? a[3]);
    const el = mk(m[2], attrs);
    stack.at(-1).children.push(el);
    if (!m[4]) stack.push(el);
  }
  if (stack.length !== 1) throw new Error('unclosed elements');
  return { documentElement: stack[0].children[0] };
}

/** Decodes the PNG this script writes (8-bit RGBA, filter 0). */
export function decodeOwnPng(buf) {
  const width = buf.readUInt32BE(16);
  const height = buf.readUInt32BE(20);
  const idat = [];
  for (let p = 8; p < buf.length; ) {
    const len = buf.readUInt32BE(p);
    const type = buf.toString('latin1', p + 4, p + 8);
    if (type === 'IDAT') idat.push(buf.subarray(p + 8, p + 8 + len));
    p += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    if (raw[y * (stride + 1)] !== 0) throw new Error('unexpected PNG filter');
    data.set(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)), y * stride);
  }
  return { width, height, data };
}

/** An RGBA canvas for the review sheets. */
class Canvas {
  constructor(width, height, bg) {
    Object.assign(this, { width, height, data: new Uint8ClampedArray(width * height * 4) });
    this.fill(0, 0, width, height, bg);
  }
  fill(x0, y0, w, h, rgb) {
    for (let y = Math.max(0, y0); y < Math.min(this.height, y0 + h); y++)
      for (let x = Math.max(0, x0); x < Math.min(this.width, x0 + w); x++) this.data.set([...rgb, 255], (y * this.width + x) * 4);
  }
  /** Draws a tile of `sheet` scaled to w x h device pixels, nearest neighbour as the strip does. */
  tile(sheet, tilesx, frame, x0, y0, w, h, flip = false) {
    this.region(sheet, (frame % tilesx) * TILE, Math.floor(frame / tilesx) * TILE, TILE, TILE, x0, y0, w, h, flip);
  }
  /** Draws sw x sh art pixels at (sx, sy) of `sheet` into w x h device pixels, nearest neighbour. */
  region(sheet, sx, sy, sw, sh, x0, y0, w, h, flip = false) {
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++) {
        const tx = Math.floor(((flip ? w - 1 - x : x) + 0.5) * (sw / w));
        const ty = Math.floor((y + 0.5) * (sh / h));
        const s = ((sy + ty) * sheet.width + sx + tx) * 4;
        const a = sheet.data[s + 3];
        if (!a) continue;
        const X = x0 + x;
        const Y = y0 + y;
        if (X < 0 || Y < 0 || X >= this.width || Y >= this.height) continue;
        const d = (Y * this.width + X) * 4;
        for (let k = 0; k < 3; k++) this.data[d + k] = (sheet.data[s + k] * a + this.data[d + k] * (255 - a)) / 255;
      }
  }
  text(str, x, y, rgb, scale = 1) {
    let cx = x;
    for (const ch of String(str).toUpperCase()) {
      const g = FONT[ch] ?? FONT['?'];
      for (let r = 0; r < 5; r++) for (let c = 0; c < 3; c++) if (g[r] & (4 >> c)) this.fill(cx + c * scale, y + r * scale, scale, scale, rgb);
      cx += 4 * scale;
    }
    return cx;
  }
  blit(other, x0, y0) {
    for (let y = 0; y < other.height; y++) this.data.set(other.data.subarray(y * other.width * 4, (y + 1) * other.width * 4), ((y0 + y) * this.width + x0) * 4);
  }
}

/** A frame as the strip draws it: the tile (mirrored when `flip`), then its overlays, never mirrored. */
function drawFrame(c, view, i, f, x0, y0, w, h, flip = false) {
  c.tile(view.sheets[i], view.model.tilesx, f, x0, y0, w, h, flip);
  for (const o of overlayPlacements(view.overlays, f, { x: x0, y: y0, width: w, tileW: TILE, flip })) {
    c.region(view.sheets[i], (o.tile % view.model.tilesx) * TILE, Math.floor(o.tile / view.model.tilesx) * TILE, o.sw, o.sh, o.dx, o.dy, Math.round(o.dw), Math.round(o.dh));
  }
}

// A 3x5 pixel font for the sheets' labels: each glyph is five rows of three bits.
const FONT = Object.fromEntries(
  Object.entries({
    A: '25755', B: '67576', C: '34443', D: '65556', E: '74747', F: '74744', G: '34553', H: '55755', I: '72227', J: '11153', K: '55655', L: '44447', M: '57755', N: '65555', O: '25552', P: '65644', Q: '25563', R: '65655', S: '34216', T: '72222', U: '55557', V: '55522', W: '55775', X: '55255', Y: '55222', Z: '71247',
    0: '75557', 1: '26227', 2: '61247', 3: '61216', 4: '55711', 5: '74616', 6: '34757', 7: '71122', 8: '75757', 9: '75711',
    ' ': '00000', '.': '00002', ',': '00024', ':': '02020', '-': '00700', '+': '02720', '/': '11244', '(': '12221', ')': '42224', '#': '57575', '=': '07070', '?': '61202', x: '05250', '>': '42124', '*': '52725',
  }).map(([k, v]) => [k, [...v].map(Number)]),
);

const LIGHT = [0xf5, 0xf5, 0xf5];
const DARK = [0x1e, 0x1e, 0x1e];

/** The pet as the player sees it: parsed with pet.mjs, the sheet decoded, tinted once per palette colour with horns.mjs. */
export function playerView(xml, palette = DEFAULT_PALETTE, overlays = {}) {
  const model = parsePetDocument(miniDom(xml));
  const base = decodeOwnPng(Buffer.from(model.png, 'base64'));
  const sheets = palette.map((hex) => {
    const data = new Uint8ClampedArray(base.data);
    tintHorns(data, hex);
    return { width: base.width, height: base.height, data };
  });
  return { model, base, sheets, overlays };
}

/** The review rows: each animation in pet.json order, then the alert sequence. */
function reviewSequences(model, behaviours) {
  const seqs = [];
  for (const a of Object.values(model.animations).sort((p, q) => p.id - q.id)) seqs.push({ label: a.name, frames: a.frames.map((f) => ({ f })) });
  const ids = (b) => [behaviours[b]].flat().map((n) => model.byName[n]);
  const turn = model.animations[ids('turn')[0]].frames;
  const alert = model.animations[ids('alert')[0]].frames;
  const hold = turn.at(-1);
  const loop = model.animations[ids('sleep').at(-1)].frames;
  seqs.push({ label: 'sleep loop facing right: the strip mirrors the sheep, never the z', frames: loop.map((f) => ({ f, flip: true })) });
  seqs.push({ label: 'alert sequence: turn, hold (bounce), alert, turn', frames: [...turn.map((f) => ({ f })), ...[0, 2, 4, 2].map((lift) => ({ f: hold, lift })), ...alert.map((f) => ({ f })), ...turn.map((f) => ({ f }))] });
  return seqs;
}

/** One review sheet: every sequence, one row per palette colour, at one scale, density and background. */
export function reviewSheet(view, behaviours, { cssScale, dpr, bg, title, palette = DEFAULT_PALETTE }) {
  const w = Math.round(TILE * cssScale) * dpr;
  const gap = 4 * dpr;
  const seqs = reviewSequences(view.model, behaviours);
  const fg = bg === DARK ? [0xdd, 0xdd, 0xdd] : [0x33, 0x33, 0x33];
  const lab = 2;
  const left = 70;
  const maxFrames = Math.max(...seqs.map((s) => s.frames.length));
  const width = left + maxFrames * (w + gap) + 10;
  const rowH = w + gap;
  const height = 30 + seqs.reduce((h, s) => h + 16 + palette.length * rowH + 10, 0);
  const c = new Canvas(width, height, bg);
  c.text(title, 6, 6, fg, lab);
  let y = 30;
  for (const s of seqs) {
    c.text(s.label, 6, y, fg, lab);
    y += 16;
    palette.forEach((hex, i) => {
      c.fill(6, y + 2, 10, 10, rgbOf(hex));
      c.text(hex.slice(1), 20, y + 4, fg, 1);
      s.frames.forEach(({ f, lift = 0, flip = false }, k) => drawFrame(c, view, i, f, left + k * (w + gap), y - lift * dpr, w, w, flip));
      y += rowH;
    });
    y += 10;
  }
  return c;
}

/** overview.png: the frames up close (light and dark side by side), then the pet at strip sizes in every colour. */
export function overview(view, behaviours, name, palette = DEFAULT_PALETTE) {
  const seqs = reviewSequences(view.model, behaviours).slice(0, -1);
  const zoom = seqs.length > 6 ? 5 : 8;
  const Z = TILE * zoom;
  const zoomed = [];
  for (const [bg, ci] of [[LIGHT, 0], [DARK, 2]]) {
    const fg = bg === DARK ? [0xdd, 0xdd, 0xdd] : [0x33, 0x33, 0x33];
    const cols = Math.max(...seqs.map((s) => s.frames.length));
    const p = new Canvas(20 + cols * (Z + 8), 40 + seqs.length * (Z + 30), bg);
    p.text(`${name}: every frame at ${zoom}x, ${palette[ci]} on ${bg === DARK ? 'dark' : 'light'}`, 10, 8, fg, 4);
    seqs.forEach((s, r) => {
      const y = 40 + r * (Z + 30);
      p.text(s.label, 10, y + 4, fg, 3);
      s.frames.forEach(({ f, flip = false }, k) => {
        p.fill(10 + k * (Z + 8), y + 22, Z, Z, bg === DARK ? [0x2a, 0x2a, 0x2a] : [0xea, 0xea, 0xea]);
        drawFrame(p, view, ci, f, 10 + k * (Z + 8), y + 22, Z, Z, flip);
      });
    });
    zoomed.push(p);
  }
  const row = new Canvas(zoomed.reduce((w, p) => w + p.width, 0), Math.max(...zoomed.map((p) => p.height)), [0x80, 0x80, 0x80]);
  let x = 0;
  for (const p of zoomed) {
    row.blit(p, x, 0);
    x += p.width;
  }
  const panels = [row];
  // Strip sizes: one frame from the middle of every animation, sheep then lamb, in four
  // colours (all eight are in the per-size sheets), on light and dark.
  const pick = seqs.map((q) => q.frames[Math.floor(q.frames.length / 2)].f);
  const colours = [0, 1, 2, 6].filter((i) => i < palette.length);
  for (const dpr of [2, 1]) {
    const sheepW = 60 * dpr;
    const lambW = 36 * dpr;
    const rowH = sheepW + 10;
    const p = new Canvas(Math.max(row.width, 20 + pick.length * (sheepW + 4)), 40 + 2 * colours.length * 2 * rowH, LIGHT);
    p.text(`strip sizes at ${dpr}x device pixels: the middle frame of every animation (${seqs.map((q) => q.label).join(', ')}), sheep (1.5) then lamb (0.9), light and dark`, 10, 10, [0x33, 0x33, 0x33], 2);
    let y = 34;
    for (const bg of [LIGHT, DARK]) {
      p.fill(0, y, p.width, colours.length * 2 * rowH, bg);
      for (const i of colours)
        for (const w of [sheepW, lambW]) {
          pick.forEach((f, k) => drawFrame(p, view, i, f, 14 + k * (sheepW + 4), y + 6 + sheepW - w, w, w));
          y += rowH;
        }
    }
    panels.push(p);
  }
  const width = Math.max(...panels.map((p) => p.width));
  const out = new Canvas(width, panels.reduce((h, p) => h + p.height, 0), [0x80, 0x80, 0x80]);
  let y = 0;
  for (const p of panels) {
    out.blit(p, 0, y);
    y += p.height;
  }
  return out;
}

/**
 * The player's modules as one classic script: each module in its own function scope, its
 * imports taken from the modules before it. So review.html runs the strip's own parser,
 * tinter, stepper and Sprite, unchanged, with no server and no module loading.
 */
export function playerBundle() {
  const dir = path.join(AGENT, 'shell', 'src', 'player');
  const out = [];
  for (const m of ['expr', 'stepper', 'sprite', 'pet', 'horns', 'overlay']) {
    let src = fs.readFileSync(path.join(dir, `${m}.mjs`), 'utf8');
    const names = [...src.matchAll(/^export (?:async )?(?:function|const|class|let) (\w+)/gm)].map((x) => x[1]);
    src = src.replace(/^import \{([^}]+)\} from '\.\/(\w+)\.mjs';$/gm, (_, list, from) => `const {${list}} = M_${from};`).replace(/^export /gm, '');
    out.push(`const M_${m} = (() => {\n${src}\nreturn { ${names.join(', ')} };\n})();`);
  }
  return out.join('\n\n');
}

/**
 * review.html: the pet animated as the strip animates it, in one self-contained page.
 * A walk across a strip-like band (the player's Sprite moving it, turning at the ends),
 * every animation in place at its own intervals, and the alert (turn, hold and bounce,
 * the escalation, turn again); sheep (1.5) and lamb (0.9), light and dark, several colours.
 */
export function reviewHtml(name, built, palette = DEFAULT_PALETTE) {
  const data = { name, xml: built.xml, map: built.src.pet.behaviours, overlays: built.pin.overlays ?? {}, palette, title: built.src.pet.title };
  const app = String.raw`
const D = __DATA__;
const { assertSafePetXml, parsePetDocument, resolveAnimationMap } = M_pet;
const { tintHorns } = M_horns;
const { Sprite } = M_sprite;
const { totalSteps, frameAt, motionAt, pickNext, tileOf } = M_stepper;
const { cleanOverlays, overlayPlacements } = M_overlay;
const LIGHT = '#F5F5F5', DARK = '#1E1E1E';
const SHEEP = 1.5, LAMB = 0.9;
let speed = 1, oneX = false, t0 = performance.now(), virt = 0, last = performance.now();
const now = () => virt;
const warnings = [];
const model = parsePetDocument(new DOMParser().parseFromString(assertSafePetXml(D.xml), 'application/xml'));
const map = resolveAnimationMap(model, D.map, (w) => warnings.push(w));
const overlays = cleanOverlays(D.overlays, model.tilesx * model.tilesy);
let pet;
const scenes = [];

function dpr() { return oneX ? 1 : window.devicePixelRatio || 1; }
function canvas(parent, w, h, bg, label) {
  const box = document.createElement('div'); box.className = 'cell';
  const c = document.createElement('canvas'); c.style.width = w + 'px'; c.style.height = h + 'px'; c.style.background = bg;
  box.append(c);
  if (label) { const l = document.createElement('div'); l.className = 'label'; l.textContent = label; box.append(l); }
  parent.append(box);
  const fit = () => { const r = dpr(); c.width = Math.round(w * r); c.height = Math.round(h * r); };
  fit();
  return { c, w, h, fit, ctx: c.getContext('2d') };
}
function begin(k) { const r = k.c.width / k.w; k.ctx.setTransform(r, 0, 0, r, 0, 0); k.ctx.imageSmoothingEnabled = false; k.ctx.clearRect(0, 0, k.w, k.h); }
function draw(k, sheet, frame, x, y, w, h, flip) {
  const { col, row } = tileOf(model, frame);
  const g = k.ctx; g.save();
  if (flip) { g.translate(x + w, y); g.scale(-1, 1); g.drawImage(sheet, col * pet.tw, row * pet.th, pet.tw, pet.th, 0, 0, w, h); }
  else g.drawImage(sheet, col * pet.tw, row * pet.th, pet.tw, pet.th, x, y, w, h);
  g.restore();
  // Overlays as the strip draws them: placed for the facing, never mirrored.
  for (const o of overlayPlacements(overlays, frame, { x, y, width: w, tileW: pet.tw, flip })) {
    const t = tileOf(model, o.tile);
    g.drawImage(sheet, t.col * pet.tw, t.row * pet.th, o.sw, o.sh, o.dx, o.dy, o.dw, o.dh);
  }
}
function size(scale) { const w = Math.round(pet.tw * scale); return { w, h: Math.round(pet.th * scale) }; }

// 1. Walking across a strip-like band, turning at the ends: the strip's own Sprite.
function walkers(parent, bg, behaviour = 'walk') {
  const W = 940, H = 110;
  const k = canvas(parent, W, H, bg, (bg === LIGHT ? 'light' : 'dark') + ': four sheep and four lambs, each ' + (behaviour === 'run' ? 'running' : 'walking') + ' its own range');
  const list = D.palette.slice(0, 8).map((hex, i) => {
    const lamb = i >= 4, s = size(lamb ? SHEEP * 0.6 : SHEEP);
    const home = 30 + (i % 4) * 225 + (lamb ? 70 : 0);
    const sp = new Sprite({ model, map, width: s.w, height: s.h, stripWidth: W, stripHeight: H, home, wander: 90 });
    sp.setBehaviour(behaviour, now());
    return { sp, i };
  });
  scenes.push(() => { begin(k); for (const { sp, i } of list) { sp.tick(now()); const d = sp.drawInfo(now()); draw(k, pet.sheets[i], d.frame, d.x, d.y, sp.width, sp.height, d.flip); } });
  return k;
}

// 2. One animation in place at its own intervals (and its own next), restarting after a pause.
function inPlace(parent, animName, hex, scale, bg) {
  const s = size(scale), pad = 6;
  const k = canvas(parent, s.w + 2 * pad, s.h + pad, bg, null);
  const env = { screenW: 1000, screenH: 100, areaW: 1000, areaH: 100, imageW: s.w, imageH: s.h, imageX: 0, imageY: 0, random: 50, randS: 50 };
  const first = model.byName[animName];
  let st = null, restartAt = 0, since = 0;
  const start = (id) => { st = { id, step: 0, total: Math.max(1, totalSteps(model.animations[id], env)), due: now() }; };
  start(first); since = now();
  const sheet = pet.sheets[D.palette.indexOf(hex)];
  let frame = model.animations[first].frames[0];
  scenes.push(() => {
    const t = now();
    if (restartAt && t >= restartAt) { restartAt = 0; start(first); since = t; }
    if (!restartAt && t - since > 14000) { start(first); since = t; }
    while (!restartAt && t >= st.due) {
      const a = model.animations[st.id];
      const m = motionAt(a, st.step, st.total, env);
      frame = frameAt(a, st.step);
      st.due += m.interval; st.step++;
      if (st.step >= st.total) {
        const nx = pickNext(a, Math.random());
        if (nx != null && nx !== st.id) start(nx); else if (nx === st.id) start(nx); else restartAt = t + 900;
      }
    }
    begin(k); draw(k, sheet, frame, pad, pad, s.w, s.h, false);
  });
}

// Asleep, facing both ways: the strip mirrors the sheep facing right, never the z.
function sleepers(parent, bg) {
  const W = 940, H = 120;
  const k = canvas(parent, W, H, bg, (bg === LIGHT ? 'light' : 'dark') + ': facing left, then facing right, sheep and lambs');
  const list = [0, 1, 2, 6].flatMap((ci, j) => [[-1, false], [1, false], [-1, true], [1, true]].map(([facing, lamb], n) => {
    const s = size(lamb ? SHEEP * 0.6 : SHEEP);
    const sp = new Sprite({ model, map, width: s.w, height: s.h, stripWidth: W, stripHeight: H, home: 10 + j * 232 + [0, 62, 128, 170][n], wander: 0 });
    sp.setBehaviour('sleep', now());
    sp.facing = facing;
    return { sp, ci, facing };
  }));
  scenes.push(() => { begin(k); for (const { sp, ci, facing } of list) { sp.facing = facing; sp.tick(now()); const d = sp.drawInfo(now()); draw(k, pet.sheets[ci], d.frame, d.x, d.y, sp.width, sp.height, d.flip); } });
}

// 3. The alert: walking, then the turn and the held, bouncing front pose, the escalation, and back.
function alerts(parent, bg) {
  const W = 940, H = 120;
  const k = canvas(parent, W, H, bg, null);
  const list = [0, 1, 2, 6].flatMap((ci, j) => [false, true].map((lamb) => {
    const s = size(lamb ? SHEEP * 0.6 : SHEEP);
    const sp = new Sprite({ model, map, width: s.w, height: s.h, stripWidth: W, stripHeight: H, home: 20 + j * 235 + (lamb ? 90 : 0), wander: 30 });
    sp.setBehaviour('walk', now());
    return { sp, ci };
  }));
  const phase = document.getElementById('alert-phase');
  scenes.push(() => {
    const t = now() % 12000;
    const p = t < 2500 ? 'walk' : t < 6000 ? 'alert' : t < 10000 ? 'confirmed' : 'walk';
    phase.textContent = { walk: 'walking (busy)', alert: 'alert: turn, then hold the front pose and bounce', confirmed: 'confirmed: the alert animation once, then turn and a bigger bounce' }[p];
    begin(k);
    for (const { sp, ci } of list) {
      if (p === 'walk' && sp.behaviour !== 'walk') sp.setBehaviour('walk', now());
      if (p !== 'walk' && sp.behaviour !== 'alert') sp.setBehaviour('alert', now());
      sp.setConfirmed(p === 'confirmed', now());
      sp.tick(now());
      const d = sp.drawInfo(now());
      draw(k, pet.sheets[ci], d.frame, d.x, d.y, sp.width, sp.height, d.flip);
    }
  });
}

async function main() {
  document.getElementById('title').textContent = D.title + ' (' + D.name + '): animated review';
  const img = new Image(); img.src = 'data:image/png;base64,' + model.png; await img.decode();
  const base = document.createElement('canvas'); base.width = img.width; base.height = img.height;
  base.getContext('2d').drawImage(img, 0, 0);
  const sheets = D.palette.map((hex) => {
    const c = document.createElement('canvas'); c.width = base.width; c.height = base.height;
    const g = c.getContext('2d'); g.drawImage(base, 0, 0);
    const im = g.getImageData(0, 0, c.width, c.height); tintHorns(im.data, hex); g.putImageData(im, 0, 0);
    return c;
  });
  pet = { sheets, tw: img.width / model.tilesx, th: img.height / model.tilesy };
  document.getElementById('info').textContent = Object.keys(model.animations).length + ' animations, ' + pet.tw + 'x' + pet.th + ' tiles; behaviours: ' + Object.entries(map).map(([b, ids]) => b + '=' + ids.map((i) => model.animations[i].name).join('+')).join(' ') + (warnings.length ? '; WARNINGS: ' + warnings.join('; ') : '');
  const ks = [];
  const walk = document.getElementById('walk');
  ks.push(walkers(walk, LIGHT), walkers(walk, DARK), walkers(walk, LIGHT, 'run'), walkers(walk, DARK, 'run'));
  const grid = document.getElementById('grid');
  const colours = [D.palette[0], D.palette[1], D.palette[2], D.palette[6]];
  for (const a of Object.values(model.animations).sort((p, q) => p.id - q.id)) {
    const row = document.createElement('div'); row.className = 'row';
    const h = document.createElement('div'); h.className = 'rowlabel';
    h.textContent = a.name + ': ' + a.frames.length + ' frames, ' + a.start.interval + (a.end.interval !== a.start.interval ? '-' + a.end.interval : '') + ' ms' + (a.next.length ? ', then ' + a.next.map((n) => model.animations[n.id].name).join('/') : '');
    row.append(h);
    for (const bg of [LIGHT, DARK]) for (const scale of [SHEEP, SHEEP * 0.6]) for (const hex of colours) inPlace(row, a.name, hex, scale, bg);
    grid.append(row);
  }
  const zz = document.getElementById('asleep');
  sleepers(zz, LIGHT); sleepers(zz, DARK);
  const al = document.getElementById('alert');
  alerts(al, LIGHT); alerts(al, DARK);
  document.getElementById('speed').addEventListener('change', (e) => { speed = Number(e.target.value); });
  document.getElementById('onex').addEventListener('change', (e) => {
    oneX = e.target.checked;
    for (const c of document.querySelectorAll('canvas')) c.style.imageRendering = oneX ? 'pixelated' : 'auto';
    location.hash = oneX ? '1x' : '';
    location.reload();
  });
  // #t=<ms> fast-forwards the page by that much (in 16 ms steps) before it starts: a fixed
  // moment for checking the page without watching it.
  const skip = Number(/t=(\d+)/.exec(location.hash)?.[1] ?? 0);
  for (virt = 0; virt < skip; virt += 16) for (const s of scenes) s();
  const loop = (ts) => { virt += (ts - last) * speed; last = ts; for (const s of scenes) s(); requestAnimationFrame(loop); };
  requestAnimationFrame((ts) => { last = ts; loop(ts); });
  document.body.dataset.ready = '1';
}
if (/1x/.test(location.hash)) { oneX = true; document.getElementById('onex').checked = true; }
main().catch((e) => { document.getElementById('info').textContent = 'FAILED: ' + (e.stack || e); document.body.dataset.ready = 'error'; });
`;
  const css = `body{font:13px -apple-system,system-ui,sans-serif;margin:16px;background:#fafafa;color:#222}h1{font-size:18px;margin:0 0 4px}h2{font-size:15px;margin:22px 0 6px}
.cell{display:inline-block;vertical-align:bottom;margin:2px}.cell canvas{display:block}.label{font-size:11px;color:#666}.row{margin:4px 0}.rowlabel{font-weight:600;margin:6px 0 2px}
#controls{margin:8px 0;padding:6px 8px;background:#eee;display:inline-block}.note{color:#555;max-width:900px}${'' }`;
  const script = `${playerBundle()}\n\n${app.replace('__DATA__', JSON.stringify(data).replace(/</g, '\\u003c'))}`;
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>${esc(built.src.pet.title)}: animated review</title>
<style>${css}</style></head>
<body>
<h1 id="title"></h1>
<div class="note">Drawn by the strip's own code (shell/src/player: pet.mjs parses, horns.mjs tints, stepper.mjs times the frames, sprite.mjs moves the sheep), embedded in this page. Sheep at scale 1.5, lambs at 0.9 (lambScale 0.6), nearest-neighbour as on the strip, at this screen's device pixels. <span id="info"></span></div>
<div id="controls">Speed <select id="speed"><option value="1">real time</option><option value="0.5">1/2</option><option value="0.25">1/4</option></select>
 &nbsp; <label><input type="checkbox" id="onex"> show as a 1x display would (magnified)</label></div>
<h2>Walking and running across the strip</h2><div id="walk"></div>
<h2>Each animation in place, at its own intervals</h2>
<div class="note">Each row: light, then dark; sheep, then lambs; in ${esc(palette[0])}, ${esc(palette[1])}, ${esc(palette[2])} and ${esc(palette[6])}. An animation that ends restarts after a short pause; sleep runs on into sleep loop.</div>
<div id="grid"></div>
<h2>Asleep, facing both ways</h2><div class="note">The strip mirrors a sheep facing right; the "z" is drawn separately and never mirrored, beside the helmet on whichever side the head is.</div>
<div id="asleep"></div>
<h2>The alert</h2><div class="note">A 12-second cycle: walking; <b>alert</b> (turn, then hold the front-facing frame and bounce); <b>confirmed</b> (the alert animation once, then turn and bounce higher); walking again. Now: <span id="alert-phase"></span></div>
<div id="alert"></div>
<script>
${script}
</script>
</body></html>
`;
}

/** Writes the review sheets and overview.png to `out`; returns the files written. */
export function review(name, out, dir = PETS_DIR) {
  const built = buildPet(name, dir);
  const view = playerView(built.xml, DEFAULT_PALETTE, built.pin.overlays ?? {});
  const behaviours = built.src.pet.behaviours;
  fs.mkdirSync(out, { recursive: true });
  const files = [];
  for (const [kind, cssScale] of [['sheep', 1.5], ['lamb', 0.9]])
    for (const dpr of [1, 2])
      for (const [bgName, bg] of [['light', LIGHT], ['dark', DARK]]) {
        const c = reviewSheet(view, behaviours, { cssScale, dpr, bg, title: `${name} ${kind} (scale ${cssScale}) at ${dpr}x on ${bgName}` });
        const f = path.join(out, `${name}-${kind}-${dpr}x-${bgName}.png`);
        fs.writeFileSync(f, encodePng(c));
        files.push(f);
      }
  const f = path.join(out, `${name}-overview.png`);
  fs.writeFileSync(f, encodePng(overview(view, behaviours, name)));
  files.push(f);
  const html = path.join(out, 'review.html');
  fs.writeFileSync(html, reviewHtml(name, built));
  files.push(html);
  return { files, built, view };
}

// ------------------------------------------------------------------ the command

function main(argv) {
  const [cmd, ...rest] = argv;
  if (cmd === '--check') {
    if (!fs.existsSync(LOCK_FILE)) fail(`${LOCK_FILE} is missing: restore it from git`);
    const lockText = fs.readFileSync(LOCK_FILE, 'utf8');
    const lock = readJson(LOCK_FILE);
    const names = bundledPets();
    const problems = [];
    const pins = {};
    for (const n of names) {
      const b = buildPet(n);
      pins[n] = b.pin;
      const file = path.join(PETS_DIR, n, 'animations.xml');
      const have = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
      if (have !== b.xml) problems.push(`pets/${n}/animations.xml ${have === null ? 'is missing' : 'differs from what its sources build'}`);
    }
    for (const n of Object.keys(lock.bundled ?? {})) if (!names.includes(n)) problems.push(`pets.lock.json pins a bundled pet ${n} that has no sources in pets/`);
    if (lockWith(lockText, pins) !== lockText) problems.push('pets.lock.json\'s "bundled" pins differ from what the sources build');
    if (problems.length) {
      for (const p of problems) console.error(`build-pet --check: ${p}`);
      console.error('Run `node scripts/build-pet.mjs build --all` and commit the result.');
      return 1;
    }
    console.log(`build-pet --check: ${names.length} bundled pet(s) up to date (${names.join(', ') || 'none'})`);
    return 0;
  }
  if (cmd === 'build') {
    const names = rest[0] === '--all' ? bundledPets() : rest.slice(0, 1);
    if (!names.length) {
      console.error('usage: node scripts/build-pet.mjs build <name> | --all');
      return 2;
    }
    const pins = {};
    for (const n of names) {
      if (!BUNDLED.includes(n)) {
        console.error(`build-pet: ${n} isn't a bundled pet (${BUNDLED.join(', ')})`);
        return 2;
      }
      const b = buildPet(n);
      fs.writeFileSync(path.join(PETS_DIR, n, 'animations.xml'), b.xml);
      pins[n] = b.pin;
      const bad = contrastReport(b.src).filter((r) => !r.ok);
      console.log(`${n}: ${b.src.order.length} frames, ${Object.keys(b.src.pet.animations).length} animations, ${b.sheet.tilesx}x${b.sheet.tilesy} tiles, ${b.png.length} byte PNG, ${b.pin.bytes} byte animations.xml, sha256 ${b.pin.sha256}`);
      for (const r of bad) console.log(`  contrast: ${r.char} ${r.hex} (${r.role}) rule ${r.rule} needs ${r.need}, is ${r.worst.toFixed(1)} against ${r.against}`);
    }
    if (!fs.existsSync(LOCK_FILE)) fail(`${LOCK_FILE} is missing: restore it from git`);
    readJson(LOCK_FILE);
    fs.writeFileSync(LOCK_FILE, lockWith(fs.readFileSync(LOCK_FILE, 'utf8'), pins));
    return 0;
  }
  if (cmd === 'review') {
    const name = rest[0];
    if (!name || !BUNDLED.includes(name)) {
      console.error(`usage: node scripts/build-pet.mjs review <${BUNDLED.join('|')}> [--out <dir>]`);
      return 2;
    }
    const i = rest.indexOf('--out');
    const out = i >= 0 ? path.resolve(rest[i + 1]) : path.join(AGENT, 'data', 'pet-review', name);
    const { files, built } = review(name, out);
    for (const f of files) console.log(f);
    for (const r of contrastReport(built.src)) console.log(`contrast ${r.ok ? 'ok  ' : 'FAIL'} ${r.char} ${r.hex} ${r.role.padEnd(12)} rule ${r.rule} (>= ${r.need}): worst ${r.worst.toFixed(1)} against ${r.against}`);
    return 0;
  }
  console.error('usage: node scripts/build-pet.mjs build <name>|--all | --check | review <name> [--out <dir>]');
  return 2;
}

// realpaths: run through a symlinked path (macOS /tmp and /var are links), this is still the main module.
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (e) {
    // A reason, never a stack trace (BUILD_PET_DEBUG=1 shows the stack of an unexpected error).
    if (!(e instanceof PetError) && process.env.BUILD_PET_DEBUG) throw e;
    console.error(`build-pet: ${e instanceof PetError ? e.message : `unexpected error: ${e?.message ?? e}`}`);
    process.exitCode = 1;
  }
}
