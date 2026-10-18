/**
 * Home slots along the strip. Slot n has a fixed x, so a sheep returns to the same
 * spot whenever its session does (the brain persists the slot). Slots that don't fit
 * the strip's width wrap round rather than leaving the screen.
 */
export function homeX(slot, { stripWidth, slotWidth, spriteWidth, margin = 24 }) {
  const usable = Math.max(slotWidth, stripWidth - 2 * margin);
  const capacity = Math.max(1, Math.floor(usable / slotWidth));
  const index = ((Number.isInteger(slot) ? slot : 0) % capacity + capacity) % capacity;
  const x = margin + index * slotWidth + (slotWidth - spriteWidth) / 2;
  return Math.min(Math.max(0, x), Math.max(0, stripWidth - spriteWidth));
}

/** The strip's frame for one monitor's work area (logical px): full width, at the bottom. */
export function stripRect(workArea, heightPx) {
  const height = Math.min(heightPx, workArea.height);
  return { x: workArea.x, y: workArea.y + workArea.height - height, width: workArea.width, height };
}

/**
 * Where each sheep's family (the sheep and its lambs) lives on the strip, so families
 * never interleave (Milestone 6, D2).
 *
 * Slots keep their order and stay where they are when nothing changes: each slot up to
 * the highest one in use gets a band, `slotWidth` wide when empty and at least the
 * family's width when a sheep with lambs is in it. When the strip has room to spare,
 * every family also gets a gap (half on each side), up to the widest family's width,
 * so a mostly empty strip spreads its sheep out. If the bands don't fit, they are scaled down.
 *
 * A sheep may wander only as far as its family stays inside its band (half of each
 * gap belongs to each neighbour), and never more than `wanderMax`.
 *
 * @param {Array<{id, slot, extent: {left, right}}>} sheep
 *   extent: how far its family reaches left and right of the sheep's centre
 * @returns {Map<id, {centre, wander}>}
 */
export function familyHomes(sheep, { stripWidth, slotWidth, wanderMax, margin = 24, dock = null }) {
  // A Dock extent that isn't a usable range (NaN, null, reversed, empty) counts as none.
  const dockUsable = dock && Number.isFinite(dock.lo) && Number.isFinite(dock.hi) && Math.min(dock.hi, stripWidth) - Math.max(dock.lo, 0) > 0;
  if (dockUsable && sheep.length) {
    const clamped = { lo: Math.max(0, dock.lo), hi: Math.min(stripWidth, dock.hi) };
    const homes = onTheDock(sheep, { stripWidth, slotWidth, wanderMax, margin, dock: clamped });
    // On the Dock and beside it must hold every family apart and inside the strip; if
    // they can't, the strip uses the full-width layout instead (verification D2).
    if (disjoint(sheep, homes, stripWidth)) return homes;
  }
  const out = new Map();
  if (!sheep.length) return out;
  const bySlot = new Map();
  for (const s of sheep) bySlot.set(Number.isInteger(s.slot) && s.slot >= 0 ? s.slot : 0, s);
  const top = Math.max(...bySlot.keys());
  const usable = Math.max(1, stripWidth - 2 * margin);
  const width = (s) => Math.max(slotWidth, s.extent.left + s.extent.right + 8);
  const widths = [];
  for (let k = 0; k <= top; k++) widths.push(bySlot.has(k) ? width(bySlot.get(k)) : slotWidth);
  const total = widths.reduce((a, b) => a + b, 0);
  const widest = Math.max(...sheep.map(width));
  const gap = total < usable ? Math.min(widest, (usable - total) / sheep.length) : 0;
  const scale = total > usable ? usable / total : 1;
  let x = margin;
  for (let k = 0; k <= top; k++) {
    const w = widths[k] * scale;
    const s = bySlot.get(k);
    if (s) {
      // The band: half a gap, its own width, half a gap.
      const lo = x;
      const hi = x + w + gap;
      const min = Math.min(lo + s.extent.left, hi - s.extent.right);
      const max = Math.max(lo + s.extent.left, hi - s.extent.right);
      const centre = Math.min(Math.max((min + max) / 2, s.extent.left), stripWidth - s.extent.right);
      out.set(s.id, { centre, wander: Math.max(0, Math.min(wanderMax, (max - min) / 2)) });
      x += w + gap;
    } else {
      x += w;
    }
  }
  return out;
}

/**
 * Dock-width confinement (Milestone 10): with the Dock's extent known (Accessibility),
 * the sheep stand on its top edge. Families take bands inside [dock.lo, dock.hi] in slot
 * order, spread out when there is room, so their sheep turn at the Dock's ends. A family
 * that doesn't fit stands beside the Dock at the same height, alternately to its right
 * and left, so sheep are on thin air only when the Dock is too short for them all.
 */
function onTheDock(sheep, { stripWidth, slotWidth, wanderMax, margin, dock }) {
  const width = (s) => Math.max(slotWidth, s.extent.left + s.extent.right + 8);
  const ordered = [...sheep].sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
  const span = Math.max(0, dock.hi - dock.lo);
  const inside = [];
  let used = 0;
  for (const s of ordered) {
    if (used + width(s) > span) break;
    inside.push(s);
    used += width(s);
  }
  const outside = ordered.slice(inside.length);
  const out = band(inside, dock.lo, dock.hi, { width, wanderMax, stripWidth });
  // Beside the Dock: right, then left, while there is room; past that, the strip's ends.
  let right = dock.hi;
  let left = dock.lo;
  const rightEnd = stripWidth - margin;
  for (const [i, s] of outside.entries()) {
    const w = width(s);
    const toRight = (i % 2 === 0 && right + w <= rightEnd) || left - w < margin;
    if (toRight) {
      const at = Math.min(right, rightEnd - w);
      band([s], at, at + w, { width, wanderMax, stripWidth }).forEach((v, k) => out.set(k, v));
      right += w;
    } else {
      const at = Math.max(left - w, margin);
      band([s], at, at + w, { width, wanderMax, stripWidth }).forEach((v, k) => out.set(k, v));
      left -= w;
    }
  }
  return out;
}

/** Families side by side in [lo, hi], spread with equal gaps (half on each side), each wandering only within its band. */
function band(families, lo, hi, { width, wanderMax, stripWidth }) {
  const out = new Map();
  if (!families.length) return out;
  const total = families.reduce((a, s) => a + width(s), 0);
  const span = Math.max(1, hi - lo);
  const gap = total < span ? (span - total) / families.length : 0;
  const scale = total > span ? span / total : 1;
  let x = lo;
  for (const s of families) {
    const w = width(s) * scale;
    const b0 = x;
    const b1 = x + w + gap;
    const min = Math.min(b0 + s.extent.left, b1 - s.extent.right);
    const max = Math.max(b0 + s.extent.left, b1 - s.extent.right);
    const centre = Math.min(Math.max((min + max) / 2, s.extent.left), stripWidth - s.extent.right);
    out.set(s.id, { centre, wander: Math.max(0, Math.min(wanderMax, (max - min) / 2)) });
    x = b1;
  }
  return out;
}

/** Whether every family (at its furthest wander) is inside the strip and clear of the others. */
function disjoint(sheep, homes, stripWidth) {
  const boxes = sheep.map((s) => {
    const h = homes.get(s.id);
    return h && Number.isFinite(h.centre) && Number.isFinite(h.wander) ? [h.centre - h.wander - s.extent.left, h.centre + h.wander + s.extent.right] : null;
  });
  if (boxes.some((b) => !b || b[0] < -0.5 || b[1] > stripWidth + 0.5)) return false;
  boxes.sort((a, b) => a[0] - b[0]);
  return boxes.every((b, i) => i === 0 || boxes[i - 1][1] <= b[0] + 0.5);
}


/**
 * Where a dropped sheep's family lives (D10, milestone 12). Slotted sheep never move for
 * a dropped one: the dropped sheep is the one that gives way.
 *
 * drop    {centre, extent: {left, right}, dir}: where it was let go (its centre), how far
 *         its family reaches either side of that, and which way it was being carried
 *         (-1 or 1: a tie goes that way)
 * others  [{lo, hi}]: every other family's reach (home ± wander, plus its extent)
 *
 * - The drop centre is kept on the strip, with its whole family.
 * - Clear of every reach there: it lives at the drop centre, wandering `wanderMax`, cut to
 *   the room on the nearer side.
 * - Inside another reach: it lives at the nearest centre where its family fits clear (it
 *   lands where it was dropped, then walks there), with the room there.
 * - No room anywhere: it stays where it was dropped, with no wander (`noRoom`).
 *
 * @returns {{centre, wander, noRoom: boolean, moved: boolean}}
 */
export function dropHome(drop, others, { stripWidth, wanderMax }) {
  const { left, right } = drop.extent;
  const lo = left;
  const hi = Math.max(lo, stripWidth - right);
  const c = Math.min(Math.max(drop.centre, lo), hi);
  // The centres that would put the family inside another reach (touching is allowed).
  const blocked = others
    .filter((r) => Number.isFinite(r?.lo) && Number.isFinite(r?.hi))
    .map((r) => [r.lo - right, r.hi + left])
    .sort((a, b) => a[0] - b[0]);
  const free = [];
  let from = lo;
  for (const [a, b] of blocked) {
    if (a > from + 0.5) free.push([from, Math.min(a, hi)]);
    from = Math.max(from, b);
    if (from >= hi) break;
  }
  if (from <= hi) free.push([from, hi]);
  const room = free.filter(([a, b]) => b >= a);
  const wanderAt = (x, [a, b]) => Math.max(0, Math.min(wanderMax, x - a, b - x));
  const inside = room.find(([a, b]) => c >= a - 0.5 && c <= b + 0.5);
  if (inside) return { centre: c, wander: wanderAt(c, inside), noRoom: false, moved: false };
  if (!room.length) return { centre: c, wander: 0, noRoom: true, moved: false };
  // The nearest clear centre; on a tie, the way it was being carried.
  let best = null;
  for (const span of room) {
    const x = c < span[0] ? span[0] : span[1];
    const d = Math.abs(x - c);
    const better = !best || d < best.d - 0.5 || (Math.abs(d - best.d) <= 0.5 && Math.sign(x - c) === (drop.dir < 0 ? -1 : 1));
    if (better) best = { x, d, span };
  }
  return { centre: best.x, wander: wanderAt(best.x, best.span), noRoom: false, moved: true };
}

/**
 * Every family's home with drops (D10): `familyHomes` for the slotted families, then each
 * dropped family, in drop order (`seq`), by `dropHome` against the reaches of all the
 * others placed so far, so a later drop gives way to an earlier one.
 *
 * A family dropped on the strip where placement would put it anyway (`drop.ghost`) still
 * holds its slot's band, empty, so picking it up and dropping it moves nobody else. One
 * dropped onto another monitor (a guest) takes no band there.
 *
 * @param {Array<{id, slot, extent, drop?: {centre, seq, ghost, dir}}>} families
 * @returns {Map<id, {centre, wander, noRoom?: boolean, moved?: boolean}>}
 */
export function homesWithDrops(families, opts) {
  const slotted = families.filter((f) => !f.drop || f.drop.ghost);
  const homes = familyHomes(slotted, opts);
  const reach = (h, e) => ({ lo: h.centre - h.wander - e.left, hi: h.centre + h.wander + e.right });
  const reaches = families.filter((f) => !f.drop && homes.has(f.id)).map((f) => reach(homes.get(f.id), f.extent));
  const dropped = families.filter((f) => f.drop).sort((a, b) => (a.drop.seq ?? 0) - (b.drop.seq ?? 0));
  for (const f of dropped) {
    const h = dropHome({ centre: f.drop.centre, extent: f.extent, dir: f.drop.dir }, reaches, { stripWidth: opts.stripWidth, wanderMax: opts.wanderMax });
    homes.set(f.id, h);
    reaches.push(reach(h, f.extent));
  }
  return homes;
}
