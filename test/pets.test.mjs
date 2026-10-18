import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { FetchError, PET_SOURCE, PINNED_PETS, fetchPets, parseArgs, petUrl } from '../scripts/fetch-pets.mjs';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * scripts/fetch-pets.mjs (the dev script that fetches the eSheep art, milestone 11), with a
 * fake network: the pinned URL and the sha256 check; and `bellwether pets import` (the
 * app's, plan D8).
 */

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
  await assert.rejects(fetchPets(tempDir(), { pinned, only: ['nope'], fetchImpl: fakeFetch(bytes) }), (e) => e instanceof FetchError && e.exitCode === 3 && /unknown pet "nope"/.test(e.message));
});

test('the script fetches into --data, else BELLWETHER_HOME, else the checkout\'s data/; it imports nothing from brain/', () => {
  assert.equal(parseArgs(['--data', '/x/d'], {}).data, '/x/d');
  assert.equal(parseArgs(['--data=/x/e', 'esheep64'], {}).data, '/x/e');
  assert.deepEqual(parseArgs(['esheep64'], { BELLWETHER_HOME: '/x/home' }), { data: '/x/home', only: ['esheep64'] });
  assert.equal(parseArgs([], {}).data, path.join(AGENT_DIR, 'data'));
  assert.throws(() => parseArgs(['--bogus'], {}), /unknown option/);
  assert.throws(() => parseArgs(['a', 'b'], {}), /one pet at a time/);
  const src = fs.readFileSync(path.join(AGENT_DIR, 'scripts', 'fetch-pets.mjs'), 'utf8');
  assert.doesNotMatch(src, /from ['"][^'"]*brain\//);
});

// BR-3K's milestone 3 probes of the fetch (tester-m3.test.mjs until milestone 11), now of the script.
describe('the fetch, probed', () => {
  const GOOD = Buffer.from('<animations>good</animations>');
  const hash = (b) => createHash('sha256').update(b).digest('hex');
  const pinned = { esheep64: { path: 'Pets/esheep64/animations.xml', sha256: hash(GOOD) } };
  const resp = (body, init = {}) => async () => new Response(body, { status: 200, ...init });
  const dirs = () => tempDir('pets-');

  test('a good file is written and verified; a second run downloads nothing', async () => {
    const d = dirs();
    let n = 0;
    const f = async () => (n++, new Response(GOOD));
    await fetchPets(d, { fetchImpl: f, pinned });
    assert.equal(fs.readFileSync(path.join(d, 'pets', 'esheep64', 'animations.xml')).equals(GOOD), true);
    await fetchPets(d, { fetchImpl: f, pinned });
    assert.equal(n, 1);
  });
  test('a hash mismatch writes nothing, leaves an existing good file alone, and replaces a corrupted one only with a verified one', async () => {
    const d = dirs();
    await assert.rejects(fetchPets(d, { fetchImpl: resp('<animations>evil</animations>'), pinned }), /sha256 mismatch/);
    assert.equal(fs.existsSync(path.join(d, 'pets')), false, 'a rejected download left files behind');
    await fetchPets(d, { fetchImpl: resp(GOOD), pinned });
    const file = path.join(d, 'pets', 'esheep64', 'animations.xml');
    fs.writeFileSync(file, 'tampered');
    await fetchPets(d, { fetchImpl: resp(GOOD), pinned });
    assert.equal(fs.readFileSync(file).equals(GOOD), true);
    fs.writeFileSync(file, 'tampered');
    await assert.rejects(fetchPets(d, { fetchImpl: resp('nope'), pinned }));
    assert.equal(fs.readFileSync(file, 'utf8'), 'tampered', 'a failed download must not touch the file');
  });
  test('folder names that are not pinned pets are refused, including prototype keys and traversal', async () => {
    const d = dirs();
    for (const name of ['../evil', '..', '/etc', 'esheep64/../../x', '__proto__', 'constructor', 'toString', 'hasOwnProperty', '']) {
      let called = false;
      await assert.rejects(fetchPets(d, { only: [name], fetchImpl: async () => ((called = true), new Response(GOOD)), pinned }), /unknown pet|HTTP|mismatch/, JSON.stringify(name));
      assert.equal(called && name !== '__proto__' && name !== 'constructor' && name !== 'toString' && name !== 'hasOwnProperty', false, `${name} reached the network`);
    }
    assert.deepEqual(fs.readdirSync(d), [], 'something was written');
  });
  test('HTTP errors, a redirect to another host, and a connection that dies mid-body each give a cause, not a stack', async () => {
    const d = dirs();
    await assert.rejects(fetchPets(d, { fetchImpl: async () => new Response('x', { status: 404 }), pinned }), /HTTP 404/);
    await assert.rejects(fetchPets(d, { fetchImpl: async () => new Response('x', { status: 500 }), pinned }), /HTTP 500/);
    await assert.rejects(fetchPets(d, { fetchImpl: async () => { throw Object.assign(new TypeError('fetch failed'), { cause: { code: 'ENOTFOUND' } }); }, pinned }), /download failed \(ENOTFOUND\)/);
    const dying = async () => new Response(new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode('<anim')); setTimeout(() => c.error(new TypeError('terminated')), 10); } }));
    const err = await fetchPets(d, { fetchImpl: dying, pinned }).catch((e) => e);
    assert.ok(err instanceof Error);
    assert.equal(err.constructor.name, 'FetchError', `a partial download surfaced as ${err.constructor.name}: ${err.message}`);
    assert.equal(fs.existsSync(path.join(d, 'pets', 'esheep64', 'animations.xml')), false);
  });
  test('a response that never ends is bounded (size cap enforced while reading, not after)', async () => {
    const d = dirs();
    let sent = 0;
    const endless = async () => new Response(new ReadableStream({ pull(c) { c.enqueue(new Uint8Array(1024 * 1024)); sent += 1024 * 1024; if (sent > 400 * 1024 * 1024) c.close(); } }));
    await assert.rejects(fetchPets(d, { fetchImpl: endless, pinned }));
    assert.ok(sent <= 64 * 1024 * 1024, `${sent >> 20} MB were read before refusing (cap is 16 MB)`);
  });
});

// A 1×1 PNG, base64.
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const petXml = (extra = '') => `<?xml version="1.0" encoding="UTF-8"?>\n<animations><header><title>Invented test pet</title></header><image><tilesx>4</tilesx><tilesy>2</tilesy><png>${PNG}</png></image><animations><animation id="1"><name>walk</name><sequence><frame>0</frame></sequence></animation><animation id="2"><name>eat</name><sequence><frame>1</frame></sequence></animation>${extra}</animations></animations>`;

test('pets import (the app, plan D8): checked, copied, pinned, and a file that fails a check writes nothing', () => {
  const data = path.join(tempDir(), 'data');
  const src = path.join(tempDir(), 'mypet', 'animations.xml');
  fs.mkdirSync(path.dirname(src), { recursive: true });
  fs.writeFileSync(src, petXml());
  const cli = (...a) => spawnBrainSync(['pets', 'import', ...a], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: tempDir(), BELLWETHER_DATA_DIR: data } });
  const imported = path.join(data, 'pets', 'mypet', 'animations.xml');
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
