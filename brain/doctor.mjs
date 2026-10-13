import { PALETTE_UPGRADED } from './defaults.mjs';
import { readShellState, sharingProcessWarnings } from './privacy.mjs';
import { execFileSync, spawn } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import http from 'node:http';
import { loadConfig, readToken } from './config.mjs';
import { ENV_HEADERS, EVENTS, HTTP_UNSUPPORTED_EVENTS, MAX_TIMEOUT_SEC, handlerToken, hookBlock, hookUrl, isBellwetherHandler, isLegacyHandler, bellwetherLikePort, shapeProblems } from './hooks.mjs';
import { LEGACY_ENV_USED, adoptLegacyEnv } from './envpath.mjs';
import { BIND_HOST, DATA_DIR, DOCTOR_PING, EXIT, USER_SETTINGS, setOwn } from './util.mjs';
import { SHELL_LABEL, findOldAgents, plistPath, shellBinary } from './autostart.mjs';
import { stripRunning } from './shellctl.mjs';
import { checkPets } from './pets.mjs';
import { CARGO_FIX, cargoVersion, findCargo } from './cargo.mjs';
import { makeTextCleaner } from './redact.mjs';

export const MIN_NODE = [22, 9];

export function nodeSupported(version) {
  const [major, minor] = version.replace(/^v/, '').split('.').map(Number);
  return major > MIN_NODE[0] || (major === MIN_NODE[0] && minor >= MIN_NODE[1]);
}

/**
 * Probes everything bellwether depends on and says, for each failure, what caused it.
 *
 * Exit code: 3 if config, .env or the settings file is wrong; 2 if the brain isn't
 * running or the hooks aren't installed; 1 for any other failure; 0 when all pass.
 */

/** A refused request older than this is history, not a problem (doctor shows it as INFO). */
export const REJECTION_RECENT_MS = 15 * 60 * 1000;

export async function runDoctor({ settingsFile = USER_SETTINGS, env = process.env, probeTimeoutMs = 1000, configLoader = loadConfig, projectDir = null, managedFile = MANAGED_SETTINGS[process.platform] ?? null, e2e = false, runClaude = defaultRunClaude, dataDir = DATA_DIR, home = os.homedir(), isStripRunning = stripRunning, findCargoFor = findCargo } = {}) {
  const checks = [];
  const add = (name, status, detail, exitCode = EXIT.UNEXPECTED) => checks.push({ name, status, detail, exitCode: status === 'FAIL' ? exitCode : EXIT.OK });

  if (nodeSupported(process.versions.node)) add('node', 'PASS', `v${process.versions.node}`);
  else add('node', 'FAIL', `v${process.versions.node}; bellwether needs Node ${MIN_NODE.join('.')} or newer (process.loadEnvFile and --env-file-if-exists need it): https://nodejs.org`, EXIT.BAD_CONFIG);

  let config = null;
  try {
    const loaded = configLoader();
    config = loaded.config;
    add('config', 'PASS', loaded.usingExample ? 'config.example.json (no config.json; copy it to customise)' : 'config.json', EXIT.BAD_CONFIG);
  } catch (e) {
    add('config', 'FAIL', e.message, EXIT.BAD_CONFIG);
  }

  // Old PASTURE_* names, read in place of BELLWETHER_* for one release.
  let legacyNames = [...LEGACY_ENV_USED];
  if (env !== process.env) {
    env = { ...env };
    legacyNames = [...adoptLegacyEnv(env, new Set())];
  }
  if (legacyNames.length) {
    const rename = legacyNames.map((k) => `${k} -> BELLWETHER_${k.slice('PASTURE_'.length)}`).join(', ');
    add('old names', 'INFO', `read the names from before the rename (${rename}); they work for one release. Rename them in your .env or shell profile; \`bellwether migrate\` does the .env`);
  }

  let token = null;
  try {
    token = readToken(env);
    add('token', 'PASS', `BELLWETHER_HOOK_TOKEN set (${token.length} chars)`);
  } catch (e) {
    add('token', 'FAIL', e.message, EXIT.BAD_CONFIG);
  }

  add('claude', 'INFO', claudeVersion());
  // Only for building the strip (`bellwether shell --dev`); running it needs no Rust.
  const cargo = findCargoFor();
  add('cargo', 'INFO', cargo ? `${cargoVersion(cargo) ?? 'found'} (${cargo}); only needed to build the strip` : `not found; only needed to build the strip. ${CARGO_FIX}`);

  if (config) {
    const port = config.port;
    const health = await probeHealth(port, probeTimeoutMs);
    if (health.kind === 'refused') {
      add('brain', 'FAIL', `brain not running — run \`bellwether serve\` (nothing listening on ${BIND_HOST}:${port})`, EXIT.NOT_RUNNING);
    } else if (health.kind === 'bellwether' && health.body.mode === 'hang') {
      add('brain', 'FAIL', `pid ${health.body.pid} is running with the debug flag --hang: /hook never answers, so every hook stalls for its full timeout. Restart it as plain \`bellwether serve\``, EXIT.NOT_RUNNING);
    } else if (health.kind === 'bellwether') {
      const h = health.body;
      const rejected = Object.entries(h.counters?.rejected ?? {}).filter(([, n]) => n > 0);
      add('brain', 'PASS', `pid ${h.pid}, v${h.version}, mode ${h.mode}, ${h.counters?.accepted ?? 0} events accepted` + (h.lastEventAt ? `, last ${new Date(h.lastEventAt).toLocaleTimeString()}` : ''));
      if (rejected.length) {
        // The counts run from the brain's start; only a refusal in the last 15 minutes
        // (or one of unknown age, from an older brain) is worth a warning.
        const last = h.counters?.lastRejectedAt;
        const recent = typeof last !== 'number' || Date.now() - last < REJECTION_RECENT_MS;
        const counts = rejected.map(([k, n]) => `${k}=${n}`).join(', ');
        if (recent) add('rejections', 'WARN', `the brain has refused requests: ${counts}${typeof last === 'number' ? `, the last at ${new Date(last).toLocaleTimeString()}` : ''}. token=N usually means the settings file holds an old token: re-run \`bellwether hooks install --write\``);
        else add('rejections', 'INFO', `${counts} refused since the brain started, the last at ${new Date(last).toLocaleTimeString()}; none in the last 15 minutes (the counts reset when the brain restarts)`);
      }
      if (token) {
        const ping = await probeHook(port, token, probeTimeoutMs);
        if (ping.status === 204) add('brain token', 'PASS', 'the brain accepts the token in .env');
        else if (ping.status === 403) add('brain token', 'FAIL', 'the brain refused the token in .env (403). It was started with a different BELLWETHER_HOOK_TOKEN: restart `bellwether serve`', EXIT.BAD_CONFIG);
        else if (ping.error === 'timeout') add('brain token', 'FAIL', `/health answers but /hook didn't answer within ${probeTimeoutMs} ms: the brain is hung or unresponsive. Restart \`bellwether serve\``, EXIT.NOT_RUNNING);
        else add('brain token', 'FAIL', `unexpected answer from /hook: ${ping.status ?? ping.error}`);
      }
    } else if (health.kind === 'timeout') {
      add('brain', 'FAIL', `something on ${BIND_HOST}:${port} accepted the connection but didn't answer /health within ${probeTimeoutMs} ms (a brain started with --hang, or a hung process${portOwner(port)})`, EXIT.NOT_RUNNING);
    } else {
      add('brain', 'FAIL', `port ${port} is held by another process${portOwner(port)}; it answered ${health.detail}. Stop it, or set a different "port" in config.json and re-run \`bellwether hooks install\``, EXIT.NOT_RUNNING);
    }
  }

  if (config) {
    const settingsChecks = checkSettingsFile(settingsFile, { port: config.port, token });
    for (const c of settingsChecks) add(c.name, c.status, c.detail, c.exitCode);
    const installed = settingsChecks.some((c) => c.name === 'hooks installed' && c.found > 0);
    // Only a stopped brain (connection refused) prints errors; a hung one stalls silently.
    const down = checks.find((c) => c.name === 'brain' && c.status === 'FAIL' && c.detail.startsWith('brain not running'));
    if (installed && down) {
      add('hook errors', 'WARN', 'hooks are installed but the brain is not answering: every Claude Code session prints a visible "<Event> hook error … ECONNREFUSED" line on each prompt and tool call until `bellwether serve` runs (measured)');
    }
    for (const c of checkOtherLevels({ projectDir, managedFile, port: config.port })) add(c.name, c.status, c.detail, c.exitCode);

    if (config[PALETTE_UPGRADED]) add('palette', 'INFO', 'config.json has the old default sheep colours (white and pale blue first, hard to tell apart), so the new default palette is used. Your config.json is unchanged; to keep the old colours, change any one of them');

    // Milestone 7: sharing-process names that can never match, and a hidden strip.
    const sharing = sharingProcessWarnings(config);
    if (sharing.length) add('sharing names', 'WARN', `${sharing.join('; ')}. The strip won't hide for these while you share`);
    else add('sharing names', 'PASS', `${(config.privacy?.sharingProcesses?.[process.platform] ?? []).length} process name(s) that hide the strip while sharing`);
    if (readShellState(dataDir).hidden) add('strip', 'INFO', 'the strip is hidden (the tray\'s "Hide the strip", or `bellwether hide`); `bellwether show` brings it back');

    // Milestone 8: what the shell's locator ran into since it last started.
    for (const c of checkLocatorLog(dataDir)) add(c.name, c.status, c.detail);

    // The shell's pet, and whether the brain starts at login (Milestone 3).
    const pet = checkPets(dataDir, config.pets?.default ?? 'esheep64');
    add('pets', pet.ok ? 'PASS' : 'WARN', pet.detail);
    if (process.platform === 'darwin') {
      const plist = plistPath(home);
      const strip = fs.existsSync(plistPath(home, SHELL_LABEL));
      if (fs.existsSync(plist)) add('autostart', 'PASS', `the brain starts at login (${plist})${strip ? ', and the strip too' : '; the strip does not (`bellwether autostart install --write` adds it)'}`);
      else if (installed) add('autostart', 'WARN', 'hooks are installed but the brain does not start at login, so after a reboot every session shows hook errors until `bellwether serve` runs. Preview: `bellwether autostart install`, then add --write');
      else add('autostart', 'INFO', 'the brain does not start at login (`bellwether autostart install` previews it)');
      const old = findOldAgents(home).map((a) => a.label);
      if (old.length) add('old LaunchAgents', 'WARN', `pasture's LaunchAgents from before the rename are still installed (${old.join(', ')}); the old brain may hold the port. \`bellwether migrate\` (or \`bellwether autostart install\`) previews removing them`);
      if (strip && !isStripRunning()) add('strip running', 'WARN', 'the strip\'s LaunchAgent is installed but no strip is running (quit from the menu bar?): `./bellwether shell start`');
    }
    add('next', 'INFO', nextStep({ checks, installed, home, dataDir, petOk: pet.ok }));
  }

  if (e2e) {
    const healthy = checks.some((c) => c.name === 'brain token' && c.status === 'PASS');
    if (!healthy || !config || !token) add('e2e', 'FAIL', 'skipped: needs a running brain that accepts the token (see the checks above)', EXIT.NOT_RUNNING);
    else {
      const r = await runE2E({ port: config.port, token, probeTimeoutMs, runClaude });
      add('e2e', r.ok ? 'PASS' : 'FAIL', r.detail, r.ok ? EXIT.OK : EXIT.UNEXPECTED);
    }
  }

  const failed = checks.filter((c) => c.status === 'FAIL');
  const exitCode = failed.some((c) => c.exitCode === EXIT.BAD_CONFIG)
    ? EXIT.BAD_CONFIG
    : failed.some((c) => c.exitCode === EXIT.NOT_RUNNING)
      ? EXIT.NOT_RUNNING
      : failed.length
        ? EXIT.UNEXPECTED
        : EXIT.OK;
  return { checks, exitCode, settingsFile };
}

/** Reads a settings file and reports on bellwether's entries in it. Never writes. */
export function checkSettingsFile(file, { port, token }) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (e) {
    return [{ name: 'hooks installed', status: 'FAIL', detail: `${file}: ${e.code === 'ENOENT' ? 'no such file' : e.code}. Run \`bellwether hooks install\``, exitCode: EXIT.NOT_RUNNING }];
  }
  let settings;
  try {
    settings = JSON.parse(text);
  } catch (e) {
    return [{ name: 'hooks installed', status: 'FAIL', detail: `${file} is not valid JSON (${e.message}); Claude Code will ignore it`, exitCode: EXIT.BAD_CONFIG }];
  }
  return checkSettings(settings, { port, token, file });
}

export function checkSettings(settings, { port, token, file = 'settings' }) {
  const out = [];
  const add = (name, status, detail, exitCode = EXIT.BAD_CONFIG) => out.push({ name, status, detail, exitCode: status === 'FAIL' ? exitCode : EXIT.OK });

  const problems = shapeProblems(settings);
  if (problems.length) {
    add('settings shape', 'FAIL', `${file}: ${problems.slice(0, 3).join('; ')}. Claude Code rejects a file shaped like this; fix it by hand (bellwether won't edit it)`);
    return out;
  }

  const perEvent = Object.create(null);
  const handlers = [];
  const strays = [];
  for (const [event, groups] of Object.entries(settings?.hooks ?? {})) {
    if (!Array.isArray(groups)) continue;
    for (const g of groups) {
      for (const h of g?.hooks ?? []) {
        if (isBellwetherHandler(h, port)) {
          setOwn(perEvent, event, (perEvent[event] ?? 0) + 1);
          handlers.push({ event, h, matcher: g.matcher });
        } else if (bellwetherLikePort(h) !== null) {
          strays.push({ url: h.url, port: bellwetherLikePort(h), hasToken: handlerToken(h) !== undefined });
        }
      }
    }
  }

  const missing = EVENTS.filter((e) => !perEvent[e]);
  const installedCheck = () => Object.assign(out.at(-1), { found: handlers.length });
  const dupes = Object.entries(perEvent).filter(([, n]) => n > 1).map(([e]) => e);
  if (handlers.length === 0) {
    add('hooks installed', 'FAIL', `no bellwether entries in ${file}. Run \`bellwether hooks install\` to see the diff, then add --write`, EXIT.NOT_RUNNING);
  } else if (missing.length) {
    add('hooks installed', 'FAIL', `${EVENTS.length - missing.length}/${EVENTS.length} events; missing ${missing.join(', ')}. Re-run \`bellwether hooks install --write\``, EXIT.NOT_RUNNING);
  } else {
    add('hooks installed', 'PASS', `${EVENTS.length}/${EVENTS.length} events in ${file}`);
  }
  installedCheck();
  if (dupes.length) add('hooks duplicated', 'FAIL', `more than one bellwether entry on ${dupes.join(', ')}. Run \`bellwether hooks install --write\` to collapse them`);
  const dead = Object.keys(perEvent).filter((e) => HTTP_UNSUPPORTED_EVENTS.includes(e));
  if (dead.length) add('hooks unsupported', 'WARN', `bellwether entries on ${dead.join(', ')} never fire: Claude Code skips HTTP hooks there. \`bellwether hooks install --write\` removes them`);
  if (strays.length) {
    const bellwetherLike = strays.filter((x) => x.hasToken);
    const foreign = strays.filter((x) => !x.hasToken);
    const fix = (list) => {
      const ports = [...new Set(list.map((x) => x.port).filter(Number.isInteger))];
      return ports.length ? ` To remove them: ${ports.map((p) => `\`bellwether hooks uninstall --port ${p} --write\``).join(', ')}.` : '';
    };
    if (bellwetherLike.length) add('hooks stray', 'WARN', `bellwether-like entries (they carry X-Bellwether-Token) that aren't on port ${port}'s exact URL: ${[...new Set(bellwetherLike.map((x) => x.url))].join(', ')}. bellwether leaves them alone (exact-URL ownership).${fix(bellwetherLike)}`);
    if (foreign.length) add('hooks foreign', 'INFO', `another tool's hook on a bellwether-style URL (no X-Bellwether-Token): ${[...new Set(foreign.map((x) => x.url))].join(', ')}. Not bellwether's; nothing to do unless it's a leftover of yours (\`hooks uninstall --port <port>\` would remove it).`);
  }
  const legacy = handlers.filter(({ h }) => isLegacyHandler(h, port));
  if (legacy.length) add('hooks from pasture', 'WARN', `${legacy.length} entr${legacy.length === 1 ? 'y was' : 'ies were'} written by pasture, before the rename (X-Pasture-* headers). The brain still reads them for one release; \`bellwether hooks install --write\` (or \`bellwether migrate --write\`) replaces them`);
  const tokenless = handlers.filter(({ h }) => handlerToken(h) === undefined);
  if (tokenless.length) add('hooks foreign', 'WARN', `an http hook on bellwether's exact URL with no X-Bellwether-Token header (${tokenless.map(({ event }) => event).join(', ')}): probably another tool's hook on bellwether's port. install and uninstall treat it as bellwether's (exact-URL rule), and the brain refuses its requests (403)`);

  if (handlers.length) {
    const badTimeout = handlers.filter(({ h }) => typeof h.timeout !== 'number' || h.timeout <= 0 || h.timeout > MAX_TIMEOUT_SEC);
    if (badTimeout.length) {
      add('hook timeout', 'FAIL', `${badTimeout.map(({ event, h }) => `${event} (timeout ${h.timeout ?? 'unset'})`).join(', ')}: every bellwether entry needs "timeout" of at most ${MAX_TIMEOUT_SEC} s, or a hung brain stalls tool calls for up to 600 s. Re-run \`bellwether hooks install --write\``);
    } else {
      add('hook timeout', 'PASS', `every entry has timeout ≤ ${MAX_TIMEOUT_SEC} s`);
    }
    const matchers = handlers.filter(({ matcher }) => matcher !== undefined && matcher !== '' && matcher !== '*');
    if (matchers.length) add('hook matcher', 'WARN', `matcher set on ${matchers.map(({ event }) => event).join(', ')}; bellwether expects every event unfiltered`);
    if (token) {
      const stale = handlers.filter(({ h }) => handlerToken(h) !== token);
      if (stale.length) add('hook token', 'FAIL', `the token in ${stale.length} entr${stale.length === 1 ? 'y' : 'ies'} differs from BELLWETHER_HOOK_TOKEN in .env, so the brain will refuse them (403). Re-run \`bellwether hooks install --write\``);
      else add('hook token', 'PASS', 'entries carry the token from .env');
    }
  }

  if (settings?.disableAllHooks === true) {
    add('disableAllHooks', 'FAIL', `${file} sets "disableAllHooks": true, so no hook runs (bellwether's or anyone else's)`);
  }
  if (Array.isArray(settings?.allowedHttpHookUrls)) {
    const url = hookUrl(port);
    if (!settings.allowedHttpHookUrls.some((p) => globMatch(String(p), url))) {
      add('allowedHttpHookUrls', 'FAIL', `${file} sets allowedHttpHookUrls without a pattern matching ${url}; Claude Code will skip bellwether's hooks. Add "${url}"`);
    } else {
      add('allowedHttpHookUrls', 'PASS', `allows ${url}`);
    }
  }
  if (Array.isArray(settings?.httpHookAllowedEnvVars)) {
    const absent = Object.values(ENV_HEADERS).filter((v) => !settings.httpHookAllowedEnvVars.includes(v));
    if (absent.length) add('httpHookAllowedEnvVars', 'WARN', `does not allow ${absent.join(', ')}; those terminal headers will arrive empty and click-to-focus will degrade`);
  }
  return out;
}

/** `*` matches any run of characters; everything else is literal. */
export function globMatch(pattern, value) {
  const re = new RegExp('^' + pattern.split('*').map((s) => s.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*') + '$');
  return re.test(value);
}

function claudeVersion() {
  try {
    return execFileSync('claude', ['--version'], { encoding: 'utf8', timeout: 5000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  } catch {
    return 'claude not found on PATH (fine for the brain; needed for --e2e)';
  }
}

function portOwner(port) {
  if (process.platform === 'win32') return '';
  try {
    const out = execFileSync('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fpc'], { encoding: 'utf8', timeout: 3000, stdio: ['ignore', 'pipe', 'ignore'] });
    const pid = out.match(/^p(\d+)/m)?.[1];
    const cmd = out.match(/^c(.+)$/m)?.[1];
    return pid ? ` (pid ${pid}, ${cmd})` : '';
  } catch {
    return '';
  }
}

export function probeHealth(port, timeoutMs) {
  return new Promise((resolve) => {
    const req = http.get({ host: BIND_HOST, port, path: '/health', timeout: timeoutMs }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        try {
          const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
          if (body?.name === 'bellwether' && body.ok) return resolve({ kind: 'bellwether', body });
        } catch {}
        resolve({ kind: 'other', detail: `HTTP ${res.statusCode} that isn't bellwether's /health` });
      });
    });
    req.on('timeout', () => {
      req.destroy();
      resolve({ kind: 'timeout' });
    });
    req.on('error', (e) => {
      if (e.code === 'ECONNREFUSED') resolve({ kind: 'refused' });
      else resolve({ kind: 'other', detail: e.code ?? e.message });
    });
  });
}

function probeHook(port, token, timeoutMs) {
  return new Promise((resolve) => {
    const body = JSON.stringify({ hook_event_name: DOCTOR_PING });
    const req = http.request(
      { host: BIND_HOST, port, path: '/hook', method: 'POST', timeout: timeoutMs, headers: { 'Content-Type': 'application/json', 'X-Bellwether-Token': token, 'Content-Length': Buffer.byteLength(body) } },
      (res) => {
        res.resume();
        res.on('end', () => resolve({ status: res.statusCode }));
      },
    );
    req.on('timeout', () => {
      req.destroy();
      resolve({ error: 'timeout' });
    });
    req.on('error', (e) => resolve({ error: e.code ?? e.message }));
    req.end(body);
  });
}

export function printDoctor(result, write = (s) => process.stdout.write(s)) {
  write(`bellwether doctor (settings: ${result.settingsFile})\n`);
  for (const c of result.checks) write(`  ${c.status.padEnd(4)}  ${c.name.padEnd(22)} ${c.detail}\n`);
  const fails = result.checks.filter((c) => c.status === 'FAIL').length;
  write(fails ? `\n${fails} check(s) failed (exit ${result.exitCode}).\n` : '\nAll checks passed.\n');
}

/** Where Claude Code reads managed (policy) settings on each OS. */
export const MANAGED_SETTINGS = {
  darwin: '/Library/Application Support/ClaudeCode/managed-settings.json',
  linux: '/etc/claude-code/managed-settings.json',
  win32: 'C:\\ProgramData\\ClaudeCode\\managed-settings.json',
};

/**
 * Claude Code merges user, project, local and managed settings. Hooks in the user
 * file can still be switched off from the other levels, so doctor reads those too
 * (never writes them): `disableAllHooks`, `allowedHttpHookUrls` (a merged allowlist:
 * if any level defines one, bellwether's URL must match an entry from some level),
 * `httpHookAllowedEnvVars`, and managed `allowManagedHooksOnly`.
 * Project and local mean `<projectDir>/.claude/settings.json` and
 * `settings.local.json`; doctor uses the directory it was run from.
 */
export function checkOtherLevels({ projectDir, managedFile, port, userSettings = null }) {
  const out = [];
  const add = (name, status, detail, exitCode = EXIT.BAD_CONFIG) => out.push({ name, status, detail, exitCode: status === 'FAIL' ? exitCode : EXIT.OK });
  const levels = [];
  if (projectDir) {
    levels.push({ label: 'project', file: path.join(projectDir, '.claude', 'settings.json') });
    levels.push({ label: 'local', file: path.join(projectDir, '.claude', 'settings.local.json') });
  }
  if (managedFile) levels.push({ label: 'managed', file: managedFile });
  const found = [];
  const allow = [];
  let allowDefined = false;
  const envAllow = [];
  let envDefined = false;
  if (userSettings && Array.isArray(userSettings.allowedHttpHookUrls)) {
    allowDefined = true;
    allow.push(...userSettings.allowedHttpHookUrls);
  }
  for (const { label, file } of levels) {
    let text;
    try {
      text = fs.readFileSync(file, 'utf8');
    } catch {
      continue;
    }
    let s;
    try {
      s = JSON.parse(text);
    } catch {
      add(`${label} settings`, 'WARN', `${file} is not valid JSON; Claude Code ignores it, and doctor can't check it`);
      continue;
    }
    if (!s || typeof s !== 'object' || Array.isArray(s)) continue;
    found.push(`${label} (${file})`);
    if (s.disableAllHooks === true) add(`${label} settings`, 'FAIL', `${file} sets "disableAllHooks": true, so no hook runs in that ${label === 'managed' ? 'machine' : 'project'}, bellwether's included`);
    if (label === 'managed' && s.allowManagedHooksOnly === true) add('managed settings', 'FAIL', `${file} sets "allowManagedHooksOnly": true, so hooks from ~/.claude/settings.json (bellwether's) never run`);
    if (Array.isArray(s.allowedHttpHookUrls)) {
      allowDefined = true;
      allow.push(...s.allowedHttpHookUrls);
    }
    if (Array.isArray(s.httpHookAllowedEnvVars)) {
      envDefined = true;
      envAllow.push(...s.httpHookAllowedEnvVars);
    }
  }
  if (levels.length) add('other settings levels', 'INFO', found.length ? `also read: ${found.join(', ')}` : `no project, local or managed settings file found${projectDir ? ` for ${projectDir}` : ''}`);
  if (found.length && allowDefined) {
    const url = hookUrl(port);
    if (!allow.some((p) => globMatch(String(p), url))) add('allowedHttpHookUrls (merged)', 'FAIL', `the merged allowedHttpHookUrls across settings levels has no pattern matching ${url}; Claude Code will skip bellwether's hooks`);
  }
  if (found.length && envDefined) {
    const absent = Object.values(ENV_HEADERS).filter((v) => !envAllow.includes(v));
    if (absent.length) add('httpHookAllowedEnvVars (merged)', 'WARN', `the merged list doesn't allow ${absent.join(', ')}; those terminal headers arrive empty, and sheep identity and click-to-focus degrade`);
  }
  return out;
}

/**
 * `doctor --e2e`: one real, cheap `claude -p` run (haiku) with bellwether's hooks from a
 * throwaway settings file, in a temp directory, with user settings and MCP servers
 * excluded (`--setting-sources project,local --strict-mcp-config`), so nothing of
 * yours runs. Passes when the brain's accepted-event counter rises by at least the
 * three events such a run produces (UserPromptSubmit, Stop, SessionEnd).
 */
export async function runE2E({ port, token, probeTimeoutMs = 1000, runClaude = defaultRunClaude }) {
  const before = await probeHealth(port, probeTimeoutMs);
  if (before.kind !== 'bellwether') return { ok: false, detail: 'the brain stopped answering /health before the run' };
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bellwether-e2e-'));
  const hooksFile = path.join(dir, 'hooks-only.json');
  fs.writeFileSync(hooksFile, JSON.stringify(hookBlock(port, token), null, 2) + '\n', { mode: 0o600 });
  const started = Date.now();
  try {
    const r = await runClaude({ cwd: dir, hooksFile });
    if (r.error) return { ok: false, detail: `could not run claude: ${r.error}` };
    if (r.code !== 0) {
      // Say why: the last lines of claude's stderr, through the same redaction as a
      // bubble (it can echo an API key or a URL with credentials).
      const tail = r.stderr ? makeTextCleaner({ maxChars: 300 })(r.stderr.trim().split('\n').slice(-3).join(' | ')) : '';
      return { ok: false, detail: `claude -p exited ${r.code}${tail ? `: ${tail}` : ' with nothing on stderr'}` };
    }
    await new Promise((res) => setTimeout(res, 300));
    const after = await probeHealth(port, probeTimeoutMs);
    const delta = (after.body?.counters?.accepted ?? 0) - (before.body?.counters?.accepted ?? 0);
    const secs = ((Date.now() - started) / 1000).toFixed(1);
    if (delta >= 3) return { ok: true, detail: `a live \`claude -p\` (haiku) delivered ${delta} hook events to the brain in ${secs} s` };
    return { ok: false, detail: `claude -p ran (${secs} s) but the brain saw only ${delta} new events (expected ≥ 3). Is Claude Code skipping HTTP hooks? Run with --debug-file to see` };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

function defaultRunClaude({ cwd, hooksFile }) {
  // When doctor runs inside Claude Code, the child would inherit the parent's
  // session markers; drop them so it behaves like a fresh terminal.
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !(k === 'CLAUDECODE' || k === 'AI_AGENT' || k === 'CLAUDE_PID' || k === 'CLAUDE_EFFORT' || k.startsWith('CLAUDE_CODE_'))));
  const args = ['-p', 'Reply with the single word OK.', '--model', 'haiku', '--settings', hooksFile, '--setting-sources', 'project,local', '--strict-mcp-config', '--no-session-persistence'];
  return new Promise((resolve) => {
    let stderr = '';
    let child;
    try {
      child = spawn('claude', args, { cwd, env, stdio: ['ignore', 'pipe', 'pipe'] });
    } catch (e) {
      return resolve({ error: e.message });
    }
    const timer = setTimeout(() => child.kill('SIGTERM'), 120_000);
    // claude -p reports some failures (authentication, for one) on stdout, so both
    // streams feed the cause; only the last few lines are ever shown.
    child.stdout.on('data', (d) => (stderr = (stderr + d).slice(-4000)));
    child.stderr.on('data', (d) => (stderr = (stderr + d).slice(-4000)));
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ error: e.code === 'ENOENT' ? 'claude is not on PATH' : e.message });
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code, stderr });
    });
  });
}

/**
 * The one thing to do next, in the first-run order (README, "First run"): token, pet
 * art, build the strip, start at login, then hooks. Hooks come last on purpose: with
 * hooks installed and no brain, every Claude Code session prints hook errors.
 */
export function nextStep({ checks, installed, home = os.homedir(), dataDir = DATA_DIR, petOk = true, platform = process.platform, shellBuilt = fs.existsSync(shellBinary()) }) {
  const status = (n) => checks.find((c) => c.name === n)?.status;
  const P = './bellwether';
  if (status('config') === 'FAIL') return 'fix config.json (the config line above says what is wrong)';
  if (status('token') === 'FAIL') return 'put a token in the .env: `./bellwether token init` (README, Setup step 1)';
  if (!petOk) return `fetch the pet art: \`${P} pets fetch\``;
  if (!shellBuilt) return `build the strip that starts at login (a few minutes, once): \`${P} shell build --release\` (to try it in a terminal first: \`${P} shell --dev --release\`)`;
  const autostarted = fs.existsSync(plistPath(home));
  if (platform === 'darwin' && (!autostarted || !fs.existsSync(plistPath(home, SHELL_LABEL)))) return `start bellwether at login: \`${P} autostart install\` (preview), then the same with --write`;
  if (status('brain') !== 'PASS') return autostarted ? `the brain should be running from its LaunchAgent but isn't: see ${path.join(dataDir, 'brain.log')}, then \`launchctl kickstart -k gui/$(id -u)/local.bellwether.brain\`` : `start the brain: \`${P} serve\``;
  // Anything else that failed comes before hooks and before "all set": name the first
  // one, with its own fix (a token mismatch, disableAllHooks, a bad timeout, …).
  const failing = checks.find((c) => c.status === 'FAIL' && !['config', 'token', 'brain', 'e2e'].includes(c.name));
  if (failing) return `fix "${failing.name}": ${failing.detail}`;
  if (!installed) return `install the hooks: \`${P} hooks install\` (preview), then the same with --write; then open a new Claude Code session`;
  if (checks.some((c) => c.status === 'FAIL')) return 'fix the FAIL lines above first';
  return 'all set: open a Claude Code session and its sheep appears above the Dock';
}

/**
 * The shell logs what its TerminalLocator couldn't do (Milestone 8): Automation
 * refused (no exact iTerm2/Terminal window, and clicks only activate the app), or the
 * herdr CLI missing. Only lines since the shell last started count.
 */
export function checkLocatorLog(dataDir) {
  let text;
  try {
    const file = path.join(dataDir, 'shell.log');
    const size = fs.statSync(file).size;
    const fd = fs.openSync(file, 'r');
    const len = Math.min(size, 512 * 1024);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, size - len);
    fs.closeSync(fd);
    text = buf.toString('utf8');
  } catch {
    return [];
  }
  // Only lines the shell wrote itself: "<ms> bellwether-shell: <message>", where the
  // message isn't one the webview sent through shell_log ("webview: …"). A pet's text
  // or anything else the webview logs can never pose as the locator, or as a restart.
  const own = text
    .split('\n')
    .map((l) => /^\d+ bellwether-shell: (?!webview:)(.*)$/.exec(l)?.[1])
    .filter((m) => m !== undefined);
  const start = own.findLastIndex((m) => m.startsWith('starting:'));
  const recent = own.slice(Math.max(0, start)).filter((m) => m.startsWith('locator: ')).join('\n');
  const out = [];
  const refused = [...new Set([...recent.matchAll(/^locator: Automation for (\S+) was refused/gm)].map((m) => m[1]))];
  if (refused.length) out.push({ name: 'automation', status: 'WARN', detail: `the overlay was refused Automation for ${refused.join(' and ')}: sheep go by the terminal app's frontmost window, and a click only brings ${refused.join('/')} forward instead of selecting the tab. Allow it in System Settings > Privacy & Security > Automation (bellwether-shell, or the app that started it > ${refused.join(', ')}), then restart the shell` });
  const herdr = /^locator: herdr CLI not found at ("[^"]*")/m.exec(recent);
  // Dock-width confinement (Milestone 10): the shell says at start whether it may.
  const dockLines = own.slice(Math.max(0, start)).filter((m) => m.startsWith('dock: Accessibility'));
  const dock = dockLines[dockLines.length - 1];
  // A strip that had the grant in an earlier run and starts without it now: the binary
  // was rebuilt, and System Settings still shows the old bellwether-shell entry switched
  // on, which belongs to the previous build. Turning it off and on doesn't help; it has
  // to be removed and granted again.
  const grantedBefore = own.slice(0, Math.max(0, start)).some((m) => /^dock: Accessibility granted/.test(m));
  if (dock && /not granted/.test(dock) && grantedBefore) out.push({ name: 'dock', status: 'WARN', detail: 'Accessibility was granted to an earlier build of the strip and is not granted to this one, so the sheep use the full-width strip. System Settings > Privacy & Security > Accessibility still lists the old bellwether-shell entry, which belongs to the previous binary: select it and remove it with −, then run `./bellwether shell dock-permission`, turn the new bellwether-shell on, and `bellwether shell restart`. After every rebuild it must be given again' });
  else if (dock && /not granted|withdrawn/.test(dock)) out.push({ name: 'dock', status: 'INFO', detail: 'the strip uses its full width; to have the sheep stand on the Dock itself, run `./bellwether shell dock-permission` and turn bellwether-shell on in System Settings > Privacy & Security > Accessibility, then `bellwether shell restart`. The grant belongs to the built binary, so after a rebuild it must be given again. If bellwether-shell is already listed there and switched on, that entry is from an earlier build: remove it with − and grant again' });
  else if (dock) out.push({ name: 'dock', status: 'PASS', detail: 'Accessibility granted: sheep stand on the Dock\'s top edge' });
  if (herdr) out.push({ name: 'herdr CLI', status: 'WARN', detail: `the overlay couldn't run herdr at ${herdr[1]} (locator.herdrPath), so sheep in herdr panes are placed and focused through iTerm2/Terminal only` });
  return out;
}

