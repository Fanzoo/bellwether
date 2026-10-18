import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, TOKEN, spawnBrainSync, tempDir } from './helpers.mjs';
// The Rust brain (test/impl.mjs): the sharing rules are the strip's Rust copy.
import { readShellState, runDoctor, sharingProcessWarnings, writeShellState } from './impl.mjs';

/** Milestone 7 fixes (verification D1, D2): the way back for a hidden strip, and sharing names that can't match. */

describe('privacy.sharingProcesses entries that can never match (D2)', () => {
  const cfg = (darwin) => ({ privacy: { sharingProcesses: { darwin, win32: ['CptHost.exe'], linux: [] } } });
  test('each kind is reported, and good names are not', () => {
    const w = sharingProcessWarnings(cfg(['CptHost', 'zoom.us Screen Share', '', 'x'.repeat(32), 'a'.repeat(31), '/Applications/zoom.us.app', 'Teams.exe', ' zoom', 7]), 'darwin');
    assert.equal(w.length, 6, w.join('\n'));
    assert.match(w.join('\n'), /"" is empty/);
    assert.match(w.join('\n'), /"x{32}" is longer than 31 bytes/);
    assert.doesNotMatch(w.join('\n'), /"a{31}"|"CptHost"|zoom\.us Screen Share/);
    assert.match(w.join('\n'), /is a path/);
    assert.match(w.join('\n'), /Windows program name/);
    assert.match(w.join('\n'), /spaces at either end/);
    assert.match(w.join('\n'), /7 is not a string/);
    assert.deepEqual(sharingProcessWarnings(cfg(['CptHost']), 'win32'), [], '.exe is fine on Windows');
    assert.match(sharingProcessWarnings({ privacy: { sharingProcesses: { darwin: 'CptHost' } } }, 'darwin')[0], /not a list/);
    assert.deepEqual(sharingProcessWarnings({}, 'darwin'), []);
  });
  test('the defaults in config.example.json are all usable', () => {
    const example = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    for (const platform of ['darwin', 'win32', 'linux']) assert.deepEqual(sharingProcessWarnings(example, platform), []);
  });
  test('doctor warns about them, and says when the strip is hidden and how to bring it back', async () => {
    const dataDir = tempDir();
    writeShellState(dataDir, { hidden: true, showLabels: false });
    const config = { port: 1, ...cfg(['CptHost', 'y'.repeat(40)]) };
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: TOKEN }, settingsFile: path.join(dataDir, 'none.json'), configLoader: () => ({ config, usingExample: false }), dataDir, probeTimeoutMs: 200 });
    const by = (n) => r.checks.find((c) => c.name === n);
    if (process.platform === 'darwin') {
      assert.equal(by('sharing names').status, 'WARN');
      assert.match(by('sharing names').detail, /longer than 31 bytes/);
    }
    assert.match(by('strip').detail, /`bellwether show` brings it back/);
  });
  test('the shell has the same rules, and logs them at start', () => {
    const rs = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'privacy.rs'), 'utf8');
    for (const phrase of ['is empty', 'has spaces at either end', 'is a path', 'is longer than 31 bytes', 'is a Windows program name', 'is not a string']) assert.ok(rs.includes(phrase), phrase);
    const lib = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.match(lib, /for w in privacy::sharing_warnings\(&settings\.config\)/);
  });
});

describe('the way back for a hidden strip (D1)', () => {
  const cli = (args, dataDir) => spawnBrainSync([...args], { env: { ...process.env, BELLWETHER_DATA_DIR: dataDir, BELLWETHER_HOOK_TOKEN: TOKEN }, encoding: 'utf8', timeout: 10000 });
  test('`bellwether hide` and `bellwether show` write shell-state.json and keep Show labels as it was', () => {
    const dataDir = tempDir();
    writeShellState(dataDir, { hidden: false, showLabels: true });
    const h = cli(['hide'], dataDir);
    assert.equal(h.status, 0, h.stderr);
    assert.match(h.stdout, /^hidden: a running shell follows within a second/);
    assert.deepEqual(readShellState(dataDir), { hidden: true, showLabels: true });
    const s = cli(['show'], dataDir);
    assert.equal(s.status, 0, s.stderr);
    assert.deepEqual(readShellState(dataDir), { hidden: false, showLabels: true });
  });
  test('`bellwether show` works with no shell-state.json yet, and a bad file reads as shown', () => {
    const dataDir = tempDir();
    assert.equal(cli(['show'], dataDir).status, 0);
    assert.deepEqual(readShellState(dataDir), { hidden: false, showLabels: false });
    fs.writeFileSync(path.join(dataDir, 'shell-state.json'), 'not json');
    assert.deepEqual(readShellState(dataDir), { hidden: false, showLabels: false });
  });
  test('the shell watches the file, names the way back when hidden, and fails open without a tray', () => {
    const lib = fs.readFileSync(path.join(AGENT_DIR, 'shell', 'src-tauri', 'src', 'lib.rs'), 'utf8');
    assert.match(lib, /privacy::kept_stamp\(&data_dir\)/);
    assert.match(lib, /to show it again: the tray's Hide everything \(for screen sharing\), or `bellwether show`/);
    assert.match(lib, /if kept\.hidden \{\s*\/\/ Fail open[\s\S]*?kept\.hidden = false;/);
    // The strip is opened only after the tray decides, so a failed tray never leaves it hidden.
    assert.ok(lib.indexOf('tray::build(&handle') < lib.indexOf('strips::open(&handle'));
  });
});
