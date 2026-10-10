// Frozen copy of shell/src/player/layout.mjs from before the Dock work: the oracle for
// test/tester-grass-dock.test.mjs (without a Dock, the layout must equal this). Do not edit.
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
export function familyHomes(sheep, { stripWidth, slotWidth, wanderMax, margin = 24 }) {
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
