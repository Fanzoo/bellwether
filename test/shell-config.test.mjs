import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { AGENT_DIR } from './helpers.mjs';

/**
 * The webview's privileges: pasture's own commands
 * only, no plugin permissions, and a CSP with no network and no unsafe-eval.
 */

const TAURI = path.join(AGENT_DIR, 'shell', 'src-tauri');
const ALLOWED = ['allow-get-pet', 'allow-get-snapshot', 'allow-report-hitboxes', 'allow-sprite-clicked', 'allow-shell-log', 'allow-debug-snapshot'];

test('capabilities grant only pasture\'s own commands, to strip windows only', () => {
  const files = fs.readdirSync(path.join(TAURI, 'capabilities')).filter((f) => f.endsWith('.json'));
  assert.deepEqual(files, ['strip.json']);
  for (const f of files) {
    const cap = JSON.parse(fs.readFileSync(path.join(TAURI, 'capabilities', f), 'utf8'));
    assert.deepEqual(cap.windows, ['strip-*']);
    for (const p of cap.permissions) {
      const name = typeof p === 'string' ? p : p.identifier;
      assert.ok(ALLOWED.includes(name), `${f}: ${name} is not one of pasture's own commands`);
      assert.doesNotMatch(name, /^(core|fs|shell|http|opener|clipboard|dialog|process|os|notification|global-shortcut|updater):/);
    }
    assert.equal(cap.remote, undefined, 'no remote URLs may use these permissions');
  }
});

test('build.rs declares exactly the commands the capability grants', () => {
  const build = fs.readFileSync(path.join(TAURI, 'build.rs'), 'utf8');
  const commands = [...build.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
  assert.deepEqual(commands.map((c) => `allow-${c.replace(/_/g, '-')}`).sort(), [...ALLOWED].sort());
});

test('the CSP allows only self, inline images and Tauri IPC: no network, no unsafe-eval', () => {
  const conf = JSON.parse(fs.readFileSync(path.join(TAURI, 'tauri.conf.json'), 'utf8'));
  const csp = conf.app.security.csp;
  assert.equal(csp, "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src ipc: http://ipc.localhost");
  assert.doesNotMatch(csp, /unsafe-eval|unsafe-inline|https?:\/\/(?!ipc\.localhost)|\*/);
  assert.deepEqual(conf.app.windows, [], 'windows are created by the shell, per monitor');
  assert.equal(conf.app.security.dangerousDisableAssetCspModification, undefined);
  assert.equal(conf.app.macOSPrivateApi, undefined, 'not needed since Tauri 2.12.1');
  assert.deepEqual(conf.app.security.capabilities, ['strip']);
});

test('the shell has no Tauri plugin that reaches files, processes or the network from the webview', () => {
  const cargo = fs.readFileSync(path.join(TAURI, 'Cargo.toml'), 'utf8');
  const deps = [...cargo.matchAll(/^(tauri-plugin-[a-z-]+)\s*=/gm)].map((m) => m[1]);
  assert.deepEqual(deps, ['tauri-plugin-single-instance']);
  assert.match(cargo, /^tauri = \{ version = "=2\.12\.1"/m);
});

test('the pet art is never part of the shell or the repository', () => {
  const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.name === 'target' || e.name === 'gen' || e.name === 'node_modules' ? [] : e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]));
  for (const f of walk(path.join(AGENT_DIR, 'shell'))) {
    assert.doesNotMatch(path.basename(f), /^animations\.xml$/i, f);
    if (/\.(js|mjs|html|css|json|rs)$/.test(f)) assert.doesNotMatch(fs.readFileSync(f, 'utf8'), /iVBORw0KGgo[A-Za-z0-9+/]{200}/, `${f} embeds image data`);
  }
});
