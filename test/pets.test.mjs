import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { PET_SOURCE, PINNED_PETS, fetchPets, petUrl } from '../brain/pets.mjs';
import { tempDir } from './helpers.mjs';

/** `bellwether pets fetch`, with a fake network: the pinned URL and the sha256 check. */

const bytes = Buffer.from('<animations>invented test pet</animations>');
const sha = createHash('sha256').update(bytes).digest('hex');
const pinned = { testpet: { path: 'Pets/testpet/animations.xml', sha256: sha } };
const fakeFetch = (body, status = 200) => async (url) => {
  fakeFetch.urls.push(url);
  return new Response(body, { status });
};
fakeFetch.urls = [];

test('esheep64 is pinned to a commit and a sha256', () => {
  assert.match(PET_SOURCE.commit, /^[0-9a-f]{40}$/);
  assert.match(PINNED_PETS.esheep64.sha256, /^[0-9a-f]{64}$/);
  assert.equal(petUrl(PINNED_PETS.esheep64), `https://raw.githubusercontent.com/Adrianotiger/desktopPet/${PET_SOURCE.commit}/Pets/esheep64/animations.xml`);
});

test('a download with the pinned hash is written, with a manifest; a second run downloads nothing', async () => {
  const dir = tempDir();
  const r = await fetchPets(dir, { pinned, fetchImpl: fakeFetch(bytes) });
  assert.deepEqual(r, [{ folder: 'testpet', status: 'downloaded', bytes: bytes.length }]);
  assert.deepEqual(fs.readFileSync(path.join(dir, 'pets', 'testpet', 'animations.xml')), bytes);
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'pets', 'manifest.json'), 'utf8')).testpet.sha256, sha);
  const again = await fetchPets(dir, { pinned, fetchImpl: () => assert.fail('no download expected') });
  assert.equal(again[0].status, 'present');
});

test('a download whose hash differs is refused and nothing is written', async () => {
  const dir = tempDir();
  await assert.rejects(fetchPets(dir, { pinned, fetchImpl: fakeFetch(Buffer.from('tampered')) }), /sha256 mismatch/);
  assert.equal(fs.existsSync(path.join(dir, 'pets', 'testpet')), false);
});

test('HTTP errors and unknown pets are reported', async () => {
  await assert.rejects(fetchPets(tempDir(), { pinned, fetchImpl: fakeFetch(bytes, 404) }), /HTTP 404/);
  await assert.rejects(fetchPets(tempDir(), { pinned, only: ['nope'], fetchImpl: fakeFetch(bytes) }), /unknown pet "nope"/);
});
