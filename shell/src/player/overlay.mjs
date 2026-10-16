/**
 * Overlays: small images a pet draws over some frames, never mirrored (plan D8, deviation
 * 125). Bellwether's sheep uses one for the sleeping "z", which would read backwards if the
 * strip mirrored it with a sheep facing right.
 *
 * A bundled pet declares them in its pin (pets.lock.json "bundled".<name>.overlays, built
 * by scripts/build-pet.mjs from pet.json); the images are extra tiles of its own sheet, so
 * the animations.xml stays plain desktopPet. An imported pet has none.
 *
 *   overlays: { "<frame index>": [{ tile, x, y, w, h }] }
 *
 * `tile` is the sheet tile holding the image (at its top left, w x h art pixels); x and y
 * place it in the frame's tile, as drawn facing left (the art's own direction).
 */

/** Keeps only well-formed entries: whatever the shell passes, nothing else is drawn. */
export function cleanOverlays(raw, maxTile) {
  const out = {};
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return out;
  for (const [frame, list] of Object.entries(raw)) {
    if (!/^\d+$/.test(frame) || !Array.isArray(list)) continue;
    const ok = list.filter((o) => o && [o.tile, o.x, o.y, o.w, o.h].every(Number.isInteger) && o.tile >= 0 && o.tile < maxTile && o.w > 0 && o.h > 0 && o.w <= 64 && o.h <= 64);
    if (ok.length) out[frame] = ok.map(({ tile, x, y, w, h }) => ({ tile, x, y, w, h }));
  }
  return out;
}

/**
 * Where each overlay of `frame` goes on screen, for a sprite drawn at (x, y), `width` px
 * wide from a tile `tileW` art pixels wide. Facing right (`flip`), the position mirrors
 * across the tile, so the image stays beside the same part of the pet, but the image
 * itself is never mirrored. Returns [{ tile, sx, sy, sw, sh, dx, dy, dw, dh }].
 */
export function overlayPlacements(overlays, frame, { x, y, width, tileW, flip }) {
  const list = overlays?.[frame];
  if (!list) return [];
  const s = width / tileW;
  return list.map((o) => ({
    tile: o.tile,
    sw: o.w,
    sh: o.h,
    dx: Math.round(x + (flip ? tileW - o.x - o.w : o.x) * s),
    dy: Math.round(y + o.y * s),
    dw: o.w * s,
    dh: o.h * s,
  }));
}
