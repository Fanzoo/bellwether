/**
 * Where alert bubbles go on the strip. No DOM: text widths come from a `measure`
 * function (the canvas's measureText in the webview, a fixed width per character in
 * tests).
 *
 * Every bubble sits above its sheep, inside the strip, with a tail pointing at the
 * sheep. Near an edge it shifts inwards (the tail still points at the sheep).
 *
 * Two steps:
 *   1. Rows. In priority order, each bubble takes the first row where its whole
 *      question fits centred over its sheep, else the first where it fits whole beside
 *      the others (shifted sideways, every tail still reaching its sheep), else
 *      whichever row leaves it more room.
 *   2. Widths, per row, so only bubbles that share a row ever narrow each other.
 *      Every question starts whole (up to maxWidth, which fits the brain's longest);
 *      if the row doesn't fit, the widest bubbles give way first, down to minWidth.
 *      Then the project labels take whatever room is left, shortened with "…" if
 *      need be. If a row can't fit even at minWidth, its least important bubble is
 *      left out (its sheep shows a "!" badge) and the row is tried again.
 *
 * Priority: the 6 s-confirmed alerts first, then other alerts, then the "a lamb needs
 * you" indicators; within a priority, left to right.
 */

export const BUBBLE = {
  maxWidth: 440, // fits bubble.maxChars (60) of 12 px semibold text (about 7 px a character) plus padding
  minWidth: 70,
  padX: 8,
  lineHeight: 15,
  padY: 4,
  gap: 12, // between the sheep's top (at rest) and the bubble's bottom: room for the bounce
  rowGap: 4,
  margin: 2,
  tail: 7,
  subGap: 8, // between the question and the project label on the same line
  subMinChars: 2, // a label shorter than this (plus "…") is left out rather than shown
};

/** Shortens `text` with an ellipsis until `measure(text) <= width`. */
export function ellipsise(text, width, measure) {
  if (measure(text) <= width) return text;
  const chars = [...text];
  let lo = 0;
  let hi = chars.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measure(chars.slice(0, mid).join('') + '…') <= width) lo = mid;
    else hi = mid - 1;
  }
  return lo === 0 ? '…' : chars.slice(0, lo).join('') + '…';
}

/**
 * One-line bubbles: the question in bold, then the project label in grey.
 *
 * @param {Array<{id, cx, top, text, sub?, priority}>} items
 *   cx: the sheep's centre x; top: its top y at rest; priority: higher goes first
 * @param {{stripWidth, measure: (text, bold) => number, measureSub?: (text) => number}} o
 * @returns {{placed: Array<{id, x, y, w, h, row, tailX, text, sub, subX}>, dropped: string[]}}
 */
export function layoutBubbles(items, { stripWidth, measure, measureSub = measure }) {
  const B = BUBBLE;
  const h = B.padY * 2 + B.lineHeight;
  const lo = B.margin;
  const hi = stripWidth - B.margin;
  const cap = Math.max(1, Math.min(B.maxWidth, hi - lo));
  const rowY = (item, row) => item.top - B.gap - B.tail - h - row * (h + B.rowGap);
  // need: the whole question, no label; want: the question and the whole label.
  const need = new Map(items.map((i) => [i.id, fitText({ ...i, sub: undefined }, cap, measure, measureSub).w]));
  const want = new Map(items.map((i) => [i.id, fitText(i, cap, measure, measureSub).w]));
  const ordered = [...items].sort((a, b) => b.priority - a.priority || a.cx - b.cx);
  const byCx = (a, b) => a.cx - b.cx || (a.id < b.id ? -1 : 1);

  // 1. Rows: the first row where the whole question fits centred over its sheep; else
  // the first where it fits whole beside the others (shifted, tails still reaching
  // their sheep); else whichever row's nearest sheep is farthest away. Bubbles never
  // narrow across rows.
  const rowOf = new Map();
  const needW = (i) => need.get(i.id);
  for (const item of ordered) {
    const rows = [0, 1].filter((r) => rowY(item, r) >= 0);
    if (!rows.length) {
      rowOf.set(item.id, null);
      continue;
    }
    const others = (r) => ordered.filter((o) => rowOf.get(o.id) === r);
    const centred = (i) => {
      const x = clamp(i.cx - needW(i) / 2, lo, hi - needW(i));
      return [x - B.rowGap, x + needW(i) + B.rowGap];
    };
    const [a, b] = centred(item);
    const free =
      rows.find((r) => others(r).every((o) => centred(o)[1] <= a + B.rowGap || centred(o)[0] >= b - B.rowGap)) ??
      rows.find((r) => pack([...others(r), item].sort(byCx), needW, lo, hi));
    if (free !== undefined) {
      rowOf.set(item.id, free);
      continue;
    }
    const room = (r) => Math.min(Infinity, ...others(r).map((o) => Math.abs(o.cx - item.cx)));
    rowOf.set(item.id, rows.reduce((best, r) => (room(r) > room(best) ? r : best)));
  }

  // 2. Widths, row by row.
  const dropped = ordered.filter((i) => rowOf.get(i.id) === null).map((i) => i.id);
  const placed = [];
  for (const r of [0, 1]) {
    let row = ordered.filter((i) => rowOf.get(i.id) === r);
    let fit;
    // Too tight even with every bubble at minWidth: the least important gives way.
    while (row.length && !(fit = fitRow([...row].sort(byCx), need, want, lo, hi))) {
      dropped.push(row[row.length - 1].id);
      row = row.slice(0, -1);
    }
    if (!row.length) continue;
    const sorted = [...row].sort(byCx);
    const fitted = sorted.map((item, k) => fitText(item, fit.w[k], measure, measureSub));
    const xs = pack(sorted, (_, k) => fitted[k].w, lo, hi) ?? fit.x;
    sorted.forEach((item, k) => {
      const f = fitted[k];
      const x = xs[k];
      placed.push({
        id: item.id,
        x: Math.round(x * 100) / 100,
        y: Math.round(rowY(item, r)),
        w: f.w,
        h,
        row: r,
        tailX: clamp(item.cx, x + Math.min(TAIL_REACH, f.w / 2), x + f.w - Math.min(TAIL_REACH, f.w / 2)),
        text: f.text,
        sub: f.sub,
        subX: x + B.padX + f.textW + B.subGap,
      });
    });
  }
  const rank = new Map(ordered.map((i, k) => [i.id, k]));
  placed.sort((a, b) => rank.get(a.id) - rank.get(b.id));
  return { placed, dropped };
}

/**
 * Widths and positions for one row (sorted by cx), or null if it can't fit. Starts
 * with every question whole; while the row doesn't pack, the widest bubble gives up
 * a tenth (down to minWidth). Then, in the order given, each bubble whose question is
 * whole grows towards room for its label, as far as the row still packs.
 */
function fitRow(row, need, want, lo, hi) {
  const w = row.map((i) => need.get(i.id));
  const floor = row.map((i) => Math.min(BUBBLE.minWidth, need.get(i.id)));
  const at = (_, k) => w[k];
  let x;
  while (!(x = pack(row, at, lo, hi))) {
    let k = -1;
    for (let j = 0; j < w.length; j++) if (w[j] > floor[j] + 0.01 && (k < 0 || w[j] > w[k])) k = j;
    if (k < 0) return null;
    w[k] = Math.max(floor[k], w[k] - Math.max(4, w[k] / 10));
  }
  row.forEach((item, k) => {
    if (w[k] < need.get(item.id)) return;
    let a = w[k];
    let b = want.get(item.id);
    w[k] = b;
    if (pack(row, at, lo, hi)) return;
    while (b - a > 1) {
      w[k] = (a + b) / 2;
      if (pack(row, at, lo, hi)) a = w[k];
      else b = w[k];
    }
    w[k] = a;
  });
  return { w, x: pack(row, at, lo, hi) };
}

/**
 * Positions for one row's bubbles at the given widths, or null if they don't fit:
 * centred where possible, pushed apart left to right and back right to left, each
 * kept within the strip and with its sheep at least TAIL_REACH inside its edges.
 */
const TAIL_REACH = 10;
function pack(row, width, lo, hi) {
  if (!row.length) return [];
  const gap = BUBBLE.rowGap;
  const w = row.map((i, k) => width(i, k));
  // (A sheep right at the strip's edge gets as close as the strip allows.)
  const minX = row.map((i, k) => clamp(i.cx + Math.min(TAIL_REACH, w[k] / 2) - w[k], lo, hi - w[k]));
  const maxX = row.map((i, k) => clamp(i.cx - Math.min(TAIL_REACH, w[k] / 2), lo, hi - w[k]));
  const x = row.map((i, k) => clamp(i.cx - w[k] / 2, minX[k], maxX[k]));
  for (let k = 1; k < x.length; k++) x[k] = Math.max(x[k], x[k - 1] + w[k - 1] + gap);
  for (let k = x.length - 1; k >= 0; k--) {
    if (k < x.length - 1) x[k] = Math.min(x[k], x[k + 1] - w[k] - gap);
    x[k] = Math.min(x[k], maxX[k]);
  }
  const ok = x.every((v, k) => v >= minX[k] - 0.01 && v <= maxX[k] + 0.01 && (k === 0 || v >= x[k - 1] + w[k - 1] + gap - 0.01));
  return ok ? x : null;
}

/**
 * The bubble's text and label within `maxW`. The question always wins: if it fits
 * whole, the label gets what is left (whole, or shortened with "…", or left out if
 * that would be under two characters); if the question doesn't fit whole, it takes
 * all the room and the label is left out.
 */
function fitText(item, maxW, measure, measureSub) {
  const B = BUBBLE;
  const inner = Math.max(0, maxW - 2 * B.padX);
  const fullText = measure(item.text, true);
  let text = item.text;
  let sub;
  if (fullText <= inner) {
    if (item.sub) {
      const room = inner - fullText - B.subGap;
      const s = ellipsise(item.sub, room, measureSub);
      const shownChars = [...s.replace(/…$/, '')].length;
      if (room > 0 && measureSub(s) <= room && (s === item.sub || shownChars >= B.subMinChars)) sub = s;
    }
  } else {
    text = ellipsise(item.text, inner, (t) => measure(t, true));
  }
  const textW = measure(text, true);
  const subW = sub ? measureSub(sub) + B.subGap : 0;
  const w = Math.min(maxW, Math.max(Math.min(B.minWidth, maxW), Math.ceil(textW + subW) + 2 * B.padX));
  return { text, sub, textW, w };
}

function clamp(v, lo, hi) {
  return Math.min(Math.max(v, lo), Math.max(lo, hi));
}
