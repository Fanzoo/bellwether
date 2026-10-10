/**
 * Session colour on the horns only (Milestone 10): the wool, face and
 * legs stay as drawn, and the horns take the session's colour.
 *
 * The horns are found by colour, not by coordinates, so every frame of every animation
 * works (turned, front-facing, sleeping, lying). esheep64 draws them in three magenta
 * shades (#730073, #b300b3, #ff7aff, all at hue 300°). Nothing else on its sheet has a
 * hue in that window: checked over every pixel by test/horns.test.mjs.
 *
 * Recolouring keeps the shading: the mid tone becomes the session colour exactly, the
 * dark tone is that colour darkened by the same ratio as the original, and the
 * highlight is lifted towards white by its own whiteness, so the dark, mid and
 * highlight tones stay dark, mid and highlight.
 */

/** Hue window (degrees) and the least saturation a horn pixel has. */
export const HORN_HUE = [285, 315];
export const HORN_MIN_SAT = 0.35;
/** The brightness of the horns' mid tone (#b300b3), which maps to the session colour exactly. */
const HORN_MID = 0xb3 / 255;

function hsv(r, g, b) {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const c = max - min;
  let h = 0;
  if (c) {
    if (max === r) h = ((g - b) / c) % 6;
    else if (max === g) h = (b - r) / c + 2;
    else h = (r - g) / c + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  return [h, max ? c / max : 0, max / 255];
}

/** Whether one RGBA pixel is horn: magenta-hued, saturated, opaque, and not the transparency key. */
export function isHorn(r, g, b, a) {
  if (a < 128) return false;
  if (r > 240 && g < 16 && b > 240) return false; // the magenta key colour (made transparent)
  const [h, s] = hsv(r, g, b);
  return s >= HORN_MIN_SAT && h >= HORN_HUE[0] && h <= HORN_HUE[1];
}

function rgbOf(hex) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex ?? '');
  return m ? m.slice(1).map((x) => Number.parseInt(x, 16)) : null;
}

/** Recolours the horn pixels of an RGBA buffer in place; returns how many it changed. */
export function tintHorns(data, hex) {
  const t = rgbOf(hex);
  if (!t) return 0;
  let n = 0;
  for (let i = 0; i < data.length; i += 4) {
    const [r, g, b, a] = [data[i], data[i + 1], data[i + 2], data[i + 3]];
    if (!isHorn(r, g, b, a)) continue;
    const [, s, v] = hsv(r, g, b);
    const white = 1 - s; // 0 for the dark and mid shades, about 0.5 for the highlight
    // The horn's mid tone (brightness 0.70) becomes exactly the session colour; the dark
    // tone is darker by the same ratio, and the highlight is lifted towards white.
    const shade = Math.min(1, v / HORN_MID);
    for (let k = 0; k < 3; k++) {
      const shaded = t[k] * shade;
      data[i + k] = Math.round(shaded + (255 - shaded) * white);
    }
    n++;
  }
  return n;
}

/** The whole-sheep tint (`pets.tint: "whole"`): multiply every pixel by the colour. */
export function tintWhole(data, hex) {
  const t = rgbOf(hex);
  if (!t) return 0;
  for (let i = 0; i < data.length; i += 4) {
    data[i] = (data[i] * t[0]) / 255;
    data[i + 1] = (data[i + 1] * t[1]) / 255;
    data[i + 2] = (data[i + 2] * t[2]) / 255;
  }
  return data.length / 4;
}
