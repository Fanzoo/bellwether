import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { TOKEN, tempDir } from './helpers.mjs';
import { runDoctor } from './impl.mjs';

/**
 * M10a review D1: doctor judges `pets.default` by the strip's own rules, in the same words
 * from the Node and the Rust brain (test/conformance.json runs this file against both). A
 * bundled pet passes only under its exact pinned name with its file matching the pin;
 * the goose `brah` passes the same way (M10b); a wrong case and a trailing dot are named,
 * never passed.
 */

const pets = async (petsConfig) => {
  const dir = tempDir('pets-doctor-');
  const settingsFile = path.join(dir, 'settings.json');
  fs.writeFileSync(settingsFile, '{}\n');
  const config = { port: 1, ...(petsConfig ? { pets: petsConfig } : {}) };
  const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile, configLoader: () => ({ config, usingExample: false }), dataDir: path.join(dir, 'home') });
  return r.checks.find((c) => c.name === 'pets');
};

describe('doctor\'s pets line, by the strip\'s rules', () => {
  test('no pets.default: the sheep, bundled and matching its pin', async () => {
    const c = await pets(null);
    assert.equal(c.status, 'PASS', c.detail);
    assert.equal(c.detail, "bellwether, Bellwether's own pet, bundled; sha256 matches its pin");
  });
  test('"bellwether": the same', async () => {
    const c = await pets({ default: 'bellwether' });
    assert.equal(c.status, 'PASS', c.detail);
    assert.match(c.detail, /sha256 matches its pin/);
  });
  test('"brah": the goose, bundled since M10b, matching its pin', async () => {
    const c = await pets({ default: 'brah' });
    assert.equal(c.status, 'PASS', c.detail);
    assert.equal(c.detail, "brah, Bellwether's own pet, bundled; sha256 matches its pin");
  });
  test('"bryan": the zombie, bundled, matching its pin', async () => {
    const c = await pets({ default: 'bryan' });
    assert.equal(c.status, 'PASS', c.detail);
    assert.equal(c.detail, "bryan, Bellwether's own pet, bundled; sha256 matches its pin");
  });
  for (const name of ['BELLWETHER', 'Bellwether', 'bellwether.', 'Brah', 'Bryan']) {
    test(`${JSON.stringify(name)}: names are exact, never a PASS`, async () => {
      const c = await pets({ default: name });
      assert.equal(c.status, 'WARN', c.detail);
      assert.equal(c.detail, `pets.default is ${JSON.stringify(name)}, which the strip won't load: pet names are exact. Did you mean "${name.replace(/\.$/, '').toLowerCase()}"?`);
    });
  }
});
