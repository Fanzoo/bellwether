import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { SHELL_LABEL, agentBackupDir, findOldAgents, isRealHome, plistPath, shellBinary, swapAgents } from './autostart.mjs';
import { portHolder as realPortHolder } from './porthold.mjs';
import { withDefaults } from './defaults.mjs';
import { envAssignments } from './envfile.mjs';
import { envFilePath } from './envpath.mjs';
import { Flock } from './flock.mjs';
import { applyPlan, isLegacyHandler, planEdit } from './hooks.mjs';
import { loadState, saveState } from './persist.mjs';
import { OLD_KEY, tokenInit } from './token.mjs';
import { AGENT_DIR, CliError, DATA_DIR, EXIT, USER_SETTINGS } from './util.mjs';

/**
 * `bellwether migrate [--from <dir>] [--env <file>] [--settings <file>] [--write]`:
 * moves a pasture install (the name before the rename) over to bellwether, in one
 * step. A preview unless `--write`; nothing is changed without it.
 *
 * What it moves, in this order:
 * 1. the hook token: PASTURE_HOOK_TOKEN from the old .env becomes BELLWETHER_HOOK_TOKEN
 *    in this repository's .env, with the same value, so nothing has to be re-trusted;
 * 2. config.json, if this repository has none;
 * 3. data/state.json (each session's colour and slot, and mute) and
 *    data/shell-state.json (Hide the strip, Show labels), if this one has none;
 * 4. pasture's LaunchAgents, replaced by bellwether's (macOS): the port is checked,
 *    the old agents are backed up and booted out, the new ones loaded, and the new
 *    brain must answer with the token; if not, the old agents are restored. With the
 *    strip not built, pasture's strip agent is left alone and only the brain moves;
 * 5. only then, the hook entries pasture wrote into the Claude Code settings file,
 *    replaced by bellwether's (a backup goes to data/backups/ first).
 *
 * Settings that can't be rewritten are reported before anything changes. A run that
 * stops part-way can be run again: it finishes what is left.
 *
 * `--from` is the old install: its repository directory (the one with brain/ and
 * data/) or its data directory. Without it, this repository is upgraded in place.
 * The token is never printed; launchctl is only ever called when HOME is the real home.
 */
export function migrate({
  from = null,
  envFrom = null,
  write = false,
  force = false,
  home = os.homedir(),
  settingsFile = USER_SETTINGS,
  agentDir = AGENT_DIR,
  dataDir = DATA_DIR,
  targetEnv = envFilePath(),
  run,
  uid,
  realHome,
  platform = process.platform,
  shellBin = shellBinary(agentDir),
  portHolder = realPortHolder,
} = {}) {
  const src = sourceOf(from, agentDir, dataDir);
  const lines = [`From: ${src.root ?? '(a data directory only)'}, data in ${src.dataDir}`, `To:   ${agentDir}, data in ${dataDir}`, ''];
  const did = [];
  const step = (n, title) => lines.push(`${n}. ${title}`);
  const say = (...l) => lines.push(...l.map((x) => `   ${x}`));
  const real = (realHome ?? isRealHome)(home);

  // Before anything is written: the port, how many hook entries pasture left, and
  // whether the settings file can be rewritten at the end. A file that can't is
  // reported now, while nothing has changed.
  const oldConfig = src.root ? path.join(src.root, 'config.json') : null;
  const newConfig = path.join(agentDir, 'config.json');
  let port = 47417;
  try {
    const cfgFile = fs.existsSync(newConfig) ? newConfig : oldConfig && fs.existsSync(oldConfig) ? oldConfig : path.join(agentDir, 'config.example.json');
    port = withDefaults(JSON.parse(fs.readFileSync(cfgFile, 'utf8'))).port;
  } catch {}
  const legacy = countLegacy(settingsFile, port);
  if (write && legacy > 0) {
    planEdit(settingsFile, 'install', { port, token: 'precheck', dataDir: null });
    for (const f of [settingsFile, path.dirname(settingsFile)]) {
      try {
        fs.accessSync(f, fs.constants.W_OK);
      } catch (e) {
        throw new CliError(`${f} can't be written (${e.code}), so the hook entries couldn't be moved at the end. Nothing was changed. Fix the permission, then run this again.`, EXIT.BAD_CONFIG);
      }
    }
  }

  // 1. The token.
  step(1, 'the hook token');
  const oldEnv = envFrom ?? (src.root ? legacyEnvFile(src.root) : null);
  const oldToken = oldEnv && fs.existsSync(oldEnv) ? envAssignments(fs.readFileSync(oldEnv, 'utf8'), OLD_KEY).at(-1)?.value || null : null;
  const targetText = fs.existsSync(targetEnv) ? fs.readFileSync(targetEnv, 'utf8') : '';
  const have = envAssignments(targetText, 'BELLWETHER_HOOK_TOKEN').at(-1)?.value || null;
  let token = have;
  if (have) {
    say(`BELLWETHER_HOOK_TOKEN is already set in ${targetEnv}; left alone${oldToken && oldToken !== have ? '. It differs from the old token, so the hook entries are rewritten with it (step 4)' : ''}.`);
  } else if (oldToken) {
    token = oldToken;
    say(`${write ? 'copied' : 'would copy'} ${OLD_KEY} from ${oldEnv} into ${targetEnv} as BELLWETHER_HOOK_TOKEN (same value, not shown; ${same(oldEnv, targetEnv) ? 'the old line stays' : `${oldEnv} isn't changed`})`);
    if (write) did.push(tokenInit(targetEnv, { value: oldToken }).message);
  } else {
    say(`no ${OLD_KEY} found${oldEnv ? ` in ${oldEnv}` : ''} (pass --env <file> to name the old .env); run \`bellwether token init\` for a new one`);
  }

  // 2. config.json.
  step(2, 'config.json');
  if (!oldConfig || !fs.existsSync(oldConfig) || same(oldConfig, newConfig)) say('nothing to copy');
  else if (fs.existsSync(newConfig) && !force) say(`${newConfig} exists; kept (--force replaces it with ${oldConfig})`);
  else {
    say(`${write ? 'copied' : 'would copy'} ${oldConfig} to ${newConfig}`);
    if (write) fs.copyFileSync(oldConfig, newConfig);
  }

  // 3. Colours, slots, mute; the tray's settings.
  step(3, 'colours and slots (data/state.json), and the tray settings (data/shell-state.json)');
  if (same(src.dataDir, dataDir)) say('the data directory is the same; nothing to copy');
  else {
    for (const name of ['state.json', 'shell-state.json']) {
      const a = path.join(src.dataDir, name);
      const b = path.join(dataDir, name);
      if (!fs.existsSync(a)) {
        say(`${name}: none in the old data directory`);
        continue;
      }
      if (fs.existsSync(b) && !force) {
        say(`${name}: ${b} exists; kept (--force replaces it)`);
        continue;
      }
      if (name === 'state.json') {
        // Through the same checks the brain applies on load, so a bad file can't get in.
        const persisted = loadState(a, (w) => say(`warning: ${w}`));
        if (!persisted) continue;
        const clean = new Flock({ config: withDefaults({}), t: Date.now(), persisted }).persistable();
        say(`${name}: ${write ? 'copied' : 'would copy'} ${Object.keys(clean.identities).length} session colour(s) and slot(s)${clean.muted ? ', and mute' : ''}`);
        if (write) saveState(b, clean);
      } else {
        let v = null;
        try {
          v = JSON.parse(fs.readFileSync(a, 'utf8'));
        } catch {}
        if (!v || typeof v !== 'object' || Array.isArray(v)) {
          say(`${name}: not valid JSON; skipped`);
          continue;
        }
        const keep = { ...(typeof v.hidden === 'boolean' ? { hidden: v.hidden } : {}), ...(typeof v.labels === 'boolean' ? { labels: v.labels } : {}) };
        say(`${name}: ${write ? 'copied' : 'would copy'} ${JSON.stringify(keep)}`);
        if (write) {
          fs.mkdirSync(dataDir, { recursive: true });
          fs.writeFileSync(b, JSON.stringify(keep) + '\n', { mode: 0o600 });
        }
      }
    }
  }

  // 4. LaunchAgents: swap them before any hook changes, and only keep the swap if the
  //    new brain answers (swapAgents rolls back otherwise).
  step(4, 'start at login (LaunchAgents)');
  let swapped = false;
  if (platform !== 'darwin') say('LaunchAgents are macOS only; nothing to do here');
  else {
    const found = findOldAgents(home, { from: src.root });
    const built = fs.existsSync(shellBin);
    const oldShell = found.filter((a) => a.kind === 'shell');
    if (oldShell.length && !built) say(`pasture's strip agent (${oldShell.map((a) => a.label).join(', ')}) stays as it is: bellwether's strip isn't built yet, so only the brain moves over now. Build it (\`bellwether shell --dev --release\`), then run this again`);
    const retire = found.filter((a) => a.kind === 'brain' || built);
    if (retire.length === 0) say(found.length ? 'nothing else to replace' : `no pasture LaunchAgents in ${path.join(home, 'Library', 'LaunchAgents')}`);
    else {
      const shell = built && (oldShell.length > 0 || fs.existsSync(plistPath(home, SHELL_LABEL)));
      const r = swapAgents({ old: retire, write, home, run, uid, realHome, shell, shellBin, agentDir, dataDir, backupDir: agentBackupDir(home), port, portHolder, verify: () => brainAnswers(port, targetEnv, run, agentDir) });
      say(...r.lines);
      swapped = r.wrote;
      if (r.wrote) did.push('LaunchAgents');
    }
  }

  // 5. Hook entries, last: only once a brain that reads them is answering.
  step(5, `hook entries in ${settingsFile}`);
  if (legacy === 0) say(`no pasture entries (on port ${port}); nothing to replace`);
  else if (!token) say(`${legacy} pasture entr${legacy === 1 ? 'y' : 'ies'} found, but there is no token to write into the new ones: set one (step 1), then run this again`);
  else if (!write) say(`would replace ${legacy} pasture entr${legacy === 1 ? 'y' : 'ies'} with bellwether's, once the new brain answers (\`bellwether hooks install\` shows the full diff)`);
  else {
    // A run that didn't load the brain itself (it was moved earlier, or is started by
    // hand) still checks that one answers before pointing the hooks at it.
    const v = real && !swapped ? brainAnswers(port, targetEnv, run, agentDir) : { ok: true };
    if (!v.ok) say(`left as they are: ${v.detail}. Start the brain (\`bellwether autostart install --write\` or \`bellwether serve\`), then run this again`);
    else {
      const plan = planEdit(settingsFile, 'install', { port, token, dataDir });
      if (plan.changed) {
        const backup = applyPlan(plan, path.join(dataDir, 'backups'), dataDir);
        say(`replaced ${legacy} pasture entr${legacy === 1 ? 'y' : 'ies'} with bellwether's`);
        if (backup) say(`backup of the previous file: ${backup}`);
        did.push('hooks');
      }
    }
  }

  lines.push('', write ? 'Done. `bellwether doctor` checks the result.' : 'Preview only; nothing was changed. Re-run with --write to apply.');
  return { lines, wrote: write && did.length > 0, source: src };
}

/** Whether a bellwether brain answers on the port and accepts the token in `envFile` (brain/probe.mjs, run through `run`). */
function brainAnswers(port, envFile, run, agentDir) {
  const r = (run ?? defaultRun)(process.execPath, [path.join(agentDir, 'brain', 'probe.mjs'), '--port', String(port), '--env', envFile, '--timeout', '12']);
  return { ok: r.ok, detail: (r.out || (r.ok ? `the new brain answers on port ${port}` : `no bellwether brain answered on port ${port}`)).trim() };
}

function defaultRun(cmd, args) {
  try {
    return { ok: true, out: execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 20000 }) };
  } catch (e) {
    return { ok: false, out: `${e.stdout ?? ''}${e.stderr ?? ''}`.trim() || e.message };
  }
}

/** Where the old install is: a repository directory, or a data directory. */
export function sourceOf(from, agentDir = AGENT_DIR, dataDir = DATA_DIR) {
  if (from === null) return { root: agentDir, dataDir };
  const dir = path.resolve(from);
  if (!fs.existsSync(dir) || !fs.statSync(dir).isDirectory()) throw new CliError(`--from ${from}: no such directory. Give the old install's directory, or its data directory. Nothing was done.`, EXIT.BAD_CONFIG);
  const looksLikeData = ['state.json', 'shell-state.json', 'brain.log', 'installs.json'].some((f) => fs.existsSync(path.join(dir, f)));
  if (looksLikeData) return { root: path.basename(dir) === 'data' ? path.dirname(dir) : null, dataDir: dir };
  if (fs.existsSync(path.join(dir, 'data')) || fs.existsSync(path.join(dir, 'brain'))) return { root: dir, dataDir: path.join(dir, 'data') };
  throw new CliError(`--from ${from} doesn't look like a pasture install (no data/ or brain/ in it, and no state.json). Nothing was done.`, EXIT.BAD_CONFIG);
}

/** The .env an old install read: its own, or (at agents/<name> in a hub) the hub's. */
function legacyEnvFile(root) {
  const own = path.join(root, '.env');
  if (fs.existsSync(own)) return own;
  if (path.basename(path.dirname(root)) === 'agents') {
    const hub = path.resolve(root, '..', '..', '.env');
    if (fs.existsSync(hub)) return hub;
  }
  return own;
}

function countLegacy(file, port) {
  let settings;
  try {
    settings = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return 0;
  }
  let n = 0;
  for (const groups of Object.values(settings?.hooks ?? {})) for (const g of Array.isArray(groups) ? groups : []) for (const h of g?.hooks ?? []) if (isLegacyHandler(h, port)) n++;
  return n;
}

function same(a, b) {
  try {
    return fs.realpathSync(a) === fs.realpathSync(b);
  } catch {
    return path.resolve(a) === path.resolve(b);
  }
}

