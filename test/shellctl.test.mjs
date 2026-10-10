import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { SHELL_LABEL, plistPath } from '../brain/autostart.mjs';
import { runDoctor } from '../brain/doctor.mjs';
import { shellControl } from '../brain/shellctl.mjs';
import { AGENT_DIR, TOKEN, tempDir } from './helpers.mjs';

/** `pasture shell start | stop | restart` (temp HOME and fakes only: launchd and the live strip are never touched). */

const withAgent = () => {
  const home = tempDir();
  fs.mkdirSync(path.dirname(plistPath(home, SHELL_LABEL)), { recursive: true });
  fs.writeFileSync(plistPath(home, SHELL_LABEL), '');
  return home;
};

describe('with the strip LaunchAgent installed: launchctl, behind the real-home guard', () => {
  test('restart is kickstart -k, start is kickstart, stop is kill TERM; each says what it did', async () => {
    const calls = [];
    const run = (cmd, args) => (calls.push([cmd, ...args].join(' ')), { ok: true, out: '' });
    const o = { home: withAgent(), uid: 501, realHome: () => true, run, platform: 'darwin', processes: () => assert.fail('no ps with launchd') };
    assert.deepEqual((await shellControl('restart', o)).lines, ['restarted the strip (LaunchAgent)']);
    assert.deepEqual((await shellControl('start', o)).lines, ['started the strip (LaunchAgent)']);
    assert.match((await shellControl('stop', o)).lines[0], /^stopped the strip \(LaunchAgent\)/);
    assert.deepEqual(calls, [`launchctl kickstart -k gui/501/${SHELL_LABEL}`, `launchctl kickstart gui/501/${SHELL_LABEL}`, `launchctl kill TERM gui/501/${SHELL_LABEL}`]);
  });
  test('a temp HOME never reaches launchctl; a launchctl failure is one clean line', async () => {
    const r = await shellControl('restart', { home: withAgent(), uid: 501, platform: 'darwin', run: () => assert.fail('launchctl was called') });
    assert.match(r.lines.join('\n'), /launchctl not called/);
    await assert.rejects(shellControl('start', { home: withAgent(), uid: 501, realHome: () => true, platform: 'darwin', run: () => ({ ok: false, out: 'Could not find service' }) }), /launchctl kickstart .* failed: Could not find service/);
  });
});

describe('without it: pasture\'s own detached copy, found by pid file or exact binary path', () => {
  const setup = () => {
    const dataDir = tempDir();
    const shellBin = path.join(tempDir(), 'pasture-shell');
    fs.writeFileSync(shellBin, '');
    const procs = new Map([[10, '/usr/bin/other'], [11, `${shellBin}-not-quite`]]);
    const killed = [];
    const spawned = [];
    const o = {
      home: tempDir(),
      dataDir,
      shellBin,
      platform: 'darwin',
      run: () => assert.fail('no launchctl without the agent'),
      processes: () => new Map(procs),
      kill: (pid, sig) => (killed.push([pid, sig]), procs.delete(pid)),
      spawnDetached: (bin, args) => (spawned.push([bin, ...args]), procs.set(4242, bin), 4242),
      brainUp: async () => true,
    };
    return { o, procs, killed, spawned, dataDir, shellBin };
  };
  test('start runs the release binary detached with --no-spawn-brain when a brain is up, and records its pid', async () => {
    const { o, spawned, dataDir, shellBin } = setup();
    assert.deepEqual((await shellControl('start', o)).lines, ['started the strip (pid 4242)']);
    assert.deepEqual(spawned, [[shellBin, '--no-spawn-brain']]);
    assert.equal(fs.readFileSync(path.join(dataDir, 'shell.pid'), 'utf8'), '4242');
    assert.match((await shellControl('start', o)).lines[0], /already running \(pid 4242\)/);
  });
  test('with no brain up, the strip may start its own', async () => {
    const { o, spawned } = setup();
    const r = await shellControl('start', { ...o, brainUp: async () => false });
    assert.deepEqual(spawned[0].slice(1), []);
    assert.match(r.lines[0], /with its own brain/);
  });
  test('stop signals only the pid in this data dir\'s shell.pid, and only if it is still exactly the binary', async () => {
    const { o, procs, killed, dataDir, shellBin } = setup();
    fs.writeFileSync(path.join(dataDir, 'shell.pid'), '10'); // now someone else's pid
    assert.deepEqual((await shellControl('stop', o)).lines, ['the strip was not running']);
    assert.deepEqual(killed, []);
    procs.set(77, shellBin); // a strip started some other way (launchd, another data dir): left alone
    assert.deepEqual((await shellControl('stop', o)).lines, ['the strip was not running']);
    assert.deepEqual(killed, []);
    fs.writeFileSync(path.join(dataDir, 'shell.pid'), '77'); // ours
    assert.deepEqual((await shellControl('stop', o)).lines, ['stopped the strip']);
    assert.deepEqual(killed, [[77, 'SIGTERM']]);
  });
  test('restart stops then starts; an unbuilt strip says how to build it; a bad action is a usage error', async () => {
    const { o, procs, killed, spawned, shellBin, dataDir } = setup();
    procs.set(55, shellBin);
    fs.writeFileSync(path.join(dataDir, 'shell.pid'), '55');
    assert.deepEqual((await shellControl('restart', o)).lines, ['restarted the strip (pid 4242)']);
    assert.deepEqual(killed, [[55, 'SIGTERM']]);
    assert.equal(spawned.length, 1);
    await assert.rejects(shellControl('start', { ...o, processes: () => new Map(), shellBin: '/nope/pasture-shell' }), /isn't built yet .* --dev --release/);
    await assert.rejects(shellControl('reboot', o), /usage: pasture shell/);
  });
});

describe('doctor and the CLI', () => {
  test('doctor says `pasture shell start` when the strip agent is installed but no strip runs', async () => {
    const home = withAgent();
    fs.writeFileSync(plistPath(home), '');
    const run = (running) => runDoctor({ env: { PASTURE_HOOK_TOKEN: TOKEN }, settingsFile: path.join(home, 'none.json'), configLoader: () => ({ config: { port: 1 }, usingExample: false }), home, dataDir: tempDir(), probeTimeoutMs: 100, isStripRunning: () => running });
    if (process.platform !== 'darwin') return;
    assert.match((await run(false)).checks.find((c) => c.name === 'strip running').detail, /pasture shell start/);
    assert.equal((await run(true)).checks.find((c) => c.name === 'strip running'), undefined);
  });
  test('`pasture shell bogus` and `shell start --dev` are refused (exit 3) without doing anything', () => {
    const env = { PATH: process.env.PATH, HOME: tempDir(), PASTURE_DATA_DIR: tempDir(), PASTURE_HOOK_TOKEN: TOKEN };
    const cli = (...a) => spawnSync(process.execPath, [path.join(AGENT_DIR, 'brain', 'cli.mjs'), 'shell', ...a], { encoding: 'utf8', env });
    assert.equal(cli('bogus').status, 3);
    assert.equal(cli('start', '--dev').status, 3);
  });
});
