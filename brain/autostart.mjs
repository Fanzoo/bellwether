import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { AGENT_DIR, CliError, DATA_DIR, EXIT } from './util.mjs';

/**
 * Starting pasture at login (macOS LaunchAgents): the brain, and (Milestone 10) the
 * strip. Milestone 1 Q1 showed that with
 * hooks installed and no brain listening, every prompt and tool call prints a visible
 * "hook error", so the brain must be running whenever the hooks are installed.
 *
 *   pasture autostart install   [--write] [--brain-only]   preview by default; --write
 *                                            writes the plists and loads them with launchctl
 *   pasture autostart uninstall [--write] [--brain-only]   unloads and removes them
 *   pasture autostart status                what is installed and loaded
 *
 * Two agents, because the brain must keep running when the strip is quit, hidden or
 * crashed: the brain (`…pasture.brain`, background, restarted if it crashes) and the
 * strip (`…pasture.shell`, the built release binary with --no-spawn-brain, restarted
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

export const LABEL = 'local.pasture.brain';
export const SHELL_LABEL = 'local.pasture.shell';

export function plistPath(home = os.homedir(), label = LABEL) {
  return path.join(home, 'Library', 'LaunchAgents', `${label}.plist`);
}

/** The release build of the strip; autostart runs this one only. */
export function shellBinary(agentDir = AGENT_DIR) {
  return path.join(agentDir, 'shell', 'src-tauri', 'target', 'release', 'pasture-shell');
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
export function autostart(action, { write = false, home = os.homedir(), platform = process.platform, run = defaultRun, uid = process.getuid?.(), realHome = isRealHome, shell = false, shellBin = shellBinary() } = {}) {
  if (platform !== 'darwin') {
    throw new CliError(`autostart for ${platform} isn't built yet (Windows and Linux are planned). Start the brain with \`pasture serve\` meanwhile.`, EXIT.UNEXPECTED);
  }
  if (!shell) return one(action, { write, home, run, uid, realHome, label: LABEL, wantText: plistText(), what: 'the brain' });
  if (action === 'install' && !fs.existsSync(shellBin)) {
    throw new CliError(`the strip isn't built yet (${shellBin}), so it can't start at login. Build it first: \`pasture shell --dev --release\` (takes a few minutes the first time). Or install the brain alone: \`pasture autostart install --brain-only\``, EXIT.NOT_RUNNING);
  }
  const brain = one(action, { write, home, run, uid, realHome, label: LABEL, wantText: plistText(), what: 'the brain' });
  const strip = one(action, { write, home, run, uid, realHome, label: SHELL_LABEL, wantText: shellPlistText({ shellBin }), what: 'the strip' });
  const indent = (ls) => ls.flatMap((l) => l.split('\n')).map((l) => (l ? `  ${l}` : ''));
  return { lines: ['brain (data/brain.log):', ...indent(brain.lines), '', 'strip (data/shell.log):', ...indent(strip.lines)], wrote: brain.wrote || strip.wrote };
}

/** One LaunchAgent: preview, install, uninstall or status. */
function one(action, { write, home, run, uid, realHome, label, wantText, what }) {
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
    fs.mkdirSync(DATA_DIR, { recursive: true });
    if (real) launchctl(['bootout', `${domain}/${LABEL}`], { run }); // replace a loaded older copy
    fs.writeFileSync(file, wantText, { mode: 0o644 });
    lines.length = 0;
    lines.push(`wrote ${file}`);
    if (!real) {
      lines.push('launchctl not called: HOME is not your real home directory (a test). Nothing was loaded.');
      return { lines, wrote: true };
    }
    const r = launchctl(['bootstrap', domain, file], { run });
    lines.push(r.ok ? `loaded: ${what} now starts at login and is starting now (log: ${path.join(DATA_DIR, label === SHELL_LABEL ? 'shell.log' : 'brain.log')})` : `launchctl bootstrap failed: ${r.out}`);
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
  throw new CliError('usage: pasture autostart install | uninstall | status [--write]', EXIT.BAD_CONFIG);
}
