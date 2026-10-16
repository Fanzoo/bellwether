#!/usr/bin/env node
/**
 * Release tooling (distribution plan D7, milestone 9). The release workflow
 * (.github/workflows/release.yml) runs these; each also runs by hand.
 *
 *   node scripts/release.mjs version 0.2.0-rc.1
 *        sets the version everywhere it lives (package.json, shell/package.json,
 *        tauri.conf.json, the three Cargo.toml files and their Cargo.lock entries,
 *        brain/util.mjs). Commit it, then tag `v0.2.0-rc.1`.
 *   node scripts/release.mjs check v0.2.0-rc.1
 *        refuses a release whose tag, versions, bundle id, update feed or updater key
 *        don't agree. Exit 1 with every problem listed.
 *   node scripts/release.mjs assets --app <Bellwether.app> --tag v0.2.0-rc.1 --out <dir>
 *        [--sign <identity SHA-1 or name> | --adhoc-dmg] [--notarize]
 *        [--base-url <url>] [--notes-file <file>]
 *        the release's files, from a built (and, for a release, notarized and stapled) app:
 *          Bellwether_<v>_<arch>.dmg          the app and an Applications link; signed with
 *                                             --sign, then with --notarize notarized,
 *                                             stapled and checked with spctl
 *          Bellwether_<v>_<arch>.app.tar.gz   the updater's archive, and its .sig (minisign,
 *                                             bound to the version), signed with the key in
 *                                             TAURI_SIGNING_PRIVATE_KEY or _PATH (and
 *                                             TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
 *          latest.json                        the updater's feed: darwin-aarch64 and
 *                                             darwin-x86_64 both point at a universal archive
 *          SHA256SUMS                         `shasum -a 256 -c SHA256SUMS` checks them all
 *        --base-url is where the files will be downloaded from (default the release's
 *        GitHub download URL; a test points it at a local server).
 *   node scripts/release.mjs cask --version 0.2.0-rc.1 (--sha256 <hex> | --sums <SHA256SUMS>)
 *        prints the Homebrew cask (packaging/homebrew/bellwether.rb) for that release.
 *
 * Nothing here prints a secret: the signing key and its password are read by the Tauri
 * CLI from the environment, never passed as arguments, and notarization goes through
 * build-app.mjs's `notarize` (key id and issuer never printed).
 */
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { maskTeam, notarize } from './build-app.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TAURI_CLI = path.join(ROOT, 'shell', 'node_modules', '.bin', 'tauri');
export const REPO = 'Fanzoo/bellwether';
export const PERMANENT_ID = 'com.fanzootechnology.bellwether';
export const CASK_TEMPLATE = path.join(ROOT, 'packaging', 'homebrew', 'bellwether.rb');

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-(0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?$/;

export function isVersion(v) {
  return SEMVER.test(String(v));
}

/** The files a version lives in, with how to read and write it. */
const JSON_FILES = ['package.json', 'shell/package.json', 'shell/src-tauri/tauri.conf.json'];
const CARGO_FILES = ['shell/src-tauri/Cargo.toml', 'crates/brain/Cargo.toml', 'crates/core/Cargo.toml'];
const LOCK = 'shell/src-tauri/Cargo.lock';
const LOCK_PACKAGES = ['bellwether-shell', 'bellwether-brain', 'bellwether-core'];
const NODE_VERSION = 'brain/util.mjs';

function cargoPackageVersion(text) {
  const pkg = /^\[package\]\n([\s\S]*?)(?=^\[)/m.exec(`${text}\n[`)?.[1] ?? '';
  return /^version = "([^"]*)"$/m.exec(pkg)?.[1] ?? null;
}

/** Every place's version, by file (the lock's three entries as `Cargo.lock:<name>`). */
export function versions(root = ROOT) {
  const read = (f) => fs.readFileSync(path.join(root, f), 'utf8');
  const out = {};
  for (const f of JSON_FILES) out[f] = JSON.parse(read(f)).version ?? null;
  for (const f of CARGO_FILES) out[f] = cargoPackageVersion(read(f));
  const lock = read(LOCK);
  for (const name of LOCK_PACKAGES) out[`${LOCK}:${name}`] = new RegExp(`\\[\\[package\\]\\]\\nname = "${name}"\\nversion = "([^"]*)"`).exec(lock)?.[1] ?? null;
  out[NODE_VERSION] = /^export const VERSION = '([^']*)';$/m.exec(read(NODE_VERSION))?.[1] ?? null;
  return out;
}

/** Writes `v` into every place `versions` reads. Only the version strings change. */
export function setVersion(v, root = ROOT) {
  if (!isVersion(v)) throw new Error(`${JSON.stringify(v)} is not a semantic version (like 0.2.0 or 0.2.0-rc.1)`);
  const edit = (f, fn) => {
    const file = path.join(root, f);
    const before = fs.readFileSync(file, 'utf8');
    const after = fn(before);
    if (after === before && !before.includes(v)) throw new Error(`${f}: version not found`);
    fs.writeFileSync(file, after);
  };
  for (const f of JSON_FILES) edit(f, (t) => t.replace(/^(\s*"version": )"[^"]*"/m, `$1"${v}"`));
  for (const f of CARGO_FILES) edit(f, (t) => t.replace(/^(\[package\]\n(?:(?!\[)[\s\S])*?^version = )"[^"]*"/m, `$1"${v}"`));
  edit(LOCK, (t) => LOCK_PACKAGES.reduce((acc, name) => acc.replace(new RegExp(`(\\[\\[package\\]\\]\\nname = "${name}"\\nversion = )"[^"]*"`), `$1"${v}"`), t));
  edit(NODE_VERSION, (t) => t.replace(/^export const VERSION = '[^']*';$/m, `export const VERSION = '${v}';`));
  const now = versions(root);
  const off = Object.entries(now).filter(([, x]) => x !== v);
  if (off.length) throw new Error(`not updated: ${off.map(([f]) => f).join(', ')}`);
  return now;
}

/** Everything that must agree before a tag is released; [] when it does. */
export function releaseProblems(tag, root = ROOT, repo = REPO) {
  const problems = [];
  const m = /^v(.+)$/.exec(String(tag));
  if (!m || !isVersion(m[1])) {
    problems.push(`the tag ${JSON.stringify(tag)} isn't v<semantic version> (like v0.2.0-rc.1)`);
    return problems;
  }
  const v = m[1];
  const off = Object.entries(versions(root)).filter(([, got]) => got !== v);
  if (off.length) problems.push(`the tag says ${v}, but ${off.map(([f, got]) => `${f} has ${got}`).join(', ')}. Run \`node scripts/release.mjs version ${v}\`, commit, and tag that commit`);
  const dist = JSON.parse(fs.readFileSync(path.join(root, 'shell', 'src-tauri', 'tauri.dist.conf.json'), 'utf8'));
  if (dist.identifier !== PERMANENT_ID) problems.push(`tauri.dist.conf.json's identifier is ${dist.identifier}, not ${PERMANENT_ID}`);
  const up = dist.plugins?.updater ?? {};
  const key = String(up.pubkey ?? '').trim();
  if (!key || key.startsWith('SET-ME')) problems.push('tauri.dist.conf.json has no updater public key: paste the .pub from `npx tauri signer generate` (docs/RELEASING.md)');
  else {
    const why = pubkeyProblem(key);
    if (why) problems.push(why);
  }
  const feed = `https://github.com/${repo}/releases/latest/download/latest.json`;
  if (JSON.stringify(up.endpoints) !== JSON.stringify([feed])) problems.push(`the updater endpoint must be exactly ${feed}`);
  if (up.requireSignedVersion !== true) problems.push('the updater must have requireSignedVersion: true');
  for (const f of secretKeyFiles(root)) problems.push(`${f} holds a minisign SECRET key: remove it from the repository (and from git history if it was ever committed), then make a new updater key, since this one must be treated as public`);
  return problems;
}

/**
 * What is wrong with the updater key pasted into tauri.dist.conf.json, or null. It must be
 * the .pub file's one line: base64 of `untrusted comment: minisign public key: <id>` and the
 * key. The secret key file (`updater.key`, "rsign encrypted secret key") has the same shape,
 * and pasting it would publish it, so it is refused by name.
 */
export function pubkeyProblem(key) {
  const text = Buffer.from(String(key).trim(), 'base64').toString('utf8');
  const [comment = '', body = ''] = text.split('\n');
  if (/secret key|encrypted/i.test(comment)) return 'tauri.dist.conf.json holds the updater\'s SECRET key (updater.key), not its public key: remove it before anything is committed, and paste the one line of updater.key.pub instead. If it was ever pushed, make a new key pair';
  if (!/^untrusted comment: minisign public key\b/.test(comment) || !/^[A-Za-z0-9+/]{40,}={0,2}$/.test(body.trim()) || text.split('\n').filter((l) => l.trim()).length !== 2) return 'the updater public key in tauri.dist.conf.json isn\'t a base64 minisign public key: paste the one line of updater.key.pub (its decoded first line says "minisign public key")';
  return null;
}

// "untrusted comment:" in base64, as a key file begins (updater.key is one base64 line).
const B64_COMMENT = /dW50cnVzdGVkIGNvbW1lbnQ6[A-Za-z0-9+/=]{20,}/g;

/** Whether one file's text carries a minisign secret key, plain or base64 (as updater.key is). */
export function hasSecretKey(text) {
  if (/^\s*untrusted comment: [^\n]*secret key\s*$/im.test(text)) return true;
  for (const m of String(text).matchAll(B64_COMMENT)) {
    const decoded = Buffer.from(m[0], 'base64').toString('utf8');
    if (/untrusted comment: [^\n]*secret key/i.test(decoded)) return true;
  }
  return false;
}

/**
 * Files in the repository holding a minisign secret key: those git tracks plus those it
 * would add (not ignored), or, outside a git checkout, every file but build output.
 */
export function secretKeyFiles(root = ROOT) {
  let files;
  const git = spawnSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8' });
  if (git.status === 0 && fs.existsSync(path.join(root, '.git'))) files = git.stdout.split('\0').filter(Boolean);
  else {
    files = [];
    const walk = (dir) => {
      for (const e of fs.readdirSync(path.join(root, dir), { withFileTypes: true })) {
        if (['.git', 'node_modules', 'target', 'target-dev', 'gen', 'data'].includes(e.name)) continue;
        const rel = path.join(dir, e.name);
        if (e.isDirectory()) walk(rel);
        else if (e.isFile()) files.push(rel);
      }
    };
    walk('');
  }
  return files.filter((f) => {
    try {
      const st = fs.statSync(path.join(root, f));
      return st.isFile() && st.size <= 8 * 1024 * 1024 && hasSecretKey(fs.readFileSync(path.join(root, f), 'utf8'));
    } catch {
      return false;
    }
  });
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

/** `SHA256SUMS`: `<hex>  <name>`, sorted by name, as `shasum -a 256` writes it. */
export function sha256sums(files) {
  return `${files
    .map((f) => [path.basename(f), sha256(f)])
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([n, h]) => `${h}  ${n}`)
    .join('\n')}\n`;
}

/** The DMG and archive's arch word, and latest.json's platform keys, from `lipo -archs`. */
export function archOf(lipoArchs) {
  const a = String(lipoArchs).trim().split(/\s+/).filter(Boolean);
  const arm = a.includes('arm64');
  const intel = a.includes('x86_64');
  if (arm && intel) return { word: 'universal', platforms: ['darwin-aarch64', 'darwin-x86_64'] };
  if (arm) return { word: 'aarch64', platforms: ['darwin-aarch64'] };
  if (intel) return { word: 'x64', platforms: ['darwin-x86_64'] };
  throw new Error(`no arm64 or x86_64 slice in the app (lipo says ${JSON.stringify(lipoArchs)})`);
}

/** latest.json, as tauri-plugin-updater reads it (plan Data model). */
export function latestJson({ version, notes, pubDate, url, signature, platforms }) {
  const entry = { signature: String(signature).trim(), url };
  return `${JSON.stringify({ version, notes, pub_date: pubDate, platforms: Object.fromEntries(platforms.map((p) => [p, entry])) }, null, 2)}\n`;
}

/** The cask for one release, from packaging/homebrew/bellwether.rb. */
export function renderCask(version, sha, template = fs.readFileSync(CASK_TEMPLATE, 'utf8')) {
  if (!isVersion(version)) throw new Error(`${JSON.stringify(version)} is not a semantic version`);
  if (!/^[0-9a-f]{64}$/.test(sha)) throw new Error('the sha256 must be 64 lower-case hex digits');
  if (!template.includes('version "0.0.0"') || !template.includes(`sha256 "${'0'.repeat(64)}"`)) throw new Error('the cask template must carry version "0.0.0" and a zero sha256');
  return template.replace('version "0.0.0"', `version "${version}"`).replace(`sha256 "${'0'.repeat(64)}"`, `sha256 "${sha}"`);
}

/** The DMG's sha256 from a SHA256SUMS text. */
export function dmgSha(sums, version) {
  const line = String(sums)
    .split('\n')
    .map((l) => /^([0-9a-f]{64})  (\S+)$/.exec(l))
    .filter(Boolean)
    .find((m) => m[2] === `Bellwether_${version}_universal.dmg`);
  if (!line) throw new Error(`SHA256SUMS has no line for Bellwether_${version}_universal.dmg`);
  return line[1];
}

function say(line) {
  process.stdout.write(`${maskTeam(line)}\n`);
}

function die(message) {
  process.stderr.write(`release: ${maskTeam(message)}\n`);
  process.exit(1);
}

function run(cmd, args, opts = {}) {
  say(`$ ${[cmd, ...args].map((a) => (/\s/.test(a) ? JSON.stringify(a) : a)).join(' ')}`);
  const r = spawnSync(cmd, args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...opts });
  const text = `${r.stdout ?? ''}${r.stderr ?? ''}`.trim();
  if (text) say(text);
  return { ok: r.status === 0, text };
}

function must(r, what) {
  if (!r.ok) die(`${what} failed`);
  return r;
}

function options(argv, known) {
  const o = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const k = a.replace(/^--/, '');
    if (!a.startsWith('--') || !(k in known)) die(`unknown option ${JSON.stringify(a)} (see the comment at the top of scripts/release.mjs)`);
    if (known[k] === 'flag') o[k] = true;
    else {
      if (argv[i + 1] === undefined) die(`${a} needs a value`);
      o[k] = argv[++i];
    }
  }
  return o;
}

function plistValue(app, key) {
  const r = spawnSync('/usr/bin/plutil', ['-extract', key, 'raw', '-o', '-', path.join(app, 'Contents', 'Info.plist')], { encoding: 'utf8' });
  return r.status === 0 ? r.stdout.trim() : null;
}

/** The updater's archive: `Bellwether.app/…` at the top (the plugin drops the first path part). */
function archive(app, file) {
  fs.rmSync(file, { force: true });
  // No AppleDouble files, extended attributes or ACLs: the signature is inside the bundle.
  must(run('/usr/bin/tar', ['--no-xattrs', '--no-acls', '--no-mac-metadata', '-czf', file, '-C', path.dirname(app), path.basename(app)], { env: { ...process.env, COPYFILE_DISABLE: '1' } }), 'tar');
}

function assets(argv) {
  const o = options(argv, { app: 'value', tag: 'value', out: 'value', sign: 'value', 'adhoc-dmg': 'flag', notarize: 'flag', 'base-url': 'value', 'notes-file': 'value' });
  for (const k of ['app', 'tag', 'out']) if (!o[k]) die(`--${k} is required`);
  if (o.sign && o['adhoc-dmg']) die('--sign and --adhoc-dmg exclude each other');
  if (o.notarize && !o.sign) die('--notarize needs --sign (a Developer ID Application identity)');
  const version = /^v(.+)$/.exec(o.tag)?.[1];
  if (!version || !isVersion(version)) die(`--tag ${o.tag} isn't v<semantic version>`);
  const app = path.resolve(o.app);
  if (path.basename(app) !== 'Bellwether.app' || !fs.existsSync(path.join(app, 'Contents', 'Info.plist'))) die(`${app} isn't a Bellwether.app`);
  const appVersion = plistValue(app, 'CFBundleShortVersionString');
  if (appVersion !== version) die(`the app is version ${appVersion}, the tag says ${version}`);
  if (!fs.existsSync(TAURI_CLI)) die('the Tauri CLI is missing: run `npm ci --prefix shell` once');
  if (!process.env.TAURI_SIGNING_PRIVATE_KEY && !process.env.TAURI_SIGNING_PRIVATE_KEY_PATH) die('set TAURI_SIGNING_PRIVATE_KEY (or _PATH) and TAURI_SIGNING_PRIVATE_KEY_PASSWORD: the updater archive must be signed');

  const exe = path.join(app, 'Contents', 'MacOS', plistValue(app, 'CFBundleExecutable') ?? 'Bellwether');
  const { word, platforms } = archOf(must(run('/usr/bin/lipo', ['-archs', exe]), 'lipo').text);
  const out = path.resolve(o.out);
  fs.mkdirSync(out, { recursive: true });
  const base = (o['base-url'] ?? `https://github.com/${process.env.GITHUB_REPOSITORY || REPO}/releases/download/${o.tag}/`).replace(/\/?$/, '/');
  const stem = `Bellwether_${version}_${word}`;

  // The DMG: the app and a link to /Applications, built with hdiutil (no Finder scripting).
  const dmg = path.join(out, `${stem}.dmg`);
  const stage = fs.mkdtempSync(path.join(os.tmpdir(), 'bellwether-dmg-'));
  fs.chmodSync(stage, 0o755); // the volume's root takes these modes
  try {
    must(run('/usr/bin/ditto', [app, path.join(stage, 'Bellwether.app')]), 'ditto');
    fs.symlinkSync('/Applications', path.join(stage, 'Applications'));
    fs.rmSync(dmg, { force: true });
    // hdiutil sometimes answers "Resource busy" on CI runners; a second try is enough.
    let made = false;
    for (let i = 0; i < 3 && !made; i++) made = run('/usr/bin/hdiutil', ['create', '-volname', 'Bellwether', '-srcfolder', stage, '-fs', 'HFS+', '-format', 'UDZO', '-ov', dmg]).ok;
    if (!made) die('hdiutil create failed three times');
  } finally {
    fs.rmSync(stage, { recursive: true, force: true });
  }
  if (o.sign) must(run('/usr/bin/codesign', ['--sign', o.sign, '--timestamp', '--force', dmg]), 'codesign (DMG)');
  else if (o['adhoc-dmg']) must(run('/usr/bin/codesign', ['--sign', '-', '--force', dmg]), 'codesign (DMG, ad hoc)');
  if (o.sign || o['adhoc-dmg']) must(run('/usr/bin/codesign', ['--verify', '-vvv', dmg]), 'codesign --verify (DMG)');
  if (o.notarize) {
    const r = notarize(dmg);
    if (!r.ok) die(r.message);
    must(run('/usr/bin/xcrun', ['stapler', 'staple', dmg]), 'stapler staple (DMG)');
    must(run('/usr/bin/xcrun', ['stapler', 'validate', dmg]), 'stapler validate (DMG)');
    must(run('/usr/sbin/spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', '-vvv', dmg]), 'spctl (DMG)');
  }
  must(run('/usr/bin/hdiutil', ['verify', dmg]), 'hdiutil verify');

  // The updater's archive and its signature, bound to this version.
  const tarball = path.join(out, `${stem}.app.tar.gz`);
  archive(app, tarball);
  fs.rmSync(`${tarball}.sig`, { force: true });
  // The key and its password come from the environment only (never argv, never printed).
  must(run(TAURI_CLI, ['signer', 'sign', '--app-version', version, tarball]), 'tauri signer sign');
  if (!fs.existsSync(`${tarball}.sig`)) die(`${tarball}.sig wasn't written`);

  const notes = o['notes-file'] ? fs.readFileSync(o['notes-file'], 'utf8').trim() : `Bellwether ${version}. Release notes: https://github.com/${process.env.GITHUB_REPOSITORY || REPO}/releases/tag/${o.tag}`;
  const feed = path.join(out, 'latest.json');
  fs.writeFileSync(feed, latestJson({ version, notes, pubDate: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'), url: `${base}${path.basename(tarball)}`, signature: fs.readFileSync(`${tarball}.sig`, 'utf8'), platforms }));

  const files = [dmg, tarball, `${tarball}.sig`, feed];
  fs.writeFileSync(path.join(out, 'SHA256SUMS'), sha256sums(files));
  must(run('/usr/bin/shasum', ['-a', '256', '-c', 'SHA256SUMS'], { cwd: out }), 'shasum -c');
  say(`release files for ${o.tag} (${word}${o.notarize ? ', DMG notarized and stapled' : o.sign ? ', DMG signed, not notarized' : o['adhoc-dmg'] ? ', DMG signed ad hoc: for testing only' : ', DMG unsigned: for testing only'}) in ${out}:`);
  for (const f of [...files, path.join(out, 'SHA256SUMS')]) say(`  ${path.basename(f)}  ${fs.statSync(f).size} bytes`);
}


function cask(argv) {
  const o = options(argv, { version: 'value', sha256: 'value', sums: 'value' });
  if (!o.version) die('--version is required');
  if (!!o.sha256 === !!o.sums) die('give one of --sha256 or --sums');
  const sha = o.sha256 ?? dmgSha(fs.readFileSync(o.sums, 'utf8'), o.version);
  process.stdout.write(renderCask(o.version, sha));
}

function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  try {
    if (cmd === 'version') {
      if (rest.length !== 1) die('usage: node scripts/release.mjs version <semantic version>');
      const now = setVersion(rest[0]);
      for (const f of Object.keys(now)) say(`${f}: ${rest[0]}`);
      say(`now commit, then tag: git tag v${rest[0]}`);
    } else if (cmd === 'check') {
      if (rest.length !== 1) die('usage: node scripts/release.mjs check <tag>');
      const problems = releaseProblems(rest[0], ROOT, process.env.GITHUB_REPOSITORY || REPO);
      if (problems.length) die(`${rest[0]} can't be released:\n  - ${problems.join('\n  - ')}`);
      say(`${rest[0]}: versions, bundle id, update feed and updater key agree`);
    } else if (cmd === 'assets') {
      assets(rest);
    } else if (cmd === 'cask') {
      cask(rest);
    } else {
      die('usage: node scripts/release.mjs version|check|assets|cask … (see the comment at the top of scripts/release.mjs)');
    }
  } catch (e) {
    die(e.message);
  }
}

// realpaths: run through a symlinked path (macOS /tmp and /var are links), this is still the main module.
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) main();
