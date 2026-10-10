import fs from 'node:fs';
import path from 'node:path';

/**
 * The shell's privacy settings, as the CLI and doctor see them (Milestone 7).
 *
 * - `privacy.sharingProcesses`: process names that mean "the screen is being shared".
 *   The shell compares whole names, ignoring case, against macOS's process names,
 *   which the kernel cuts at 31 bytes. An entry that can never match is reported.
 * - data/shell-state.json: the tray's "Hide the strip" and "Show labels". The shell
 *   watches it, so `pasture show` / `pasture hide` work with or without a running shell.
 *
 * The same rules are in shell/src-tauri/src/privacy.rs; keep the two in step.
 */

export const MAX_PROCESS_NAME_BYTES = 31;

/** Why each entry for `platform` can never match (empty if all can). */
export function sharingProcessWarnings(config, platform = process.platform) {
  const list = config?.privacy?.sharingProcesses?.[platform];
  if (list === undefined) return [];
  if (!Array.isArray(list)) return [`privacy.sharingProcesses.${platform} is not a list of process names`];
  const out = [];
  for (const name of list) {
    const why = whyNeverMatches(name, platform);
    if (why) out.push(`privacy.sharingProcesses.${platform} entry ${JSON.stringify(name)} ${why}`);
  }
  return out;
}

function whyNeverMatches(name, platform) {
  if (typeof name !== 'string') return 'is not a string';
  if (name.trim() === '') return 'is empty';
  if (name !== name.trim()) return 'has spaces at either end; process names are compared whole';
  if (name.includes('/')) return 'is a path; list process names (such as CptHost), not paths';
  if (Buffer.byteLength(name, 'utf8') > MAX_PROCESS_NAME_BYTES && platform === 'darwin') return `is longer than ${MAX_PROCESS_NAME_BYTES} bytes, and macOS cuts process names there; use its first ${MAX_PROCESS_NAME_BYTES} bytes`;
  if (platform === 'darwin' && /\.exe$/i.test(name)) return 'is a Windows program name; it never runs on macOS';
  return null;
}

export const shellStateFile = (dataDir) => path.join(dataDir, 'shell-state.json');

/** The tray switches; missing or unreadable means the defaults (shown, no labels). */
export function readShellState(dataDir) {
  try {
    const v = JSON.parse(fs.readFileSync(shellStateFile(dataDir), 'utf8'));
    return { hidden: v?.hidden === true, showLabels: v?.showLabels === true };
  } catch {
    return { hidden: false, showLabels: false };
  }
}

/** Writes the switches the way the shell does (a temporary file, then a rename). */
export function writeShellState(dataDir, { hidden, showLabels }) {
  fs.mkdirSync(dataDir, { recursive: true });
  const tmp = path.join(dataDir, 'shell-state.json.tmp');
  fs.writeFileSync(tmp, JSON.stringify({ version: 1, hidden: !!hidden, showLabels: !!showLabels }, null, 2));
  fs.renameSync(tmp, shellStateFile(dataDir));
}
