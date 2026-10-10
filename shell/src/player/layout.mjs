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

