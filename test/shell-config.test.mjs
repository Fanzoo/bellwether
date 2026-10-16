import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { AGENT_DIR } from './helpers.mjs';

/**
 * The webview's privileges: bellwether's own commands
 * only, no plugin permissions, and a CSP with no network and no unsafe-eval.
 */

const TAURI = path.join(AGENT_DIR, 'shell', 'src-tauri');
const ALLOWED = ['allow-get-pet', 'allow-get-snapshot', 'allow-report-hitboxes', 'allow-sprite-clicked', 'allow-shell-log', 'allow-debug-snapshot'];
// Milestone 7 (plan D6): the onboarding window's eleven, in their own capability.
const ONBOARDING = ['allow-setup-status', 'allow-hooks-preview', 'allow-hooks-apply', 'allow-login-items-set', 'allow-permission-request', 'allow-pet-select', 'allow-pet-import', 'allow-migration-preview', 'allow-migration-apply', 'allow-cli-tool-install', 'allow-setup-close'];
const WINDOWS = { 'strip.json': [['strip-*'], ALLOWED], 'onboarding.json': [['onboarding'], ONBOARDING] };

test('capabilities grant only bellwether\'s own commands: the strip\'s six to strip windows, the onboarding eleven to the onboarding window', () => {
  const files = fs.readdirSync(path.join(TAURI, 'capabilities')).filter((f) => f.endsWith('.json')).sort();
  assert.deepEqual(files, ['onboarding.json', 'strip.json']);
  for (const f of files) {
    const cap = JSON.parse(fs.readFileSync(path.join(TAURI, 'capabilities', f), 'utf8'));
    const [windows, allowed] = WINDOWS[f];
    assert.deepEqual(cap.windows, windows);
    assert.deepEqual(cap.permissions, allowed, `${f}: exactly its own list`);
    for (const p of cap.permissions) {
      const name = typeof p === 'string' ? p : p.identifier;
      assert.doesNotMatch(name, /^(core|fs|shell|http|opener|clipboard|dialog|process|os|notification|global-shortcut|updater):/);
    }
    assert.equal(cap.remote, undefined, 'no remote URLs may use these permissions');
  }
  assert.ok(!ALLOWED.some((p) => ONBOARDING.includes(p)), 'no permission in both');
});

test('build.rs declares exactly the commands the capabilities grant', () => {
  const build = fs.readFileSync(path.join(TAURI, 'build.rs'), 'utf8');
  const commands = [...build.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
  assert.deepEqual(commands.map((c) => `allow-${c.replace(/_/g, '-')}`).sort(), [...ALLOWED, ...ONBOARDING].sort());
});

test('the CSP allows only self, inline images and Tauri IPC: no network, no unsafe-eval', () => {
  const conf = JSON.parse(fs.readFileSync(path.join(TAURI, 'tauri.conf.json'), 'utf8'));
  const csp = conf.app.security.csp;
  assert.equal(csp, "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src ipc: http://ipc.localhost");
  assert.doesNotMatch(csp, /unsafe-eval|unsafe-inline|https?:\/\/(?!ipc\.localhost)|\*/);
  assert.deepEqual(conf.app.windows, [], 'windows are created by the shell, per monitor');
  assert.equal(conf.app.security.dangerousDisableAssetCspModification, undefined);
  assert.equal(conf.app.macOSPrivateApi, undefined, 'not needed since Tauri 2.12.1');
  assert.deepEqual(conf.app.security.capabilities, ['strip', 'onboarding']);
});

test('the shell has no Tauri plugin that reaches files, processes or the network from the webview', () => {
  const cargo = fs.readFileSync(path.join(TAURI, 'Cargo.toml'), 'utf8');
  const deps = [...cargo.matchAll(/^(tauri-plugin-[a-z-]+)\s*=/gm)].map((m) => m[1]);
  // Milestone 9: the updater, driven from Rust only (updater.rs); no capability grants an
  // `updater:` permission (checked above), so no webview reaches it.
  assert.deepEqual(deps, ['tauri-plugin-single-instance', 'tauri-plugin-updater']);
  assert.match(cargo, /^tauri-plugin-updater = \{ version = "=2\.13\.2", default-features = false, features = \["rustls-tls"\] \}$/m);
  assert.match(cargo, /^tauri = \{ version = "=2\.12\.1"/m);
});

test('the pet art is never part of the shell or the repository', () => {
  const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.name === 'target' || e.name === 'gen' || e.name === 'node_modules' ? [] : e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]));
  for (const f of walk(path.join(AGENT_DIR, 'shell'))) {
    assert.doesNotMatch(path.basename(f), /^animations\.xml$/i, f);
    if (/\.(js|mjs|html|css|json|rs)$/.test(f)) assert.doesNotMatch(fs.readFileSync(f, 'utf8'), /iVBORw0KGgo[A-Za-z0-9+/]{200}/, `${f} embeds image data`);
  }
});
