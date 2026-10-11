import fs from 'node:fs';
import { call } from './client.mjs';
import { READ_FIELDS, SUMMARY_FIELDS, TERMINAL_HEADERS } from './normalise.mjs';
import { CliError, EXIT } from './util.mjs';

/**
 * `bellwether replay`: posts a recording or fixture to a running brain with its original
 * timing (scaled by --speed), so the overlay can be driven without Claude, and
 * measures /hook latency and the snapshot rate while it does.
 *
 * Recordings hold field names, not values, so each payload is rebuilt:
 *   - every field the normalised event has goes back under its real Claude Code name
 *     (session_id, tool_use_id, notification_type, …);
 *   - tool_input is rebuilt from toolSummary through the same per-tool allowlist
 *     the normaliser reads (Bash command, file basename, URL host, …);
 *   - every other recorded field name (the prompt, the tool response, the transcript path,
 *     compact_summary, …, and the other tool_input keys) is sent with a marker value
 *     «replay:<name>». None of those may ever show up in a snapshot; a test checks.
 *   - a field the normaliser reads (READ_FIELDS, SUMMARY_FIELDS) never gets a marker:
 *     without a recorded value it is left out, and the bubble shows the tool name only.
 */

const FROM_NORMALISED = {
  sessionId: 'session_id',
  agentId: 'agent_id',
  agentType: 'agent_type',
  toolName: 'tool_name',
  toolUseId: 'tool_use_id',
  notificationType: 'notification_type',
  source: 'source',
  reason: 'reason',
  trigger: 'trigger',
  mcpServer: 'mcp_server_name',
  mode: 'mode',
  permissionMode: 'permission_mode',
  cwd: 'cwd',
  message: 'message',
  isInterrupt: 'is_interrupt',
  stopHookActive: 'stop_hook_active',
};

export const MARKER = (name) => `«replay:${name}»`;

export function readRecording(file) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (e) {
    throw new CliError(`cannot read ${file}: ${e.code}`, EXIT.BAD_CONFIG);
  }
  const lines = text.split('\n').filter((l) => l.trim());
  return lines.map((l, i) => {
    let o;
    try {
      o = JSON.parse(l);
    } catch {
      throw new CliError(`${file}:${i + 1} is not JSON; replay takes a bellwether recording (.jsonl)`, EXIT.BAD_CONFIG);
    }
    if (!o?.normalised?.event || typeof o.normalised.receivedAt !== 'number') throw new CliError(`${file}:${i + 1} is not a recording line ({normalised, rawKeys, headersPresent})`, EXIT.BAD_CONFIG);
    return o;
  });
}

/**
 * Rebuilds a payload and the X-Bellwether-* headers from one recording line. With a
 * `tag`, the line plays as a different session in a different terminal pane (session
 * ids get `-<tag>`, the terminal becomes herdr pane `<tag>`), so several fixtures can
 * drive several sheep at once.
 */
export function rebuild(line, { tag = null } = {}) {
  const n = tag ? { ...line.normalised, sessionId: line.normalised.sessionId && `${line.normalised.sessionId}-${tag}`, terminal: { program: 'iTerm.app', herdrPane: tag } } : line.normalised;
  const payload = { hook_event_name: n.event };
  for (const [from, to] of Object.entries(FROM_NORMALISED)) if (n[from] !== undefined) payload[to] = n[from];
  if (n.event === 'StopFailure' && n.errorType) payload.error = n.errorType;
  if (Number.isInteger(n.backgroundTaskCount)) payload.background_tasks = Array.from({ length: n.backgroundTaskCount }, (_, i) => ({ id: MARKER(`background_tasks.${i}`) }));
  const top = line.rawKeys?.top ?? [];
  if (top.includes('tool_input') || n.toolName) {
    if (top.includes('tool_input') || n.toolSummary !== undefined || n.toolFingerprint !== undefined) payload.tool_input = rebuildToolInput(n, line.rawKeys?.tool_input ?? []);
  }
  // A field the normaliser reads is sent only with a real value; without one it is
  // left out, so a real recording (field names only) gives tool-name-only bubbles
  // such as `Bash?` rather than a marker.
  for (const k of top) if (!(k in payload) && !READ_FIELDS.includes(k)) payload[k] = MARKER(k);
  const headers = {};
  for (const [field, header] of Object.entries(TERMINAL_HEADERS)) headers[header] = n.terminal?.[field] ?? '';
  return { payload, headers };
}

function rebuildToolInput(n, keys) {
  const input = {};
  const s = n.toolSummary;
  if (s !== undefined) {
    switch (n.toolName) {
      case 'Bash':
        input.command = s;
        break;
      case 'Edit':
      case 'Write':
      case 'Read':
        input.file_path = `${n.cwd ?? '/replay'}/${s}`;
        break;
      case 'NotebookEdit':
        input.notebook_path = `${n.cwd ?? '/replay'}/${s}`;
        break;
      case 'WebFetch':
        input.url = `https://${s}/`;
        break;
      case 'WebSearch':
        input.query = s;
        break;
      case 'Grep':
      case 'Glob':
        input.pattern = s;
        break;
      case 'Task':
      case 'Agent':
        input.subagent_type = s;
        break;
      default:
    }
  }
  for (const k of keys) if (!(k in input) && !SUMMARY_FIELDS.includes(k)) input[k] = MARKER(`tool_input.${k}`);
  return input;
}

/** A synthetic load: `pairs` PreToolUse/PostToolUse pairs at `rate` events per second. */
export function burst({ rate, seconds }) {
  const total = Math.max(2, Math.round(rate * seconds));
  const terminal = { herdrPane: 'w9:p9', program: 'iTerm.app' };
  const lines = [];
  for (let i = 0; i < total; i++) {
    const pre = i % 2 === 0;
    const id = `toolu_burst_${Math.floor(i / 2)}`;
    lines.push({
      normalised: { receivedAt: Math.round((i * 1000) / rate), event: pre ? 'PreToolUse' : 'PostToolUse', sessionId: 'burst-session', toolName: 'Read', toolUseId: id, cwd: '/replay/burst', toolSummary: `file${i % 7}.txt`, terminal },
      rawKeys: { top: ['cwd', 'duration_ms', 'hook_event_name', 'session_id', 'tool_input', 'tool_name', 'tool_use_id'], tool_input: ['file_path'] },
      headersPresent: [],
    });
  }
  return lines;
}

/**
 * Posts the lines to /hook on schedule, long-polling /state alongside to count
 * snapshots. Returns the measurements; the CLI prints them.
 */
export async function replay(lines, { port, token, speed = 1, tag = null, log = () => {} }) {
  if (!lines.length) throw new CliError('nothing to replay: the file has no events', EXIT.BAD_CONFIG);
  const t0 = lines[0].normalised.receivedAt;
  const latencies = [];
  const failures = [];
  const snapshots = [];
  let polling = true;

  // Count snapshots while replaying.
  const poller = (async () => {
    let rev = -1;
    while (polling) {
      let r;
      try {
        r = await call(port, { path: `/state?after=${rev}&wait=1000`, token, timeoutMs: 5000 });
      } catch {
        await new Promise((res) => setTimeout(res, 100));
        continue;
      }
      if (r.status !== 200 || !r.json) {
        failures.push(`/state answered ${r.status}`);
        return;
      }
      if (r.json.rev > rev) {
        if (rev >= 0) snapshots.push(Date.now());
        rev = r.json.rev;
      }
    }
  })();
  await new Promise((res) => setTimeout(res, 50));

  const start = Date.now();
  for (const [i, line] of lines.entries()) {
    const due = start + (line.normalised.receivedAt - t0) / speed;
    const wait = due - Date.now();
    if (wait > 0) await new Promise((res) => setTimeout(res, wait));
    const { payload, headers } = rebuild(line, { tag });
    const sentAt = Date.now();
    const sent = process.hrtime.bigint();
    let r;
    try {
      r = await call(port, { method: 'POST', path: '/hook', token, body: payload, headers, timeoutMs: 5000 });
    } catch (e) {
      if (e.code === 'ECONNREFUSED') throw new CliError(`brain not running — run \`bellwether serve\` (nothing listening on 127.0.0.1:${port})`, EXIT.NOT_RUNNING);
      failures.push(`line ${i + 1}: ${e.code ?? e.message}`);
      continue;
    }
    latencies.push(Number(process.hrtime.bigint() - sent) / 1e6);
    if (r.status !== 204 || r.text.length) failures.push(`line ${i + 1}: HTTP ${r.status}${r.text.length ? ` with a ${r.text.length}-byte body` : ''}`);
    log(i, line, r.status, sentAt);
  }
  const elapsed = (Date.now() - start) / 1000;
  await new Promise((res) => setTimeout(res, 600));
  polling = false;
  await poller;

  latencies.sort((a, b) => a - b);
  const pct = (p) => latencies[Math.min(latencies.length - 1, Math.floor((p / 100) * latencies.length))] ?? NaN;
  let maxPerSecond = 0;
  for (let i = 0; i < snapshots.length; i++) {
    let j = i;
    while (j < snapshots.length && snapshots[j] - snapshots[i] < 1000) j++;
    maxPerSecond = Math.max(maxPerSecond, j - i);
  }
  return { events: lines.length, elapsed, p50: pct(50), p99: pct(99), max: latencies.at(-1) ?? NaN, failures, snapshots: snapshots.length, maxPerSecond };
}
