import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { nextStep } from '../brain/doctor.mjs';
import { tokenInit } from '../brain/token.mjs';
import { AGENT_DIR, IMPL, notUnderRust, spawnBrainSync, tempDir } from './helpers.mjs';
// brain/*.mjs, or the Rust brain under BELLWETHER_IMPL=rust (test/impl.mjs).
import { CliError, envValue, maskToken } from './impl.mjs';

// The in-place .env writer of `token init` is retired with the Node CLI: the app writes the
// token file instead (plan D3; test/DIVERGENCES.md, "token init writes the token file").
const ENV_WRITER = 'the .env writer is retired with the Node CLI; the app writes a token file (test/DIVERGENCES.md)';

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
  test('hooks install preview, hooks print to stdout and errors show 4 characters and "…"; print --out and --write keep the value', notUnderRust('hooks install and hooks print are ported in milestone 4'), () => {
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
  test('a failing check names itself and its fix, before hooks and instead of "all set"', notUnderRust('doctor is ported in milestone 4'), () => {
    const h = home();
    const fail = { name: 'disableAllHooks', status: 'FAIL', detail: 'settings set disableAllHooks: true; remove it' };
    const s = nextStep({ checks: [...base, fail], installed: true, home: h, platform: 'darwin', shellBuilt: true });
    assert.equal(s, 'fix "disableAllHooks": settings set disableAllHooks: true; remove it');
    assert.match(nextStep({ checks: [...base, { name: 'brain token', status: 'FAIL', detail: 'the brain refused the token' }], installed: true, home: h, platform: 'darwin', shellBuilt: true }), /^fix "brain token"/);
    assert.match(nextStep({ checks: base, installed: true, home: h, platform: 'darwin', shellBuilt: true }), /all set/);
  });
});

describe('bellwether token init (D4)', () => {
  const env = (text, mode = 0o640) => {
    const f = path.join(tempDir(), '.env');
    if (text !== null) fs.writeFileSync(f, text, { mode });
    return f;
  };
  test('appends after a last line with no newline, without gluing onto it, and keeps the mode', notUnderRust(ENV_WRITER), () => {
    const f = env('OTHER_KEY=abc');
    assert.equal(tokenInit(f, { generate: () => 'TOK_1234567890abcdef' }).changed, true);
    assert.equal(fs.readFileSync(f, 'utf8'), 'OTHER_KEY=abc\nBELLWETHER_HOOK_TOKEN=TOK_1234567890abcdef\n');
    assert.equal(fs.statSync(f).mode & 0o777, 0o640);
  });
  test('safe to run twice: an existing token is left alone, in any of the forms Node reads', notUnderRust(ENV_WRITER), () => {
    for (const text of ['BELLWETHER_HOOK_TOKEN=abc\n', 'export BELLWETHER_HOOK_TOKEN="abc"\n', '  BELLWETHER_HOOK_TOKEN = abc']) {
      const f = env(text);
      const r = tokenInit(f);
      assert.equal(r.changed, false);
      assert.equal(fs.readFileSync(f, 'utf8'), text);
      assert.match(r.message, /already set/);
    }
  });
  test('fills an empty line in place (as in .env.example); creates a missing file 0600; never prints the token', notUnderRust(ENV_WRITER), () => {
    const f = env('A=1\nBELLWETHER_HOOK_TOKEN=\nB=2\n');
    const r = tokenInit(f, { generate: () => 'TOK_FILLED_0123456789' });
    assert.equal(fs.readFileSync(f, 'utf8'), 'A=1\nBELLWETHER_HOOK_TOKEN=TOK_FILLED_0123456789\nB=2\n');
    assert.doesNotMatch(r.message, /TOK_FILLED/);
    const g = env(null);
    tokenInit(g, { generate: () => 'TOK_NEW_0123456789ab' });
    assert.equal(fs.statSync(g).mode & 0o777, 0o600);
    assert.equal(fs.readFileSync(g, 'utf8'), 'BELLWETHER_HOOK_TOKEN=TOK_NEW_0123456789ab\n');
  });
  test('the README\'s first step is `token init`, and the CLI only accepts `token init`', () => {
    const r = fs.readFileSync(path.join(AGENT_DIR, 'README.md'), 'utf8');
    const first = r.slice(r.indexOf('## First run'), r.indexOf('## Setup'));
    assert.match(first, /\.\/bellwether token init/);
    assert.doesNotMatch(first, />> \.env/);
    assert.equal(cli(['token', 'bogus'], { PATH: process.env.PATH }).status, 3);
  });
});

describe('token init and a token that is there but unusable (both brains)', () => {
  // The Node CLI keeps its token in a .env, the app in its hook-token file (DIVERGENCES 6);
  // either way an unusable value is left alone and named (exit 3), and only --force
  // replaces it, saying so.
  test('left alone and named, exit 3; --force replaces it with a usable token', () => {
    const d = tempDir();
    const env = { ...process.env, PATH: process.env.PATH };
    delete env.BELLWETHER_HOOK_TOKEN;
    let where;
    let read;
    if (IMPL === 'rust') {
      env.BELLWETHER_HOME = d;
      where = path.join(d, 'hook-token');
      fs.writeFileSync(where, 'short\n', { mode: 0o600 });
      read = () => fs.readFileSync(where, 'utf8').trim();
    } else {
      where = path.join(d, '.env');
      env.BELLWETHER_ENV = where;
      fs.writeFileSync(where, 'BELLWETHER_HOOK_TOKEN=short\n', { mode: 0o600 });
      read = () => envValue(fs.readFileSync(where, 'utf8'), 'BELLWETHER_HOOK_TOKEN');
    }
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

describe('M10-8: token init never risks the .env, and reads it the way Node does', () => {
  const env = (text, mode = 0o640) => {
    const d = tempDir();
    const f = path.join(d, '.env');
    fs.writeFileSync(f, text, { mode });
    fs.chmodSync(f, mode);
    return { d, f };
  };
  const G = { generate: () => 'TOK_M108_0123456789ab' };
  test('the last BELLWETHER_HOOK_TOKEN line is the one Node uses, and `KEY= # note` counts as empty', notUnderRust(ENV_WRITER), async () => {
    for (const text of ['BELLWETHER_HOOK_TOKEN=abc\nBELLWETHER_HOOK_TOKEN=\n', 'BELLWETHER_HOOK_TOKEN= # fill me in\n', 'BELLWETHER_HOOK_TOKEN=\n']) {
      const { f } = env(text);
      assert.equal(tokenInit(f, G).changed, true, JSON.stringify(text));
      assert.equal(envValue(fs.readFileSync(f, 'utf8'), 'BELLWETHER_HOOK_TOKEN'), 'TOK_M108_0123456789ab');
    }
    const { f } = env('BELLWETHER_HOOK_TOKEN=\nBELLWETHER_HOOK_TOKEN=set_later\n');
    assert.equal(tokenInit(f, G).changed, false, 'the last one is set');
  });
  test('CRLF stays CRLF, filled or appended', notUnderRust(ENV_WRITER), () => {
    const a = env('A=1\r\nBELLWETHER_HOOK_TOKEN=\r\nB=2\r\n');
    tokenInit(a.f, G);
    assert.equal(fs.readFileSync(a.f, 'utf8'), 'A=1\r\nBELLWETHER_HOOK_TOKEN=TOK_M108_0123456789ab\r\nB=2\r\n');
    const b = env('A=1\r\nB=2');
    tokenInit(b.f, G);
    assert.equal(fs.readFileSync(b.f, 'utf8'), 'A=1\r\nB=2\r\nBELLWETHER_HOOK_TOKEN=TOK_M108_0123456789ab\r\n');
  });
  test('temp file and rename: same mode, a 0600 .env.bak of the old contents, a symlink stays a symlink, no temp left behind', notUnderRust(ENV_WRITER), () => {
    const { d, f } = env('SECRET_A=xyz\n', 0o640);
    const ino = fs.statSync(f).ino;
    tokenInit(f, G);
    assert.notEqual(fs.statSync(f).ino, ino, 'replaced by rename, not rewritten in place');
    assert.equal(fs.statSync(f).mode & 0o777, 0o640);
    assert.equal(fs.readFileSync(`${f}.bak`, 'utf8'), 'SECRET_A=xyz\n');
    assert.equal(fs.statSync(`${f}.bak`).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(d).sort(), ['.env', '.env.bak']);
    const real = path.join(d, 'real.env');
    fs.writeFileSync(real, 'B=1\n');
    const link = path.join(d, 'link.env');
    fs.symlinkSync(real, link);
    tokenInit(link, G);
    assert.ok(fs.lstatSync(link).isSymbolicLink());
    assert.match(fs.readFileSync(real, 'utf8'), /^B=1\nBELLWETHER_HOOK_TOKEN=/);
  });
  test('a read-only .env is refused with one clean line, and left as it was', notUnderRust(ENV_WRITER), () => {
    const { f } = env('A=1\n', 0o444);
    assert.throws(() => tokenInit(f, G), (e) => e instanceof CliError && /can't write .*\.env \(EACCES\); nothing was changed/.test(e.message));
    assert.equal(fs.readFileSync(f, 'utf8'), 'A=1\n');
  });
  test('the mask hides an old token under any header case, and wherever the same value repeats', () => {
    const seen = new Set();
    const out = maskToken('{"x-BELLWETHER-token": "oldTokenCanary_ZZ99", "X-Other": "oldTokenCanary_ZZ99"}\nX-Bellwether-Token: plainHeaderValue_1', 'current_123456789', seen);
    assert.doesNotMatch(out, /oldTokenCanary_ZZ99|plainHeaderValue_1/);
    assert.equal(maskToken('later: oldTokenCanary_ZZ99', 'current_123456789', seen), 'later: oldT…', 'remembered for later output');
  });
});
