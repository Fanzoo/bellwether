import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';
import { maskToken, nextStep } from './impl.mjs';

// The in-place .env writer of the Node CLI's `token init` was retired with it (milestone
// 11): the app writes a token file (plan D3; test/DIVERGENCES.md 6).

/** M10 verification fixes: the token never shown (D1), "all set" only when nothing fails (D2), `token init` (D4). */

const cli = (args, env) => spawnBrainSync([...args], { encoding: 'utf8', env });

describe('the token is masked in everything a person sees (D1)', () => {
  const T = 'maskCanaryToken_0123456789abcdef';
  const home = () => {
    const h = tempDir();
    fs.mkdirSync(path.join(h, '.claude'), { recursive: true });
    fs.writeFileSync(path.join(h, '.claude', 'settings.json'), '{}\n');
    return h;
  };
  test('hooks install preview, hooks print to stdout and errors show 4 characters and "…"; print --out and --write keep the value', () => {
    const h = home();
    const env = { PATH: process.env.PATH, HOME: h, BELLWETHER_DATA_DIR: path.join(h, 'data'), BELLWETHER_HOOK_TOKEN: T };
    const prev = cli(['hooks', 'install'], env);
    assert.equal(prev.status, 0, prev.stderr);
    assert.ok(!(prev.stdout + prev.stderr).includes(T));
    assert.match(prev.stdout + prev.stderr, /"X-Bellwether-Token": "mask…"/);
    const print = cli(['hooks', 'print'], env);
    assert.ok(!(print.stdout + print.stderr).includes(T));
    const out = path.join(h, 'hooks.json');
    assert.equal(cli(['hooks', 'print', '--out', out], env).status, 0);
    assert.ok(fs.readFileSync(out, 'utf8').includes(T), 'the file written on purpose keeps the real value');
    const w = cli(['hooks', 'install', '--write'], env);
    assert.equal(w.status, 0, w.stderr);
    assert.ok(!(w.stdout + w.stderr).includes(T));
    assert.ok(fs.readFileSync(path.join(h, '.claude', 'settings.json'), 'utf8').includes(T), '--write writes the real value');
  });
  test('an old token in the settings diff is masked too, and nothing shorter than 5 characters is touched', () => {
    assert.equal(maskToken('"X-Bellwether-Token": "oldTokenValue123456"', 'currentToken_xyz'), '"X-Bellwether-Token": "oldT…"');
    assert.equal(maskToken('a currentToken_xyz b', 'currentToken_xyz'), 'a curr… b');
    assert.equal(maskToken('"X-Bellwether-Token": "$X"', 't'), '"X-Bellwether-Token": "$X"');
    assert.equal(maskToken('nothing here', undefined), 'nothing here');
  });
});

describe('"all set" only when nothing fails (D2)', () => {
  const home = () => {
    const h = tempDir();
    fs.mkdirSync(path.join(h, 'Library', 'LaunchAgents'), { recursive: true });
    for (const l of ['brain', 'shell']) fs.writeFileSync(path.join(h, 'Library', 'LaunchAgents', `local.bellwether.${l}.plist`), '');
    return h;
  };
  const base = [{ name: 'config', status: 'PASS' }, { name: 'token', status: 'PASS' }, { name: 'brain', status: 'PASS' }];
  test('a failing check names itself and its fix, before hooks and instead of "all set"', () => {
    const h = home();
    const fail = { name: 'disableAllHooks', status: 'FAIL', detail: 'settings set disableAllHooks: true; remove it' };
    const s = nextStep({ checks: [...base, fail], installed: true, home: h, platform: 'darwin', shellBuilt: true });
    assert.equal(s, 'fix "disableAllHooks": settings set disableAllHooks: true; remove it');
    assert.match(nextStep({ checks: [...base, { name: 'brain token', status: 'FAIL', detail: 'the brain refused the token' }], installed: true, home: h, platform: 'darwin', shellBuilt: true }), /^fix "brain token"/);
    assert.match(nextStep({ checks: base, installed: true, home: h, platform: 'darwin', shellBuilt: true }), /all set/);
  });
});

describe('bellwether token init (D4)', () => {
  test('docs/DEVELOPMENT.md\'s first run makes the token with `token init`, and the CLI only accepts `token init`', () => {
    const r = fs.readFileSync(path.join(AGENT_DIR, 'docs', 'DEVELOPMENT.md'), 'utf8');
    const first = r.slice(r.indexOf('## First run'), r.indexOf('## Working from a checkout'));
    assert.match(first, /bw token init {2,}# data\/hook-token/);
    assert.doesNotMatch(first, />> \.env|\.env\b/);
    assert.equal(cli(['token', 'bogus'], { PATH: process.env.PATH }).status, 3);
  });
});

describe('token init and a token that is there but unusable', () => {
  // The app keeps its token in its hook-token file (DIVERGENCES 6): an unusable value is
  // left alone and named (exit 3), and only --force replaces it, saying so.
  test('left alone and named, exit 3; --force replaces it with a usable token', () => {
    const d = tempDir();
    const env = { ...process.env, PATH: process.env.PATH };
    delete env.BELLWETHER_HOOK_TOKEN;
    env.BELLWETHER_HOME = d;
    const where = path.join(d, 'hook-token');
    fs.writeFileSync(where, 'short\n', { mode: 0o600 });
    const read = () => fs.readFileSync(where, 'utf8').trim();
    const left = cli(['token', 'init'], env);
    assert.equal(left.status, 3, left.stderr);
    assert.match(left.stdout, /already set .* but isn't a usable token \(shorter than 16 characters/);
    assert.match(left.stdout, /bellwether token init --force/);
    assert.equal(read(), 'short');
    const forced = cli(['token', 'init', '--force'], env);
    assert.equal(forced.status, 0, forced.stderr);
    assert.match(forced.stdout, /in place of the unusable one/);
    assert.match(read(), /^[A-Za-z0-9_-]{32}$/);
    assert.ok(!forced.stdout.includes(read()), 'the new token is never printed');
    const again = cli(['token', 'init', '--force'], env);
    assert.equal(again.status, 0);
    assert.match(again.stdout, /already set/, '--force never replaces a usable token');
  });
});

describe('M10-8: an old token is masked wherever it appears', () => {
  test('the mask hides an old token under any header case, and wherever the same value repeats', () => {
    const seen = new Set();
    const out = maskToken('{"x-BELLWETHER-token": "oldTokenCanary_ZZ99", "X-Other": "oldTokenCanary_ZZ99"}\nX-Bellwether-Token: plainHeaderValue_1', 'current_123456789', seen);
    assert.doesNotMatch(out, /oldTokenCanary_ZZ99|plainHeaderValue_1/);
    assert.equal(maskToken('later: oldTokenCanary_ZZ99', 'current_123456789', seen), 'later: oldT…', 'remembered for later output');
  });
});
