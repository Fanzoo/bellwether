import { evaluate } from './expr.mjs';

/**
 * Frame stepping for one animation of a PetModel.
 *
 * An animation plays its frames once, then repeats the tail from `repeatfrom`
 * `repeat` more times (`repeat` is an expression, evaluated when the animation
 * starts). Movement (x, y) and the frame interval are interpolated linearly from
 * `start` to `end` across the whole run, which is how the format describes
 * accelerating falls and slowing walks.
 */

export function totalSteps(anim, env, warn) {
  const n = anim.frames.length;
  if (n === 0) return 0;
  const repeat = Math.max(0, Math.min(10_000, Math.round(evaluate(anim.repeat, env, warn))));
  return n + (n - anim.repeatfrom) * repeat;
}

export function frameAt(anim, step) {
  const n = anim.frames.length;
  if (step < n) return anim.frames[step];
  const loop = n - anim.repeatfrom;
  return anim.frames[anim.repeatfrom + ((step - n) % loop)];
}

/** Movement and timing at `step` of `total`: {dx, dy, interval, offsety, opacity}. */
export function motionAt(anim, step, total, env, warn) {
  const t = total <= 1 ? 0 : step / (total - 1);
  const lerp = (a, b) => a + (b - a) * t;
  const num = (e) => evaluate(e, env, warn);
  return {
    dx: lerp(num(anim.start.x), num(anim.end.x)),
    dy: lerp(num(anim.start.y), num(anim.end.y)),
    interval: Math.max(20, Math.min(5000, lerp(num(anim.start.interval), num(anim.end.interval)))),
    offsety: lerp(anim.start.offsety ?? 0, anim.end.offsety ?? 0),
    opacity: Math.max(0, Math.min(1, lerp(anim.start.opacity ?? 1, anim.end.opacity ?? 1))),
  };
}

/**
 * Chooses the next animation from a `next` list by probability. pasture's sheep live
 * on the strip above the Dock, so only `only="none"` and `only="taskbar"` entries
 * apply. `allowed` (a Set of ids) narrows the choice to the current behaviour's
 * animations; with nothing allowed it returns null. `rand` is in [0, 1).
 */
export function pickNext(anim, rand, allowed = null) {
  const options = anim.next.filter((n) => (n.only === 'none' || n.only === 'taskbar' || !n.only) && (!allowed || allowed.has(n.id)) && n.probability > 0);
  const sum = options.reduce((s, n) => s + n.probability, 0);
  if (!options.length || sum <= 0) return null;
  let r = rand * sum;
  for (const n of options) {
    if (r < n.probability) return n.id;
    r -= n.probability;
  }
  return options.at(-1).id;
}

/** Where a frame sits in the sprite sheet, in tile units. */
export function tileOf(model, frame) {
  return { col: frame % model.tilesx, row: Math.floor(frame / model.tilesx) };
}
