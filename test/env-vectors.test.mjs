import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { FIXTURES, tempDir } from './helpers.mjs';

/**
 * test/fixtures/env-vectors.json pins how `node --env-file` reads BELLWETHER_HOOK_TOKEN.
 * The shell's Rust parser is tested against the same file (settings.rs), so the two
 * can't drift apart. This test re-checks every vector against the Node running it.
 */
const { vectors } = JSON.parse(fs.readFileSync(path.join(FIXTURES, 'env-vectors.json'), 'utf8'));

test('every shared .env vector matches what node --env-file actually does', () => {
  const dir = tempDir();
  const env = { ...process.env };
  delete env.BELLWETHER_HOOK_TOKEN;
  const mismatches = [];
  vectors.forEach((v, i) => {
    const f = path.join(dir, `e${i}`);
    fs.writeFileSync(f, v.text);
    const r = spawnSync(process.execPath, [`--env-file=${f}`, '-e', 'process.stdout.write(JSON.stringify(process.env.BELLWETHER_HOOK_TOKEN ?? null))'], { env, encoding: 'utf8' });
    const got = JSON.parse(r.stdout);
    if (got !== v.expected) mismatches.push(`${v.name}: vector says ${JSON.stringify(v.expected)}, node says ${JSON.stringify(got)}`);
  });
  assert.deepEqual(mismatches, []);
  assert.ok(vectors.length >= 20);
});

test('brain/envfile.mjs (the one JS parser, used by `token init`) agrees with every vector', async () => {
  const { envValue } = await import('../brain/envfile.mjs');
  const mismatches = vectors.filter((v) => envValue(v.text, 'BELLWETHER_HOOK_TOKEN') !== v.expected).map((v) => `${v.name}: parser says ${JSON.stringify(envValue(v.text, 'BELLWETHER_HOOK_TOKEN'))}, vector ${JSON.stringify(v.expected)}`);
  assert.deepEqual(mismatches, []);
});
