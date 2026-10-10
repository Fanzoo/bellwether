import { createHash } from 'node:crypto';
import path from 'node:path';
import { cleanLabel, truncate } from './redact.mjs';

/**
 * Builds the internal HookEvent from an allowlist of payload fields. The input is
 * never copied and pruned: a field Claude Code adds in a future version is dropped
 * because nothing here names it.
 *
 * Free text is kept only when the caller passes a `clean` function (redact, then
 * truncate; see redact.mjs), and only from two places: `toolSummary`, from one
 * allowlisted tool_input field per tool, and `message`, from Notification and
 * Elicitation. The recorder passes no cleaner, so recordings never hold free text.
 */

/** Terminal identity headers set by pasture's own hook block (see hooks.mjs). */
export const TERMINAL_HEADERS = {
  program: 'x-pasture-term-program',
  itermSession: 'x-pasture-iterm-session',
  termSession: 'x-pasture-term-session',
  wtSession: 'x-pasture-wt-session',
  tmuxPane: 'x-pasture-tmux-pane',
  herdrPane: 'x-pasture-herdr-pane',
  herdrSocket: 'x-pasture-herdr-socket',
};

/** Payload field -> HookEvent field. Every value must be a short identifier or enum. */
const SCALARS = {
  session_id: 'sessionId',
  agent_id: 'agentId',
  agent_type: 'agentType',
  tool_name: 'toolName',
  tool_use_id: 'toolUseId',
  notification_type: 'notificationType',
  source: 'source',
  reason: 'reason',
  trigger: 'trigger',
  mcp_server_name: 'mcpServer',
  mode: 'mode',
  permission_mode: 'permissionMode',
};

const BOOLEANS = { is_interrupt: 'isInterrupt', stop_hook_active: 'stopHookActive' };

/** The tool_input fields `toolSummary` reads, for any tool. */
export const SUMMARY_FIELDS = ['command', 'file_path', 'notebook_path', 'url', 'query', 'pattern', 'subagent_type'];

/**
 * Every top-level payload field this module reads. Replay must never fill these with
 * a placeholder, or the placeholder would surface as text (M2 verification D2).
 */
export const READ_FIELDS = ['hook_event_name', ...Object.keys(SCALARS), ...Object.keys(BOOLEANS), 'cwd', 'error', 'background_tasks', 'tool_input', 'message'];

const MAX_SCALAR = 200;
// Scalars must look like identifiers or enums. Anything else (free text a future
// version might put in one of these fields) is dropped rather than kept.
const IDENTIFIER = /^[A-Za-z0-9_.:@-]+$/;

/** A lamb's label (its agent type) is cut to this many characters, ending in "…". */
export const AGENT_TYPE_CHARS = 24;

export function normalise(payload, headers = {}, receivedAt = Date.now(), { clean = null } = {}) {
  // The event name is written to logs and recordings, so it gets the identifier check too.
  const name = str(payload.hook_event_name, 64);
  const ev = { receivedAt, event: name && IDENTIFIER.test(name) ? name : 'unknown' };
  for (const [from, to] of Object.entries(SCALARS)) {
    const v = str(payload[from]);
    if (v !== undefined && IDENTIFIER.test(v)) ev[to] = v;
  }
  // The agent type becomes a lamb's label and bubble prefix: free text as far as the
  // screen is concerned, so it gets the bubble cleaner (redaction) and a short cap.
  if (ev.agentType) {
    const label = truncate(clean ? (clean(ev.agentType) ?? '') : ev.agentType, AGENT_TYPE_CHARS);
    if (label) ev.agentType = label;
    else delete ev.agentType;
  }
  for (const [from, to] of Object.entries(BOOLEANS)) {
    if (typeof payload[from] === 'boolean') ev[to] = payload[from];
  }
  const cwd = str(payload.cwd, 4096);
  if (cwd) {
    ev.cwd = cwd;
    // A folder name is free text too: control, bidi and zero-width characters out.
    const label = cleanLabel(path.basename(cwd), 200);
    if (label) ev.projectLabel = label;
  }
  // StopFailure carries an error enum; PostToolUseFailure's `error` is free text and is dropped.
  if (ev.event === 'StopFailure') {
    const e = str(payload.error);
    if (e && /^[a-z_]+$/.test(e)) ev.errorType = e;
  }
  if (Array.isArray(payload.background_tasks)) ev.backgroundTaskCount = payload.background_tasks.length;
  if (payload.tool_input !== undefined && ev.toolName) {
    ev.toolFingerprint = fingerprint(ev.toolName, payload.tool_input);
  }
  if (clean) {
    const summary = ev.toolName ? toolSummary(ev.toolName, payload.tool_input) : undefined;
    const s = summary === undefined ? undefined : clean(summary);
    if (s) ev.toolSummary = s;
    if (ev.event === 'Notification' || ev.event === 'Elicitation') {
      const m = clean(payload.message);
      if (m) ev.message = m;
    }
  }
  ev.terminal = terminalFrom(headers);
  return ev;
}

/**
 * The one tool_input field each tool contributes to its bubble, as raw text before
 * cleaning. Every other tool_input field is never read. A tool not listed here
 * shows its name only.
 */
export function toolSummary(toolName, input) {
  const mcp = /^mcp__(.+?)__(.+)$/.exec(toolName);
  if (mcp) return `${mcp[1]}:${mcp[2]}`;
  if (!input || typeof input !== 'object' || Array.isArray(input)) return undefined;
  const field = (k) => (typeof input[k] === 'string' && input[k] !== '' ? input[k] : undefined);
  switch (toolName) {
    case 'Bash':
      return field('command');
    case 'Edit':
    case 'Write':
    case 'Read':
    case 'NotebookEdit': {
      const p = field('file_path') ?? field('notebook_path');
      return p === undefined ? undefined : path.basename(p.replace(/\\/g, '/'));
    }
    case 'WebFetch': {
      const u = field('url');
      try {
        return u === undefined ? undefined : new URL(u).host || undefined;
      } catch {
        return undefined;
      }
    }
    case 'WebSearch':
      return field('query');
    case 'Grep':
    case 'Glob':
      return field('pattern');
    case 'Task':
    case 'Agent':
      return field('subagent_type');
    default:
      return undefined;
  }
}

/**
 * herdr values are later handed to the herdr CLI (Milestone 8), so they must look like
 * what herdr makes: a pane id is 1 to 64 of `A-Z a-z 0-9 _ . : -`, not starting with
 * "-" (herdr would read it as an option); a socket path is absolute, at most 1024
 * bytes, with no ".." and no control characters. Anything else is dropped here and
 * never reaches state. (The shell checks both again, and that the socket is a socket
 * of the user's own.)
 */
export const HERDR_PANE_RE = /^(?!-)[A-Za-z0-9_.:-]{1,64}$/;
export function validHerdrSocket(p) {
  return typeof p === 'string' && p.startsWith('/') && Buffer.byteLength(p) <= 1024 && !p.split('/').includes('..') && !/[\u0000-\u001f\u007f]/.test(p);
}

export function terminalFrom(headers) {
  const t = {};
  for (const [field, header] of Object.entries(TERMINAL_HEADERS)) {
    // herdr values are checked whole: a long one is dropped, never cut to fit.
    const v = field.startsWith('herdr') ? headers[header] : str(headers[header], 512);
    if (typeof v !== 'string' || !v) continue;
    if (field === 'herdrPane' && !HERDR_PANE_RE.test(v)) continue;
    if (field === 'herdrSocket' && !validHerdrSocket(v)) continue;
    t[field] = v;
  }
  return t;
}

/**
 * sha256 over the tool name and its input, canonicalised (keys sorted, compact).
 * PermissionRequest has no tool_use_id, so this is how it is matched to the
 * PreToolUse it belongs to. Computed before tool_input is discarded.
 */
export function fingerprint(toolName, toolInput) {
  return createHash('sha256').update(`${toolName}\n${canonical(toolInput)}`).digest('hex').slice(0, 16);
}

export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`;
  }
  return JSON.stringify(value ?? null);
}

/**
 * Field names only, never values: the top-level keys of the payload and of
 * tool_input. This is how the spike learns what each event carries.
 */
export function rawKeys(payload) {
  const keys = { top: Object.keys(payload).sort() };
  if (payload.tool_input && typeof payload.tool_input === 'object' && !Array.isArray(payload.tool_input)) {
    keys.tool_input = Object.keys(payload.tool_input).sort();
  }
  return keys;
}

/** Names of the X-Pasture-* headers that arrived non-empty. */
export function headersPresent(headers) {
  return Object.keys(headers)
    .filter((h) => h.startsWith('x-pasture-') && h !== 'x-pasture-token' && String(headers[h]).length > 0)
    .sort();
}

function str(v, max = MAX_SCALAR) {
  if (typeof v !== 'string' || v.length === 0) return undefined;
  return v.length > max ? v.slice(0, max) : v;
}
