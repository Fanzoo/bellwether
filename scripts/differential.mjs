#!/usr/bin/env node
/**
 * Differential testing for the port's risky parts (distribution plan D2, point 4). A dev
 * tool, never shipped. It generates inputs, answers each with the Node brain's own
 * function, sends the same inputs to the Rust binary's hidden `__<area>` command (built
 * with the `conformance` feature) as JSON lines, and fails on any difference.
 *
 *   node scripts/differential.mjs                       every area, 10⁵ cases each
 *   node scripts/differential.mjs --areas env,redact    only these areas
 *   node scripts/differential.mjs --cases 20000 --seed 7
 *   node scripts/differential.mjs --strict              an area not ported yet is a failure too
 *   node scripts/differential.mjs --bin <path>          another binary (default: $BELLWETHER_BIN,
 *                                                       else the dev build in target-dev/debug)
 *
 * Areas, and what the Node side runs:
 *   env        brain/envfile.mjs envValue (a sample is also checked against `node --env-file`)
 *   redact     brain/redact.mjs makeTextCleaner; secret-like names, separators, quotes, URL
 *              credentials, base64 runs, full-width, zero-width and bidi characters, lone
 *              surrogates
 *   normalise  brain/normalise.mjs normalise; payloads and headers built from its allowlist
 *   json       brain/hooks.mjs parseLossless + serialise: settings files with odd layouts,
 *              exotic numbers, escapes, duplicate and integer-like keys, __proto__
 *   diff       brain/diff.mjs unifiedDiff and diffStats
 *   fixture    test/fixture-runner.mjs over the golden fixtures (not generated)
 *
 * Exit 0 when every ported area matches (an area the binary reports as not ported yet
 * is listed and skipped), 1 on a difference, 3 on bad usage or a binary without the
 * `conformance` feature.
 */
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { withDefaults } from '../brain/defaults.mjs';
import { diffStats, unifiedDiff } from '../brain/diff.mjs';
import { envValue } from '../brain/envfile.mjs';
import { parseLossless, serialise } from '../brain/hooks.mjs';
import { normalise } from '../brain/normalise.mjs';
import { makeTextCleaner } from '../brain/redact.mjs';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const FIXTURES = path.join(AGENT, 'test', 'fixtures');
const DEFAULT_BIN = path.join(AGENT, 'shell', 'src-tauri', 'target-dev', 'debug', 'bellwether-shell');
const AREAS = ['env', 'redact', 'normalise', 'json', 'diff', 'fixture'];

function fail(message, code = 3) {
  process.stderr.write(`differential: ${message}\n`);
  process.exit(code);
}

function parseArgs(argv) {
  const o = { areas: AREAS, cases: 100_000, seed: 1, strict: false, bin: process.env.BELLWETHER_BIN || DEFAULT_BIN, show: 5 };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      const v = argv[++i];
      if (v === undefined || v.startsWith('--')) fail(`${a} needs a value`);
      return v;
    };
    if (a === '--areas') o.areas = value().split(',').map((s) => s.trim()).filter(Boolean);
    else if (a === '--cases') o.cases = Number(value());
    else if (a === '--seed') o.seed = Number(value());
    else if (a === '--bin') o.bin = path.resolve(value());
    else if (a === '--show') o.show = Number(value());
    else if (a === '--strict') o.strict = true;
    else fail(`unknown option ${JSON.stringify(a)} (see the comment at the top of scripts/differential.mjs)`);
  }
  for (const a of o.areas) if (!AREAS.includes(a)) fail(`unknown area ${JSON.stringify(a)} (${AREAS.join(', ')})`);
  if (!Number.isInteger(o.cases) || o.cases < 1) fail('--cases takes a positive integer');
  if (!Number.isInteger(o.seed)) fail('--seed takes an integer');
  return o;
}

// ------------------------------------------------------------------ generators

/** mulberry32: small, seeded, the same sequence on every machine. */
function rng(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const int = (n) => Math.floor(next() * n);
  const pick = (xs) => xs[int(xs.length)];
  const chance = (p) => next() < p;
  const repeat = (n, f) => Array.from({ length: n }, f).join('');
  return { next, int, pick, chance, repeat };
}

const ALNUM = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
const word = (r, n = 1 + r.int(12), chars = ALNUM) => r.repeat(n, () => chars[r.int(chars.length)]);

function envCase(r) {
  const key = 'BELLWETHER_HOOK_TOKEN';
  const keys = [key, `export ${key}`, `export  ${key}`, ` ${key} `, `${key}_X`, 'OTHER', `\t${key}`, `${key.toLowerCase()}`];
  const quotes = ['"', "'", '`', ''];
  const value = () => {
    const q = r.pick(quotes);
    const parts = r.repeat(1 + r.int(4), () => r.pick([word(r), ' ', '#', '\\n', '\t', '"', "'", '`', '=', ' # c', 'é', '😀']));
    return `${r.pick(['', ' ', '  '])}${q}${parts}${r.chance(0.8) ? q : ''}${r.pick(['', ' ', ' # comment', '#x'])}`;
  };
  const line = () => r.pick([`${r.pick(keys)}=${value()}`, `${r.pick(keys)} = ${value()}`, `# ${key}=commented`, `   #${key}=x`, '', `${key}`, `=${value()}`]);
  const eol = r.pick(['\n', '\r\n']);
  return { text: r.repeat(1 + r.int(5), () => line() + eol) + (r.chance(0.3) ? line() : ''), key };
}

function redactCase(r) {
  const names = ['password', 'PASSWORD', 'passwd', 'token', 'api_key', 'apiKey', 'GITHUB_TOKEN', 'client_secret', 'authorization', 'Authorization', 'auth', 'key', 'sig', 'pin', 'author', 'tokenizer', 'PASSWORDS', 'x-api-key', '_authToken', 'secretKey'];
  const seps = ['=', ': ', ':', ' = ', '\t=\t', '="', "='", '":"', '\\":\\"', ' '];
  const odd = ['​', '‍', '﻿', '­', '‮', '⁦', '‎', ' ', ' ', '\u0007', '\u001b', '\r\n', 'Ａ', 'ｐａｓｓ', 'ﬁ', '١٢٣', '\ud800', '\udc00', '😀', '漢字'];
  const piece = () =>
    r.pick([
      () => `${r.pick(names)}${r.pick(seps)}${word(r, 4 + r.int(24))}`,
      () => `--${r.pick(names)} ${word(r, 6 + r.int(10))}`,
      () => `-p${word(r, 3 + r.int(10))}`,
      () => `https://${word(r, 3)}:${word(r, 4 + r.int(8))}@${word(r, 5)}.test/${word(r, 3)}?${r.pick(names)}=${word(r, 8)}`,
      () => `${r.pick(['Bearer', 'Basic', 'bearer'])} ${word(r, 8 + r.int(30), ALNUM + '+/=._-')}`,
      () => word(r, 16 + r.int(40), ALNUM + '+/') + r.pick(['', '=', '==']),
      () => r.pick(['sk-', 'ghp_', 'xoxb-', 'AKIA', 'glpat-', 'AIza', 'eyJ']) + word(r, 10 + r.int(30), ALNUM + '_-.'),
      () => r.pick(odd),
      () => r.pick(['git push origin main', 'ls -la', 'docker run -p 8080:80 x', 'echo "hi"', 'curl -u', 'mysql -uroot', 'aws configure set', 'vault login']),
      () => r.pick(['"', "'", '`', '\\', ' ', '  ', ';', '|', '&&', '{', '}']),
    ])();
  const patterns = r.chance(0.2) ? [r.pick(['(?i)hunter\\d', 'secret-[a-z]+', '(?i)corp_[0-9]{3,}'])] : [];
  return { text: r.repeat(1 + r.int(8), () => piece() + (r.chance(0.6) ? ' ' : '')), maxChars: r.pick([8, 20, 60, 200, 500]), redactPatterns: patterns };
}

function normaliseCase(r) {
  const ident = () => r.pick([word(r), `${word(r)}-${word(r)}`, `${word(r)} ${word(r)}`, '', '<b>x</b>', 'a‮b', 123, null, true, { x: 1 }]);
  const payload = { hook_event_name: r.pick(['PreToolUse', 'PostToolUse', 'Stop', 'Notification', 'Elicitation', 'StopFailure', 'SubagentStop', 'SessionEnd', 'free text', 42]) };
  for (const k of ['session_id', 'agent_id', 'agent_type', 'tool_name', 'tool_use_id', 'notification_type', 'source', 'reason', 'trigger', 'mcp_server_name', 'mode', 'permission_mode']) if (r.chance(0.4)) payload[k] = ident();
  if (r.chance(0.5)) payload.tool_name = r.pick(['Bash', 'Read', 'Edit', 'Write', 'WebFetch', 'WebSearch', 'Grep', 'Glob', 'Task', 'NotebookEdit', 'mcp__srv__do_thing', 'Unknown']);
  if (r.chance(0.6)) payload.tool_input = r.pick([{ command: redactCase(r).text }, { file_path: `/u/${word(r)}/${word(r)}.txt` }, { url: `https://${word(r)}:${word(r)}@${word(r)}.test/x?token=${word(r)}` }, { query: word(r) }, { pattern: word(r) }, { subagent_type: word(r) }, 'str', [1], null]);
  if (r.chance(0.3)) payload.message = redactCase(r).text;
  if (r.chance(0.3)) payload.cwd = `/w/${word(r)}${r.pick(['', '‮', '\u0007', ' x'])}`;
  if (r.chance(0.2)) payload.error = r.pick(['rate_limit', 'Free text', 'x_y']);
  if (r.chance(0.2)) payload.is_interrupt = r.pick([true, false, 'true']);
  if (r.chance(0.2)) payload.background_tasks = r.pick([[], [1, 2], 'x']);
  const headers = {};
  for (const h of ['x-bellwether-term-program', 'x-bellwether-iterm-session', 'x-bellwether-term-session', 'x-bellwether-wt-session', 'x-bellwether-tmux-pane', 'x-bellwether-herdr-pane', 'x-bellwether-herdr-socket']) if (r.chance(0.3)) headers[h] = r.pick([word(r), 'w1:p2', '-x', '/s/herdr.sock', 'rel.sock', '%3', '']);
  const clean = r.chance(0.7) ? { maxChars: r.pick([20, 60, 120]), redactPatterns: [] } : null;
  return { payload, headers, receivedAt: r.int(2e12), clean };
}

function jsonCase(r) {
  const nums = ['1.0', '1e2', '-0', '12345678901234567890', '1E+2', '0.1', '-1.5e-10', '42', '1180591620717411303424'];
  const strs = ['"plain"', '"tab\\there"', '"\\u00e9"', '"\\ud83d\\ude00"', '"\\/slash"', '"q\\"q"', '"é"', '"\\u0001"'];
  const keys = ['"a"', '"b"', '"1"', '"10"', '"2"', '"__proto__"', '"constructor"', '"hooks"', '"a"'];
  const indent = r.pick(['  ', '    ', '\t', ' ']);
  const eol = r.pick(['\n', '\r\n']);
  const value = (depth) => {
    if (depth > 2 || r.chance(0.5)) return r.pick([...nums, ...strs, 'true', 'false', 'null']);
    if (r.chance(0.5)) return `[${Array.from({ length: r.int(3) }, () => value(depth + 1)).join(', ')}]`;
    return obj(depth + 1);
  };
  const obj = (depth) => {
    const n = r.int(4);
    if (n === 0) return '{}';
    const pad = indent.repeat(depth + 1);
    const members = Array.from({ length: n }, () => `${pad}${r.pick(keys)}${r.pick([': ', ':', ' : '])}${value(depth)}`);
    return `{${eol}${members.join(`,${eol}`)}${eol}${indent.repeat(depth)}}`;
  };
  return { text: obj(0) + (r.chance(0.7) ? eol : '') };
}

function diffCase(r) {
  const alphabet = ['a', 'b', 'c', '', '  x', '}', '{'];
  const lines = () => Array.from({ length: r.int(30) }, () => r.pick(alphabet));
  const old = lines();
  const neu = old.filter(() => r.chance(0.85)).flatMap((l) => (r.chance(0.1) ? [l, r.pick(alphabet)] : [l]));
  const eol = r.pick(['\n', '\r\n']);
  const text = (ls) => (ls.length ? ls.join(eol) + (r.chance(0.8) ? eol : '') : '');
  return { old: text(old), new: text(neu), options: r.chance(0.3) ? { context: r.int(5) } : {} };
}

// ------------------------------------------------------------------ Node's answers

let cleaners = new Map();
const cleanerFor = (o) => {
  const k = JSON.stringify(o);
  if (!cleaners.has(k)) cleaners.set(k, makeTextCleaner(o));
  return cleaners.get(k);
};
const errorOf = (e) => ({ error: true, message: e.message });

const NODE = {
  env: (c) => ({ value: envValue(c.text, c.key) }),
  redact: (c) => {
    try {
      return { value: cleanerFor({ maxChars: c.maxChars, redactPatterns: c.redactPatterns })(c.text) };
    } catch (e) {
      return errorOf(e);
    }
  },
  normalise: (c) => normalise(c.payload, c.headers, c.receivedAt, { clean: c.clean ? cleanerFor(c.clean) : null }),
  json: (c) => {
    try {
      return { value: serialise(parseLossless(c.text), c.text) };
    } catch (e) {
      return errorOf(e);
    }
  },
  diff: (c) => {
    const s = diffStats(c.old, c.new);
    return { value: unifiedDiff(c.old, c.new, c.options), added: s.added, removed: s.removed };
  },
};

const GENERATE = { env: envCase, redact: redactCase, normalise: normaliseCase, json: jsonCase, diff: diffCase };

/** The golden fixtures, one case each, with the Node reducer's transitions. */
async function fixtureCases() {
  const { readJsonl, runFixture } = await import('../test/fixture-runner.mjs');
  return fs
    .readdirSync(FIXTURES)
    .filter((f) => /^s\d+-.*\.jsonl$/.test(f) && !f.includes('.expected.'))
    .sort()
    .map((f) => {
      const lines = readJsonl(path.join(FIXTURES, f));
      return { case: { name: f, lines }, node: { transitions: runFixture(lines, { config: withDefaults({ port: 47417 }) }).transitions } };
    });
}

// ------------------------------------------------------------------ the Rust side

/** `__list`: the areas the binary has, and which are ported. */
function rustAreas(bin) {
  if (!fs.existsSync(bin)) fail(`no binary at ${bin}: build it with \`cargo build --manifest-path shell/src-tauri/Cargo.toml --features bellwether-shell/conformance\` or pass --bin`);
  const r = spawnSync(bin, ['__list'], { argv0: 'bellwether', encoding: 'utf8', timeout: 20_000, env: { PATH: process.env.PATH, HOME: os.tmpdir(), BELLWETHER_HOME: fs.mkdtempSync(path.join(os.tmpdir(), 'bw-diff-home-')) } });
  if (r.status !== 0) {
    if (/unknown command/.test(r.stderr)) fail(`${bin} has no conformance commands ("unknown command"): it was built without the \`conformance\` feature (a release build never has it). Build with --features bellwether-shell/conformance`);
    fail(`${bin} __list failed (exit ${r.status}): ${r.stderr.trim()}`);
  }
  return JSON.parse(r.stdout).areas;
}

/** Sends every case as one JSON line; resolves with the parsed answer lines. */
function rustAnswers(bin, area, cases) {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, [`__${area}`], { argv0: 'bellwether', stdio: ['pipe', 'pipe', 'pipe'], env: { PATH: process.env.PATH, HOME: os.tmpdir(), BELLWETHER_HOME: fs.mkdtempSync(path.join(os.tmpdir(), 'bw-diff-home-')) } });
    const out = [];
    let err = '';
    child.stdout.on('data', (d) => out.push(d));
    child.stderr.on('data', (d) => (err += d));
    child.on('error', reject);
    child.on('close', (code) => {
      if (code !== 0) return reject(new Error(`__${area} exited ${code}: ${err.trim()}`));
      const lines = Buffer.concat(out).toString('utf8').split('\n').filter(Boolean);
      resolve(lines.map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return { unparseable: l };
        }
      }));
    });
    // One line per case: JSON.stringify writes a lone surrogate as a \udxxx escape.
    child.stdin.end(cases.map((c) => JSON.stringify(c)).join('\n') + '\n');
  });
}

function same(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

// ------------------------------------------------------------------ main

async function main() {
  const o = parseArgs(process.argv.slice(2));
  const areas = rustAreas(o.bin);
  const report = [];
  let failed = false;
  for (const area of o.areas) {
    const t0 = Date.now();
    let cases;
    let node;
    if (area === 'fixture') {
      const all = await fixtureCases();
      cases = all.map((x) => x.case);
      node = all.map((x) => x.node);
    } else {
      const r = rng(o.seed * 1000003 + AREAS.indexOf(area));
      cases = Array.from({ length: o.cases }, () => GENERATE[area](r));
      node = cases.map(NODE[area]);
    }
    const status = areas[area] ?? 'missing';
    if (status !== 'ported') {
      const line = `${area.padEnd(9)} ${String(cases.length).padStart(7)} cases, Node answered all; rust: not ported yet (${status})`;
      report.push(line);
      if (o.strict) failed = true;
      continue;
    }
    let rust;
    try {
      rust = await rustAnswers(o.bin, area, cases);
    } catch (e) {
      report.push(`${area.padEnd(9)} rust failed: ${e.message}`);
      failed = true;
      continue;
    }
    const diffs = [];
    for (let i = 0; i < cases.length; i++) if (!same(node[i], rust[i])) diffs.push(i);
    if (rust.length !== cases.length) diffs.push(`(${rust.length} answers for ${cases.length} cases)`);
    report.push(`${area.padEnd(9)} ${String(cases.length).padStart(7)} cases, ${diffs.length} differences (${((Date.now() - t0) / 1000).toFixed(1)} s)`);
    for (const i of diffs.slice(0, o.show)) {
      if (typeof i === 'string') report.push(`          ${i}`);
      else report.push(`          case ${i}: ${JSON.stringify(cases[i]).slice(0, 300)}\n            node: ${JSON.stringify(node[i]).slice(0, 300)}\n            rust: ${JSON.stringify(rust[i]).slice(0, 300)}`);
    }
    if (diffs.length) failed = true;
  }

  // The Node side's own oracle for .env: a sample against `node --env-file` itself. The
  // shared parser (brain/envfile.mjs, and settings.rs, which matches it) is known to
  // differ from Node 22.14 in two shapes, both recorded in test/DIVERGENCES.md; any
  // other difference fails the run.
  if (o.areas.includes('env')) {
    const r = rng(o.seed * 7919);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bw-diff-env-'));
    const known = { 'a quoted value left open (Node reads on across lines)': 0, 'a line with no key, or no "=" (Node stops reading there)': 0 };
    const unexplained = [];
    const sample = Math.min(200, o.cases);
    for (let i = 0; i < sample; i++) {
      const c = envCase(r);
      const f = path.join(dir, `e${i}`);
      fs.writeFileSync(f, c.text);
      const p = spawnSync(process.execPath, [`--env-file=${f}`, '-e', 'process.stdout.write(JSON.stringify(process.env.BELLWETHER_HOOK_TOKEN ?? null))'], { encoding: 'utf8', env: { PATH: process.env.PATH } });
      if (JSON.parse(p.stdout || 'null') === envValue(c.text, c.key)) continue;
      if (/^ *=/m.test(c.text) || /^(?! *#)[^=\r\n]*\S[^=\r\n]*\r?$/m.test(c.text)) known['a line with no key, or no "=" (Node stops reading there)']++;
      else if (/=\s*(["'`])(?:(?!\1)[^\r\n])*\r?$/m.test(c.text)) known['a quoted value left open (Node reads on across lines)']++;
      else unexplained.push(JSON.stringify(c.text));
    }
    fs.rmSync(dir, { recursive: true, force: true });
    const knownText = Object.entries(known).filter(([, n]) => n).map(([k, n]) => `${n} ${k}`).join(', ');
    report.push(`env       ${sample} of them also against \`node --env-file\` ${process.version}: ${unexplained.length} unexplained differences${knownText ? `; known (DIVERGENCES.md): ${knownText}` : ''}${unexplained.length ? `\n          ${unexplained.slice(0, o.show).join('\n          ')}` : ''}`);
    if (unexplained.length || (o.strict && knownText)) failed = true;
  }

  process.stdout.write(`differential (seed ${o.seed}, ${path.relative(AGENT, o.bin) || o.bin}):\n${report.map((l) => `  ${l}`).join('\n')}\n`);
  process.exit(failed ? 1 : 0);
}

main();
