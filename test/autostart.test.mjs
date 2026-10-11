import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, test } from 'node:test';
import { LABEL, SHELL_LABEL, autostart, isRealHome, plistPath, plistText } from '../brain/autostart.mjs';
import { nextStep } from '../brain/doctor.mjs';
import { tempDir } from './helpers.mjs';

/** The brain's login autostart. Every test uses a temporary HOME; launchctl is faked. */

const neverRun = () => assert.fail('launchctl must not be called');

describe('autostart', () => {
  test('preview writes nothing', () => {
    const home = tempDir();
    const r = autostart('install', { home, platform: 'darwin', run: neverRun });
    assert.match(r.lines.join('\n'), /Preview only; nothing was written/);
    assert.equal(fs.existsSync(plistPath(home)), false);
  });

  test('--write with a temporary HOME writes the plist and never calls launchctl', () => {
    const home = tempDir();
    assert.equal(isRealHome(home), false);
    const r = autostart('install', { home, platform: 'darwin', write: true, run: neverRun });
    assert.ok(r.wrote);
    assert.match(r.lines.join('\n'), /launchctl not called/);
    const text = fs.readFileSync(plistPath(home), 'utf8');
    assert.equal(text, plistText());
    assert.match(text, new RegExp(`<string>${LABEL}</string>`));
    assert.match(text, /<string>serve<\/string>/);
    assert.doesNotMatch(text, /BELLWETHER_HOOK_TOKEN|throwaway|testToken/i, 'the token is never in the plist');
    const again = autostart('install', { home, platform: 'darwin', run: neverRun });
    assert.match(again.lines.join(), /already installed/);
    const un = autostart('uninstall', { home, platform: 'darwin', write: true, run: neverRun });
    assert.ok(un.wrote);
    assert.equal(fs.existsSync(plistPath(home)), false);
  });

  test('with the real home, launchctl bootstraps on install and boots out on uninstall (faked)', () => {
    const home = tempDir();
    const calls = [];
    const run = (cmd, args) => {
      calls.push([cmd, ...args].join(' '));
      return { ok: true, out: '' };
    };
    autostart('install', { home, platform: 'darwin', write: true, run, uid: 501, realHome: () => true });
    autostart('uninstall', { home, platform: 'darwin', write: true, run, uid: 501, realHome: () => true });
    assert.deepEqual(calls, [
      `launchctl bootout gui/501/${LABEL}`,
      `launchctl bootstrap gui/501 ${plistPath(home)}`,
      `launchctl bootout gui/501/${LABEL}`,
    ]);
  });

  test('the plist is valid (plutil -lint)', { skip: process.platform !== 'darwin' }, () => {
    const f = path.join(tempDir(), 'p.plist');
    fs.writeFileSync(f, plistText());
    const r = spawnSync('plutil', ['-lint', f], { encoding: 'utf8' });
    assert.equal(r.status, 0, r.stdout + r.stderr);
  });

  test('other platforms are refused with a message', () => {
    assert.throws(() => autostart('install', { home: tempDir(), platform: 'linux', run: neverRun }), /linux isn't built yet/);
  });

  test('the real home is recognised only as the real home', () => {
    assert.equal(isRealHome(os.userInfo().homedir), true);
    assert.equal(isRealHome(tempDir()), false);
  });
});

describe('the strip at login too (Milestone 10)', () => {
  test('with shell: both plists, valid, no token; the strip runs the release binary without spawning a brain and stays quit after Quit', () => {
    const home = tempDir();
    const bin = path.join(tempDir(), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    process.env.BELLWETHER_HOOK_TOKEN = 'm10TokenCanary_0123456789abcdef';
    try {
      const prev = autostart('install', { home, platform: 'darwin', uid: 501, shell: true, shellBin: bin, run: () => assert.fail('launchctl') });
      assert.equal(prev.wrote, false);
      assert.match(prev.lines.join('\n'), /^brain \(data\/brain\.log\):[\s\S]*^strip \(data\/shell\.log\):/m);
      autostart('install', { home, platform: 'darwin', uid: 501, write: true, shell: true, shellBin: bin, run: () => assert.fail('launchctl') });
      const strip = fs.readFileSync(plistPath(home, SHELL_LABEL), 'utf8');
      assert.match(strip, new RegExp(`<string>${bin}</string>\\s*<string>--no-spawn-brain</string>`));
      assert.match(strip, /<key>KeepAlive<\/key>\s*<dict>\s*<key>SuccessfulExit<\/key>\s*<false\/>/, 'restarted after a crash, not after Quit');
      assert.doesNotMatch(strip + fs.readFileSync(plistPath(home), 'utf8'), /m10TokenCanary|BELLWETHER_HOOK_TOKEN/);
      if (process.platform === 'darwin') assert.equal(spawnSync('plutil', ['-lint', plistPath(home, SHELL_LABEL)]).status, 0);
      autostart('uninstall', { home, platform: 'darwin', uid: 501, write: true, shell: true, shellBin: bin, run: () => assert.fail('launchctl') });
      assert.equal(fs.existsSync(plistPath(home, SHELL_LABEL)), false);
      assert.equal(fs.existsSync(plistPath(home)), false);
    } finally {
      delete process.env.BELLWETHER_HOOK_TOKEN;
    }
  });
  test('an unbuilt strip refuses before writing anything, and says how to build it or go brain-only', () => {
    const home = tempDir();
    assert.throws(() => autostart('install', { home, platform: 'darwin', uid: 501, write: true, shell: true, shellBin: path.join(home, 'missing') }), /bellwether shell --dev --release[\s\S]*--brain-only/);
    assert.equal(fs.existsSync(plistPath(home)), false);
  });
  test('with the real home faked, both are booted out and in, brain first', () => {
    const home = tempDir();
    const bin = path.join(tempDir(), 'bellwether-shell');
    fs.writeFileSync(bin, '');
    const calls = [];
    autostart('install', { home, platform: 'darwin', write: true, uid: 501, shell: true, shellBin: bin, realHome: () => true, run: (c, a) => (calls.push(a.join(' ')), { ok: true, out: '' }) });
    assert.deepEqual(calls, [`bootout gui/501/${LABEL}`, `bootstrap gui/501 ${plistPath(home)}`, `bootout gui/501/${SHELL_LABEL}`, `bootstrap gui/501 ${plistPath(home, SHELL_LABEL)}`]);
  });
});

describe('doctor names the next first-run step (Milestone 10)', () => {
  const checks = (o) => Object.entries(o).map(([name, status]) => ({ name, status }));
  const step = (o, extra = {}) => nextStep({ checks: checks(o), installed: false, home: tempDir(), platform: 'darwin', shellBuilt: true, ...extra });
  test('token, pets, build, autostart, brain, hooks, then all set, in that order', () => {
    assert.match(step({ token: 'FAIL' }), /token in the \.env/);
    assert.match(step({ token: 'PASS' }, { petOk: false }), /pets fetch/);
    assert.match(step({ token: 'PASS' }, { shellBuilt: false }), /shell --dev --release/);
    assert.match(step({ token: 'PASS' }), /autostart install/);
    const home = tempDir();
    fs.mkdirSync(path.join(home, 'Library', 'LaunchAgents'), { recursive: true });
    fs.writeFileSync(plistPath(home), '');
    fs.writeFileSync(plistPath(home, SHELL_LABEL), '');
    assert.match(step({ token: 'PASS', brain: 'FAIL' }, { home }), /LaunchAgent but isn't: see .*brain\.log/);
    assert.match(step({ token: 'PASS', brain: 'PASS' }, { home }), /hooks install/);
    assert.match(step({ token: 'PASS', brain: 'PASS' }, { home, installed: true }), /all set/);
  });
});
