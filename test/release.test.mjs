import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, tempDir } from './helpers.mjs';
import { archOf, dmgSha, hasSecretKey, isVersion, latestJson, pubkeyProblem, releaseProblems, renderCask, secretKeyFiles, setVersion, sha256sums, versions } from '../scripts/release.mjs';
import { notaryCredentials, updaterKeySet } from '../scripts/build-app.mjs';

/**
 * Milestone 9 (plan D7): the release workflow's secret handling, the release scripts, the
 * Homebrew cask. The workflows are read as text: what they may and may not contain is the
 * contract (no secret in ci.yml or on a pull request, write access only where no secret is,
 * every action pinned by SHA).
 */

const WF = path.join(AGENT_DIR, '.github', 'workflows');
const read = (...p) => fs.readFileSync(path.join(AGENT_DIR, ...p), 'utf8');
const workflow = (name) => fs.readFileSync(path.join(WF, name), 'utf8');

/** The workflow's jobs, by name: the text under `jobs:` at two-space indent. */
function jobs(text) {
  const body = text.slice(text.indexOf('\njobs:\n') + 7);
  const out = {};
  let name = null;
  for (const line of body.split('\n')) {
    const m = /^ {2}([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (m) {
      name = m[1];
      out[name] = '';
    } else if (name) out[name] += `${line}\n`;
  }
  return out;
}

/** The text before `jobs:`: triggers, top-level permissions, env. */
const head = (text) => text.slice(0, text.indexOf('\njobs:\n'));
const code = (text) => text.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n');
const secretsIn = (text) => [...new Set([...code(text).matchAll(/secrets\.([A-Z0-9_]+)/g)].map((m) => m[1]))].sort();

describe('the workflows keep secrets where D7 puts them', () => {
  const names = fs.readdirSync(WF).filter((f) => f.endsWith('.yml')).sort();

  test('three workflows: ci, release, tap', () => {
    assert.deepEqual(names, ['ci.yml', 'release.yml', 'tap.yml']);
  });

  test('every action is pinned by a full commit SHA, and no workflow uses pull_request_target or set -x', () => {
    for (const f of names) {
      const t = code(workflow(f));
      const uses = [...t.matchAll(/uses:\s*(\S+)/g)].map((m) => m[1]);
      assert.ok(uses.length > 0, f);
      for (const u of uses) assert.match(u, /^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/, `${f}: ${u} is not pinned by SHA`);
      assert.doesNotMatch(t, /pull_request_target/, f);
      assert.doesNotMatch(t, /set -[a-z]*x|set -o xtrace|bash -x/, f);
      assert.match(head(t), /\npermissions:\n {2}contents: read\n/, `${f}: default permissions are contents: read`);
    }
  });

  test('ci.yml: every pull request, no secret, no environment, no write access', () => {
    const t = code(workflow('ci.yml'));
    assert.match(head(t), /\n {2}pull_request:\n/);
    assert.deepEqual(secretsIn(t), []);
    assert.doesNotMatch(t, /environment:/);
    assert.doesNotMatch(t, /:\s*write\b/);
    assert.ok(jobs(t).bundle, 'the unsigned universal build (D7) runs on every pull request');
    assert.match(jobs(t).bundle, /build-app\.mjs --universal --adhoc/);
    assert.match(jobs(t).bundle, /unknown command "__fixture"/);
  });

  test('release.yml: a pushed v* tag only; secrets only in the job behind the release environment; write only where no secret is', () => {
    const t = code(workflow('release.yml'));
    assert.match(head(t), /\non:\n {2}push:\n {4}tags: \["v\*"\]\n/);
    assert.doesNotMatch(head(t), /pull_request|workflow_dispatch|branches|schedule|workflow_run/);
    const j = jobs(t);
    assert.deepEqual(Object.keys(j), ['build', 'publish']);
    assert.match(j.build, /\n {4}environment: release\n/);
    assert.match(j.build, /\n {4}permissions:\n {6}contents: read\n/);
    assert.deepEqual(secretsIn(j.build), ['APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_P8', 'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY', 'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD']);
    assert.match(j.publish, /\n {4}permissions:\n {6}contents: write\n/);
    assert.deepEqual(secretsIn(j.publish), [], 'the job that can write holds no secret');
    assert.doesNotMatch(j.publish, /environment:/);
    assert.match(j.publish, /sha256sum -c SHA256SUMS/);
    assert.match(j.publish, /gh release create "\$TAG" [^\n]*--draft --verify-tag/);
    assert.equal([...t.matchAll(/contents: write/g)].length, 1);
  });

  test('release.yml: a throwaway keychain with a random password, the Team ID masked, and everything deleted in an always() step', () => {
    const j = jobs(code(workflow('release.yml'))).build;
    assert.match(j, /kpass="\$\(openssl rand -base64 32\)"\n\s*echo "::add-mask::\$kpass"/);
    assert.match(j, /security create-keychain -p "\$kpass" "\$keychain"/);
    assert.match(j, /echo "::add-mask::\$team"/);
    assert.ok(j.indexOf('::add-mask::$team') < j.indexOf('build-app.mjs'), 'the Team ID is masked before anything signs');
    assert.match(j, /rm -f "\$cert"/);
    const cleanup = j.slice(j.indexOf('- name: Delete the keychain and the key files'));
    assert.match(cleanup, /if: always\(\)/);
    assert.match(cleanup, /security delete-keychain "\$BELLWETHER_KEYCHAIN"/);
    assert.match(cleanup, /rm -f "\$RUNNER_TEMP\/certificate\.p12" "\$RUNNER_TEMP\/notary-key\.p8"/);
    // A secret is only ever handed to a step through env, never echoed or put on a line of its own.
    for (const line of j.split('\n')) if (/secrets\./.test(line)) assert.match(line, /^ {10}[A-Z0-9_]+: \$\{\{ secrets\.[A-Z0-9_]+ \}\}$/, line);
    assert.doesNotMatch(j, /echo[^\n]*\$(APPLE_|TAURI_SIGNING)/);
  });

  test('release.yml: the tag must be on main before any secret, and the signing job\'s inputs are pinned', () => {
    const j = jobs(code(workflow('release.yml'))).build;
    const gate = j.indexOf('- name: The tagged commit is on main');
    assert.ok(gate > 0 && gate < j.indexOf('secrets.'), 'the main check runs before any step that holds a secret');
    assert.match(j, /fetch-depth: 0/);
    assert.match(j, /git merge-base --is-ancestor "\$commit" refs\/remotes\/origin\/main/);
    assert.match(j, /TAG: \$\{\{ github\.ref_name \}\}\n\s+run: \|\n\s+set -eu/, 'the tag reaches the shell only through env');
    assert.doesNotMatch(j, /\$\{\{[^}]*\}\}[^\n]*\n?[^\n]*git (rev-parse|merge-base)/);
    assert.match(j, /\n {4}runs-on: macos-15\n/);
    assert.match(code(workflow('release.yml')), /RUST_TOOLCHAIN: "1\.\d+\.\d+"/);
    assert.doesNotMatch(j, /stable|macos-latest|--no-package-lock|npm install/);
    assert.match(j, /npm ci --prefix shell/);
    const lock = JSON.parse(read('shell', 'package-lock.json'));
    assert.equal(lock.packages['node_modules/@tauri-apps/cli'].version, JSON.parse(read('shell', 'package.json')).devDependencies['@tauri-apps/cli']);
    for (const [k, v] of Object.entries(lock.packages)) if (k) assert.match(v.integrity, /^sha512-/, k);
  });

  test('tap.yml: on a full (not pre-) release only, one secret, its own environment', () => {
    const t = code(workflow('tap.yml'));
    assert.match(head(t), /\non:\n {2}release:\n {4}types: \[released\]\n/);
    assert.match(t, /if: startsWith\(github\.event\.release\.tag_name, 'v'\) && !github\.event\.release\.prerelease && !github\.event\.release\.draft\n/);
    assert.doesNotMatch(head(t), /pull_request|push:/);
    const j = jobs(t);
    assert.deepEqual(Object.keys(j), ['cask']);
    assert.match(j.cask, /\n {4}environment: homebrew-tap\n/);
    assert.deepEqual(secretsIn(t), ['TAP_GITHUB_TOKEN']);
    assert.doesNotMatch(t, /:\s*write\b/);
    assert.match(j.cask, /sha256sum -c --ignore-missing SHA256SUMS/);
  });
});

describe('scripts/release.mjs', () => {
  test('the version is the same everywhere it lives', () => {
    const v = versions();
    assert.equal(Object.keys(v).length, 10);
    assert.equal(new Set(Object.values(v)).size, 1, JSON.stringify(v));
    assert.ok(isVersion(Object.values(v)[0]));
  });

  test('`version` changes only the version strings, in every file', () => {
    const root = tempDir();
    const files = ['package.json', 'shell/package.json', 'shell/src-tauri/tauri.conf.json', 'shell/src-tauri/Cargo.toml', 'crates/brain/Cargo.toml', 'crates/core/Cargo.toml', 'shell/src-tauri/Cargo.lock', 'brain/util.mjs', 'shell/src-tauri/tauri.dist.conf.json'];
    for (const f of files) {
      fs.mkdirSync(path.dirname(path.join(root, f)), { recursive: true });
      fs.copyFileSync(path.join(AGENT_DIR, f), path.join(root, f));
    }
    const was = Object.values(versions(root))[0];
    const now = setVersion('0.2.0-rc.1', root);
    assert.ok(Object.values(now).every((v) => v === '0.2.0-rc.1'));
    for (const f of files) {
      const before = read(f).split('\n');
      const after = fs.readFileSync(path.join(root, f), 'utf8').split('\n');
      assert.equal(after.length, before.length, f);
      const changed = before.map((l, i) => [l, after[i]]).filter(([a, b]) => a !== b);
      for (const [a, b] of changed) assert.equal(a.replace(was, '0.2.0-rc.1'), b, `${f}: only the version may change`);
    }
    assert.throws(() => setVersion('0.2', root), /not a semantic version/);
    // The tag check: the versions now agree with v0.2.0-rc.1, the placeholder key doesn't.
    const problems = releaseProblems('v0.2.0-rc.1', root);
    assert.deepEqual(problems.length, 1, problems.join('\n'));
    assert.match(problems[0], /no updater public key/);
    assert.match(releaseProblems('v0.2.0-rc.2', root)[0], /the tag says 0\.2\.0-rc\.2, but package\.json has 0\.2\.0-rc\.1/);
    assert.match(releaseProblems('0.2.0', root)[0], /isn't v<semantic version>/);
    // With a minisign public key in place the check passes.
    const dist = JSON.parse(fs.readFileSync(path.join(root, 'shell/src-tauri/tauri.dist.conf.json'), 'utf8'));
    dist.plugins.updater.pubkey = Buffer.from('untrusted comment: minisign public key: 0123456789ABCDEF\nRWTbFnn95HDssSP6fw6ObUGPZhfItnBBkUtpVDirMTkvyp4ckge5Mx1j\n').toString('base64');
    fs.writeFileSync(path.join(root, 'shell/src-tauri/tauri.dist.conf.json'), JSON.stringify(dist));
    assert.deepEqual(releaseProblems('v0.2.0-rc.1', root), []);
    assert.ok(updaterKeySet(dist));
    dist.plugins.updater.requireSignedVersion = false;
    dist.plugins.updater.endpoints = ['https://example.com/latest.json'];
    fs.writeFileSync(path.join(root, 'shell/src-tauri/tauri.dist.conf.json'), JSON.stringify(dist));
    assert.equal(releaseProblems('v0.2.0-rc.1', root).length, 2);
    fs.rmSync(root, { recursive: true, force: true });
  });

  test('the repository carries the placeholder key, so no build updates itself until the maintainer sets one', () => {
    const dist = JSON.parse(read('shell', 'src-tauri', 'tauri.dist.conf.json'));
    assert.equal(updaterKeySet(dist), dist.plugins.updater.pubkey.startsWith('SET-ME') ? false : true);
    assert.deepEqual(dist.plugins.updater.endpoints, ['https://github.com/Fanzoo/bellwether/releases/latest/download/latest.json']);
    assert.equal(dist.plugins.updater.requireSignedVersion, true);
  });

  test('latest.json: both Mac platform keys for a universal build, the same archive and signature', () => {
    assert.deepEqual(archOf('x86_64 arm64\n'), { word: 'universal', platforms: ['darwin-aarch64', 'darwin-x86_64'] });
    assert.deepEqual(archOf('arm64'), { word: 'aarch64', platforms: ['darwin-aarch64'] });
    assert.throws(() => archOf('i386'), /no arm64 or x86_64/);
    const j = JSON.parse(latestJson({ version: '0.2.0-rc.1', notes: 'n', pubDate: '2026-10-15T00:00:00Z', url: 'https://x/B.app.tar.gz', signature: 'c2ln\n', platforms: archOf('x86_64 arm64').platforms }));
    assert.deepEqual(Object.keys(j), ['version', 'notes', 'pub_date', 'platforms']);
    assert.deepEqual(j.platforms['darwin-aarch64'], { signature: 'c2ln', url: 'https://x/B.app.tar.gz' });
    assert.deepEqual(j.platforms['darwin-x86_64'], j.platforms['darwin-aarch64']);
  });

  test('SHA256SUMS is what `shasum -a 256 -c` reads, and the cask takes the DMG line', () => {
    const dir = tempDir();
    fs.writeFileSync(path.join(dir, 'Bellwether_0.2.0_universal.dmg'), 'dmg');
    fs.writeFileSync(path.join(dir, 'latest.json'), '{}');
    const sums = sha256sums([path.join(dir, 'latest.json'), path.join(dir, 'Bellwether_0.2.0_universal.dmg')]);
    assert.match(sums, /^[0-9a-f]{64} {2}Bellwether_0\.2\.0_universal\.dmg\n[0-9a-f]{64} {2}latest\.json\n$/, 'sorted by name, two spaces');
    assert.ok(sums.includes('44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a  latest.json'), 'sha256 of "{}"');
    assert.equal(dmgSha(sums, '0.2.0'), sums.slice(0, 64));
    assert.throws(() => dmgSha(sums, '0.3.0'), /no line for Bellwether_0\.3\.0_universal\.dmg/);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test('notarization never prints the API key id or issuer', () => {
    const c = notaryCredentials({ APPLE_API_KEY_PATH: '/k/AuthKey.p8', APPLE_API_KEY: 'KEYID12345', APPLE_API_ISSUER: '00000000-1111-2222-3333-444444444444' });
    assert.deepEqual(c.args, ['--key', '/k/AuthKey.p8', '--key-id', 'KEYID12345', '--issuer', '00000000-1111-2222-3333-444444444444']);
    assert.doesNotMatch(`${c.shown.join(' ')} ${c.hint}`, /KEYID12345|00000000-1111/);
    assert.match(notaryCredentials({ APPLE_API_KEY: 'x' }).error, /go together/);
    assert.deepEqual(notaryCredentials({}).args, ['--keychain-profile', 'bellwether']);
  });
});

describe('the Homebrew cask', () => {
  const template = read('packaging', 'homebrew', 'bellwether.rb');

  test('the stanzas D7 lists: the app, the CLI on PATH, auto_updates, quit and zap, the hooks caveat', () => {
    assert.match(template, /^ {2}app "Bellwether\.app"$/m);
    assert.match(template, /^ {2}binary "#\{appdir\}\/Bellwether\.app\/Contents\/MacOS\/Bellwether", target: "bellwether"$/m);
    assert.match(template, /^ {2}auto_updates true$/m);
    assert.match(template, /quit: +"com\.fanzootechnology\.bellwether"/);
    assert.match(template, /launchctl: "com\.fanzootechnology\.bellwether\.brain"/);
    for (const p of ['~/Library/Application Support/Bellwether', '~/Library/Logs/Bellwether', '~/Library/Preferences/com.fanzootechnology.bellwether.plist']) assert.ok(template.includes(`"${p}"`), p);
    assert.match(template, /bellwether hooks uninstall --write/);
    assert.match(template, /depends_on macos: :ventura/);
    assert.match(template, /url "https:\/\/github\.com\/Fanzoo\/bellwether\/releases\/download\/v#\{version\}\/Bellwether_#\{version\}_universal\.dmg"/);
  });

  test('rendering fills in the version and sha256 and nothing else', () => {
    const sha = 'a'.repeat(64);
    const out = renderCask('0.2.0-rc.1', sha, template);
    const diff = template.split('\n').map((l, i) => [l, out.split('\n')[i]]).filter(([a, b]) => a !== b);
    assert.deepEqual(diff, [['  version "0.0.0"', '  version "0.2.0-rc.1"'], [`  sha256 "${'0'.repeat(64)}"`, `  sha256 "${sha}"`]]);
    assert.throws(() => renderCask('0.2.0', 'A'.repeat(64), template), /lower-case hex/);
    assert.throws(() => renderCask('latest', sha, template), /not a semantic version/);
  });
});

describe('signing material stays out of git', () => {
  const b64 = (t) => Buffer.from(t).toString('base64');
  const SECRET = b64(['untrusted comment', ' rsign encrypted secret key\nRWRTY0IyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n'].join(':'));
  const PUBLIC = b64('untrusted comment: minisign public key: 0123456789ABCDEF\nRWTbFnn95HDssSP6fw6ObUGPZhfItnBBkUtpVDirMTkvyp4ckge5Mx1j\n');

  test('no file in the repository holds a minisign secret key', () => {
    assert.deepEqual(secretKeyFiles(AGENT_DIR), []);
  });

  test('the scan finds a secret key file, plain or base64, and never a public key', () => {
    assert.ok(hasSecretKey(SECRET));
    assert.ok(hasSecretKey(`{"pubkey": "${SECRET}"}`));
    assert.ok(hasSecretKey(Buffer.from(SECRET, 'base64').toString()));
    assert.ok(!hasSecretKey(PUBLIC) && !hasSecretKey(Buffer.from(PUBLIC, 'base64').toString()));
    const root = tempDir();
    fs.mkdirSync(path.join(root, 'a', 'b'), { recursive: true });
    fs.writeFileSync(path.join(root, 'a', 'b', 'updater.key'), SECRET);
    fs.writeFileSync(path.join(root, 'updater.key.pub'), PUBLIC);
    assert.deepEqual(secretKeyFiles(root), [path.join('a', 'b', 'updater.key')]);
    fs.rmSync(root, { recursive: true, force: true });
  });

  test('the secret key pasted as the public key is refused by name; the public key is accepted', () => {
    assert.equal(pubkeyProblem(PUBLIC), null);
    assert.match(pubkeyProblem(SECRET), /SECRET key \(updater\.key\), not its public key/);
    assert.match(pubkeyProblem(b64('untrusted comment: something else\nRWTbFnn95HDssSP6fw6ObUGPZhfItnBBkUtpVDirMTkvyp4ckge5Mx1j\n')), /minisign public key/);
  });

  test('.gitignore covers the updater key, Apple certificates and keys, keychains', () => {
    const ignore = read('.gitignore').split('\n');
    for (const p of ['*.key', '*.key.pub', '*.p12', '*.p8', '*.pem', '*.keychain-db', '.env']) assert.ok(ignore.includes(p), p);
  });
});
