import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, spawnBrainSync, tempDir } from './helpers.mjs';
import { hookBlock } from './impl.mjs';

/**
 * Re-check of `migrate` after the fixes, laid out like the maintainer's real install:
 * a hub with agents/pasture, hub-prefixed LaunchAgents running `<hub>/agents/pasture/brain/cli.mjs
 * serve` and `<hub>/.../target/release/pasture-shell`, 16 old hook entries, the token in the hub
 * .env. Temp directories only; nothing reaches the live machine. The app's `migrate --from`
 * names the old agents and leaves them (plan D5); the repository-to-repository migration's
 * in-process re-checks (the Node brain's migrate and its probe, launchd injected: order, health
 * wait, rollback, port holder) went with the Node brain in milestone 11 (test/DIVERGENCES.md
 * 17 and 25; the app's agent swap is `migrate --to-app`, cargo-tested).
 */

const OLD = 'oldPastureTokenNotReal_0123456789';
const PORT = 47991;
const OLD_BRAIN = 'local.example-hub.pasture.brain';
const OLD_STRIP = 'local.example-hub.pasture.shell';
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;');
const plist = (label, args, extra = '') => `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>Label</key><string>${esc(label)}</string><key>ProgramArguments</key><array>${args.map((a) => `<string>${esc(a)}</string>`).join('')}</array>${extra}</dict></plist>\n`;

function world({ binaryPlists = false } = {}) {
  const root = tempDir('bk3-re-');
  const hub = path.join(root, 'hub');
  const old = path.join(hub, 'agents', 'pasture');
  fs.mkdirSync(path.join(old, 'data'), { recursive: true });
  fs.mkdirSync(path.join(old, 'brain'));
  fs.mkdirSync(path.join(old, 'shell', 'src-tauri', 'target', 'release'), { recursive: true });
  fs.writeFileSync(path.join(old, 'shell', 'src-tauri', 'target', 'release', 'pasture-shell'), '');
  fs.writeFileSync(path.join(hub, '.env'), `OTHER=1\nPASTURE_HOOK_TOKEN=${OLD}\n`);
  fs.writeFileSync(path.join(old, 'config.json'), JSON.stringify({ port: PORT }));
  const home = path.join(root, 'home');
  const la = path.join(home, 'Library', 'LaunchAgents');
  fs.mkdirSync(la, { recursive: true });
  fs.mkdirSync(path.join(home, '.claude'));
  const brainPlist = path.join(la, `${OLD_BRAIN}.plist`);
  const stripPlist = path.join(la, `${OLD_STRIP}.plist`);
  fs.writeFileSync(brainPlist, plist(OLD_BRAIN, ['/usr/local/bin/node', `--env-file-if-exists=${hub}/.env`, `${old}/brain/cli.mjs`, 'serve']));
  fs.writeFileSync(stripPlist, plist(OLD_STRIP, [`${old}/shell/src-tauri/target/release/pasture-shell`, '--no-spawn-brain']));
  if (binaryPlists) for (const f of [brainPlist, stripPlist]) spawnSync('plutil', ['-convert', 'binary1', f]);
  const settingsFile = path.join(home, '.claude', 'settings.json');
  const legacy = JSON.parse(JSON.stringify(hookBlock(PORT, OLD)).replaceAll('X-Bellwether-', 'X-Pasture-'));
  legacy.hooks.Stop.unshift({ hooks: [{ type: 'command', command: 'afplay /Users/example/ready.wav' }] });
  fs.writeFileSync(settingsFile, JSON.stringify(legacy, null, 2) + '\n');
  const repo = path.join(root, 'bellwether');
  fs.mkdirSync(repo);
  fs.copyFileSync(path.join(AGENT_DIR, 'config.example.json'), path.join(repo, 'config.example.json'));
  return { root, hub, old, home, la, settingsFile, repo };
}
const read = (f) => fs.readFileSync(f, 'utf8');
const ok = (text) => !text.includes(OLD);

describe('D1: the old agents are found by what they run', () => {
  for (const binary of [false, true]) {
    test(`the CLI preview names the maintainer's layout (hub-prefixed labels, ${binary ? 'binary' : 'XML'} plists) and no one else's agent, and changes nothing`, () => {
      const w = world({ binaryPlists: binary });
      fs.writeFileSync(path.join(w.la, 'com.acme.pasture.brain.plist'), plist('com.acme.pasture.brain', ['/usr/bin/python3', '-m', 'http.server', '8080']));
      fs.writeFileSync(path.join(w.la, 'com.other.brain.plist'), plist('com.other.brain', ['/usr/local/bin/node', '/opt/other/brain/cli.mjs', 'serve']));
      const files = () => fs.readdirSync(w.la).sort().map((f) => [f, read(path.join(w.la, f))]);
      const before = { agents: files(), settings: read(w.settingsFile) };
      const env = { PATH: process.env.PATH, HOME: w.home, BELLWETHER_DATA_DIR: path.join(w.root, 'cli-data') };
      const r = spawnBrainSync(['migrate', '--from', w.old, '--settings', w.settingsFile], { root: w.repo, cwd: w.repo, encoding: 'utf8', env, timeout: 30000 });
      assert.equal(r.status, 0, r.stderr);
      assert.match(r.stdout, new RegExp(OLD_BRAIN.replace(/\./g, '\\.')));
      assert.match(r.stdout, new RegExp(OLD_STRIP.replace(/\./g, '\\.')));
      assert.doesNotMatch(r.stdout, /com\.acme\.pasture\.brain|com\.other\.brain/, 'another program\'s agent was named');
      assert.ok(ok(r.stdout + r.stderr), 'the token is never shown');
      assert.deepEqual({ agents: files(), settings: read(w.settingsFile) }, before, 'a preview changed something');
    });
  }
});
