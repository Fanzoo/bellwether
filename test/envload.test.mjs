import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { envFilePath } from '../brain/envpath.mjs';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Which .env the CLI reads (brain/envpath.mjs, brain/envload.mjs): the repository's
 * own, BELLWETHER_ENV, or a hub's when the repository sits at agents/<name>. Temp
 * directories only; the real .env is never read.
 */

describe('which .env', () => {
  test('a repository on its own: its own .env, also when there is none yet (token init creates it there)', () => {
    const repo = path.join(tempDir('envp-'), 'bellwether');
    fs.mkdirSync(repo);
    assert.equal(envFilePath({}, repo), path.join(repo, '.env'));
    fs.writeFileSync(path.join(repo, '.env'), '');
    assert.equal(envFilePath({}, repo), path.join(repo, '.env'));
  });
  test('inside a hub at agents/<name>: the hub .env only when the repository has none of its own', () => {
    const hub = tempDir('envp-hub-');
    const repo = path.join(hub, 'agents', 'bellwether');
    fs.mkdirSync(repo, { recursive: true });
    assert.equal(envFilePath({}, repo), path.join(repo, '.env'), 'no hub .env either: the repository\'s own');
    fs.writeFileSync(path.join(hub, '.env'), '');
    assert.equal(envFilePath({}, repo), path.join(hub, '.env'));
    fs.writeFileSync(path.join(repo, '.env'), '');
    assert.equal(envFilePath({}, repo), path.join(repo, '.env'), 'its own wins');
  });
  test('a .env two levels up is not used unless the parent directory is called agents', () => {
    const top = tempDir('envp-other-');
    const repo = path.join(top, 'projects', 'bellwether');
    fs.mkdirSync(repo, { recursive: true });
    fs.writeFileSync(path.join(top, '.env'), 'BELLWETHER_HOOK_TOKEN=notMineNotMine_0123456789\n');
    assert.equal(envFilePath({}, repo), path.join(repo, '.env'));
  });
  test('BELLWETHER_ENV wins, relative to the caller\'s directory', () => {
    assert.equal(envFilePath({ BELLWETHER_ENV: '/x/y.env' }, '/r'), '/x/y.env');
    assert.equal(envFilePath({ BELLWETHER_ENV: 'my.env', BELLWETHER_CWD: '/work' }, '/r'), '/work/my.env');
  });
});

describe('the CLI loads it', () => {
  const T = 'envLoadCanaryToken_0123456789ab';
  const cli = (env, ...args) => spawnBrainSync([...args], { encoding: 'utf8', timeout: 20000, env: { PATH: process.env.PATH, ...env } });
  const hooksFile = (env) => {
    const dir = tempDir('envl-');
    const out = path.join(dir, 'hooks.json');
    const r = cli({ HOME: dir, BELLWETHER_DATA_DIR: path.join(dir, 'd'), ...env }, 'hooks', 'print', '--out', out);
    return { r, text: r.status === 0 ? fs.readFileSync(out, 'utf8') : '' };
  };
  test('from BELLWETHER_ENV, with nothing exported', () => {
    const f = path.join(tempDir('envl-f-'), 'x.env');
    fs.writeFileSync(f, `OTHER=1\nBELLWETHER_HOOK_TOKEN=${T}\n`);
    const { r, text } = hooksFile({ BELLWETHER_ENV: f });
    assert.equal(r.status, 0, r.stderr);
    assert.ok(text.includes(T));
  });
  test('an exported token wins over the file, as with node --env-file', () => {
    const f = path.join(tempDir('envl-f-'), 'x.env');
    fs.writeFileSync(f, `BELLWETHER_HOOK_TOKEN=${T}\n`);
    const { r, text } = hooksFile({ BELLWETHER_ENV: f, BELLWETHER_HOOK_TOKEN: 'exportedTokenWins_0123456789' });
    assert.equal(r.status, 0, r.stderr);
    assert.ok(text.includes('exportedTokenWins_0123456789') && !text.includes(T));
  });
  test('a missing file is not an error (the token check says what to do); an unreadable one exits 3', () => {
    const { r } = hooksFile({ BELLWETHER_ENV: path.join(tempDir('envl-none-'), 'absent.env') });
    assert.equal(r.status, 3);
    assert.match(r.stderr, /BELLWETHER_HOOK_TOKEN is not set[\s\S]*\.\/bellwether token init/);
    const dir = path.join(tempDir('envl-dir-'), 'a-directory.env');
    fs.mkdirSync(dir);
    const bad = hooksFile({ BELLWETHER_ENV: dir });
    assert.equal(bad.r.status, 3);
    assert.match(bad.r.stderr, /could not read/);
  });
});
