import { CliError, EXIT } from './util.mjs';
import { compilePatterns } from './redact.mjs';

/**
 * Session colours. Since Milestone 10's M10-13 they colour only the horns (`pets.tint`
 * "horns"), which are a few pixels across against cream wool and an orange face. So
 * the colours are strong and dark enough to read there:
 * - no yellow or grey, which vanish on the wool;
 * - no violet, which looks like the horns' own original purple;
 * - orange and blue first, the pair least confused, also with red-green colour
 *   blindness.
 * The order is orange, blue, black, red, cyan, brown, green, pink.
 * Comparison: data/screenshots/m10-horns.png.
 */
export const DEFAULT_PALETTE = ['#FF8C00', '#1F5BFF', '#202020', '#E8202A', '#00C2D4', '#8B4513', '#12A84A', '#FF4FB0'];

/** Milestone 3 to 10's default: colour 0 untinted white, colour 1 a pale blue (hard to tell apart). */
export const OLD_DEFAULT_PALETTE = ['#ffffff', '#7fb3ff', '#ffb36b', '#9be37d', '#f28bd0', '#c9a2ff', '#ffe066', '#ff7b7b'];

/** Set on a config whose palette was the old default and now uses the new one. */
export const PALETTE_UPGRADED = Symbol('palette upgraded');

/** A config.json copied from the old example has the old palette verbatim: that means "the default". */
export function isOldDefaultPalette(p) {
  return Array.isArray(p) && p.length === OLD_DEFAULT_PALETTE.length && p.every((c, i) => typeof c === 'string' && c.toLowerCase() === OLD_DEFAULT_PALETTE[i]);
}

/**
 * Defaults for every key the brain reads, merged under config.json. A key missing
 * from an older config.json therefore still has a value.
 */
export const DEFAULTS = {
  port: 47417,
  pets: { maxLambsShown: 5, palette: [...DEFAULT_PALETTE] },
  behaviour: {
    busyWindowSec: 8,
    grazeAfterSec: 20,
    sleepAfterMin: 10,
    minDwellMs: 1500,
    reapAfterHours: 12,
    maxLambsShown: 5,
    thinkingTimeoutSec: 120,
    lambTimeoutMin: 30,
    toolTimeoutMin: 30,
  },
  bubble: { detail: 'full', maxChars: 60, redactPatterns: [] },
};

/** Deep-merges config over DEFAULTS (objects merge, everything else replaces). */
export function withDefaults(config) {
  const merge = (base, over) => {
    if (!isObject(base) || !isObject(over)) return over === undefined ? base : over;
    const out = { ...base };
    for (const [k, v] of Object.entries(over)) out[k] = merge(base[k], v);
    return out;
  };
  const c = merge(DEFAULTS, config);
  if (isOldDefaultPalette(c.pets.palette)) {
    c.pets.palette = [...DEFAULT_PALETTE];
    c[PALETTE_UPGRADED] = true;
  }
  validateBrainConfig(c);
  return c;
}

function validateBrainConfig(c) {
  const b = c.behaviour;
  for (const k of ['busyWindowSec', 'grazeAfterSec', 'sleepAfterMin', 'minDwellMs', 'reapAfterHours', 'thinkingTimeoutSec', 'lambTimeoutMin', 'toolTimeoutMin']) {
    if (typeof b[k] !== 'number' || !(b[k] >= 0)) throw new CliError(`config: behaviour.${k} must be a number ≥ 0`, EXIT.BAD_CONFIG);
  }
  if (!Number.isInteger(b.maxLambsShown) || b.maxLambsShown < 0) throw new CliError('config: behaviour.maxLambsShown must be an integer ≥ 0', EXIT.BAD_CONFIG);
  if (!['full', 'tool'].includes(c.bubble.detail)) throw new CliError('config: bubble.detail must be "full" or "tool"', EXIT.BAD_CONFIG);
  if (!Number.isInteger(c.bubble.maxChars) || c.bubble.maxChars < 8 || c.bubble.maxChars > 500) throw new CliError('config: bubble.maxChars must be an integer from 8 to 500', EXIT.BAD_CONFIG);
  if (!Array.isArray(c.bubble.redactPatterns)) throw new CliError('config: bubble.redactPatterns must be an array of regular expressions', EXIT.BAD_CONFIG);
  compilePatterns(c.bubble.redactPatterns);
  if (!Array.isArray(c.pets.palette) || c.pets.palette.length === 0) throw new CliError('config: pets.palette must be a non-empty array', EXIT.BAD_CONFIG);
  if (c.pets.tint !== undefined && !['horns', 'whole'].includes(c.pets.tint)) throw new CliError('config: pets.tint must be "horns" (colour only the horns) or "whole" (tint the whole sheep)', EXIT.BAD_CONFIG);
}

function isObject(v) {
  return !!v && typeof v === 'object' && !Array.isArray(v);
}
