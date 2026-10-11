import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { crateLicence, lockedCrates, registryAvailable } from '../scripts/third-party.mjs';
import { AGENT_DIR } from './helpers.mjs';

/** NOTICE, THIRD-PARTY.md, the repository's own setup files and the first-run README. */

const read = (...p) => fs.readFileSync(path.join(AGENT_DIR, ...p), 'utf8');

describe('licensing documents', () => {
  test('NOTICE credits desktopPet (MIT), says web-esheep (GPL) is not vendored, the art is fetched not committed, and the LGPL Linux library', () => {
    const n = read('NOTICE');
    assert.match(n, /desktopPet[\s\S]*MIT/);
    assert.match(n, /web-esheep[\s\S]*GPL-3\.0: NOT vendored/);
    assert.match(n, /NOT committed[\s\S]*pets fetch[\s\S]*1ba33bebed993439f03e502b37e9bd2daf061773/);
    assert.match(n, /gtk-layer-shell[\s\S]*LGPL-3\.0/);
    assert.match(n, /THIRD-PARTY\.md/);
    const pin = JSON.parse(read('pets.lock.json'));
    assert.ok(JSON.stringify(pin).includes('1ba33bebed993439f03e502b37e9bd2daf061773'), 'NOTICE names the commit pets.lock.json pins');
  });
  test('THIRD-PARTY.md is up to date with Cargo.lock and the package files', { skip: !registryAvailable() && 'no cargo registry on this machine' }, () => {
    const r = spawnSync(process.execPath, [path.join(AGENT_DIR, 'scripts', 'third-party.mjs'), '--check'], { encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr);
    const t = read('THIRD-PARTY.md');
    assert.equal([...t.matchAll(/^\| [a-z0-9_-]+ \| \d[^|]* \| [^|]+ \|$/gm)].length, lockedCrates(read('shell', 'src-tauri', 'Cargo.lock')).length);
    assert.match(t, /The brain \(`package\.json`\) has \*\*no dependencies\*\*/);
    assert.deepEqual(JSON.parse(read('package.json')).dependencies, undefined);
  });
  test('the licence reader takes the [package] licence, not a dependency\'s', () => {
    assert.equal(crateLicence('[package]\nname = "a"\nlicense = "MIT OR Apache-2.0"\n[dependencies]\nlicense = "GPL"\n'), 'MIT OR Apache-2.0');
    assert.equal(crateLicence('[package]\nname = "a"\nlicense-file = "LICENSE"\n'), 'see its licence file');
    assert.deepEqual(lockedCrates('[[package]]\nname = "x"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n[[package]]\nname = "bellwether-shell"\nversion = "0.1.0"\n'), [{ name: 'x', version: '1.0.0' }]);
  });
});

test('--check skips cleanly with no cargo registry (a fresh clone before the first build)', () => {
  const r = spawnSync(process.execPath, [path.join(AGENT_DIR, 'scripts', 'third-party.mjs'), '--check'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: '/nonexistent', CARGO_HOME: '/nonexistent' } });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /^skipped: no cargo registry/);
});

describe('repository setup', () => {
  test('.gitignore keeps secrets, local state and build output out; .env.example has the token line, empty', () => {
    const gi = read('.gitignore').split('\n');
    for (const line of ['.env', '.env.*', '!.env.example', 'data/', 'config.json', 'shell/src-tauri/target/', 'shell/src-tauri/gen/']) assert.ok(gi.includes(line), line);
    assert.match(read('.env.example'), /^BELLWETHER_HOOK_TOKEN=$/m);
  });
  test('the README credits desktopPet, says the art is fetched rather than included, and has no hub paths', () => {
    const r = read('README.md');
    assert.match(r, /^# Bellwether/);
    assert.match(r, /desktopPet[\s\S]*eSheep/);
    assert.match(r, /never committed or bundled/);
    assert.doesNotMatch(r, /agents\/bellwether|PLAN\.md|hub `\.env`|hub-root/);
  });
  test('the README\'s first run puts autostart before hooks, and quitting the trial strip before autostart', () => {
    const r = read('README.md');
    const first = r.slice(r.indexOf('## First run'), r.indexOf('## Setup'));
    const at = (s) => first.indexOf(s);
    for (const s of ['pets fetch', 'shell --dev --release', '**Quit it**', 'autostart install --write', 'hooks install --write', 'all set']) assert.ok(at(s) > 0, s);
    assert.ok(at('pets fetch') < at('shell --dev --release') && at('**Quit it**') < at('autostart install --write') && at('autostart install --write') < at('hooks install --write'));
  });
});
