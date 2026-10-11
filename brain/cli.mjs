import './envload.mjs'; // first: loads .env before anything reads the environment
import fs from 'node:fs';
import path from 'node:path';
import { loadConfig, readToken } from './config.mjs';
import { unifiedDiff } from './diff.mjs';
import { printDoctor, probeHealth, runDoctor } from './doctor.mjs';
import { EVENTS, applyPlan, countGroups, hookBlock, planEdit } from './hooks.mjs';
import { guardOutFile } from './outfile.mjs';
import { startBrain } from './brain.mjs';
import { getState, postControl } from './client.mjs';
import { burst, readRecording, replay } from './replay.mjs';
import { renderStatus } from './status.mjs';
import { fetchPets } from './pets.mjs';
import { readShellState, shellStateFile, writeShellState } from './privacy.mjs';
import { tokenInit } from './token.mjs';
import { migrate } from './migrate.mjs';
import { envFilePath } from './envpath.mjs';
import { shellControl } from './shellctl.mjs';
import { autostart, shellBinary } from './autostart.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { AGENT_DIR, CliError, DATA_DIR, EXIT, USER_SETTINGS, fileStamp, installTokenMask, log, parseArgs } from './util.mjs';

const USAGE = `bellwether — Claude Code sessions as sheep (brain CLI)

  bellwether serve [--hang | --slow=<ms>]     run the hook listener in the foreground
  bellwether record [--out <file>]            serve, and record event shapes to data/recordings/
  bellwether status [--watch] [--json]        the flock as text (long-polls /state with --watch)
  bellwether migrate [--from <dir>] [--env <file>] [--settings <path>] [--force] [--write]
                                           move a pasture install over: token, config, colours, hooks, LaunchAgents (preview unless --write)
  bellwether token init                       add a BELLWETHER_HOOK_TOKEN to the .env, only if there is none (never printed)
  bellwether mute | unmute                    do-not-disturb (the same as the tray's Mute; kept across restarts)
  bellwether show | hide                      show or hide the strip (the same as the tray's Hide; kept across restarts)
  bellwether replay <file.jsonl> [--speed N] [--port P] [--tag <pane>]
  bellwether replay --burst <events/s> [--seconds S] [--port P]
  bellwether shell [--dev] [--release]        run the overlay in this terminal (--dev builds it with cargo first)
  bellwether shell start | stop | restart     the strip in the background (through its LaunchAgent when installed)
  bellwether shell dock-permission            ask macOS for Accessibility, so sheep can stand on the Dock (only when you run it)
  bellwether autostart install|uninstall [--write] [--brain-only] | status   start the brain and the strip at login (macOS LaunchAgents); preview by default
  bellwether pets fetch [<pet>]               download the pinned pet art into data/pets/ (sha256-checked)
  bellwether doctor [--json] [--e2e] [--settings <path>]
  bellwether hooks print [--out <file> [--force]]   the hook block, as a settings file for claude --settings
  bellwether hooks install   [--write] [--settings <path>]   dry run unless --write
  bellwether hooks uninstall [--write] [--settings <path>] [--port <old port>]   dry run unless --write

--hang and --slow are debug flags for measuring hook timeouts: --hang accepts /hook
and never answers; --slow answers after <ms>. Neither ever sends a body.
Relative paths are relative to the directory you ran bellwether from.
Exit codes: 0 ok, 2 not installed or not running, 3 bad config or .env, 1 unexpected.`;

const LATER = {};

async function main(argv) {
  installTokenMask();
  const [command, ...rest] = argv;
  switch (command) {
    case 'serve':
      return serve(command, args(rest, { booleans: ['hang'], values: ['slow'] }));
    case 'record':
      return serve(command, args(rest, { booleans: ['hang'], values: ['slow', 'out'] }));
    case 'status':
      return status(args(rest, { booleans: ['watch', 'json'], values: [] }));
    case 'mute':
    case 'unmute':
      return muteCommand(command, args(rest, { booleans: [], values: [] }));
    case 'token':
      return tokenCommand(args(rest, { booleans: [], values: [], positionals: 1 }));
    case 'show':
    case 'hide':
      return showCommand(command, args(rest, { booleans: [], values: [] }));
    case 'replay':
      return replayCommand(args(rest, { booleans: [], values: ['speed', 'port', 'burst', 'seconds', 'tag'], positionals: 1 }));
    case 'autostart':
      return autostartCommand(args(rest, { booleans: ['write', 'brain-only'], values: [], positionals: 1 }));
    case 'shell':
      return shellCommand(args(rest, { booleans: ['dev', 'release'], values: [], positionals: 1 }));
    case 'pets':
      return pets(args(rest, { booleans: [], values: [], positionals: 2 }));
    case 'doctor':
      return doctor(args(rest, { booleans: ['json', 'e2e'], values: ['settings'] }));
    case 'migrate':
      return migrateCommand(args(rest, { booleans: ['write', 'force'], values: ['from', 'env', 'settings'] }));
    case 'hooks':
      return hooks(args(rest, { booleans: ['write', 'force'], values: ['settings', 'out', 'port'], positionals: 1 }));
    case undefined:
    case 'help':
    case '--help':
    case '-h':
      process.stdout.write(USAGE + '\n');
      return EXIT.OK;
    default:
      if (LATER[command]) throw new CliError(`\`bellwether ${command}\` arrives in Milestone ${LATER[command]}; it isn't built yet.`, EXIT.UNEXPECTED);
      throw new CliError(`unknown command "${command}".\n\n${USAGE}`, EXIT.UNEXPECTED);
  }
}

/**
 * Parses flags and refuses anything ambiguous. A flag that takes a value must have a
 * non-empty one: `--settings` with nothing after it (an unset shell variable, say)
 * must never fall back to the real ~/.claude/settings.json. Unknown flags and extra
 * positional arguments are refused for the same reason (`--setings x` is a typo, not
 * a request to edit the default file).
 */
function args(argv, { booleans = [], values = [], positionals = 0 }) {
  const parsed = parseArgs(argv, booleans);
  for (const [k, v] of Object.entries(parsed)) {
    if (k === '_') continue;
    // Own-key check against the allowlist; `parsed` has no prototype to inherit from.
    if (!Object.hasOwn(parsed, k)) continue;
    if (booleans.includes(k)) {
      if (v !== true) throw new CliError(`--${k} takes no value`, EXIT.BAD_CONFIG);
    } else if (values.includes(k)) {
      if (typeof v !== 'string' || v.trim() === '') throw new CliError(`--${k} needs a value, e.g. --${k} <${k === 'slow' ? 'ms' : 'path'}>. Nothing was done.`, EXIT.BAD_CONFIG);
    } else {
      throw new CliError(`unknown option --${k}. Nothing was done.`, EXIT.BAD_CONFIG);
    }
  }
  if (parsed._.length > positionals) throw new CliError(`unexpected argument "${parsed._[positionals]}". Nothing was done.`, EXIT.BAD_CONFIG);
  return parsed;
}

// The shim runs from the agent directory (for the .env path), so it passes the
// caller's directory along; relative paths given on the command line resolve there.
// `npm --prefix <repo> run …` sets INIT_CWD to the caller's directory.
const CALLER_DIR = process.env.BELLWETHER_CWD || process.env.INIT_CWD || process.cwd();
const userPath = (p) => path.resolve(CALLER_DIR, p);

async function serve(command, args) {
  const { config } = loadConfig();
  const token = readToken();
  const hang = args.hang === true;
  const slowMs = args.slow === undefined ? 0 : Number(args.slow);
  if (hang && slowMs) throw new CliError('--hang and --slow are exclusive', EXIT.BAD_CONFIG);
  if (!Number.isFinite(slowMs) || slowMs < 0) throw new CliError('--slow takes a delay in milliseconds, e.g. --slow=1500', EXIT.BAD_CONFIG);

  let recordTo = null;
  if (command === 'record') {
    recordTo = args.out !== undefined ? userPath(args.out) : path.join(DATA_DIR, 'recordings', `${fileStamp()}.jsonl`);
    guardOutFile(recordTo, 'record');
  }

  const brain = await startBrain({ config, token, dataDir: DATA_DIR, hang, slowMs, recordTo, log: (line) => log(line.startsWith('bellwether') || /^\d\d:/.test(line) ? line : `bellwether: ${line}`) });
  const mode = hang ? ' (debug: --hang, /hook never answers)' : slowMs ? ` (debug: --slow=${slowMs})` : '';
  log(`bellwether: listening on http://127.0.0.1:${brain.port}${mode}, pid ${process.pid}`);
  if (recordTo) log(`bellwether: recording to ${recordTo}`);

  // A brain the shell started carries BELLWETHER_PARENT_PID: if the shell disappears
  // (even by SIGKILL, which it can't intercept), this brain follows it. The variable
  // is never set for a brain started by autostart or by hand.
  const parent = Number(process.env.BELLWETHER_PARENT_PID);
  let orphanWatch = null;

  await new Promise((resolve) => {
    if (Number.isInteger(parent) && parent > 1) {
      orphanWatch = setInterval(() => {
        if (process.ppid !== parent) {
          log(`bellwether: the shell that started this brain (pid ${parent}) is gone; stopping`);
          resolve();
        }
      }, 1000);
    }
    const stop = (sig) => {
      log(`bellwether: ${sig}, stopping (${brain.counters.accepted} events accepted)`);
      resolve();
    };
    process.once('SIGINT', stop);
    process.once('SIGTERM', stop);
  });
  clearInterval(orphanWatch);
  await brain.close();
  return EXIT.OK;
}

async function status(args) {
  const { config } = loadConfig();
  const token = readToken();
  if (args.watch && args.json) throw new CliError('--watch and --json are exclusive', EXIT.BAD_CONFIG);
  const render = (snap) => renderStatus(snap, { palette: config.pets.palette, color: process.stdout.isTTY });
  let snap = await getState(config.port, token);
  if (args.json) {
    process.stdout.write(JSON.stringify(snap, null, 2) + '\n');
    return EXIT.OK;
  }
  if (!args.watch) {
    process.stdout.write(render(snap));
    return EXIT.OK;
  }
  let stop = false;
  process.once('SIGINT', () => (stop = true));
  const clear = process.stdout.isTTY ? '\x1b[2J\x1b[H' : '\n';
  for (;;) {
    process.stdout.write(clear + render(snap));
    if (stop) return EXIT.OK;
    try {
      snap = await getState(config.port, token, { after: snap.rev, waitMs: 25_000 });
    } catch (e) {
      // The brain went away: say so, keep watching, and pick up when it comes back.
      process.stdout.write(`${clear}${e.message}\nwaiting for the brain…\n`);
      await new Promise((r) => setTimeout(r, 2000));
      if (stop) return EXIT.OK;
      try {
        snap = await getState(config.port, token);
      } catch {}
    }
    if (stop) return EXIT.OK;
  }
}

/**
 * Do-not-disturb, the same switch as the tray's Mute: the strip stops animating and
 * shows only a small badge on a sheep that needs you. Kept in data/state.json.
 */
async function muteCommand(command) {
  const { config } = loadConfig();
  const token = readToken();
  await postControl(config.port, token, { action: command });
  process.stdout.write(command === 'mute' ? 'muted: the strip stops animating; only a sheep that needs you shows a badge\n' : 'unmuted\n');
  return EXIT.OK;
}

/** `bellwether token init`: a token into the .env (envpath.mjs), only if there isn't one. */
/** `bellwether migrate`: see migrate.mjs. Preview unless --write. */
function migrateCommand(args) {
  const { lines } = migrate({
    from: args.from !== undefined ? userPath(args.from) : null,
    envFrom: args.env !== undefined ? userPath(args.env) : null,
    settingsFile: args.settings !== undefined ? userPath(args.settings) : USER_SETTINGS,
    write: args.write === true,
    force: args.force === true,
  });
  process.stdout.write(lines.join('\n') + '\n');
  return EXIT.OK;
}

function tokenCommand(args) {
  if (args._[0] !== 'init') throw new CliError('usage: bellwether token init', EXIT.BAD_CONFIG);
  const { message } = tokenInit(envFilePath());
  process.stdout.write(message + '\n');
  return EXIT.OK;
}

/**
 * The tray's "Hide the strip", from the terminal: the way back for a strip hidden
 * from a tray that isn't there. Writes data/shell-state.json; a running shell picks
 * it up within a second, and a shell started later starts that way.
 */
function showCommand(command) {
  const state = readShellState(DATA_DIR);
  writeShellState(DATA_DIR, { ...state, hidden: command === 'hide' });
  process.stdout.write(`${command === 'hide' ? 'hidden' : 'shown'}: a running shell follows within a second, and the shell starts this way next time (${shellStateFile(DATA_DIR)})\n`);
  return EXIT.OK;
}

async function replayCommand(args) {
  const { config } = loadConfig();
  const token = readToken();
  const num = (name, v, min, max) => {
    const n = Number(v);
    if (!Number.isFinite(n) || n < min || n > max) throw new CliError(`--${name} must be a number from ${min} to ${max}`, EXIT.BAD_CONFIG);
    return n;
  };
  const port = args.port !== undefined ? num('port', args.port, 1, 65535) : config.port;
  const speed = args.speed !== undefined ? num('speed', args.speed, 0.01, 1000) : 1;
  let lines;
  let what;
  if (args.burst !== undefined) {
    if (args._.length) throw new CliError('give either a file or --burst, not both', EXIT.BAD_CONFIG);
    const rate = num('burst', args.burst, 1, 5000);
    const seconds = args.seconds !== undefined ? num('seconds', args.seconds, 0.1, 60) : 2;
    lines = burst({ rate, seconds });
    what = `a synthetic burst of ${lines.length} events at ${rate}/s`;
  } else {
    if (!args._[0]) throw new CliError('usage: bellwether replay <file.jsonl> [--speed N] [--port P]  |  bellwether replay --burst <events/s> [--seconds S]', EXIT.BAD_CONFIG);
    if (args.seconds !== undefined) throw new CliError('--seconds only goes with --burst', EXIT.BAD_CONFIG);
    lines = readRecording(userPath(args._[0]));
    what = `${lines.length} events from ${args._[0]}`;
  }
  if (args.tag !== undefined && !/^[A-Za-z0-9_.:-]{1,40}$/.test(args.tag)) throw new CliError('--tag must be 1–40 characters of [A-Za-z0-9_.:-]', EXIT.BAD_CONFIG);
  log(`replaying ${what} to 127.0.0.1:${port} at ${speed}×${args.tag ? ` as ${args.tag}` : ''} …`);
  const r = await replay(lines, {
    port,
    token,
    speed,
    tag: args.tag ?? null,
    log: (i, line, status, sentAt) => {
      if (lines.length <= 200) log(`  ${String(i + 1).padStart(4)}  ${sentAt}  ${line.normalised.event}${line.normalised.toolName ? ' ' + line.normalised.toolName : ''}${line.normalised.notificationType ? ' ' + line.normalised.notificationType : ''}`);
    },
  });
  const f = (x) => (Number.isFinite(x) ? x.toFixed(2) : '–');
  process.stdout.write(
    `replayed ${r.events} events in ${r.elapsed.toFixed(2)} s\n` +
      `/hook latency: p50 ${f(r.p50)} ms, p99 ${f(r.p99)} ms, max ${f(r.max)} ms\n` +
      `snapshots: ${r.snapshots} new, at most ${r.maxPerSecond} in any one second\n` +
      (r.failures.length ? `FAILURES (${r.failures.length}):\n  ${r.failures.slice(0, 10).join('\n  ')}\n` : 'every /hook answer was an empty 204\n'),
  );
  return r.failures.length ? EXIT.UNEXPECTED : EXIT.OK;
}

async function autostartCommand(args) {
  const [sub] = args._;
  if (!['install', 'uninstall', 'status'].includes(sub)) throw new CliError('usage: bellwether autostart install | uninstall | status [--write] [--brain-only]', EXIT.BAD_CONFIG);
  if (sub === 'status' && args.write) throw new CliError('`autostart status` takes no --write', EXIT.BAD_CONFIG);
  // The brain and the strip, unless --brain-only (Milestone 10).
  const { lines } = autostart(sub, { write: args.write === true, shell: args['brain-only'] !== true });
  process.stdout.write(lines.join('\n') + '\n');
  return EXIT.OK;
}

/**
 * `bellwether shell`: runs the overlay. Uses the release build if there is one, else the
 * debug build; `--dev` (re)builds the debug binary with cargo first.
 */
async function shellCommand(args) {
  const [sub] = args._;
  if (sub === 'dock-permission') {
    // macOS grants Accessibility per binary: ask from the strip's own release binary
    // (the one the LaunchAgent runs), not from node.
    const bin = shellBinary();
    if (!fs.existsSync(bin)) throw new CliError(`the strip isn't built yet (${bin}). Build it with \`bellwether shell --dev --release\``, EXIT.NOT_RUNNING);
    const r = spawnSync(bin, ['--ax-prompt'], { stdio: 'inherit' });
    return r.status ?? EXIT.UNEXPECTED;
  }
  if (sub !== undefined) {
    if (args.dev || args.release) throw new CliError('`bellwether shell start|stop|restart` takes no --dev or --release (they run the release build)', EXIT.BAD_CONFIG);
    const { config } = loadConfig();
    const { lines } = await shellControl(sub, { brainUp: async () => (await probeHealth(config.port, 1000)).kind === 'bellwether' });
    process.stdout.write(lines.join('\n') + '\n');
    return EXIT.OK;
  }
  const tauriDir = path.join(AGENT_DIR, 'shell', 'src-tauri');
  const bin = (profile) => path.join(tauriDir, 'target', profile, process.platform === 'win32' ? 'bellwether-shell.exe' : 'bellwether-shell');
  if (args.dev) {
    const build = spawn('cargo', ['build', ...(args.release ? ['--release'] : [])], { cwd: tauriDir, stdio: 'inherit' });
    const code = await new Promise((r) => build.on('close', r));
    if (code !== 0) throw new CliError('cargo build failed (is Rust installed? see README, Setup)', EXIT.UNEXPECTED);
  }
  const candidates = args.release ? [bin('release')] : [bin('release'), bin('debug')];
  const found = candidates.find((f) => fs.existsSync(f));
  if (!found) throw new CliError(`the shell isn't built yet. Build it with \`bellwether shell --dev\` or \`cd shell/src-tauri && cargo build --release\``, EXIT.NOT_RUNNING);
  log(`starting ${found}`);
  const child = spawn(found, [], { stdio: 'inherit' });
  return await new Promise((r) => child.on('close', (code) => r(code ?? EXIT.OK)));
}

async function pets(args) {
  const [sub, name] = args._;
  if (sub !== 'fetch') throw new CliError('usage: bellwether pets fetch [<pet>]', EXIT.BAD_CONFIG);
  const results = await fetchPets(DATA_DIR, { only: name ? [name] : null, log: (l) => log(`pets: ${l}`) });
  for (const r of results) process.stdout.write(`${r.folder}: ${r.status}${r.bytes ? ` (${r.bytes} bytes, sha256 verified)` : ''} -> ${path.join(DATA_DIR, 'pets', r.folder)}\n`);
  return EXIT.OK;
}

async function doctor(args) {
  const settingsFile = args.settings !== undefined ? userPath(args.settings) : USER_SETTINGS;
  if (args.e2e) log('doctor --e2e: running one `claude -p` with haiku (one small API call)…');
  const result = await runDoctor({ settingsFile, projectDir: CALLER_DIR, e2e: args.e2e === true });
  if (args.json) process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  else printDoctor(result);
  return result.exitCode;
}

async function hooks(args) {
  const [sub] = args._;
  const { config } = loadConfig();
  // --port manages entries on another port (after a port change). Ownership is the
  // exact URL http://127.0.0.1:<port>/hook, so entries on the old port are untouched
  // until named this way.
  let port = config.port;
  if (args.port !== undefined) {
    port = Number(args.port);
    if (!Number.isInteger(port) || port < 1024 || port > 65535 || String(port) !== args.port) throw new CliError('--port takes an integer from 1024 to 65535', EXIT.BAD_CONFIG);
    if (sub !== 'uninstall') throw new CliError('--port is only for `hooks uninstall` (to remove entries left on an old port); change "port" in config.json to move the install', EXIT.BAD_CONFIG);
  }
  if (sub === 'print') {
    if (args.write || args.settings !== undefined) throw new CliError('`hooks print` takes only --out <file> [--force]', EXIT.BAD_CONFIG);
    if (args.force && args.out === undefined) throw new CliError('--force only applies to `hooks print --out <file>`', EXIT.BAD_CONFIG);
    const text = JSON.stringify(hookBlock(port, readToken()), null, 2) + '\n';
    if (args.out === undefined) {
      process.stdout.write(text);
      return EXIT.OK;
    }
    // The block holds the token, so the file is private to its owner (0600).
    const out = userPath(args.out);
    guardOutFile(out, 'print', { force: args.force === true });
    fs.mkdirSync(path.dirname(out), { recursive: true });
    fs.writeFileSync(out, text, { mode: 0o600 });
    fs.chmodSync(out, 0o600);
    log(`wrote ${out} (mode 0600). Try it with: claude --settings ${out}`);
    return EXIT.OK;
  }
  if (sub !== 'install' && sub !== 'uninstall') throw new CliError('usage: bellwether hooks print [--out <file>] | install | uninstall [--write] [--settings <path>]', EXIT.UNEXPECTED);
  if (args.out !== undefined || args.force) throw new CliError(`\`hooks ${sub}\` takes --settings and --write, not --out or --force`, EXIT.BAD_CONFIG);

  const file = args.settings !== undefined ? userPath(args.settings) : USER_SETTINGS;
  const token = sub === 'install' ? readToken() : null;
  // The install record belongs to the configured port's install; an uninstall of
  // old-port leftovers neither reads nor rewrites it.
  const recordDir = port === config.port ? DATA_DIR : null;
  const plan = planEdit(file, sub, { port, token, dataDir: recordDir });
  for (const w of plan.warnings) log(`warning: ${w}`);
  const shown = plan.warnings.length;
  const before = countGroups(plan.before, port);
  const after = countGroups(plan.after, port);
  const sum = (c) => Object.values(c).reduce((a, b) => a + b, 0);

  if (!plan.changed) {
    log(sub === 'install' ? `already installed, no changes (${sum(after)} bellwether matcher groups in ${file})` : `nothing to remove: no bellwether entries in ${file}`);
    return EXIT.OK;
  }
  process.stdout.write(unifiedDiff(plan.oldText, plan.newText, { oldName: `${file} (current)`, newName: `${file} (after ${sub})` }));
  const summary =
    `bellwether matcher groups ${sum(before)} -> ${sum(after)} (${EVENTS.length} events subscribed); ` +
    (plan.layoutKept
      ? 'every other line of the file is unchanged.'
      : 'every other entry keeps its value, BUT this file is not laid out the way bellwether writes JSON ' +
        '(spacing, inline arrays or mixed line endings), so the write also reformats existing lines, as the diff shows. ' +
        'An uninstall afterwards will not restore the original bytes; the backup in data/backups/ keeps them.');
  if (args.write !== true) {
    log(`\ndry run: ${summary}\nNothing was written. Re-run with --write to apply.`);
    return EXIT.OK;
  }
  const backup = applyPlan(plan, path.join(DATA_DIR, 'backups'), recordDir);
  log(`\nwritten: ${summary}` + (backup ? `\nbackup of the previous file: ${backup}` : ''));
  for (const w of plan.warnings.slice(shown)) log(`warning: ${w}`);
  return EXIT.OK;
}

// process.exit() can cut off stdout when it is a pipe, so wait for it to drain first.
const drained = () => new Promise((resolve) => process.stdout.write('', resolve));

main(process.argv.slice(2)).then(
  async (code) => {
    await drained();
    process.exit(code ?? EXIT.OK);
  },
  (e) => {
    if (e instanceof CliError) {
      log(`bellwether: ${e.message}`);
      process.exit(e.exitCode);
    }
    log(`bellwether: unexpected error: ${e?.stack ?? e}`);
    process.exit(EXIT.UNEXPECTED);
  },
);
