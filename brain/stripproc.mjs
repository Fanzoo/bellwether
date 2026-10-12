import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { readPlist } from './plist.mjs';

/**
 * Strips that are running, as processes. Only one strip can run at a time (the shell
 * is single-instance), so a strip started by hand (`bellwether shell --dev --release`)
 * keeps the login item's strip from starting: launchd sees it exit at once, and the
 * one on screen is the hand-started one. These helpers find such a strip, read-only.
 * Nothing is matched for signalling except the pid this data dir recorded itself.
 */

/** pid -> executable path, for this user's processes (`ps -o comm` gives the full path). */
export function defaultProcesses() {
  const out = new Map();
  try {
    const text = execFileSync('/bin/ps', ['-x', '-o', 'pid=,comm='], { encoding: 'utf8', timeout: 5000 });
    for (const line of text.split('\n')) {
      const m = /^\s*(\d+)\s+(.*\S)\s*$/.exec(line);
      if (m) out.set(Number(m[1]), m[2]);
    }
  } catch {}
  return out;
}

/**
 * This checkout's strip binaries: the login item's (target/release) and every
 * development build (target/debug, and target-dev/release and target-dev/debug, where
 * `bellwether shell --dev` builds). Any of them holds the single instance.
 */
export function checkoutBinaries(shellBin) {
  const m = /^(.*)[\\/]target[\\/]release[\\/]([^\\/]+)$/.exec(shellBin);
  if (!m) return [shellBin];
  const [, base, exe] = m;
  return [shellBin, ...[['target', 'debug'], ['target-dev', 'release'], ['target-dev', 'debug']].map(([t, p]) => path.join(base, t, p, exe))];
}

/** pid -> full command line, for this user's processes. */
export function defaultCommands() {
  const out = new Map();
  try {
    const text = execFileSync('/bin/ps', ['-x', '-o', 'pid=,args='], { encoding: 'utf8', timeout: 5000 });
    for (const line of text.split('\n')) {
      const m = /^\s*(\d+)\s+(.*\S)\s*$/.exec(line);
      if (m) out.set(Number(m[1]), m[2]);
    }
  } catch {}
  return out;
}

/** The strip LaunchAgent's ProgramArguments, from its installed plist (null: not installed). */
export function agentArgs(plistFile) {
  return fs.existsSync(plistFile) ? (readPlist(plistFile).args ?? null) : null;
}

/** pid -> parent pid, for this user's processes. A strip whose parent is launchd (1) was started by launchd. */
export function defaultParents() {
  const out = new Map();
  try {
    const text = execFileSync('/bin/ps', ['-x', '-o', 'pid=,ppid='], { encoding: 'utf8', timeout: 5000 });
    for (const line of text.split('\n')) {
      const m = /^\s*(\d+)\s+(\d+)\s*$/.exec(line);
      if (m) out.set(Number(m[1]), Number(m[2]));
    }
  } catch {}
  return out;
}

/**
 * This checkout's strips that are running: [{ pid, comm, launchd }]. `launchd` (the
 * login item's) needs both: launchd is its parent, and its command line is exactly the
 * LaunchAgent's ProgramArguments (`agent`). A parent of 1 alone isn't enough: a strip
 * opened from Finder, or one whose parent exited, has it too. Read from `ps` only.
 */
export function runningStrips({ shellBin, processes = defaultProcesses, parents = defaultParents, commands = defaultCommands, agent = null }) {
  const bins = checkoutBinaries(shellBin);
  const found = [...processes()].filter(([, comm]) => bins.includes(comm));
  if (found.length === 0) return [];
  const pp = parents();
  const cmd = agent ? commands() : new Map();
  return found.map(([pid, comm]) => ({ pid, comm, launchd: !!agent && pp.get(pid) === 1 && cmd.get(pid) === agent.join(' ') }));
}

/** Running strips that weren't started by launchd: started by hand, they hold the single instance. */
export function strayStrips(o) {
  return runningStrips(o).filter((s) => !s.launchd);
}

/** The pid this data dir recorded in shell.pid (`bellwether shell start`), while it is still exactly that binary. */
export function recordedStrip({ dataDir, shellBin, processes = defaultProcesses }) {
  const file = path.join(dataDir, 'shell.pid');
  const pid = Number.parseInt(fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '', 10);
  return Number.isInteger(pid) && processes().get(pid) === shellBin ? pid : null;
}

export const sleepSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/**
 * Before the login item's strip is loaded: the strip this data dir started itself
 * (shell.pid) is stopped (SIGTERM, then a short wait); any other strip that launchd
 * didn't start stops the command with what to do. Returns the lines to show; throws
 * `makeError(message)` otherwise. Nothing else is ever signalled.
 */
export function clearStripsFor({ shellBin, dataDir, processes = defaultProcesses, parents = defaultParents, commands = defaultCommands, agent = null, kill = (pid, sig) => process.kill(pid, sig), makeError }) {
  const lines = [];
  const mine = recordedStrip({ dataDir, shellBin, processes });
  for (const s of runningStrips({ shellBin, processes, parents, commands, agent })) {
    if (s.pid === mine) {
      kill(s.pid, 'SIGTERM');
      for (let i = 0; i < 50 && processes().get(s.pid) === shellBin; i++) sleepSync(100);
      fs.rmSync(path.join(dataDir, 'shell.pid'), { force: true });
      lines.push(`stopped the strip started by \`bellwether shell start\` (pid ${s.pid}), so the login item's strip can start`);
    } else if (!s.launchd) {
      throw makeError(`a strip is already running (pid ${s.pid}, ${s.comm}), not the login item's. Only one strip can run, so the login item's would exit at once. Quit it from the menu bar (Quit Bellwether), then run this again.`);
    }
  }
  return lines;
}

/**
 * After loading (or kickstarting) the login item's strip: whether a strip started by
 * launchd is running, waiting up to `waitMs` for it. Read from the process table, not
 * launchctl.
 */
export function stripAgentStatus({ shellBin, processes = defaultProcesses, parents = defaultParents, commands = defaultCommands, agent = null, waitMs = 2500 }) {
  for (let waited = 0; ; waited += 250) {
    const s = runningStrips({ shellBin, processes, parents, commands, agent }).find((x) => x.launchd);
    if (s) return { ok: true, pid: s.pid, line: `the strip is running (pid ${s.pid}, started by its LaunchAgent)` };
    if (waited >= waitMs) break;
    sleepSync(250);
  }
  const stray = strayStrips({ shellBin, processes, parents, commands, agent })[0];
  return { ok: false, line: `the strip's LaunchAgent is loaded, but its strip is not running${stray ? `: another strip (pid ${stray.pid}) holds the single instance. Quit it from the menu bar (Quit Bellwether), then \`bellwether shell start\`` : '. See data/shell.log, then `bellwether shell start`'}` };
}
