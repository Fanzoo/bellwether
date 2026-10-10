/**
 * Where a sheep's lambs stand: in a row on either side of their parent, the first to
 * its right, the next to its left, then further out, alternating. Each lamb keeps the
 * place it got when it arrived (the brain reorders lambs when one alerts; they must
 * not swap places on screen), and a place frees up once its lamb has gone.
 *
 * Places are a lamb's width plus 4 px apart, clear of the parent, so five lambs span
 * about two and a half slots.
 */

/** The centre of place `k`, relative to the parent's centre. */
export function lambOffset(k, { parentW, lambW }) {
  const side = k % 2 === 0 ? 1 : -1;
  const out = Math.floor(k / 2);
  return side * (parentW / 2 + lambW / 2 + 2 + out * (lambW + 4));
}

/** The lowest place not in `taken`. */
export function freePlace(taken) {
  let k = 0;
  while (taken.has(k)) k++;
  return k;
}

/**
 * Left edge x for a lamb in place `k` beside a parent whose left edge is `parentX`.
 * Near an edge of the strip, places that would fall off it are skipped, so the lambs
 * line up on the other side instead of piling up against the edge. If even that runs
 * out (a narrow strip, or a large behaviour.maxLambsShown), further lambs stand in a
 * row stepped out from the parent's centre, in front of it if need be, but never on
 * top of another lamb while the strip has room for them all.
 */
export function lambX(k, { parentX, parentW, lambW, stripWidth }) {
  const centre = parentX + parentW / 2;
  const inStrip = (x) => x >= 0 && x <= stripWidth - lambW;
  const xs = [];
  for (let j = 0; xs.length <= k && j < 2 * k + 64; j++) {
    const x = centre + lambOffset(j, { parentW, lambW }) - lambW / 2;
    if (inStrip(x)) xs.push(x);
  }
  const step = lambW + 4;
  const clear = (x) => xs.every((y) => Math.abs(x - y) >= lambW);
  for (let m = 0; xs.length <= k && m < 2 * (stripWidth / step + 2); m++) {
    const off = m === 0 ? 0 : (m % 2 ? 1 : -1) * Math.ceil(m / 2);
    for (const base of [centre - lambW / 2, centre - lambW / 2 + step / 2]) {
      const x = Math.min(Math.max(0, base + off * step), Math.max(0, stripWidth - lambW));
      if (xs.length <= k && inStrip(x) && clear(x)) xs.push(x);
    }
  }
  const x = xs[k] ?? centre + lambOffset(k, { parentW, lambW }) - lambW / 2;
  return Math.min(Math.max(0, x), Math.max(0, stripWidth - lambW));
}

/** How far a family reaches left and right of the parent's centre with lambs in places 0..places-1. */
export function familyExtent(places, { parentW, lambW }) {
  let left = parentW / 2;
  let right = parentW / 2;
  for (let k = 0; k < places; k++) {
    const o = lambOffset(k, { parentW, lambW });
    left = Math.max(left, -(o - lambW / 2));
    right = Math.max(right, o + lambW / 2);
  }
  return { left, right };
}
