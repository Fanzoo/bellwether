import fs from 'node:fs';
import path from 'node:path';
import { AGENT_DIR, CliError, EXIT } from './util.mjs';
import { withDefaults } from './defaults.mjs';

/**
 * Loads config.json (in the repository root), falling back to config.example.json when it is
 * missing. Validates the port and that nobody tried
 * to configure a bind address, then merges defaults under it (defaults.mjs), which
 * also validates the behaviour and bubble keys the brain reads.
 */
export function loadConfig({ dir = AGENT_DIR, env = process.env } = {}) {
  const real = path.join(dir, 'config.json');
  const example = path.join(dir, 'config.example.json');
  // BELLWETHER_CONFIG (a test aid, as in the shell's settings.rs): another config file
  // for this checkout, e.g. one on a port that isn't the live brain's.
  const other = dir === AGENT_DIR && env.BELLWETHER_CONFIG ? path.resolve(env.BELLWETHER_CONFIG) : null;
  const file = other ?? (fs.existsSync(real) ? real : example);
  let raw;
  try {
    raw = fs.readFileSync(file, 'utf8');
  } catch (e) {
    throw new CliError(`cannot read ${file}: ${e.code ?? e.message}`, EXIT.BAD_CONFIG);
  }
  let config;
  try {
    config = JSON.parse(raw);
  } catch (e) {
    throw new CliError(`${path.basename(file)} is not valid JSON: ${e.message}`, EXIT.BAD_CONFIG);
  }
  validateConfig(config, path.basename(file));
  return { config: withDefaults(config), file, usingExample: file === example };
}

export function validateConfig(config, name = 'config') {
  if (!config || typeof config !== 'object' || Array.isArray(config)) {
    throw new CliError(`${name} must be a JSON object`, EXIT.BAD_CONFIG);
  }
  const hostPath = findKey(config, 'host');
  if (hostPath) {
    throw new CliError(
      `${name} sets "${hostPath}". The listener always binds 127.0.0.1 and that is not configurable; remove the key.`,
      EXIT.BAD_CONFIG,
    );
  }
  const { port } = config;
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    throw new CliError(`${name}: "port" must be an integer between 1024 and 65535 (got ${JSON.stringify(port)})`, EXIT.BAD_CONFIG);
  }
  return config;
}

/** Depth-first search for a key anywhere in the config; returns its dotted path. */
function findKey(value, key, prefix = '') {
  if (!value || typeof value !== 'object') return null;
  for (const [k, v] of Object.entries(value)) {
    const here = prefix ? `${prefix}.${k}` : k;
    if (k.toLowerCase() === key) return here;
    const deeper = findKey(v, key, here);
    if (deeper) return deeper;
  }
  return null;
}

/**
 * BELLWETHER_HOOK_TOKEN from the environment, which envload.mjs fills from the .env. It
 * is copied into the settings file as a literal header value, so it must survive
 * JSON and HTTP headers unchanged.
 */
export function readToken(env = process.env) {
  const token = env.BELLWETHER_HOOK_TOKEN;
  if (!token) {
    throw new CliError(
      'BELLWETHER_HOOK_TOKEN is not set. Add one to the .env in the repository root with\n' +
        '  ./bellwether token init\n' +
        '(it adds one only if there is none, and never prints it).',
      EXIT.BAD_CONFIG,
    );
  }
  if (token.length < 16 || !/^[A-Za-z0-9_-]+$/.test(token)) {
    throw new CliError(
      'BELLWETHER_HOOK_TOKEN must be at least 16 characters of [A-Za-z0-9_-] (base64url). Regenerate it with the command in the README.',
      EXIT.BAD_CONFIG,
    );
  }
  if (KNOWN_EXAMPLE_TOKENS.includes(token)) {
    throw new CliError(
      'BELLWETHER_HOOK_TOKEN is a published example value (it appeared in published docs), so anyone can read it. Generate your own with the command in the README.',
      EXIT.BAD_CONFIG,
    );
  }
  return token;
}

/**
 * Why a token can't be used, in a few words, or null when it can. The same rules as
 * readToken (and the Rust brain's token check).
 */
export function tokenProblem(token) {
  if (token.length < 16 || !/^[A-Za-z0-9_-]+$/.test(token)) return 'shorter than 16 characters, or not all of [A-Za-z0-9_-]';
  if (KNOWN_EXAMPLE_TOKENS.includes(token)) return 'a published example value';
  return null;
}

/** Token values that have been committed as examples somewhere. Never accepted. */
export const KNOWN_EXAMPLE_TOKENS = ['q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh'];
