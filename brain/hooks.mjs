import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { CliError, EXIT, fileStamp, setOwn } from './util.mjs';

/**
 * bellwether's hook block, and the installer that merges it into a Claude Code
 * settings file.
 *
 * The settings file already holds other tools' hooks (afplay sounds, other tools,
 * herdr). The installer only ever appends its own matcher groups and removes its
 * own handlers. A handler is bellwether's if, and only if, it is an http handler whose
 * url is exactly http://127.0.0.1:<port>/hook (Decision 13, exact-URL rule since
 * Milestone 2). A foreign `…/hooks` or `…/hook2` is never claimed. Nothing else is
 * touched, reordered or deduplicated, and `assertOthersPreserved` checks that before
 * any write. Entries written by pasture (the name before the rename) are on the same
 * URL, so they are bellwether's too and get replaced. Entries left on another port after a port change are bellwether-like but
 * not bellwether's: doctor flags them, and `hooks uninstall --port <old>` removes them.
 */

/**
 * Subscribed events. SessionStart is deliberately absent: Claude Code 2.1.295 skips
 * HTTP hooks on SessionStart and Setup ("HTTP hooks are not supported for
 * SessionStart" in its debug log), so an entry there would never fire (measured
 * against Claude Code).
 */
export const EVENTS = [
  'SessionEnd',
  'UserPromptSubmit',
  'PreToolUse',
  'PostToolUse',
  'PostToolUseFailure',
  'PermissionRequest',
  'PermissionDenied',
  'Notification',
  'SubagentStart',
  'SubagentStop',
  'Stop',
  'StopFailure',
  'PreCompact',
  'PostCompact',
  'Elicitation',
  'ElicitationResult',
];

/** Events on which Claude Code silently skips every HTTP hook (measured, 2.1.295). */
export const HTTP_UNSUPPORTED_EVENTS = ['SessionStart', 'Setup'];

/** Header name -> environment variable interpolated by Claude Code at hook time. */
export const ENV_HEADERS = {
  'X-Bellwether-Term-Program': 'TERM_PROGRAM',
  'X-Bellwether-Iterm-Session': 'ITERM_SESSION_ID',
  'X-Bellwether-Term-Session': 'TERM_SESSION_ID',
  'X-Bellwether-Wt-Session': 'WT_SESSION',
  'X-Bellwether-Tmux-Pane': 'TMUX_PANE',
  'X-Bellwether-Herdr-Pane': 'HERDR_PANE_ID',
  'X-Bellwether-Herdr-Socket': 'HERDR_SOCKET_PATH',
};

/** Above this, a hung brain would stall tool calls noticeably. `doctor` enforces it. */
export const MAX_TIMEOUT_SEC = 2;

export const hookUrl = (port) => `http://127.0.0.1:${port}/hook`;

export function hookHandler(port, token) {
  const headers = { 'X-Bellwether-Token': token };
  for (const [h, env] of Object.entries(ENV_HEADERS)) headers[h] = `$${env}`;
  return {
    type: 'http',
    url: hookUrl(port),
    timeout: 1,
    headers,
    allowedEnvVars: Object.values(ENV_HEADERS),
  };
}

/** A complete settings object holding only bellwether's hooks: what `hooks print` emits. */
export function hookBlock(port, token) {
  const hooks = {};
  for (const event of EVENTS) hooks[event] = [{ hooks: [hookHandler(port, token)] }];
  return { hooks };
}

export function isBellwetherHandler(handler, port) {
  return (
    !!handler &&
    handler.type === 'http' &&
    handler.url === hookUrl(port)
  );
}

/** The port of a handler that looks like bellwether's on some port, else null. */
export function bellwetherLikePort(handler) {
  if (!handler || handler.type !== 'http' || typeof handler.url !== 'string') return null;
  const m = /^http:\/\/127\.0\.0\.1:(\d{1,5})\/hook$/.exec(handler.url);
  if (m) return Number(m[1]);
  return handlerToken(handler) !== undefined ? NaN : null;
}

/** The token header's value: X-Bellwether-Token, or X-Pasture-Token on an entry written before the rename. */
export function handlerToken(handler) {
  const h = handler?.headers;
  if (!h || typeof h !== 'object') return undefined;
  return h['X-Bellwether-Token'] ?? h['X-Pasture-Token'];
}

/**
 * An entry written by pasture, before the rename: on the exact URL, with the old
 * X-Pasture-Token header and not the new one. The exact-URL rule already makes it ours,
 * so `hooks install` replaces it and `hooks uninstall` removes it.
 */
export function isLegacyHandler(handler, port) {
  const h = handler?.headers;
  return isBellwetherHandler(handler, port) && !!h && typeof h === 'object' && h['X-Pasture-Token'] !== undefined && h['X-Bellwether-Token'] === undefined;
}

/**
 * Returns a new settings object with exactly one bellwether matcher group per
 * subscribed event. An event whose only bellwether entry is already a lone, current
 * handler is left alone, so a second install is a no-op. Otherwise stale bellwether
 * handlers on that event are removed and a fresh group is appended at the end.
 */
export function install(settings, port, token) {
  assertShape(settings);
  const out = cloneJson(settings);
  const wanted = hookHandler(port, token);
  if (out.hooks === undefined) out.hooks = {};
  for (const event of EVENTS) {
    const groups = out.hooks[event] ?? [];
    const ours = groups.filter((g) => isBellwetherGroup(g, port));
    const current =
      ours.length === 1 &&
      ours[0].matcher === undefined &&
      ours[0].hooks.length === 1 &&
      deepEqual(ours[0].hooks[0], wanted) &&
      Object.keys(ours[0]).length === 1;
    if (current) continue;
    const kept = removeFromGroups(groups, port);
    kept.push({ hooks: [cloneJson(wanted)] });
    setOwn(out.hooks, event, kept);
  }
  // Bellwether entries on events it no longer subscribes to (e.g. SessionStart) go.
  for (const [event, groups] of Object.entries(out.hooks)) {
    if (EVENTS.includes(event)) continue;
    const kept = removeFromGroups(groups, port);
    if (kept.length === groups.length && kept.every((g, i) => g === groups[i])) continue;
    if (kept.length === 0) delete out.hooks[event];
    else setOwn(out.hooks, event, kept);
  }
  return out;
}

/**
 * Removes every bellwether handler from every event (not just the subscribed ones, so
 * an event dropped in a later version is cleaned up too). A matcher group is removed
 * only when bellwether's removal emptied it. An event key, or the hooks object, that
 * bellwether's removal emptied is removed only if bellwether created it: `created` comes
 * from the install record (see installRecord below). Without a record, an emptied
 * key is assumed to be bellwether's and removed.
 */
export function uninstall(settings, port, created = null) {
  assertShape(settings);
  const out = cloneJson(settings);
  if (out.hooks === undefined) return out;
  let emptied = false;
  for (const [event, groups] of Object.entries(out.hooks)) {
    const kept = removeFromGroups(groups, port);
    if (kept.length === groups.length && kept.every((g, i) => g === groups[i])) continue;
    if (kept.length > 0 || (created && !created.events.includes(event))) {
      setOwn(out.hooks, event, kept);
    } else {
      delete out.hooks[event];
      emptied = true;
    }
  }
  if (emptied && Object.keys(out.hooks).length === 0 && (!created || created.hooks)) delete out.hooks;
  return out;
}

function removeFromGroups(groups, port) {
  const kept = [];
  for (const g of groups) {
    if (!isBellwetherGroup(g, port)) {
      kept.push(g);
      continue;
    }
    const rest = g.hooks.filter((h) => !isBellwetherHandler(h, port));
    if (rest.length > 0) kept.push({ ...g, hooks: rest });
  }
  return kept;
}

function isBellwetherGroup(g, port) {
  return Array.isArray(g?.hooks) && g.hooks.some((h) => isBellwetherHandler(h, port));
}

/**
 * Where the settings shape is something Claude Code itself would reject, name the
 * first few places. Used by the installer (refuse, exit 3) and by doctor (report).
 */
export function shapeProblems(settings) {
  const problems = [];
  if (!isPlainObject(settings)) return ['the file does not hold a JSON object'];
  if (settings.hooks === undefined) return problems;
  if (!isPlainObject(settings.hooks)) return ['"hooks" is not an object'];
  for (const [event, groups] of Object.entries(settings.hooks)) {
    if (!Array.isArray(groups)) {
      problems.push(`"hooks.${event}" is not an array`);
      continue;
    }
    groups.forEach((g, i) => {
      if (!isPlainObject(g)) problems.push(`"hooks.${event}[${i}]" is not an object`);
      else if (!Array.isArray(g.hooks)) problems.push(`"hooks.${event}[${i}].hooks" is not an array`);
    });
  }
  return problems;
}

function assertShape(settings) {
  const problems = shapeProblems(settings);
  if (problems.length) {
    throw new CliError(`the settings file has a shape Claude Code would reject: ${problems.slice(0, 3).join('; ')}. Fix it first; nothing was written.`, EXIT.BAD_CONFIG);
  }
}

/**
 * The guard behind every write: with bellwether's handlers removed from both versions,
 * and empty containers ignored on both sides, they must be deep-equal, including
 * order. Throws otherwise.
 */
export function assertOthersPreserved(before, after, port) {
  if (!deepEqual(othersOnly(before, port), othersOnly(after, port))) {
    throw new CliError('internal check failed: the edit would change a non-bellwether setting. Nothing was written.', EXIT.UNEXPECTED);
  }
}

function othersOnly(settings, port) {
  const out = cloneJson(settings);
  if (!isPlainObject(out.hooks)) return out;
  for (const [event, groups] of Object.entries(out.hooks)) {
    const kept = removeFromGroups(groups, port);
    if (kept.length === 0) delete out.hooks[event];
    else setOwn(out.hooks, event, kept);
  }
  if (Object.keys(out.hooks).length === 0) delete out.hooks;
  return out;
}

/** Counts bellwether matcher groups per event. Tolerates any shape. */
export function countGroups(settings, port) {
  const hooks = isPlainObject(settings?.hooks) ? settings.hooks : {};
  // Object.fromEntries defines own properties, so an event key "__proto__" is counted
  // like any other instead of reaching the prototype setter.
  return Object.fromEntries(
    Object.entries(hooks)
      .filter(([, groups]) => Array.isArray(groups))
      .map(([event, groups]) => [event, groups.filter((g) => isBellwetherGroup(g, port)).length])
      .filter(([, n]) => n > 0),
  );
}

/**
 * The install record, data/installs.json: for each settings file bellwether installed
 * into, which event keys (and whether the hooks object) the install created. Uninstall
 * uses it so `"Stop": []` or `"hooks": {}` that were already there come back as they
 * were. Contains paths and event names only.
 *
 * The record is advisory. Anything in it that isn't the expected shape is ignored
 * with a warning (it is "no record"), and failing to write it never fails a command
 * whose settings write already happened.
 */
export function readInstallRecords(dataDir) {
  const f = path.join(dataDir, 'installs.json');
  let raw;
  try {
    raw = fs.readFileSync(f, 'utf8');
  } catch (e) {
    return { all: Object.create(null), warning: e.code === 'ENOENT' ? null : `could not read ${f} (${e.code}); treating it as empty` };
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    parsed = undefined;
  }
  const all = Object.create(null);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { all, warning: `${f} is not a valid install record; ignoring it (uninstall falls back to removing event keys bellwether emptied)` };
  }
  let dropped = 0;
  for (const [k, r] of Object.entries(parsed)) {
    if (validRecord(r)) setOwn(all, k, { events: [...r.events], hooks: r.hooks });
    else dropped++;
  }
  return { all, warning: dropped ? `${f}: ignored ${dropped} malformed entr${dropped === 1 ? 'y' : 'ies'}` : null };
}

function validRecord(r) {
  return !!r && typeof r === 'object' && !Array.isArray(r) && Array.isArray(r.events) && r.events.every((e) => typeof e === 'string') && typeof r.hooks === 'boolean';
}

export function readInstallRecord(dataDir, file) {
  const { all, warning } = readInstallRecords(dataDir);
  const key = recordKey(file);
  return { record: Object.hasOwn(all, key) ? all[key] : null, warning };
}

function recordKey(file) {
  try {
    return fs.realpathSync(file);
  } catch {
    return path.resolve(file);
  }
}

/**
 * Plans an install or uninstall against a settings file. Reads only; `applyPlan`
 * does the write. A missing file counts as `{}` (Claude Code treats it the same).
 */
export function planEdit(file, action, { port, token, dataDir = null }) {
  let bytes = null;
  try {
    bytes = fs.readFileSync(file);
  } catch (e) {
    if (e.code !== 'ENOENT') throw new CliError(`cannot read ${file}: ${e.code}`, EXIT.BAD_CONFIG);
  }
  let before = {};
  const oldText = bytes === null ? '' : bytes.toString('utf8');
  if (bytes !== null) {
    try {
      before = parseLossless(oldText);
    } catch (e) {
      throw new CliError(`${file} is not valid JSON (${e.message}). Fix it first; nothing was written.`, EXIT.BAD_CONFIG);
    }
    if (!isPlainObject(before)) throw new CliError(`${file} does not hold a JSON object; nothing was written.`, EXIT.BAD_CONFIG);
  }
  const { record: previous, warning: recordWarning } = dataDir ? readInstallRecord(dataDir, file) : { record: null, warning: null };
  const after = action === 'install' ? install(before, port, token) : uninstall(before, port, previous);
  assertOthersPreserved(before, after, port);
  const changed = !deepEqual(before, after);
  const newText = changed ? serialise(after, oldText) : oldText;
  let record = null;
  if (action === 'install') {
    const had = before.hooks ?? {};
    const prevEvents = previous?.events ?? [];
    record = {
      events: EVENTS.filter((e) => !Object.hasOwn(had, e) || (prevEvents.includes(e) && countGroups({ hooks: { [e]: had[e] } }, port)[e] === had[e].length)),
      hooks: before.hooks === undefined || (!!previous?.hooks && Object.keys(had).every((e) => prevEvents.includes(e))),
    };
  }
  return {
    file,
    action,
    exists: bytes !== null,
    originalHash: bytes === null ? null : sha256(bytes),
    originalBytes: bytes,
    before,
    after,
    changed,
    oldText,
    newText,
    // Whether this file was already laid out exactly as bellwether writes JSON. When it
    // isn't (key spacing, inline arrays, mixed line endings), a write reformats
    // existing lines too, although every value stays the same.
    layoutKept: bytes === null || serialise(before, oldText) === oldText,
    record,
    warnings: recordWarning ? [recordWarning] : [],
  };
}

/**
 * Writes a planned edit: back up the original bytes, re-read and compare the hash
 * (another tool may have edited the file meanwhile), then replace the file
 * atomically through a temp file in the same directory. Backups hold the token, so
 * they are 0600; a settings file bellwether creates is 0600 too.
 */
export function applyPlan(plan, backupDir, dataDir = null) {
  if (!plan.changed) return null;
  // Everything that can be decided before writing is decided here: the new install
  // record's content, built only from valid entries.
  let recordText = null;
  if (dataDir) {
    const { all } = readInstallRecords(dataDir);
    const key = recordKey(plan.file);
    if (plan.action === 'install' && plan.record) setOwn(all, key, plan.record);
    else delete all[key];
    recordText = JSON.stringify(all, null, 2) + '\n';
  }
  const target = plan.exists ? fs.realpathSync(plan.file) : plan.file;
  let backup = null;
  if (plan.exists) {
    fs.mkdirSync(backupDir, { recursive: true });
    backup = path.join(backupDir, `settings-${fileStamp()}.json`);
    fs.writeFileSync(backup, plan.originalBytes, { flag: 'wx', mode: 0o600 });
    const now = fs.readFileSync(target);
    if (sha256(now) !== plan.originalHash) {
      throw new CliError(`${plan.file} changed while bellwether was editing it. Nothing was written; run the command again.`, EXIT.UNEXPECTED);
    }
  }
  const mode = plan.exists ? fs.statSync(target).mode & 0o777 : 0o600;
  fs.mkdirSync(path.dirname(target), { recursive: true });
  const tmp = path.join(path.dirname(target), `.${path.basename(target)}.bellwether-${process.pid}.tmp`);
  fs.writeFileSync(tmp, plan.newText, { mode });
  fs.chmodSync(tmp, mode);
  fs.renameSync(tmp, target);
  // The settings file is written. From here on nothing may fail the command: a
  // problem with the advisory record becomes a warning.
  if (recordText !== null) {
    try {
      fs.mkdirSync(dataDir, { recursive: true });
      const rf = path.join(dataDir, 'installs.json');
      fs.writeFileSync(rf, recordText, { mode: 0o600 });
      fs.chmodSync(rf, 0o600);
    } catch (e) {
      plan.warnings.push(`could not update ${path.join(dataDir, 'installs.json')} (${e.code ?? e.message}); a later uninstall falls back to removing event keys bellwether emptied`);
    }
  }
  return backup;
}

/**
 * JSON.parse that keeps every number literal exactly as written. A literal a double
 * can't reproduce (12345678901234567890, 1.0, 1e2) comes back as JSON.rawJSON, which
 * JSON.stringify writes out verbatim. Needs Node 22's reviver `context.source`.
 */
export function parseLossless(text) {
  return JSON.parse(text, (key, value, context) =>
    typeof value === 'number' && context?.source !== undefined && String(value) !== context.source ? JSON.rawJSON(context.source) : value,
  );
}

/** A deep copy that keeps JSON.rawJSON values (structuredClone would turn them into objects). */
export function cloneJson(v) {
  if (v === null || typeof v !== 'object' || JSON.isRawJSON(v)) return v;
  if (Array.isArray(v)) return v.map(cloneJson);
  // setOwn, not assignment: a "__proto__" key in a settings file must stay an
  // ordinary key and keep its contents.
  const out = {};
  for (const [k, x] of Object.entries(v)) setOwn(out, k, cloneJson(x));
  return out;
}

/**
 * JSON in the original file's own layout: its indentation (detected from the first
 * indented line), its line ending, and its trailing-newline choice. A new file gets
 * 2 spaces and LF.
 */
export function serialise(value, oldText = '') {
  const indent = oldText.match(/^[\r\n]*[{[][ \t]*\r?\n([ \t]+)\S/)?.[1] ?? '  ';
  const eol = oldText.includes('\r\n') ? '\r\n' : '\n';
  const text = JSON.stringify(value, null, indent).replace(/\n/g, eol);
  return oldText === '' || /\n$/.test(oldText) ? text + eol : text;
}

export function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function isPlainObject(v) {
  return !!v && typeof v === 'object' && !Array.isArray(v) && !JSON.isRawJSON(v);
}

export function deepEqual(a, b) {
  if (a === b) return true;
  if (typeof a !== typeof b || a === null || b === null || typeof a !== 'object') return false;
  if (JSON.isRawJSON(a) || JSON.isRawJSON(b)) return JSON.isRawJSON(a) && JSON.isRawJSON(b) && a.rawJSON === b.rawJSON;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) return a.length === b.length && a.every((x, i) => deepEqual(x, b[i]));
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  return ka.length === kb.length && ka.every((k) => Object.hasOwn(b, k) && deepEqual(a[k], b[k]));
}
