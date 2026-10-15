import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { PET_SOURCE, PINNED_PETS, fetchPets, petUrl } from '../brain/pets.mjs';
import { IMPL, notUnderRust, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * `bellwether pets fetch` (the repository's), with a fake network: the pinned URL and the
 * sha256 check; and `bellwether pets import` (the app's, plan D8).
 */

const FETCH = notUnderRust("`pets fetch` downloads the eSheep art into the repository's data/pets; the app imports a pet file instead (`pets import`, below)");

const bytes = Buffer.from('<animations>invented test pet</animations>');
const sha = createHash('sha256').update(bytes).digest('hex');
const pinned = { testpet: { path: 'Pets/testpet/animations.xml', sha256: sha } };
const fakeFetch = (body, status = 200) => async (url) => {
  fakeFetch.urls.push(url);
  return new Response(body, { status });
};
fakeFetch.urls = [];

test('esheep64 is pinned to a commit and a sha256', FETCH, () => {
  assert.match(PET_SOURCE.commit, /^[0-9a-f]{40}$/);
  assert.match(PINNED_PETS.esheep64.sha256, /^[0-9a-f]{64}$/);
  assert.equal(petUrl(PINNED_PETS.esheep64), `https://raw.githubusercontent.com/Adrianotiger/desktopPet/${PET_SOURCE.commit}/Pets/esheep64/animations.xml`);
});

test('a download with the pinned hash is written, with a manifest; a second run downloads nothing', FETCH, async () => {
  const dir = tempDir();
  const r = await fetchPets(dir, { pinned, fetchImpl: fakeFetch(bytes) });
  assert.deepEqual(r, [{ folder: 'testpet', status: 'downloaded', bytes: bytes.length }]);
  assert.deepEqual(fs.readFileSync(path.join(dir, 'pets', 'testpet', 'animations.xml')), bytes);
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'pets', 'manifest.json'), 'utf8')).testpet.sha256, sha);
  const again = await fetchPets(dir, { pinned, fetchImpl: () => assert.fail('no download expected') });
  assert.equal(again[0].status, 'present');
});

test('a download whose hash differs is refused and nothing is written', FETCH, async () => {
  const dir = tempDir();
  await assert.rejects(fetchPets(dir, { pinned, fetchImpl: fakeFetch(Buffer.from('tampered')) }), /sha256 mismatch/);
  assert.equal(fs.existsSync(path.join(dir, 'pets', 'testpet')), false);
});

test('HTTP errors and unknown pets are reported', FETCH, async () => {
  await assert.rejects(fetchPets(tempDir(), { pinned, fetchImpl: fakeFetch(bytes, 404) }), /HTTP 404/);
  await assert.rejects(fetchPets(tempDir(), { pinned, only: ['nope'], fetchImpl: fakeFetch(bytes) }), /unknown pet "nope"/);
});

// A 1×1 PNG, base64.
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const petXml = (extra = '') => `<?xml version="1.0" encoding="UTF-8"?>\n<animations><header><title>Invented test pet</title></header><image><tilesx>4</tilesx><tilesy>2</tilesy><png>${PNG}</png></image><animations><animation id="1"><name>walk</name><sequence><frame>0</frame></sequence></animation><animation id="2"><name>eat</name><sequence><frame>1</frame></sequence></animation>${extra}</animations></animations>`;

test('pets import (the app, plan D8): checked, copied, pinned, and a file that fails a check writes nothing; the Node CLI has only `pets fetch`', () => {
  const data = path.join(tempDir(), 'data');
  const src = path.join(tempDir(), 'mypet', 'animations.xml');
  fs.mkdirSync(path.dirname(src), { recursive: true });
  fs.writeFileSync(src, petXml());
  const cli = (...a) => spawnBrainSync(['pets', 'import', ...a], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: tempDir(), BELLWETHER_DATA_DIR: data } });
  const imported = path.join(data, 'pets', 'mypet', 'animations.xml');
  if (IMPL === 'node') {
    // The repository's CLI predates D8: `pets import` is a usage error, and nothing is written.
    const r = cli(src);
    assert.equal(r.status, 3, r.stderr);
    assert.match(r.stderr, /usage: bellwether pets fetch/);
    assert.equal(fs.existsSync(imported), false);
    return;
  }
  const r = cli(src);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(fs.readFileSync(imported), fs.readFileSync(src), 'copied byte for byte');
  const lock = JSON.parse(fs.readFileSync(path.join(data, 'pets.user.lock.json'), 'utf8'));
  assert.equal(lock.pets.mypet.sha256, createHash('sha256').update(fs.readFileSync(src)).digest('hex'), 'pinned by its sha256');
  assert.match(r.stdout, /"animationMap"[\s\S]*"walk": "walk"[\s\S]*"graze": "eat"/, 'a proposed map, by name matching');
  assert.match(r.stdout, /\{\n {2}"walk": "walk",\n/, 'the map keeps its line breaks (milestone 5: they printed as "?")');
  assert.equal(cli(src).status, 3, 'an imported pet is replaced only with --force');
  assert.equal(cli(src, '--force').status, 0);
  for (const [why, text] of [
    ['a DOCTYPE', `<!DOCTYPE a [<!ENTITY x "y">]>${petXml()}`],
    ['XML that is not well-formed', petXml('<animation id="3">')],
    ['tiles over 64', petXml().replace('<tilesx>4</tilesx>', '<tilesx>65</tilesx>')],
    ['no PNG', petXml().replace(PNG, 'bm90IGEgcG5n')],
    ['not UTF-8', Buffer.from([0x3c, 0xff, 0xfe, 0x3e])],
  ]) {
    const bad = path.join(tempDir(), 'animations.xml');
    fs.writeFileSync(bad, text);
    const b = cli(bad, '--name', 'bad');
    assert.equal(b.status, 3, `${why}: ${b.stderr}`);
    assert.match(b.stderr, /was refused: .*Nothing was imported/, why);
    assert.equal(fs.existsSync(path.join(data, 'pets', 'bad')), false, `${why}: something was written`);
  }
  // Hostile files (BR-3K's M4 finding D4): a million nested elements is refused, not a
  // stack overflow; a title with terminal escapes prints none of them.
  const deep = path.join(tempDir(), 'animations.xml');
  fs.writeFileSync(deep, `<animations>${'<a>'.repeat(1e6)}${'</a>'.repeat(1e6)}</animations>`);
  const d = cli(deep, '--name', 'deep');
  assert.equal(d.status, 3, d.stderr);
  assert.match(d.stderr, /nested more than 64 deep/);
  const esc = path.join(tempDir(), 'esc', 'animations.xml');
  fs.mkdirSync(path.dirname(esc));
  fs.writeFileSync(esc, petXml().replace('Invented test pet', '&#27;]0;owned&#7;'));
  const e = cli(esc);
  assert.equal(e.status, 0, e.stderr);
  assert.doesNotMatch(e.stdout + e.stderr, /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/, 'a control character from the file reached the terminal');
  assert.match(e.stdout, /\?\]0;owned\?/);
});
