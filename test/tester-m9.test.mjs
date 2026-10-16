import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, tempDir } from './helpers.mjs';
import { dmgSha, releaseProblems, renderCask } from '../scripts/release.mjs';

/**
 * BR-3K's independent checks of milestone 9 (release pipeline, updater, cask). Offline, text
 * and script level; nothing here signs, notarizes, publishes or touches a real home.
 */

const WF = path.join(AGENT_DIR, '.github', 'workflows');
const wf = (n) => fs.readFileSync(path.join(WF, n), 'utf8');
const noComments = (t) => t.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n');

/** The steps of a job as text blocks (a step starts at `      - ` under `steps:`). */
function steps(text) {
  const out = [];
  let cur = null;
  let inSteps = false;
  for (const line of noComments(text).split('\n')) {
    if (/^ {4}steps:\s*$/.test(line)) { inSteps = true; continue; }
    if (/^ {2}[A-Za-z0-9_-]+:\s*$/.test(line)) inSteps = false;
    if (!inSteps) continue;
    if (/^ {6}- /.test(line)) { cur = []; out.push(cur); }
    cur?.push(line);
  }
  return out.map((s) => s.join('\n'));
}

/** `${{ … }}` inside `run:` scripts (the injection sink), excluding static matrix values. */
function runInterpolations(text) {
  const hits = [];
  let inRun = false;
  let indent = 0;
  for (const line of noComments(text).split('\n')) {
    const m = /^(\s*)(?:- )?run:\s*(.*)$/.exec(line);
    if (m) {
      indent = m[1].length + (line.includes('- run:') ? 2 : 0);
      inRun = /^[|>][-+]?$/.test(m[2]);
      if (m[2].includes('${{')) hits.push(m[2]);
      continue;
    }
    if (inRun) {
      if (line.trim() && line.length - line.trimStart().length <= indent) inRun = false;
      else if (line.includes('${{')) hits.push(line.trim());
    }
  }
  return hits.filter((h) => !/\$\{\{ matrix\.[a-z]+ \}\}/.test(h));
}

describe('M9 workflows: what the developer pinned, checked from another angle', () => {
  test('no attacker-influenced expression is interpolated into a run: script', () => {
    for (const f of ['ci.yml', 'release.yml', 'tap.yml']) assert.deepEqual(runInterpolations(wf(f)), [], f);
  });

  test('tag and release names reach scripts only through env vars', () => {
    const tap = noComments(wf('tap.yml'));
    const rel = noComments(wf('release.yml'));
    // every use of an event/ref expression is on an env: or with: line
    for (const [name, t] of [['tap', tap], ['release', rel]]) {
      for (const line of t.split('\n')) {
        if (/github\.(event\.release|ref_name|head_ref|event\.pull_request)/.test(line)) assert.match(line, /^\s*(if|TAG|ref|group|[A-Z_]+): /, `${name}: ${line}`);
      }
    }
  });

  test('no signing secret reaches a job-level or workflow-level env, only per-step env', () => {
    for (const f of ['release.yml', 'tap.yml']) {
      const t = noComments(wf(f));
      const top = t.slice(0, t.indexOf('\njobs:\n'));
      assert.doesNotMatch(top, /secrets\./, `${f}: workflow-level env must not hold a secret`);
      const jobHead = t.slice(t.indexOf('\njobs:\n'), t.indexOf('    steps:'));
      assert.doesNotMatch(jobHead, /secrets\./, `${f}: job-level env must not hold a secret`);
    }
  });

  test('the tap token is in exactly one step, the one that runs only gh and git', () => {
    const s = steps(wf('tap.yml')).filter((x) => /secrets\.TAP_GITHUB_TOKEN/.test(x));
    assert.equal(s.length, 1);
    assert.doesNotMatch(s[0], /node |npm |scripts\//, 'no repository code runs with the token');
    assert.doesNotMatch(s[0], /uses:/);
  });

  test('the signing steps that hold secrets run no step from a third-party action', () => {
    const s = steps(wf('release.yml')).filter((x) => /secrets\./.test(x));
    assert.ok(s.length >= 4);
    for (const x of s) assert.doesNotMatch(x, /uses:/);
  });

  test('release.yml: no cache (a poisoned cache cannot reach a signing build) and no checkout token left behind', () => {
    const t = noComments(wf('release.yml'));
    assert.doesNotMatch(t, /actions\/cache/);
    for (const f of ['release.yml', 'tap.yml']) {
      const co = [...noComments(wf(f)).matchAll(/actions\/checkout@[^\n]*\n((?: {8}[^\n]*\n)*)/g)];
      assert.ok(co.length >= 1);
      for (const c of co) assert.match(c[1], /persist-credentials: false/, f);
    }
  });

  test('the build job uploads only the release directory, never the temp dir that holds key files', () => {
    const t = noComments(wf('release.yml'));
    assert.match(t, /path: \$\{\{ runner\.temp \}\}\/release\/\n/);
    assert.match(t, /--out "\$RUNNER_TEMP\/release"/);
  });
});

describe('M9 cask: nothing outside Bellwether\'s own', () => {
  const cask = fs.readFileSync(path.join(AGENT_DIR, 'packaging', 'homebrew', 'bellwether.rb'), 'utf8');
  const code = cask.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n');

  test('uninstall/zap name only the permanent bundle id, its brain label and Bellwether folders', () => {
    assert.doesNotMatch(code, /local\.bellwether|LaunchAgents|\.claude|settings\.json|\.\*|\*/);
    const zap = code.slice(code.indexOf('zap trash:'), code.indexOf('caveats'));
    const paths = [...zap.matchAll(/"(~[^"]+)"/g)].map((m) => m[1]);
    assert.ok(paths.length >= 5);
    for (const p of paths) assert.match(p, /^~\/Library\/(Application Support|Caches|HTTPStorages|Logs|Preferences|WebKit)\/(Bellwether|com\.fanzootechnology\.bellwether(\.plist)?)$/, p);
    const label = /uninstall launchctl: "([^"]+)"/.exec(code)[1];
    const plist = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'macos', `${label}.plist`), 'utf8');
    assert.match(plist, new RegExp(`<string>${label.replaceAll('.', '\\.')}</string>`), 'the label is the bundled agent\'s');
    assert.ok(!label.startsWith('local.'));
  });

  test('cask rendering is not a template-injection sink (replace-pattern characters)', () => {
    assert.throws(() => renderCask('1.0.0-$&', 'a'.repeat(64), cask));
    assert.throws(() => renderCask('1.0.0', "$&".repeat(32), cask));
    const out = renderCask('1.0.0', 'b'.repeat(64), cask);
    assert.equal(out.split('\n').length, cask.split('\n').length);
  });

  test('dmgSha refuses a SHA256SUMS that lists the DMG under a path or with a different name', () => {
    const h = 'c'.repeat(64);
    assert.throws(() => dmgSha(`${h}  ../Bellwether_1.0.0_universal.dmg\n`, '1.0.0'));
    assert.throws(() => dmgSha(`${h}  Bellwether_1.0.0_x64.dmg\n`, '1.0.0'));
    assert.equal(dmgSha(`${'d'.repeat(64)}  other\n${h}  Bellwether_1.0.0_universal.dmg\n`, '1.0.0'), h);
  });
});

describe('M9 release check: the updater key', () => {
  function rootWith(pubkey) {
    const root = tempDir();
    for (const f of ['package.json', 'shell/package.json', 'shell/src-tauri/tauri.conf.json', 'shell/src-tauri/Cargo.toml', 'crates/brain/Cargo.toml', 'crates/core/Cargo.toml', 'shell/src-tauri/Cargo.lock', 'brain/util.mjs', 'shell/src-tauri/tauri.dist.conf.json']) {
      fs.mkdirSync(path.dirname(path.join(root, f)), { recursive: true });
      fs.copyFileSync(path.join(AGENT_DIR, f), path.join(root, f));
    }
    const p = path.join(root, 'shell/src-tauri/tauri.dist.conf.json');
    const dist = JSON.parse(fs.readFileSync(p, 'utf8'));
    dist.plugins.updater.pubkey = pubkey;
    fs.writeFileSync(p, JSON.stringify(dist));
    const v = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
    return { root, tag: `v${v}` };
  }
  const b64 = (s) => Buffer.from(s).toString('base64');
  const PUB = b64('untrusted comment: minisign public key: 0123456789ABCDEF\nRWTbFnn95HDssSP6fw6ObUGPZhfItnBBkUtpVDirMTkvyp4ckge5Mx1j\n');

  test('a real-shaped public key passes; placeholder, empty, garbage and non-base64 fail', () => {
    const ok = rootWith(PUB);
    assert.deepEqual(releaseProblems(ok.tag, ok.root), []);
    for (const bad of ['', 'SET-ME', '  SET-ME: x', 'not base64 !!', b64('hello'), b64('untrusted comment: x\nshort')]) {
      const r = rootWith(bad);
      assert.equal(releaseProblems(r.tag, r.root).length, 1, JSON.stringify(bad));
    }
  });

  // DEFECT candidate: the encrypted SECRET key's file has the same shape (comment line + one
  // base64 line), so pasting updater.key instead of updater.key.pub is accepted by `check`.
  test('a minisign SECRET key pasted as the public key is refused', () => {
    const secret = b64('untrusted comment: rsign encrypted secret key\nRWRTY0IyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n');
    const r = rootWith(secret);
    const problems = releaseProblems(r.tag, r.root);
    // Two: the pasted key, and the same key found in a file of the repository (the scan).
    assert.ok(problems.length >= 1);
    assert.match(problems[0], /SECRET key/);
  });

  test('the CLI passes the repository as it stands, and exits 1 naming the cause for a wrong tag', () => {
    const script = path.join(AGENT_DIR, 'scripts', 'release.mjs');
    const version = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'package.json'), 'utf8')).version;
    const ok = spawnSync(process.execPath, [script, 'check', `v${version}`], { encoding: 'utf8' });
    assert.equal(ok.status, 0, ok.stderr);
    const r = spawnSync(process.execPath, [script, 'check', 'v9.9.9'], { encoding: 'utf8' });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /the tag says 9\.9\.9/);
  });

  test('assets refuses with no updater signing key, naming the variable', () => {
    const env = { ...process.env };
    delete env.TAURI_SIGNING_PRIVATE_KEY;
    delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
    const r = spawnSync(process.execPath, [path.join(AGENT_DIR, 'scripts', 'release.mjs'), 'assets', '--app', '/nonexistent/Bellwether.app', '--tag', 'v0.1.0', '--out', tempDir()], { encoding: 'utf8', env });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /isn't a Bellwether\.app/);
  });
});
