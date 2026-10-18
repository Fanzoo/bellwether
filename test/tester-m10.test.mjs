import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Tester probes for Milestone 10, against the binary under temp HOMEs. Nothing touches
 * ~/Library/LaunchAgents. The repository's LaunchAgent autostart, its first-run `next` walk
 * and the .env `token init` went with the Node brain in milestone 11 (the app's `next`
 * order: crates/brain doctor.rs and tester-m6.test.mjs; its token file: token.test.mjs).
 */

describe('docs/DEVELOPMENT.md "First run": every command exists, with the flags shown', () => {
  const readme = fs.readFileSync(path.join(AGENT_DIR, 'docs', 'DEVELOPMENT.md'), 'utf8');
  const section = readme.slice(readme.indexOf('## First run'), readme.indexOf('## Working from a checkout'));
  const block = /```\n([\s\S]*?)```/.exec(section)[1];
  // Each `bw …` command in the block, up to a comment or `&&`.
  const commands = block.split('\n').filter((l) => l.startsWith('bw ')).map((l) => l.replace(/#.*$/, '').split('&&')[0].trim().split(/\s+/).slice(1));
  test('the block names the expected commands, in order', () => {
    assert.deepEqual(commands.map((c) => c.join(' ')), ['token init', 'doctor', 'serve', 'hooks print --out data/hooks-only.json']);
    assert.match(block, /^npm run build:dev/m);
    assert.match(block, /^export BELLWETHER_HOME="\$PWD\/data"/m);
  });
  test('every one runs as shown under a temp BELLWETHER_HOME (`serve`, which would stay up, is checked in the help)', () => {
    const root = tempDir('m10-first-run-');
    fs.mkdirSync(path.join(root, 'data'));
    const config = path.join(root, 'config.json');
    fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), config);
    const env = { PATH: process.env.PATH, HOME: tempDir('m10-first-run-HOME-'), BELLWETHER_HOME: path.join(root, 'data'), BELLWETHER_CONFIG: config };
    for (const c of commands) {
      if (c[0] === 'serve') continue;
      const r = spawnBrainSync(c, { cwd: root, encoding: 'utf8', env, timeout: 30000 });
      assert.doesNotMatch(r.stderr + r.stdout, /unknown (command|option)|unexpected argument|usage:/i, `${c.join(' ')}: ${r.stderr.slice(0, 200)}`);
      // doctor fails the brain and hooks lines (nothing runs yet): exit 2, never 3 or 1.
      assert.ok(c[0] === 'doctor' ? r.status === 2 : r.status === 0, `${c.join(' ')}: exit ${r.status} ${r.stderr.slice(0, 200)}`);
    }
    assert.ok(fs.existsSync(path.join(root, 'data', 'hook-token')), 'token init wrote data/hook-token');
    assert.ok(fs.existsSync(path.join(root, 'data', 'hooks-only.json')), 'hooks print --out wrote data/hooks-only.json');
    const usage = spawnBrainSync(['help'], { encoding: 'utf8', env }).stdout;
    assert.match(usage, /bellwether serve \[--port <n>\]/);
  });
});

describe('previews', () => {
  test('the previews it tells the model to show do not contain the token', () => {
    const T = 'previewCanaryToken_0123456789ab';
    const home = tempDir('m10-prev-');
    fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
    fs.writeFileSync(path.join(home, '.claude', 'settings.json'), '{}\n');
    const env = { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'data'), BELLWETHER_HOOK_TOKEN: T };
    const r = spawnBrainSync(['hooks', 'install'], { encoding: 'utf8', env });
    assert.equal(r.status, 0);
    assert.ok(!(r.stdout + r.stderr).includes(T), 'the `hooks install` preview prints the token (16 times); previews are meant to be shown and pasted');
  });
});

describe('licensing documents', () => {
  const scripts = path.join(AGENT_DIR, 'scripts', 'third-party.mjs');
  test('--check passes here, and also on a machine with no cargo registry (a fresh clone before the first build)', () => {
    assert.equal(spawnSync(process.execPath, [scripts, '--check'], { encoding: 'utf8' }).status, 0);
    const r = spawnSync(process.execPath, [scripts, '--check'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: tempDir('m10-nocargo-'), CARGO_HOME: '/nonexistent' } });
    assert.equal(r.status, 0, `--check failed with no cargo registry: ${r.stdout}${r.stderr}`);
  });
  test('NOTICE names the pinned commit and says the art is not included; THIRD-PARTY.md lists the five MPL-2.0 crates', () => {
    const n = fs.readFileSync(path.join(AGENT_DIR, 'NOTICE'), 'utf8');
    assert.match(n, /1ba33bebed993439f03e502b37e9bd2daf061773/);
    assert.match(n, /NOT included|not included/i);
    const t = fs.readFileSync(path.join(AGENT_DIR, 'THIRD-PARTY.md'), 'utf8');
    for (const c of ['cssparser', 'selectors', 'option-ext']) assert.match(t, new RegExp(c));
    const pins = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
    assert.ok(JSON.stringify(pins).includes('0e0cc153ec5795dd3c37a7094d2eb912b95544a58fcfb1b9a203707ea96329cf'));
  });
});

describe('the token mask (stdout, stderr, serve log, doctor, status, replay, crafted settings)', () => {
  const OLD = 'oldTokenCanary_ZZZZZZZZZZ9876';
  const CUR = 'currentTokenCanary_0123456789AB';
  const run = (file, args) => {
    const home = tempDir('m10-mask-');
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    return spawnBrainSync([...args, '--settings', file], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: home, BELLWETHER_DATA_DIR: path.join(home, 'd'), BELLWETHER_HOOK_TOKEN: CUR } });
    void cfg;
  };
  const stale = (headerKey, extra = '') => {
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    const f = path.join(tempDir('m10-set-'), 's.json');
    fs.writeFileSync(f, JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'http', url: `http://127.0.0.1:${cfg.port}/hook`, timeout: 1, headers: { [headerKey]: OLD, ...(extra ? { [extra]: OLD } : {}) } }] }] } }, null, 2) + '\n');
    return f;
  };
  test('the current token is never in a preview, a doctor run or an error, and neither is an old token under the canonical header name', () => {
    for (const args of [['hooks', 'install'], ['hooks', 'uninstall'], ['doctor'], ['doctor', '--json']]) {
      const r = run(stale('X-Bellwether-Token'), args);
      assert.ok(!(r.stdout + r.stderr).includes(OLD) && !(r.stdout + r.stderr).includes(CUR), args.join(' '));
    }
    const bad = path.join(tempDir('m10-bad-'), 's.json');
    fs.writeFileSync(bad, `{"hooks":{"Stop":[{"hooks":[{"headers":{"X-Bellwether-Token":"${OLD}" "oops":1}}]}]}}`);
    const r = run(bad, ['hooks', 'install']);
    assert.ok(!(r.stdout + r.stderr).includes(OLD), 'a JSON error message carried the old token');
  });
  test('an old token under a differently-cased header name, or repeated in another header, is masked too', () => {
    for (const key of ['x-bellwether-token', 'X-BELLWETHER-TOKEN']) {
      const r = run(stale(key), ['hooks', 'install']);
      assert.ok(!(r.stdout + r.stderr).includes(OLD), `old token printed in the diff under ${key}`);
    }
    const r = run(stale('X-Bellwether-Token', 'X-Other'), ['hooks', 'install']);
    assert.ok(!(r.stdout + r.stderr).includes(OLD), 'an old token repeated under another header name was printed');
  });
});
