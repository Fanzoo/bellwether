#!/usr/bin/env node
/**
 * Builds the signed Bellwether.app (distribution plan, milestone 1): `tauri build` with
 * the distribution overlay (shell/src-tauri/tauri.dist.conf.json: the permanent bundle
 * id, the main binary `Bellwether`, the hardened runtime, the one entitlement, the
 * brain's login-agent plist), signed with a Developer ID Application certificate from
 * the login keychain; then, with --notarize, notarized with notarytool and stapled.
 *
 *   node scripts/build-app.mjs                     build and sign
 *   node scripts/build-app.mjs --notarize          … then notarize, staple and check
 *   node scripts/build-app.mjs --adhoc             sign ad hoc (no keychain), still with the
 *        hardened runtime and the entitlements, under its own id
 *        (com.fanzootechnology.bellwether.adhoc): for proving things locally only.
 *        It writes to the same place, replacing a signed build there.
 *   node scripts/build-app.mjs --test-port 47418 --test-home <dir>
 *        a test build: the bundled brain agent runs `serve --port 47418` with
 *        BELLWETHER_HOME=<dir>, so it can't collide with a brain already on 47417.
 *        Never ship one.
 *   node scripts/build-app.mjs --universal       Apple silicon and Intel in one binary
 *        (decision 3; needs `rustup target add x86_64-apple-darwin aarch64-apple-darwin`).
 *        The release workflow builds this way.
 *   node scripts/build-app.mjs --adhoc --debug --test-port <p> --test-home <dir>
 *        --updater-pubkey <file.pub>
 *        a debug test bundle whose updater trusts a throwaway key: how the updater is
 *        tested against a local latest.json (milestone 9; the endpoint override exists in
 *        debug builds only). --debug needs --adhoc and both test flags.
 *
 * Everything goes to shell/src-tauri/target-dev/ (the app in release/bundle/macos/, or
 * universal-apple-darwin/release/… and debug/… for the options above); target/ (the
 * repository strip's login-item binary) is never written, and the script checks that
 * afterwards. The generated config lives in target-dev/bundle-config/.
 *
 * The app left by an earlier build is deleted first, so a build without the test flags
 * never leaves a test build behind; afterwards the bundled agent plist is checked against
 * the flags given, and the last line says which kind of build it is.
 *
 * Signing identity: BELLWETHER_SIGNING_IDENTITY (a name or SHA-1), else the only
 * "Developer ID Application" identity in the keychain. Notarization: an App Store Connect
 * API key when APPLE_API_KEY_PATH, APPLE_API_KEY and APPLE_API_ISSUER are all set (the
 * release workflow), else the notarytool keychain profile BELLWETHER_NOTARY_PROFILE
 * (default "bellwether"), made once with `xcrun notarytool store-credentials`. The Team
 * ID is never written to a file, and is masked in everything this script prints; the key
 * id and issuer are never printed.
 */
import { execFileSync, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TAURI_DIR = path.join(AGENT, 'shell', 'src-tauri');
const TARGET_DEV = path.join(TAURI_DIR, 'target-dev');
const OUT_DIR = path.join(TARGET_DEV, 'bundle-config');
const LIVE_BIN = path.join(TAURI_DIR, 'target', 'release', 'bellwether-shell');
const UNIVERSAL = 'universal-apple-darwin';

/** Where `tauri build` leaves the app for these options. */
export function appPath(o) {
  return path.join(TARGET_DEV, ...(o.universal ? [UNIVERSAL] : []), o.debug ? 'debug' : 'release', 'bundle', 'macos', 'Bellwether.app');
}
const TAURI_CLI = path.join(AGENT, 'shell', 'node_modules', '.bin', 'tauri');

/** "(ABCDE12345)" after a certificate name, and TeamIdentifier=… lines: masked. */
export function maskTeam(text) {
  return String(text).replace(/\(([A-Z0-9]{10})\)/g, '(TEAM ID)').replace(/(TeamIdentifier=|team(?:ID| id)?[:=]\s*)[A-Z0-9]{10}\b/gi, '$1(TEAM ID)');
}

/** The test port and home a bundled agent plist carries (a release build carries neither). */
export function testFlags(plist) {
  const strings = [...String(plist).matchAll(/<string>([^<]*)<\/string>/g)].map((m) => m[1]);
  const i = strings.indexOf('--port');
  const port = i >= 0 ? Number(strings[i + 1]) : null;
  const home = /<key>BELLWETHER_HOME<\/key>\s*<string>([^<]*)<\/string>/.exec(String(plist))?.[1]?.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&') ?? null;
  return { port, home };
}

/** Whether the built bundle's plist matches the flags the build was given. */
export function bundleMatches(plist, o) {
  const f = testFlags(plist);
  return f.port === o.testPort && f.home === o.testHome;
}

/** Whether the overlay carries a real updater public key (not the SET-ME placeholder). */
export function updaterKeySet(overlay) {
  const key = String(overlay?.plugins?.updater?.pubkey ?? '').trim();
  return key !== '' && !key.startsWith('SET-ME');
}

function fail(message) {
  process.stderr.write(`build-app: ${maskTeam(message)}\n`);
  process.exit(1);
}

function say(line) {
  process.stdout.write(`${maskTeam(line)}\n`);
}

function parse(argv) {
  const o = { notarize: false, adhoc: false, testPort: null, testHome: null, universal: false, debug: false, updaterPubkey: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--notarize') o.notarize = true;
    else if (a === '--adhoc') o.adhoc = true;
    else if (a === '--universal') o.universal = true;
    else if (a === '--debug') o.debug = true;
    else if (a === '--updater-pubkey') o.updaterPubkey = argv[++i] ?? '';
    else if (a === '--test-port') o.testPort = Number(argv[++i]);
    else if (a === '--test-home') o.testHome = argv[++i];
    else fail(`unknown option ${JSON.stringify(a)} (see the comment at the top of scripts/build-app.mjs)`);
  }
  if (o.testPort !== null && !(Number.isInteger(o.testPort) && o.testPort >= 1024 && o.testPort <= 65535)) fail('--test-port takes an integer from 1024 to 65535');
  if (o.testPort === 47417) fail('--test-port 47417 is the live brain\'s port; a test build must use another');
  if (o.adhoc && o.notarize) fail('an ad-hoc build cannot be notarized');
  if (o.testHome !== null && !o.testHome) fail('--test-home needs a directory');
  if (o.testHome) o.testHome = path.resolve(o.testHome);
  if (o.debug && !(o.adhoc && o.testPort !== null && o.testHome !== null)) fail('--debug builds a test bundle only: it needs --adhoc, --test-port and --test-home');
  if (o.updaterPubkey !== null && !o.adhoc) fail('--updater-pubkey is for ad-hoc test builds; a release build trusts the key in tauri.dist.conf.json');
  if (o.updaterPubkey === '') fail('--updater-pubkey needs a file (the .pub that `tauri signer generate` writes)');
  return o;
}

/**
 * notarytool's credentials: the API key from the environment (all three variables), else
 * the keychain profile. `shown` is what gets printed: never the key id or the issuer.
 */
export function notaryCredentials(env = process.env) {
  const { APPLE_API_KEY_PATH: keyPath, APPLE_API_KEY: keyId, APPLE_API_ISSUER: issuer } = env;
  const set = [keyPath, keyId, issuer].filter(Boolean).length;
  if (set === 3) return { args: ['--key', keyPath, '--key-id', keyId, '--issuer', issuer], shown: ['--key', '(API key file)', '--key-id', '(hidden)', '--issuer', '(hidden)'], hint: '--key <file> --key-id <id> --issuer <issuer>' };
  if (set > 0) return { error: 'APPLE_API_KEY_PATH, APPLE_API_KEY and APPLE_API_ISSUER go together: set all three, or none to use the keychain profile' };
  const profile = env.BELLWETHER_NOTARY_PROFILE || 'bellwether';
  return { args: ['--keychain-profile', profile], shown: ['--keychain-profile', profile], hint: `--keychain-profile ${profile}` };
}

/**
 * Submits a .zip, .dmg or .pkg to the notary service and waits. Returns the outcome; the
 * printed command never carries the key id or issuer.
 */
export function notarize(file, env = process.env) {
  const creds = notaryCredentials(env);
  if (creds.error) return { ok: false, message: creds.error };
  say(`$ xcrun notarytool submit ${file} ${creds.shown.join(' ')} --wait --timeout 30m`);
  const r = spawnSync('/usr/bin/xcrun', ['notarytool', 'submit', file, ...creds.args, '--wait', '--timeout', '30m'], { encoding: 'utf8' });
  const text = `${r.stdout ?? ''}${r.stderr ?? ''}`.trim();
  if (text) say(text);
  if (r.status === 0 && /status: Accepted/.test(text)) return { ok: true };
  const sub = /id: ([0-9a-f-]{36})/.exec(text)?.[1];
  return { ok: false, message: `notarization was not accepted${sub ? `; see why with: xcrun notarytool log ${sub} ${creds.hint}` : ''}` };
}

/** The SHA-1 of the signing identity (so the name, with its Team ID, never needs printing). */
function signingIdentity() {
  const out = execFileSync('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning'], { encoding: 'utf8' });
  const ids = [...out.matchAll(/^\s*\d+\)\s+([0-9A-F]{40})\s+"([^"]+)"/gm)].map((m) => ({ sha: m[1], name: m[2] }));
  const want = process.env.BELLWETHER_SIGNING_IDENTITY;
  const pick = want ? ids.filter((i) => i.sha === want.toUpperCase() || i.name === want || i.name.startsWith(`${want} (`)) : ids.filter((i) => i.name.startsWith('Developer ID Application:'));
  if (pick.length === 0) fail(want ? `no valid code-signing identity matches BELLWETHER_SIGNING_IDENTITY (${want}); \`security find-identity -v -p codesigning\` lists them` : 'no "Developer ID Application" identity in the keychain: import the certificate with its private key (see the README, "The signed app")');
  if (pick.length > 1) fail(`${pick.length} identities match; set BELLWETHER_SIGNING_IDENTITY to one SHA-1 from \`security find-identity -v -p codesigning\``);
  return pick[0];
}

function fingerprint(file) {
  try {
    const st = fs.statSync(file);
    return `${st.size}:${st.mtimeMs}:${crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')}`;
  } catch {
    return 'absent';
  }
}

/** The agent plist, as committed or with the test port and home baked in. */
function agentPlist(o, id) {
  const src = path.join(TAURI_DIR, 'macos', 'com.fanzootechnology.bellwether.brain.plist');
  let text = fs.readFileSync(src, 'utf8');
  if (o.testPort === null && o.testHome === null && !o.adhoc) return src;
  text = text.replace('<string>com.fanzootechnology.bellwether.brain</string>', `<string>${id}.brain</string>`);
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  if (o.testPort !== null) text = text.replace('<string>serve</string>', `<string>serve</string>\n    <string>--port</string>\n    <string>${o.testPort}</string>`);
  if (o.testHome !== null) text = text.replace('  <key>RunAtLoad</key>', `  <key>EnvironmentVariables</key>\n  <dict>\n    <key>BELLWETHER_HOME</key>\n    <string>${esc(o.testHome)}</string>\n  </dict>\n  <key>RunAtLoad</key>`);
  const out = path.join(OUT_DIR, `${id}.brain.plist`);
  fs.writeFileSync(out, text);
  return out;
}

/**
 * The overlay `tauri build --config` gets, with absolute paths. An ad-hoc build gets its
 * own id (`<permanent id>.adhoc`, and the agent label to match): a login item
 * registered from an ad-hoc build under the permanent id leaves a Background Task
 * Management record with a launch constraint that the Developer ID build then fails
 * (SIGKILL, "Launch Constraint Violation"; M1 finding F7).
 */
export function distOverlay(o) {
  fs.mkdirSync(OUT_DIR, { recursive: true });
  const overlay = JSON.parse(fs.readFileSync(path.join(TAURI_DIR, 'tauri.dist.conf.json'), 'utf8'));
  if (o.adhoc) overlay.identifier = `${overlay.identifier}.adhoc`;
  if (o.updaterPubkey) overlay.plugins.updater.pubkey = fs.readFileSync(o.updaterPubkey, 'utf8').trim();
  const mac = overlay.bundle.macOS;
  mac.entitlements = path.join(TAURI_DIR, mac.entitlements);
  mac.infoPlist = path.join(TAURI_DIR, mac.infoPlist);
  mac.files = { [`Library/LaunchAgents/${overlay.identifier}.brain.plist`]: agentPlist(o, overlay.identifier) };
  // The bundled pets (plan D8): only their built animations.xml, at Resources/pets/<name>/.
  overlay.bundle.resources = Object.fromEntries(Object.entries(overlay.bundle.resources ?? {}).map(([from, to]) => [path.resolve(TAURI_DIR, from), to]));
  return overlay;
}

function run(cmd, args, opts = {}) {
  say(`$ ${[cmd, ...args].map((a) => (/\s/.test(a) ? JSON.stringify(a) : a)).join(' ')}`);
  const r = spawnSync(cmd, args, { encoding: 'utf8', ...opts });
  const text = `${r.stdout ?? ''}${r.stderr ?? ''}`.trim();
  if (text) say(text);
  return { ok: r.status === 0, text, status: r.status };
}

function main() {
  if (process.platform !== 'darwin') fail('the signed app is built on macOS only');
  const o = parse(process.argv.slice(2));
  if (!fs.existsSync(TAURI_CLI)) fail('the Tauri CLI is missing: run `npm ci --prefix shell` once (it installs @tauri-apps/cli 2.12.1 into shell/node_modules)');
  const id = o.adhoc ? { sha: '-', name: 'ad hoc (-)' } : signingIdentity();
  say(`signing as "${id.name}"`);
  fs.mkdirSync(OUT_DIR, { recursive: true });

  const overlay = distOverlay(o);
  if (o.adhoc) say(`ad-hoc build: identifier ${overlay.identifier}, so it never shares login items or permissions with the Developer ID app`);
  if (!updaterKeySet(overlay)) say('note: tauri.dist.conf.json has no updater public key yet, so this build won\'t update itself (docs/RELEASING.md)');
  const overlayFile = path.join(OUT_DIR, 'tauri.conf.json');
  fs.writeFileSync(overlayFile, JSON.stringify(overlay, null, 2));

  const APP = appPath(o);
  // A clean output: never a test build left where a release build is expected.
  fs.rmSync(APP, { recursive: true, force: true });
  const before = fingerprint(LIVE_BIN);
  const env = { ...process.env, CARGO_TARGET_DIR: TARGET_DEV, APPLE_SIGNING_IDENTITY: id.sha };
  // Notarization is done below with the keychain profile, not by the bundler.
  for (const k of ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_API_KEY', 'APPLE_API_ISSUER', 'APPLE_API_KEY_PATH']) delete env[k];
  const args = ['build', '--bundles', 'app', '--config', overlayFile, ...(o.universal ? ['--target', UNIVERSAL] : []), ...(o.debug ? ['--debug'] : [])];
  const build = spawnSync(TAURI_CLI, args, { cwd: TAURI_DIR, env, stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  say(`${build.stdout}${build.stderr}`.split('\n').filter((l) => !/^\s*(Compiling|Downloaded|Downloading|Checking)\b/.test(l)).join('\n').trim());
  if (fingerprint(LIVE_BIN) !== before) fail(`${LIVE_BIN} changed during the build; it must never be written by this script. Check CARGO_TARGET_DIR and .cargo/config.toml`);
  if (build.status !== 0) fail(`tauri build failed (exit ${build.status})`);
  say(`built ${APP}`);
  const agentDir = path.join(APP, 'Contents', 'Library', 'LaunchAgents');
  const plists = fs.existsSync(agentDir) ? fs.readdirSync(agentDir).filter((f) => f.endsWith('.brain.plist')) : [];
  if (plists.length !== 1) fail(`expected one brain agent plist in ${agentDir}, found ${plists.length}`);
  const bundled = fs.readFileSync(path.join(agentDir, plists[0]), 'utf8');
  if (!bundleMatches(bundled, o)) fail(`the bundled agent plist doesn't match this build's flags (it carries ${JSON.stringify(testFlags(bundled))}); delete ${APP} and build again`);

  const verify = run('/usr/bin/codesign', ['--verify', '--deep', '--strict', '-vvv', APP]);
  if (!verify.ok) fail('codesign --verify failed');
  run('/usr/bin/codesign', ['-d', '--entitlements', '-', '--xml', APP]);
  const archs = run('/usr/bin/lipo', ['-archs', path.join(APP, 'Contents', 'MacOS', 'Bellwether')]);
  if (o.universal && !(archs.ok && /\barm64\b/.test(archs.text) && /\bx86_64\b/.test(archs.text))) fail('a --universal build must contain arm64 and x86_64');

  const kind = o.testPort !== null || o.testHome !== null ? `TEST BUILD (brain agent on port ${o.testPort ?? 'from config'}, home ${o.testHome ?? 'the default'}): for testing only; never install or ship it` : 'release build: no test port or home in the bundle';
  if (!o.notarize) {
    say('not notarized (add --notarize). Until it is, Gatekeeper rejects it on other Macs.');
    say(kind);
    return;
  }
  const zip = path.join(OUT_DIR, 'Bellwether-notarize.zip');
  fs.rmSync(zip, { force: true });
  if (!run('/usr/bin/ditto', ['-c', '-k', '--keepParent', APP, zip]).ok) fail('ditto failed');
  const submit = notarize(zip);
  fs.rmSync(zip, { force: true });
  if (!submit.ok) fail(submit.message);
  if (!run('/usr/bin/xcrun', ['stapler', 'staple', APP]).ok) fail('stapler staple failed');
  if (!run('/usr/bin/xcrun', ['stapler', 'validate', APP]).ok) fail('stapler validate failed');
  if (!run('/usr/sbin/spctl', ['-a', '-vvv', '-t', 'exec', APP]).ok) fail('spctl rejected the app');
  say(`notarized and stapled: ${APP}`);
  say(kind);
}

// realpaths: run through a symlinked path (macOS /tmp and /var are links), this is still the main module.
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) main();
