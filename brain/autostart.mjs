import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { AGENT_DIR, CliError, DATA_DIR, EXIT } from './util.mjs';
import { readPlist } from './plist.mjs';
import { agentArgs, clearStripsFor, defaultCommands, defaultParents, defaultProcesses, stripAgentStatus, strayStrips } from './stripproc.mjs';

/**
 * Starting bellwether at login (macOS LaunchAgents): the brain, and (Milestone 10) the
 * strip. Milestone 1 Q1 showed that with
 * hooks installed and no brain listening, every prompt and tool call prints a visible
 * "hook error", so the brain must be running whenever the hooks are installed.
 *
 *   bellwether autostart install   [--write] [--brain-only]   preview by default; --write
 *                                            writes the plists and loads them with launchctl
 *   bellwether autostart uninstall [--write] [--brain-only]   unloads and removes them
 *   bellwether autostart status                what is installed and loaded
 *
 * Two agents, because the brain must keep running when the strip is quit, hidden or
 * crashed: the brain (`…bellwether.brain`, background, restarted if it crashes) and the
 * strip (`…bellwether.shell`, the built release binary with --no-spawn-brain, restarted
 * only if it crashes, so Quit in the tray keeps it quit until the next login). The
 * function installs the brain alone unless asked for the shell too; the CLI asks.
 *
 * The agent runs `node brain/cli.mjs serve` (which loads the repository's .env) from the
 * agent directory, restarts it if it crashes, and logs to data/brain.log. The token
 * is read from that .env at start; it is never written into the plist.
 *
 * launchctl is only ever called when HOME is the real home directory of the user
 * running the command: a test with a temporary HOME writes the plist there and stops.
 */

export const LABEL = 'local.bellwether.brain';
export const SHELL_LABEL = 'local.bellwether.shell';
/** The labels a standalone pasture install wrote. Inside a hub they had a prefix; see findOldAgents. */
export const OLD_LABELS = ['local.pasture.brain', 'local.pasture.shell'];

/**
 * pasture's LaunchAgents under `home`, found by what they run rather than by a fixed
 * label (an install inside a hub used its own label prefix). A plist is pasture's if
 * - its Label (or file name) ends in `.pasture.brain` / `.pasture.shell` and its
 *   ProgramArguments, when it has any, run a pasture brain (`…/brain/cli.mjs serve` in
 *   a pasture install) or a `pasture-shell` binary; or
 * - its ProgramArguments point into the `from` install.
 * A plist with a matching name that runs something else is not pasture's and is left
 * alone. Returns [{ label, file, kind: 'brain' | 'shell' }].
 */
export function findOldAgents(home = os.homedir(), { from = null } = {}) {
  const dir = path.join(home, 'Library', 'LaunchAgents');
  let names = [];
  try {
    names = fs.readdirSync(dir).filter((n) => n.endsWith('.plist'));
  } catch {
    return [];
  }
  const fromDir = from ? path.resolve(from) + path.sep : null;
  const out = [];
  for (const n of names.sort()) {
    const file = path.join(dir, n);
    const { label = n.slice(0, -'.plist'.length), args = null } = readPlist(file);
    if (label === LABEL || label === SHELL_LABEL) continue;
    const runs = args ? programKind(args) : null;
    const inFrom = !!(fromDir && args?.some((a) => typeof a === 'string' && a.startsWith(fromDir)));
    const named = /\.pasture\.(brain|shell)$/.exec(label)?.[1] ?? null;
    let kind = null;
    if (inFrom && runs) kind = runs;
    else if (named && (args === null || runs === named)) kind = named;
    if (kind) out.push({ label, file, kind });
  }
  return out;
}

/** What a ProgramArguments array runs: a pasture brain, a pasture strip, or neither. */
function programKind(args) {
  const cli = args.find((a) => typeof a === 'string' && /[\\/]brain[\\/]cli\.mjs$/.test(a));
  if (cli && args.includes('serve')) {
    const root = path.dirname(path.dirname(cli));
    const pastureInstall = path.basename(root) === 'pasture' || (fs.existsSync(path.join(root, 'pasture')) && !fs.existsSync(path.join(root, 'bellwether')));
    if (pastureInstall) return 'brain';
  }
  if (typeof args[0] === 'string' && /[\\/]pasture-shell$/.test(args[0])) return 'shell';
  return null;
}


export function plistPath(home = os.homedir(), label = LABEL) {
  return path.join(home, 'Library', 'LaunchAgents', `${label}.plist`);
}

/** The release build of the strip; autostart runs this one only. */
export function shellBinary(agentDir = AGENT_DIR) {
  return path.join(agentDir, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
}

/** The strip's LaunchAgent. No token in it: the shell reads the .env itself. */
export function shellPlistText({ shellBin = shellBinary(), agentDir = AGENT_DIR, dataDir = DATA_DIR } = {}) {
  const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${SHELL_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${esc(shellBin)}</string>
    <string>--no-spawn-brain</string>
  </array>
  <key>WorkingDirectory</key>
  <string>${esc(agentDir)}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  <string>${esc(path.join(dataDir, 'shell-launchd.log'))}</string>
  <key>StandardErrorPath</key>
  <string>${esc(path.join(dataDir, 'shell-launchd.log'))}</string>
</dict>
</plist>
`;
}

export function plistText({ nodePath = process.execPath, agentDir = AGENT_DIR, dataDir = DATA_DIR } = {}) {
  const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  // No env-file argument: the CLI finds its .env itself each time it starts (envload.mjs).
  const args = [nodePath, path.join(agentDir, 'brain', 'cli.mjs'), 'serve'];
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
${args.map((a) => `    <string>${esc(a)}</string>`).join('\n')}
  </array>
  <key>WorkingDirectory</key>
  <string>${esc(agentDir)}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  <string>${esc(path.join(dataDir, 'brain.log'))}</string>
  <key>StandardErrorPath</key>
  <string>${esc(path.join(dataDir, 'brain.log'))}</string>
</dict>
</plist>
`;
}

/** True only when HOME is the real home of this user, so launchctl may be used. */
export function isRealHome(home = os.homedir()) {
  try {
    return fs.realpathSync(home) === fs.realpathSync(os.userInfo().homedir);
  } catch {
    return false;
  }
}

function launchctl(args, { run = defaultRun } = {}) {
  return run('launchctl', args);
}

function defaultRun(cmd, args) {
  try {
    return { ok: true, out: execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000 }) };
  } catch (e) {
    return { ok: false, out: `${e.stdout ?? ''}${e.stderr ?? ''}`.trim() || e.message };
  }
}

/**
 * @returns {{lines: string[], wrote: boolean}}
 */
export function autostart(action, opts = {}) {
  const { write = false, home = os.homedir(), platform = process.platform, run = defaultRun, uid = process.getuid?.(), realHome = isRealHome } = opts;
  if (platform !== 'darwin') {
    throw new CliError(`autostart for ${platform} isn't built yet (Windows and Linux are planned). Start the brain with \`bellwether serve\` meanwhile.`, EXIT.UNEXPECTED);
  }
  const { shell = false, shellBin = shellBinary() } = opts;
  if (action === 'install' && shell && !fs.existsSync(shellBin)) {
    throw new CliError(`the strip isn't built yet (${shellBin}), so it can't start at login. Build it first: \`bellwether shell build --release\` (takes a few minutes the first time). Or install the brain alone: \`bellwether autostart install --brain-only\``, EXIT.NOT_RUNNING);
  }
  if (action === 'status') {
    const now = current(action, opts);
    const old = findOldAgents(home).map((a) => `pasture's old LaunchAgent is still installed: ${a.file} (\`bellwether autostart install\` or \`bellwether migrate\` replaces it)`);
    return { lines: [...old, ...(old.length ? [''] : []), ...now.lines], wrote: false };
  }
  // Only the old agents this install replaces: the strip's only when bellwether's strip goes in too.
  const old = action === 'install' ? findOldAgents(home).filter((a) => a.kind === 'brain' || (shell && a.kind === 'shell')) : [];
  if (old.length === 0) return current(action, opts);
  return swapAgents({ ...opts, old, backupDir: agentBackupDir(home) });
}

/** Where replaced LaunchAgent plists are kept, under the same HOME as the agents. */
export function agentBackupDir(home = os.homedir()) {
  return path.join(home, 'Library', 'Application Support', 'bellwether', 'launchagent-backups');
}

/**
 * Replaces pasture's LaunchAgents with bellwether's, in an order that never leaves the
 * hooks without a brain: (when `port` is given) check who holds the port; back up and
 * boot out the old agents; write and load the new ones; then `verify` (the new brain
 * answers and accepts the token). If loading or `verify` fails, the new agents are
 * removed and the old ones restored from their backups, and the error says exactly
 * what changed and how to recover by hand. launchctl only runs for the real HOME.
 */
export function swapAgents({ old, write = false, home = os.homedir(), run = defaultRun, uid = process.getuid?.(), realHome = isRealHome, shell = false, shellBin = shellBinary(), agentDir = AGENT_DIR, dataDir = DATA_DIR, backupDir, port = null, verify = null, portHolder = null, processes = defaultProcesses, parents = defaultParents, commands = defaultCommands, kill, stripWaitMs }) {
  const domain = `gui/${uid}`;
  const real = realHome(home);
  const lines = [];
  const ind = (ls) => ls.flatMap((l) => l.split('\n')).filter((l) => !/^Preview only/.test(l)).map((l) => (l ? `  ${l}` : ''));
  if (!write) {
    for (const a of old) lines.push(`pasture's old LaunchAgent ${a.label} (replaced by ${a.kind === 'brain' ? LABEL : SHELL_LABEL}):`, `  would back up ${a.file} to ${backupDir}/`, `  would run: launchctl bootout ${domain}/${a.label}`, `  would remove ${a.file}`, '');
    if (real && shell) {
      for (const st of strayStrips({ shellBin, processes, parents, commands, agent: agentArgs(plistPath(home, SHELL_LABEL)) })) lines.push(`a strip is running by hand (pid ${st.pid}, ${st.comm}): quit it from the menu bar (Quit Bellwether) before --write, or the login item's strip can't start`, '');
    }
    return { lines: [...lines, ...current('install', { write, home, run, uid, realHome, shell, shellBin, agentDir, dataDir }).lines], wrote: false };
  }

  // 1. Who holds the port. Something that isn't a brain, or a brain launchd didn't
  //    start from these agents, would keep the new brain from binding: stop first.
  if (real && port && portHolder) {
    const h = portHolder(port);
    if (h.state !== 'free') {
      const pidOf = (label) => {
        const r = launchctl(['print', `${domain}/${label}`], { run });
        return r.ok ? Number(/\bpid = (\d+)/.exec(r.out)?.[1]) || null : null;
      };
      const known = [...old.filter((a) => a.kind === 'brain').map((a) => pidOf(a.label)), pidOf(LABEL)].filter(Boolean);
      const ours = h.state === 'brain' && (known.includes(h.pid) || (known.length === 0 && h.name === 'pasture'));
      if (!ours) {
        const who = h.state === 'brain' ? `a ${h.name} brain, pid ${h.pid}, not started by these LaunchAgents` : `pid ${h.pid ?? 'unknown'}${h.command ? ` (${exeName(h.command)})` : ''}, which isn't a bellwether or pasture brain`;
        throw new CliError(`port ${port} is held by ${who}. The LaunchAgents and hooks were not changed, and nothing was stopped. Stop that process yourself (or change "port" in config.json), then run this again.`, EXIT.NOT_RUNNING);
      }
    }
  }

  // A strip of this checkout started by hand would keep the login item's from
  // starting; find it now, while nothing has changed.
  const cleared = real && shell ? clearStrips({ home, shellBin, dataDir, processes, parents, commands, kill }) : [];
  lines.push(...cleared);

  // 2. Back up and boot out the old agents.
  fs.mkdirSync(backupDir, { recursive: true });
  const changed = [];
  for (const a of old) {
    const bak = path.join(backupDir, path.basename(a.file));
    fs.copyFileSync(a.file, bak);
    a.backup = bak;
    if (real) launchctl(['bootout', `${domain}/${a.label}`], { run });
    fs.rmSync(a.file);
    changed.push(`${real ? 'booted out and ' : ''}removed ${a.file} (backup: ${bak})`);
    lines.push(`pasture's old LaunchAgent ${a.label}: ${changed.at(-1)}`);
  }
  if (!real) lines.push('launchctl not called for the old agents: HOME is not your real home directory (a test).');

  // 3. The new agents, then 4. the new brain must answer.
  try {
    const now = current('install', { write: true, home, run, uid, realHome, shell, shellBin, agentDir, dataDir, processes, parents, commands, kill, stripWaitMs });
    lines.push('', ...now.lines);
    if (real && verify) {
      const v = verify();
      if (!v.ok) throw new CliError(`the new brain didn't come up: ${v.detail}`, EXIT.UNEXPECTED);
      lines.push(v.detail);
    }
    return { lines, wrote: true };
  } catch (e) {
    const undo = [];
    for (const label of shell ? [LABEL, SHELL_LABEL] : [LABEL]) {
      const f = plistPath(home, label);
      if (real) launchctl(['bootout', `${domain}/${label}`], { run });
      if (fs.existsSync(f)) {
        fs.rmSync(f);
        undo.push(`removed ${f}`);
      }
    }
    const manual = [];
    for (const a of old) {
      fs.copyFileSync(a.backup, a.file);
      const r = real ? launchctl(['bootstrap', domain, a.file], { run }) : { ok: true };
      undo.push(`restored ${a.file}${real ? (r.ok ? ' and loaded it' : ' but could not load it') : ''}`);
      if (!r.ok) manual.push(`launchctl bootstrap ${domain} ${a.file}`);
    }
    const msg = [
      e instanceof CliError ? e.message : `unexpected error: ${e.message}`,
      'What changed, and was undone:',
      ...changed.map((c) => `  - ${c}`),
      ...undo.map((c) => `  - ${c}`),
      'The hooks were not touched, so pasture keeps working as before.',
      manual.length ? `Not everything could be restored. To start pasture again by hand:\n${manual.map((c) => `  ${c}`).join('\n')}` : 'pasture\'s LaunchAgents are back as they were.',
      'Fix the cause, then run the same command again; it picks up where this left off.',
    ];
    throw new CliError(msg.join('\n'), EXIT.UNEXPECTED);
  }
}

/** clearStripsFor, with the installed strip agent's own strip (by its ProgramArguments) left alone. */
function clearStrips({ home, shellBin, dataDir, processes, parents, commands, kill }) {
  return clearStripsFor({ shellBin, dataDir, processes, parents, commands, agent: agentArgs(plistPath(home, SHELL_LABEL)), ...(kill ? { kill } : {}), makeError: (m) => new CliError(m, EXIT.NOT_RUNNING) });
}

/** The bellwether agents themselves. */
function current(action, { write = false, home = os.homedir(), run = defaultRun, uid = process.getuid?.(), realHome = isRealHome, shell = false, shellBin = shellBinary(), agentDir = AGENT_DIR, dataDir = DATA_DIR, processes = defaultProcesses, parents = defaultParents, commands = defaultCommands, kill, stripWaitMs } = {}) {
  const brainText = plistText({ agentDir, dataDir });
  if (!shell) return one(action, { write, home, run, uid, realHome, label: LABEL, wantText: brainText, what: 'the brain', dataDir });
  const live = action === 'install' && write && realHome(home);
  // Only one strip can run: one started by hand would make the login item's exit at once.
  const cleared = live ? clearStrips({ home, shellBin, dataDir, processes, parents, commands, kill }) : [];
  const brain = one(action, { write, home, run, uid, realHome, label: LABEL, wantText: brainText, what: 'the brain', dataDir });
  const strip = one(action, { write, home, run, uid, realHome, label: SHELL_LABEL, wantText: shellPlistText({ shellBin, agentDir, dataDir }), what: 'the strip', dataDir });
  strip.lines.unshift(...cleared);
  if (live && strip.wrote) strip.lines.push(stripAgentStatus({ shellBin, processes, parents, commands, agent: agentArgs(plistPath(home, SHELL_LABEL)), ...(stripWaitMs !== undefined ? { waitMs: stripWaitMs } : {}) }).line);
  const indent = (ls) => ls.flatMap((l) => l.split('\n')).map((l) => (l ? `  ${l}` : ''));
  return { lines: ['brain (data/brain.log):', ...indent(brain.lines), '', 'strip (data/shell.log):', ...indent(strip.lines)], wrote: brain.wrote || strip.wrote };
}

/** One LaunchAgent: preview, install, uninstall or status. */
function one(action, { write, home, run, uid, realHome, label, wantText, what, dataDir = DATA_DIR }) {
  const LABEL = label;
  const file = plistPath(home, label);
  const domain = `gui/${uid}`;
  const real = realHome(home);
  const lines = [];

  if (action === 'status') {
    const installed = fs.existsSync(file);
    lines.push(`plist: ${installed ? file : 'not installed'}`);
    if (installed) lines.push(fs.readFileSync(file, 'utf8') === wantText ? 'plist matches this checkout' : 'plist differs from what `autostart install` would write now (re-run install --write)');
    if (real) {
      const r = launchctl(['print', `${domain}/${LABEL}`], { run });
      lines.push(r.ok ? `launchd: loaded (${/state = (\w+)/.exec(r.out)?.[1] ?? 'unknown state'}, pid ${/pid = (\d+)/.exec(r.out)?.[1] ?? 'none'})` : 'launchd: not loaded');
    } else {
      lines.push('launchd: not checked (HOME is not your real home directory)');
    }
    return { lines, wrote: false };
  }

  if (action === 'install') {
    const current = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
    if (current === wantText && !write) {
      lines.push(`already installed: ${file}`);
      return { lines, wrote: false };
    }
    lines.push(`would write ${file}:`, '', wantText, `then: launchctl bootstrap ${domain} ${file}`);
    if (!write) {
      lines.push('', 'Preview only; nothing was written. Re-run with --write to install.');
      return { lines, wrote: false };
    }
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.mkdirSync(dataDir, { recursive: true });
    if (real) launchctl(['bootout', `${domain}/${LABEL}`], { run }); // replace a loaded older copy
    fs.writeFileSync(file, wantText, { mode: 0o644 });
    lines.length = 0;
    lines.push(`wrote ${file}`);
    if (!real) {
      lines.push('launchctl not called: HOME is not your real home directory (a test). Nothing was loaded.');
      return { lines, wrote: true };
    }
    const r = launchctl(['bootstrap', domain, file], { run });
    lines.push(r.ok ? `loaded: ${what} now starts at login and is starting now (log: ${path.join(dataDir, label === SHELL_LABEL ? 'shell.log' : 'brain.log')})` : `launchctl bootstrap failed: ${r.out}`);
    if (!r.ok) throw new CliError(lines.join('\n'), EXIT.UNEXPECTED);
    return { lines, wrote: true };
  }

  if (action === 'uninstall') {
    if (!fs.existsSync(file)) {
      lines.push(`not installed: ${file} doesn't exist`);
      return { lines, wrote: false };
    }
    if (!write) {
      lines.push(`would run: launchctl bootout ${domain}/${LABEL}`, `would remove ${file}`, '', 'Preview only; nothing was changed. Re-run with --write.');
      return { lines, wrote: false };
    }
    if (real) {
      const r = launchctl(['bootout', `${domain}/${LABEL}`], { run });
      lines.push(r.ok ? `unloaded from launchd (${what} it started has stopped)` : `launchctl bootout: ${r.out || 'not loaded'}`);
    } else {
      lines.push('launchctl not called: HOME is not your real home directory (a test).');
    }
    fs.rmSync(file);
    lines.push(`removed ${file}`);
    return { lines, wrote: true };
  }
  throw new CliError('usage: bellwether autostart install | uninstall | status [--write]', EXIT.BAD_CONFIG);
}

/** The executable's name only: a holder's arguments can carry secrets, so they are never echoed. */
function exeName(command) {
  const first = String(command).trim().split(/\s+/)[0] ?? '';
  return path.basename(first) || 'unknown';
}
