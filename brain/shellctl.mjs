import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { SHELL_LABEL, isRealHome, plistPath, shellBinary } from './autostart.mjs';
import { CliError, DATA_DIR, EXIT } from './util.mjs';
import { agentArgs, defaultCommands, defaultParents, defaultProcesses, stripAgentStatus, strayStrips } from './stripproc.mjs';

/**
 * `bellwether shell start | stop | restart`: the strip, without a terminal attached.
 *
 * - With the strip's LaunchAgent installed (`autostart install`), launchd owns it:
 *   start is `launchctl kickstart`, restart is `kickstart -k` (which also restarts a
 *   running strip), stop is `launchctl kill TERM` (the strip exits cleanly on SIGTERM,
 *   so KeepAlive leaves it stopped until the next start or login). launchctl is only
 *   called when HOME is the real home, as in autostart.
 * - Without it, bellwether runs the release binary itself, detached, and keeps its pid in
 *   data/shell.pid. Stopping signals that pid only, and only after checking that the
 *   process is exactly that binary. Nothing is matched by name or by path, so a strip
 *   started any other way (by launchd, by hand, from another data dir) is never touched.
 *
 * Everything outside is injected (`run`, `processes`, `spawnDetached`, `kill`,
 * `brainUp`) so tests use fakes and a temp HOME.
 */
export async function shellControl(action, o = {}) {
  const {
    home = os.homedir(),
    uid = process.getuid?.(),
    realHome = isRealHome,
    run = defaultRun,
    shellBin = shellBinary(),
    dataDir = DATA_DIR,
    processes = defaultProcesses,
    spawnDetached = defaultSpawn,
    kill = (pid, sig) => process.kill(pid, sig),
    brainUp = async () => false,
    platform = process.platform,
    parents = defaultParents,
    commands = defaultCommands,
    waitMs = 2500,
  } = o;
  if (!['start', 'stop', 'restart'].includes(action)) throw new CliError('usage: bellwether shell [start | stop | restart]   (no argument: run it in this terminal)', EXIT.BAD_CONFIG);
  const lines = [];
  const target = `gui/${uid}/${SHELL_LABEL}`;

  if (platform === 'darwin' && fs.existsSync(plistPath(home, SHELL_LABEL))) {
    const args = { start: ['kickstart', target], restart: ['kickstart', '-k', target], stop: ['kill', 'TERM', target] }[action];
    if (!realHome(home)) {
      lines.push(`would run: launchctl ${args.join(' ')}`, 'launchctl not called: HOME is not your real home directory (a test).');
      return { lines };
    }
    const domain = `gui/${uid}`;
    if (action !== 'stop') {
      // Only one strip can run. Another one (started by hand) would make the login
      // item's exit at once, and "started" would be untrue.
      const other = strayStrips({ shellBin, processes, parents, commands, agent: agentArgs(plistPath(home, SHELL_LABEL)) })[0];
      if (other) throw new CliError(`a strip is already running (pid ${other.pid}, not the login item's); quit it from the menu bar (Quit Bellwether), then run \`bellwether shell ${action}\``, EXIT.NOT_RUNNING);
    }
    const r = run('launchctl', args);
    if (!r.ok) {
      const hint = action === 'stop' ? ' (it may not be running)' : '; is it loaded? `bellwether autostart status`';
      throw new CliError(`launchctl ${args.join(' ')} failed: ${r.out}${hint}`, EXIT.NOT_RUNNING);
    }
    if (action === 'stop') {
      lines.push('stopped the strip (LaunchAgent); `bellwether shell start` brings it back, and so does your next login');
      return { lines };
    }
    const st = stripAgentStatus({ shellBin, processes, parents, commands, agent: agentArgs(plistPath(home, SHELL_LABEL)), waitMs });
    const did = action === 'restart' ? 'restarted' : 'started';
    lines.push(st.ok ? `${did} the strip (LaunchAgent): ${st.line}` : `asked launchd to ${action} the strip, but ${st.line.replace(/^the strip's LaunchAgent is loaded, but /, '')}`);
    return { lines };
  }

  // No LaunchAgent: bellwether's own detached copy.
  const pidFile = path.join(dataDir, 'shell.pid');
  // Only the strip this data dir started: the pid in its own shell.pid, and only while
  // that pid is still exactly the release binary. Never "whatever runs that binary":
  // that would reach a strip launchd started, or one from another data dir (verification D2).
  const ours = () => {
    const pid = Number.parseInt(fs.existsSync(pidFile) ? fs.readFileSync(pidFile, 'utf8') : '', 10);
    return Number.isInteger(pid) && processes().get(pid) === shellBin ? pid : null;
  };
  const stop = async () => {
    const pid = ours();
    if (!pid) return false;
    kill(pid, 'SIGTERM');
    for (let i = 0; i < 50 && processes().get(pid) === shellBin; i++) await new Promise((r) => setTimeout(r, 100));
    fs.rmSync(pidFile, { force: true });
    return true;
  };
  if (action === 'stop' || action === 'restart') {
    const stopped = await stop();
    if (action === 'stop') {
      lines.push(stopped ? 'stopped the strip' : 'the strip was not running');
      return { lines };
    }
  }
  if (action === 'start' && ours()) {
    lines.push(`the strip is already running (pid ${ours()})`);
    return { lines };
  }
  if (!fs.existsSync(shellBin)) throw new CliError(`the strip isn't built yet (${shellBin}). Build it with \`bellwether shell build --release\``, EXIT.NOT_RUNNING);
  const up = await brainUp();
  const pid = spawnDetached(shellBin, up ? ['--no-spawn-brain'] : []);
  fs.mkdirSync(dataDir, { recursive: true });
  fs.writeFileSync(pidFile, String(pid));
  lines.push(`${action === 'restart' ? 'restarted' : 'started'} the strip (pid ${pid}${up ? '' : ', with its own brain: none was running'})`);
  return { lines };
}

function defaultRun(cmd, args) {
  try {
    return { ok: true, out: execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000 }) };
  } catch (e) {
    return { ok: false, out: `${e.stdout ?? ''}${e.stderr ?? ''}`.trim() || e.message };
  }
}

export { defaultProcesses };

function defaultSpawn(bin, args) {
  const child = spawn(bin, args, { detached: true, stdio: 'ignore', cwd: path.dirname(bin) });
  child.unref();
  return child.pid;
}

/** Whether the strip's own binary is running (doctor). */
export function stripRunning({ shellBin = shellBinary(), processes = defaultProcesses } = {}) {
  return [...processes().values()].includes(shellBin);
}
