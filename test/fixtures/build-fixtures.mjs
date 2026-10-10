/**
 * Rebuilds the invented fixtures. Run it by hand after changing what a fixture
 * should contain, then regenerate the expected transitions:
 *
 *   node test/fixtures/build-fixtures.mjs && node test/regen-expected.mjs
 *
 * S1–S11 keep the shape and timing of the Milestone 1 spike recordings (already
 * invented: ids, paths and commands made up); this adds the Milestone 2 free-text
 * fields (toolSummary, message) to them, from the same invented inputs their
 * fingerprints were computed from.
 *
 * S12 onwards are scenarios the spike could not record (elicitation, auto mode,
 * StopFailure, sandbox prompts, agent_needs_input, lamb-alert edge cases). Their
 * field names come from the M1 recordings where the event was recorded (rawKeys
 * below), and from the hooks reference where it wasn't (Elicitation,
 * ElicitationResult, PermissionDenied, StopFailure: marked "docs").
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fingerprint } from '../../brain/normalise.mjs';

const DIR = path.dirname(fileURLToPath(import.meta.url));
const CWD = '/Users/example/projects/meadow';
const TERMINAL = {
  program: 'iTerm.app',
  itermSession: 'w0t1p0:00000000-AAAA-4BBB-8CCC-000000000001',
  termSession: 'w0t1p0:00000000-AAAA-4BBB-8CCC-000000000001',
  herdrPane: 'w1:p3',
  herdrSocket: '/Users/example/.config/herdr/herdr.sock',
};
const HEADERS = ['x-pasture-herdr-pane', 'x-pasture-herdr-socket', 'x-pasture-iterm-session', 'x-pasture-term-program', 'x-pasture-term-session'];
const SESSION = '0f0e0d0c-1111-4aaa-8bbb-000000000001';
const BASE_T = 1_800_000_000_000;

// The same invented tool inputs the S1–S11 fingerprints were made from.
const BASH = ['touch meadow-approved.txt', 'touch meadow-denied.txt', 'touch meadow-escaped.txt', 'touch meadow-parallel.txt', 'touch meadow-lamb.txt'];
const INPUTS = [
  ['Read', { file_path: `${CWD}/README.md` }, 'README.md'],
  ...BASH.map((c) => ['Bash', { command: c, description: 'Create an empty file' }, c]),
  ['Agent', { description: 'Create a file', prompt: 'Run touch meadow-lamb.txt and report back.', subagent_type: 'general-purpose' }, 'general-purpose'],
];
const SUMMARY_BY_FP = new Map(INPUTS.map(([tool, input, summary]) => [fingerprint(tool, input), summary]));
const MESSAGES = {
  permission_prompt: 'Claude needs your permission to use Bash',
  idle_prompt: 'Claude is waiting for your input',
};

// ---------------------------------------------------------------- S1–S11

for (const f of fs.readdirSync(DIR).filter((f) => /^s([1-9]|1[01])-.*\.jsonl$/.test(f) && !f.includes('.expected.'))) {
  const file = path.join(DIR, f);
  const lines = read(file).map((l) => {
    const n = { ...l.normalised };
    delete n.toolSummary;
    delete n.message;
    const summary = n.toolFingerprint ? SUMMARY_BY_FP.get(n.toolFingerprint) : undefined;
    const out = {};
    for (const [k, v] of Object.entries(n)) {
      out[k] = v;
      if (k === 'toolFingerprint' && summary) out.toolSummary = summary;
    }
    if (n.event === 'Notification' && MESSAGES[n.notificationType]) out.message = MESSAGES[n.notificationType];
    return { ...l, normalised: reorder(out) };
  });
  write(file, lines);
}

// ------------------------------------------------------------ S12 onwards

// Payload field names per event. "M1" = as recorded by the spike; "docs" = from the
// hooks reference, because the spike never saw the event.
const KEYS = {
  UserPromptSubmit: ['cwd', 'hook_event_name', 'permission_mode', 'prompt', 'prompt_id', 'scratchpad_dir', 'session_id', 'transcript_path'], // M1
  PreToolUse: ['cwd', 'effort', 'hook_event_name', 'permission_mode', 'prompt_id', 'scratchpad_dir', 'session_id', 'tool_input', 'tool_name', 'tool_use_id', 'transcript_path'], // M1
  PermissionRequest: ['cwd', 'effort', 'hook_event_name', 'permission_mode', 'permission_suggestions', 'prompt_id', 'scratchpad_dir', 'session_id', 'tool_input', 'tool_name', 'transcript_path'], // M1
  PostToolUse: ['cwd', 'duration_ms', 'effort', 'hook_event_name', 'permission_mode', 'prompt_id', 'scratchpad_dir', 'session_id', 'tool_input', 'tool_name', 'tool_response', 'tool_use_id', 'transcript_path'], // M1
  PostToolUseFailure: ['cwd', 'duration_ms', 'error', 'hook_event_name', 'is_interrupt', 'permission_mode', 'prompt_id', 'session_id', 'tool_input', 'tool_name', 'tool_use_id', 'transcript_path'], // docs
  PermissionDenied: ['cwd', 'hook_event_name', 'permission_mode', 'reason', 'session_id', 'tool_input', 'tool_name', 'tool_use_id', 'transcript_path'], // docs
  Notification: ['cwd', 'hook_event_name', 'message', 'notification_type', 'prompt_id', 'scratchpad_dir', 'session_id', 'transcript_path'], // M1
  Stop: ['background_tasks', 'cwd', 'effort', 'hook_event_name', 'last_assistant_message', 'permission_mode', 'prompt_id', 'scratchpad_dir', 'session_crons', 'session_id', 'stop_hook_active', 'transcript_path'], // M1
  StopFailure: ['cwd', 'error', 'error_details', 'hook_event_name', 'last_assistant_message', 'session_id', 'transcript_path'], // docs
  SubagentStart: ['agent_id', 'agent_type', 'cwd', 'hook_event_name', 'prompt_id', 'scratchpad_dir', 'session_id', 'transcript_path'], // M1
  SubagentStop: ['agent_id', 'agent_transcript_path', 'agent_type', 'background_tasks', 'cwd', 'effort', 'hook_event_name', 'last_assistant_message', 'permission_mode', 'prompt_id', 'scratchpad_dir', 'session_crons', 'session_id', 'stop_hook_active', 'transcript_path'], // M1
  SessionEnd: ['cwd', 'hook_event_name', 'prompt_id', 'reason', 'scratchpad_dir', 'session_id', 'transcript_path'], // M1
  Elicitation: ['cwd', 'hook_event_name', 'mcp_server_name', 'message', 'mode', 'requested_schema', 'session_id', 'transcript_path'], // docs
  ElicitationResult: ['action', 'content', 'cwd', 'hook_event_name', 'mcp_server_name', 'session_id', 'transcript_path'], // docs
};
const SUBAGENT_EXTRA = ['agent_id', 'agent_type'];

function scenario(name, steps) {
  const lines = steps.map(([dt, event, n = {}, inputKeys]) => {
    const inSubagent = !!n.agentId && !['SubagentStart', 'SubagentStop'].includes(event);
    const top = [...new Set([...(KEYS[event] ?? []), ...(inSubagent ? SUBAGENT_EXTRA : [])])].sort();
    const normalised = reorder({ receivedAt: BASE_T + dt, event, sessionId: SESSION, permissionMode: 'default', cwd: CWD, projectLabel: 'meadow', ...n, terminal: TERMINAL });
    if (!top.includes('permission_mode')) delete normalised.permissionMode;
    const rawKeys = { top };
    if (top.includes('tool_input')) rawKeys.tool_input = inputKeys ?? [];
    return { normalised, rawKeys, headersPresent: HEADERS };
  });
  write(path.join(DIR, `${name}.jsonl`), lines);
}

const tool = (name, input, summary, id, extra = {}) => ({ toolName: name, toolUseId: id, toolFingerprint: fingerprint(name, input), toolSummary: summary, ...extra });
const noId = (o) => {
  const c = { ...o };
  delete c.toolUseId;
  return c;
};
const deploy = { environment: 'staging' };
const curl = { command: 'curl -s https://status.example.com', description: 'Check status' };
const rm = { command: 'rm -rf build', description: 'Remove build output' };
const ls = { command: 'ls', description: 'List files' };
const lambBash = { command: 'npm test', description: 'Run tests' };

scenario('s12-elicitation', [
  [0, 'UserPromptSubmit'],
  [900, 'PreToolUse', tool('mcp__deploy__release', deploy, 'deploy:release', 'toolu_fixture_s12_1'), ['environment']],
  [1200, 'Elicitation', { mcpServer: 'deploy', mode: 'form', message: 'Which region should the release go to?' }],
  [7200, 'Notification', { notificationType: 'elicitation_dialog', message: 'deploy needs your input' }],
  [9000, 'ElicitationResult', { mcpServer: 'deploy' }],
  [9400, 'PostToolUse', tool('mcp__deploy__release', deploy, 'deploy:release', 'toolu_fixture_s12_1'), ['environment']],
  [10100, 'Stop'],
]);

scenario('s13-auto-mode-denied', [
  [0, 'UserPromptSubmit', { permissionMode: 'auto' }],
  [800, 'PreToolUse', tool('Bash', rm, 'rm -rf build', 'toolu_fixture_s13_1', { permissionMode: 'auto' }), ['command', 'description']],
  [820, 'PermissionRequest', noId(tool('Bash', rm, 'rm -rf build', undefined, { permissionMode: 'auto' })), ['command', 'description']],
  [2500, 'PermissionDenied', { toolName: 'Bash', toolUseId: 'toolu_fixture_s13_1', toolFingerprint: fingerprint('Bash', rm), reason: 'auto_mode_classifier' }, ['command', 'description']],
  [4000, 'Stop', { permissionMode: 'auto' }],
]);

scenario('s14-stop-failure', [
  [0, 'UserPromptSubmit'],
  [700, 'PreToolUse', tool('Bash', curl, curl.command, 'toolu_fixture_s14_1'), ['command', 'description']],
  [710, 'PermissionRequest', noId(tool('Bash', curl, curl.command)), ['command', 'description']],
  [5000, 'StopFailure', { errorType: 'rate_limit' }],
  [40000, 'UserPromptSubmit'],
  [41000, 'Stop'],
]);

scenario('s15-sandbox-notification', [
  [0, 'UserPromptSubmit'],
  [700, 'PreToolUse', tool('Bash', curl, curl.command, 'toolu_fixture_s15_1'), ['command', 'description']],
  [6700, 'Notification', { notificationType: 'permission_prompt', message: 'Claude wants to reach status.example.com' }],
  [9000, 'PostToolUse', tool('Bash', curl, curl.command, 'toolu_fixture_s15_1'), ['command', 'description']],
  [9800, 'Stop'],
]);

scenario('s16-agent-needs-input', [
  [0, 'Notification', { notificationType: 'agent_needs_input', message: 'A background session is waiting for you' }],
  [12000, 'UserPromptSubmit'],
  [13000, 'Stop'],
]);

const lamb = { agentId: 'a0000000000000011', agentType: 'Explore' };
scenario('s17-lamb-alert-outlives-main-stop', [
  [0, 'UserPromptSubmit'],
  [1000, 'PreToolUse', tool('Agent', { subagent_type: 'Explore' }, 'Explore', 'toolu_fixture_s17_1'), ['description', 'prompt', 'subagent_type']],
  [1030, 'SubagentStart', lamb],
  [1050, 'PostToolUse', tool('Agent', { subagent_type: 'Explore' }, 'Explore', 'toolu_fixture_s17_1'), ['description', 'prompt', 'subagent_type']],
  [2000, 'PreToolUse', { ...lamb, ...tool('Bash', lambBash, 'npm test', 'toolu_fixture_s17_2') }, ['command', 'description']],
  [2010, 'PermissionRequest', { ...lamb, ...noId(tool('Bash', lambBash, 'npm test')) }, ['command', 'description']],
  // The main thread's turn ends while the lamb still waits (Decision 14: the lamb keeps its alert).
  [3000, 'Stop'],
  [3800, 'SubagentStop', { agentId: 'a0000000000000012' }], // the internal helper, agent_type "": no lamb
  [8020, 'Notification', { notificationType: 'permission_prompt', message: MESSAGES.permission_prompt }],
  [11000, 'PostToolUse', { ...lamb, ...tool('Bash', lambBash, 'npm test', 'toolu_fixture_s17_2') }, ['command', 'description']],
  [11600, 'SubagentStop', lamb],
  [11650, 'UserPromptSubmit'],
  [12300, 'Stop'],
]);

const lamb2 = { agentId: 'a0000000000000021', agentType: 'general-purpose' };
scenario('s18-session-end-clears-lamb', [
  [0, 'UserPromptSubmit'],
  [900, 'SubagentStart', lamb2],
  [1500, 'PreToolUse', { ...lamb2, ...tool('Bash', lambBash, 'npm test', 'toolu_fixture_s18_1') }, ['command', 'description']],
  [1510, 'PermissionRequest', { ...lamb2, ...noId(tool('Bash', lambBash, 'npm test')) }, ['command', 'description']],
  [2200, 'Stop'],
  [9000, 'SessionEnd', { reason: 'prompt_input_exit' }],
]);

scenario('s19-progress-clears', [
  [0, 'UserPromptSubmit'],
  [600, 'PreToolUse', tool('Bash', curl, curl.command, 'toolu_fixture_s19_1'), ['command', 'description']],
  [610, 'PermissionRequest', noId(tool('Bash', curl, curl.command)), ['command', 'description']],
  // Within 2 s (a parallel batch): does not clear.
  [1500, 'PreToolUse', tool('Read', { file_path: `${CWD}/README.md` }, 'README.md', 'toolu_fixture_s19_2'), ['file_path']],
  [1520, 'PostToolUse', tool('Read', { file_path: `${CWD}/README.md` }, 'README.md', 'toolu_fixture_s19_2'), ['file_path']],
  // The curl's PostToolUse is lost; the next tool call more than 2 s later shows the prompt was answered.
  [6000, 'PreToolUse', tool('Read', { file_path: `${CWD}/CHANGELOG.md` }, 'CHANGELOG.md', 'toolu_fixture_s19_3'), ['file_path']],
  [6020, 'PostToolUse', tool('Read', { file_path: `${CWD}/CHANGELOG.md` }, 'CHANGELOG.md', 'toolu_fixture_s19_3'), ['file_path']],
  [7000, 'Stop'],
]);

scenario('s20-duplicate-fingerprints', [
  [0, 'UserPromptSubmit'],
  [500, 'PreToolUse', tool('Bash', ls, 'ls', 'toolu_fixture_s20_1'), ['command', 'description']],
  [510, 'PreToolUse', tool('Bash', ls, 'ls', 'toolu_fixture_s20_2'), ['command', 'description']],
  [520, 'PermissionRequest', noId(tool('Bash', ls, 'ls')), ['command', 'description']],
  [4000, 'PostToolUse', tool('Bash', ls, 'ls', 'toolu_fixture_s20_2'), ['command', 'description']],
  [4010, 'PermissionRequest', noId(tool('Bash', ls, 'ls')), ['command', 'description']],
  [6000, 'PostToolUse', tool('Bash', ls, 'ls', 'toolu_fixture_s20_1'), ['command', 'description']],
  [6500, 'Stop'],
]);

// Six parallel subagents (Milestone 6): five lambs show and "+1". One lamb's own
// permission prompt (it carries the agent_id), confirmed, then approved. The internal
// helper (an agent_id with no agent_type and no SubagentStart) must not become a lamb.
const six = ['Explore', 'general-purpose', 'Plan', 'code-reviewer', 'Explore', 'test-runner'].map((agentType, k) => ({ agentId: `a00000000000000${31 + k}`, agentType }));
const agentTool = (k) => tool('Agent', { subagent_type: six[k].agentType, n: k }, six[k].agentType, `toolu_fixture_s21_a${k}`);
const readme = (k) => tool('Read', { file_path: `${CWD}/README.md` }, 'README.md', `toolu_fixture_s21_r${k}`);
scenario('s21-six-lambs', [
  [0, 'UserPromptSubmit'],
  ...six.flatMap((l, k) => [
    [500 + k * 150, 'PreToolUse', agentTool(k), ['description', 'prompt', 'subagent_type']],
    [530 + k * 150, 'SubagentStart', l],
  ]),
  ...six.flatMap((l, k) => [
    [2000 + k * 100, 'PreToolUse', { ...l, ...readme(k) }, ['file_path']],
    [2400 + k * 100, 'PostToolUse', { ...l, ...readme(k) }, ['file_path']],
  ]),
  [3000, 'PreToolUse', { ...six[2], ...tool('Bash', ls, 'ls', 'toolu_fixture_s21_b2') }, ['command', 'description']],
  [3010, 'PermissionRequest', { ...six[2], ...noId(tool('Bash', ls, 'ls')) }, ['command', 'description']],
  [3500, 'PreToolUse', { agentId: 'a0000000000000039', ...readme(9) }, ['file_path']], // the internal helper: no lamb
  [3520, 'PostToolUse', { agentId: 'a0000000000000039', ...readme(9) }, ['file_path']],
  [9010, 'Notification', { notificationType: 'permission_prompt', message: MESSAGES.permission_prompt }],
  [12000, 'SubagentStop', six[0]],
  [12500, 'SubagentStop', six[1]],
  [13000, 'PostToolUse', { ...six[2], ...tool('Bash', ls, 'ls', 'toolu_fixture_s21_b2') }, ['command', 'description']],
  [14000, 'SubagentStop', six[2]],
  [14300, 'SubagentStop', six[3]],
  [14600, 'SubagentStop', six[4]],
  [14900, 'SubagentStop', six[5]],
  ...six.map((l, k) => [15200 + k * 20, 'PostToolUse', agentTool(k), ['description', 'prompt', 'subagent_type']]),
  [16000, 'Stop'],
].sort((a, b) => a[0] - b[0]));

// ---------------------------------------------------------------- helpers

function read(file) {
  return fs.readFileSync(file, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
}

function write(file, lines) {
  fs.writeFileSync(file, lines.map((l) => JSON.stringify(l)).join('\n') + '\n');
  console.log(`${path.basename(file)}: ${lines.length} events`);
}

/** A stable key order, so rebuilt files diff cleanly. */
function reorder(n) {
  const order = ['receivedAt', 'event', 'sessionId', 'agentId', 'agentType', 'toolName', 'toolUseId', 'notificationType', 'source', 'reason', 'trigger', 'mcpServer', 'mode', 'permissionMode', 'errorType', 'cwd', 'projectLabel', 'toolFingerprint', 'toolSummary', 'message', 'terminal'];
  const out = {};
  for (const k of order) if (n[k] !== undefined) out[k] = n[k];
  for (const k of Object.keys(n)) if (!(k in out) && n[k] !== undefined) out[k] = n[k];
  return out;
}
