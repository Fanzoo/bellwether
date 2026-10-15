#!/usr/bin/env node
/**
 * The conformance run (distribution plan D2): runs the test files that start the brain
 * against the Node brain and the Rust brain, and prints the parity table (test file,
 * node result, rust result). `npm run test:rust` runs it.
 *
 *   node test/conformance.mjs                    both implementations, every row
 *   node test/conformance.mjs --impl rust        the rust column only
 *   node test/conformance.mjs --files a,b        only these test files (names in test/)
 *   node test/conformance.mjs --summary <file>   also append the table there (CI sets
 *                                                GITHUB_STEP_SUMMARY, which is used too)
 *
 * The rows: every file test/conformance.json names, with the milestone that turns it
 * green, plus every other file that starts the brain through test/helpers.mjs. A file is
 * wired to the Rust brain when it starts the brain through the helper or takes the brain's
 * parts from test/impl.mjs (the Rust brain's `__session` under rust). A file that only
 * imports brain/*.mjs has no rust result until its milestone wires it: its rust cell says
 * "not wired", and counts as red. A test with nothing to check in the Rust brain yet says
 * so (`notUnderRust` in helpers.mjs, with its reason) and is counted as skipped; so does a
 * test of something only the repository has (its LaunchAgents, its `.env`, its shims).
 *
 * Exit 1 when a node cell is red, or when a row whose milestone is at or before the
 * manifest's `current` is red under rust. Most rust cells are expected to be red until
 * their milestone.
 *
 * The Rust binary is $BELLWETHER_BIN, or the dev build
 * shell/src-tauri/target-dev/debug/bellwether-shell (built with --features
 * bellwether-shell/conformance), which must not be older than its sources. This script
 * never builds it.
 */
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { AGENT_DIR, RUST_BIN, rustBin } from './helpers.mjs';

const TEST_DIR = path.join(AGENT_DIR, 'test');
const PRELOAD = pathToFileURL(path.join(TEST_DIR, 'setup.mjs')).href;
const FILE_TIMEOUT_MS = 300_000;

function parseArgs(argv) {
  const o = { impl: 'both', files: null, summary: process.env.GITHUB_STEP_SUMMARY || null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      const v = argv[++i];
      if (v === undefined || v.startsWith('--')) fail(`${a} needs a value`);
      return v;
    };
    if (a === '--impl') o.impl = value();
    else if (a === '--files') o.files = value().split(',').map((f) => f.trim()).filter(Boolean);
    else if (a === '--summary') o.summary = value();
    else fail(`unknown option ${JSON.stringify(a)} (see the comment at the top of test/conformance.mjs)`);
  }
  if (!['both', 'node', 'rust'].includes(o.impl)) fail('--impl is both, node or rust');
  return o;
}

function fail(message) {
  process.stderr.write(`conformance: ${message}\n`);
  process.exit(3);
}

/**
 * What a test file does: starts the brain through the helper or uses test/impl.mjs (either
 * makes it "wired"), and whether it still imports brain/*.mjs directly.
 */
function scan(file) {
  const text = fs.readFileSync(path.join(TEST_DIR, file), 'utf8');
  const spawns = /\bspawnBrain(Sync)?\(/.test(text) || /from '\.\/impl\.mjs'/.test(text);
  return { spawns, inProcess: /from '\.\.\/brain\//.test(text) || /import\(['"`]\.\.\/brain\//.test(text) };
}

const milestoneNumber = (m) => Number(String(m).replace(/^M/, ''));

function rows(manifest, only) {
  const milestoneOf = new Map();
  for (const [m, files] of Object.entries(manifest.milestones)) for (const f of files) milestoneOf.set(f, m);
  const all = fs.readdirSync(TEST_DIR).filter((f) => f.endsWith('.test.mjs'));
  const picked = all.filter((f) => milestoneOf.has(f) || scan(f).spawns);
  for (const f of milestoneOf.keys()) if (!all.includes(f)) fail(`test/conformance.json names ${f}, which doesn't exist`);
  const chosen = only ? only.map((f) => (f.endsWith('.test.mjs') ? f : `${f}.test.mjs`)) : picked;
  for (const f of chosen) if (!all.includes(f)) fail(`no test file test/${f}`);
  const order = (f) => (milestoneOf.has(f) ? milestoneNumber(milestoneOf.get(f)) : 99);
  return chosen
    .map((file) => ({ file, milestone: milestoneOf.get(file) ?? null, ...scan(file) }))
    .sort((a, b) => order(a.file) - order(b.file) || a.file.localeCompare(b.file));
}

/** Runs one test file under one implementation; resolves with its counts. */
function runFile(file, impl) {
  return new Promise((resolve) => {
    const env = { ...process.env, NODE_OPTIONS: `--import=${PRELOAD}`, BELLWETHER_IMPL: impl };
    const args = ['--test', '--test-concurrency=1', '--test-reporter=tap'];
    // A Rust command that isn't ported can leave a test waiting for output that never
    // comes: bound each test, as well as the file.
    if (impl === 'rust') args.push('--test-timeout=60000');
    const child = spawn(process.execPath, [...args, path.join(TEST_DIR, file)], { cwd: AGENT_DIR, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (out += d));
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill('SIGTERM');
      setTimeout(() => child.exitCode === null && child.signalCode === null && child.kill('SIGKILL'), 5000).unref();
    }, FILE_TIMEOUT_MS);
    const t0 = Date.now();
    child.on('close', (code) => {
      clearTimeout(timer);
      const count = (k) => Number(new RegExp(`^# ${k} (\\d+)$`, 'm').exec(out)?.[1] ?? 0);
      const r = { code, timedOut, ms: Date.now() - t0, tests: count('tests'), pass: count('pass'), fail: count('fail'), cancelled: count('cancelled'), skipped: count('skipped'), todo: count('todo') };
      r.ok = !timedOut && code === 0 && r.fail === 0 && r.cancelled === 0 && r.tests > 0;
      // The first failing test by name; a failure of the file as a whole (a hook that
      // failed) is reported under the file's path, shown relative to the checkout.
      r.failures = failureDetails(out);
      const failures = r.failures.map((f) => f.name);
      const firstFailure = failures.find((f) => !f.endsWith('.test.mjs')) ?? failures[0];
      r.why = timedOut ? `timed out after ${FILE_TIMEOUT_MS / 1000} s` : firstFailure ?? (r.tests === 0 ? out.trim().split('\n').slice(-3).join(' ').slice(0, 200) : null);
      resolve(r);
    });
  });
}

const relative = (text) => String(text).split(AGENT_DIR + path.sep).join('');

/**
 * Every failing test in TAP output: its name and the lines of its diagnostic block that
 * say why (error, expected, actual, the first stack line in a test file).
 */
function failureDetails(tap) {
  const lines = tap.split('\n');
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    const m = /^(\s*)not ok \d+ - (.*)$/.exec(lines[i]);
    if (!m) continue;
    const why = [];
    for (let j = i + 1; j < lines.length && !/^\s*\.\.\.$/.test(lines[j]) && !/^\s*(not )?ok \d+ - /.test(lines[j]); j++) {
      const l = lines[j].trim();
      // A multi-line error (`error: |-`): its first few lines.
      if (/^error: [|>]-?$/.test(l)) {
        why.push(`error: ${lines.slice(j + 1, j + 4).map((x) => x.trim()).filter(Boolean).join(' ')}`.slice(0, 300));
        continue;
      }
      if (/^(error|expected|actual|code|failureType):/.test(l) || (/test\/[^ ]+\.test\.mjs:\d+/.test(l) && !why.some((w) => /\.test\.mjs:\d+/.test(w)))) why.push(relative(l).slice(0, 300));
    }
    out.push({ name: relative(m[2]), why });
  }
  return out;
}

function cell(r, row = null, impl = null) {
  if (!r) return '–';
  if (r.status === 'not wired') return 'not wired';
  if (r.timedOut) return 'TIMEOUT';
  // Skipped tests are neither passes nor failures: shown apart. A skipped suite registers
  // none of its tests (TAP counts it once), so under rust the skips are counted from the
  // node run of the same file: every test node has that rust didn't run.
  let skipped = r.skipped;
  if (impl === 'rust' && row?.node?.tests) skipped = Math.max(r.skipped, row.node.tests - (r.pass + r.fail + r.cancelled + r.todo));
  const n = `${r.pass}/${r.pass + r.fail + r.cancelled}${skipped ? `, ${skipped} skipped` : ''}`;
  // A rust pass for a file that also tests brain/*.mjs in process covers only its tests
  // that start the brain.
  const star = impl === 'rust' && row?.inProcess ? ' *' : '';
  return r.ok ? `pass ${n}${star}` : `FAIL ${n}`;
}

const isRed = (r) => !r || r.status === 'not wired' || !r.ok;

function table(results, manifest, impls) {
  const lines = [];
  lines.push(`| test file | milestone | ${impls.join(' | ')} | note |`);
  lines.push(`|---|---|${impls.map(() => '---').join('|')}|---|`);
  for (const row of results) {
    const notes = [];
    if (!row.spawns) notes.push('in-process JS only');
    if (row[impls.at(-1)]?.skipped && impls.includes('rust')) notes.push('skips: each says why (notUnderRust: the repository\'s own, or not in the Rust brain yet; or the file\'s own skip)');
    else if (row.inProcess) notes.push('also tests brain/*.mjs in process');
    if (row.milestone && milestoneNumber(row.milestone) <= milestoneNumber(manifest.current)) notes.push(`required now (${manifest.current})`);
    const rust = row.rust;
    if (rust && !rust.ok && rust.why && rust.status !== 'not wired') notes.push(`first failure: ${rust.why.replace(/\|/g, '\\|').slice(0, 90)}`);
    lines.push(`| ${row.file} | ${row.milestone ?? '–'} | ${impls.map((i) => cell(row[i], row, i)).join(' | ')} | ${notes.join('; ')} |`);
  }
  return lines.join('\n');
}

/** The failing node tests, for the summary: each red node cell, test by test. */
function nodeFailures(results) {
  const red = results.filter((r) => r.node && !r.node.ok);
  if (!red.length) return [];
  const lines = ['', '### Node failures', ''];
  for (const r of red) {
    lines.push(`- **${r.file}**${r.node.timedOut ? ` (${r.node.why})` : ''}`);
    for (const f of r.node.failures) lines.push(`  - ${f.name.replace(/\|/g, '\\|')}${f.why.length ? `: \`${f.why.join(' ').replace(/`/g, "'").slice(0, 400)}\`` : ''}`);
  }
  return lines;
}

/**
 * One `__json` call before any test file runs: a binary built without the `conformance`
 * feature (`npm run test:cargo` rebuilds the dev binary without it) answers "unknown
 * command", and every file would then wait out FILE_TIMEOUT_MS. Stop at once instead, with
 * the differential's message.
 */
function probeConformance(bin) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'bw-conformance-probe-'));
  const r = spawnSync(bin, ['__json'], { argv0: 'bellwether', input: '', encoding: 'utf8', timeout: 20_000, env: { PATH: process.env.PATH, HOME: home, BELLWETHER_HOME: home } });
  fs.rmSync(home, { recursive: true, force: true });
  if (r.status === 0) return;
  if (/unknown command/.test(r.stderr ?? '')) fail(`${bin} has no conformance commands ("unknown command"): it was built without the \`conformance\` feature (a release build never has it). Build with --features bellwether-shell/conformance`);
  fail(`${bin} __json failed (${r.error ? r.error.message : `exit ${r.status}`}): ${(r.stderr ?? '').trim()}`);
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  const manifest = JSON.parse(fs.readFileSync(path.join(TEST_DIR, 'conformance.json'), 'utf8'));
  const impls = o.impl === 'both' ? ['node', 'rust'] : [o.impl];
  if (impls.includes('rust')) {
    try {
      rustBin();
    } catch (e) {
      fail(e.message);
    }
    probeConformance(RUST_BIN);
  }
  const results = rows(manifest, o.files);
  process.stderr.write(`conformance: ${results.length} test files, ${impls.join(' and ')}${impls.includes('rust') ? ` (rust: ${path.relative(AGENT_DIR, RUST_BIN) || RUST_BIN})` : ''}\n`);
  for (const row of results) {
    for (const impl of impls) {
      if (impl === 'rust' && !row.spawns) {
        row.rust = { status: 'not wired' };
        continue;
      }
      process.stderr.write(`  ${impl.padEnd(4)} ${row.file} … `);
      row[impl] = await runFile(row.file, impl);
      process.stderr.write(`${cell(row[impl])} (${(row[impl].ms / 1000).toFixed(1)} s)\n`);
      // A red node cell is a regression in today's brain: say exactly what failed.
      if (impl === 'node' && !row.node.ok) for (const f of row.node.failures) process.stderr.write(`       not ok: ${f.name}\n${f.why.map((w) => `         ${w}\n`).join('')}`);
    }
  }

  const required = results.filter((r) => r.milestone && milestoneNumber(r.milestone) <= milestoneNumber(manifest.current));
  const red = {
    node: impls.includes('node') ? results.filter((r) => isRed(r.node)) : [],
    rustRequired: impls.includes('rust') ? required.filter((r) => isRed(r.rust)) : [],
  };
  const greenRust = results.filter((r) => !isRed(r.rust)).length;
  const summary = [
    `## Parity table (${manifest.current})`,
    '',
    table(results, manifest, impls),
    '',
    impls.includes('node') ? `- node: ${results.length - red.node.length} of ${results.length} files green` : null,
    impls.includes('rust') ? `- rust: ${greenRust} of ${results.length} files green; required by ${manifest.current}: ${required.length - red.rustRequired.length} of ${required.length} green` : null,
    '- "not wired": the file tests brain/*.mjs in process, so it has no rust result until its milestone runs it against the Rust binary.',
    impls.includes('node') && impls.includes('rust')
      ? '- "skipped" in the rust column counts every test the node run of the same file has and the rust run didn\'t run, those inside a skipped suite included.'
      : '- "skipped" is TAP\'s count: a skipped suite counts once, however many tests it holds.',
    '- "pass *": the file also tests brain/*.mjs in process; the rust result speaks only for its tests that start the brain (for some, only that an unported command leaks nothing).',
    ...nodeFailures(results),
  ]
    .filter((l) => l !== null)
    .join('\n');
  process.stdout.write(`\n${summary}\n`);
  if (o.summary) fs.appendFileSync(o.summary, `${summary}\n`);

  const problems = [...red.node.map((r) => `node is red for ${r.file}`), ...red.rustRequired.map((r) => `rust is red for ${r.file}, required by ${manifest.current}`)];
  if (problems.length) {
    process.stderr.write(`\nconformance: ${problems.join('; ')}\n`);
    process.exit(1);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
