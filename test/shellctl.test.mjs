import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { TOKEN, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * `bellwether shell start | stop | restart` through the binary (temp HOME only: launchd and
 * the live strip are never touched). The app's strip control is Bellwether.app's own
 * (test/DIVERGENCES.md 19); the repository's (its LaunchAgent, or a detached copy of
 * target/release/bellwether-shell) went with the Node brain in milestone 11.
 */

describe('the CLI', () => {
  test('`bellwether shell bogus` and `shell start --dev` are refused (exit 3) without doing anything', () => {
    const env = { PATH: process.env.PATH, HOME: tempDir(), BELLWETHER_DATA_DIR: tempDir(), BELLWETHER_HOOK_TOKEN: TOKEN };
    const cli = (...a) => spawnBrainSync(['shell', ...a], { encoding: 'utf8', env });
    assert.equal(cli('bogus').status, 3);
    assert.equal(cli('start', '--dev').status, 3);
  });
  test('`shell stop` outside the app: a development binary refuses it (the app\'s strip is Bellwether.app\'s)', () => {
    const env = { PATH: process.env.PATH, HOME: tempDir(), BELLWETHER_DATA_DIR: tempDir(), BELLWETHER_HOOK_TOKEN: TOKEN };
    const r = spawnBrainSync(['shell', 'stop'], { encoding: 'utf8', env });
    assert.equal(r.status, 2, r.stderr);
    assert.match(r.stderr, /controls Bellwether\.app's strip, and this is a development binary outside the app/);
  });
});
