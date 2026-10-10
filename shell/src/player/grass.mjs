/**
 * A tuft of grass under a grazing sheep's mouth (Milestone 10). Drawn by
 * code in the pet's own pixel style (no art file): a 9×6 pattern of blades in three
 * greens, scaled by whole sprite pixels like the sheep, and pre-rendered once per size.
 *
 * Where: on the ground, under the frontmost reach of the grazing animation's frames
 * (the head is lowered there), mirrored with the sheep's facing.
 * When: it fades in while the sheep grazes or chews, stays, and fades out a moment
 * after it moves on, where it was (grass doesn't walk off with the sheep).
 */

// . blank, d dark, m mid, l light. Row 0 is the top.
export const TUFT = [
  '..l...m..',
  '.m..l.m..',
  '.m.ml.l.m',
  'dm.mlmmdm',
  'mddmdmmdm',
  'ddddddddd',
];
export const GREENS = { d: [46, 125, 50], m: [67, 160, 71], l: [124, 179, 66] };
export const FADE_IN_MS = 300;
export const FADE_OUT_MS = 500;
export const LINGER_MS = 400;

/** RGBA pixels of the tuft at `px` screen pixels per art pixel (mirrored if `flip`). */
export function tuftPixels(px, flip = false) {
  const w = TUFT[0].length * px;
  const h = TUFT.length * px;
  const data = new Uint8ClampedArray(w * h * 4);
  TUFT.forEach((row, y) => {
    [...row].forEach((c, x0) => {
      const rgb = GREENS[c];
      if (!rgb) return;
      const x = flip ? row.length - 1 - x0 : x0;
      for (let dy = 0; dy < px; dy++) for (let dx = 0; dx < px; dx++) data.set([...rgb, 255], ((y * px + dy) * w + x * px + dx) * 4);
    });
  });
  return { w, h, data };
}

/**
 * The frontmost opaque column (in tile pixels, from the side the art faces) over a set
 * of frames: where a grazing head reaches. `pixelAt(frame, x, y)` returns alpha.
 */
export function frontmost(frames, tileW, tileH, alphaAt) {
  let best = tileW;
  for (const f of frames) {
    for (let x = 0; x < best; x++) {
      let opaque = false;
      for (let y = 0; y < tileH && !opaque; y++) opaque = alphaAt(f, x, y) >= 128;
      if (opaque) {
        best = x;
        break;
      }
    }
  }
  return best === tileW ? Math.floor(tileW / 4) : best;
}

/**
 * Where the tuft goes for a sprite at `x` (left edge, screen px), `width` wide, of a
 * pet whose art faces left: centred a pixel or two inside the head's reach.
 */
export function tuftX({ x, width, tileW, front, flip, tuftW }) {
  const s = width / tileW;
  const mouth = flip ? x + width - front * s : x + front * s;
  const inset = 2 * s;
  return Math.round(flip ? mouth - inset - tuftW / 2 : mouth + inset - tuftW / 2);
}

/** Next alpha: towards 1 while eating, towards 0 a moment after it stopped. */
export function fadeStep(state, { eating, now }) {
  const s = state ?? { alpha: 0, lastEating: -Infinity, at: now };
  const dt = Math.max(0, now - s.at);
  if (eating) s.lastEating = now;
  const target = eating || now - s.lastEating < LINGER_MS ? 1 : 0;
  s.alpha = target === 1 ? Math.min(1, s.alpha + dt / FADE_IN_MS) : Math.max(0, s.alpha - dt / FADE_OUT_MS);
  s.at = now;
  return s;
}
