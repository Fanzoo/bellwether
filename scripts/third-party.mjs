#!/usr/bin/env node
/**
 * Writes THIRD-PARTY.md: every Rust crate in shell/src-tauri/Cargo.lock and every npm
 * package in package.json and shell/package.json, with its licence, read offline:
 * crates from the local cargo registry (~/.cargo/registry/src), npm packages from
 * node_modules when installed. Nothing is fetched. A crate cargo never downloaded here
 * (a Windows- or Linux-only dependency) is listed as "not downloaded on this machine".
 *
 *   node scripts/third-party.mjs            regenerate THIRD-PARTY.md
 *   node scripts/third-party.mjs --check    exit 1 if THIRD-PARTY.md is out of date
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const LOCK = path.join(AGENT, 'shell', 'src-tauri', 'Cargo.lock');
const OUT = path.join(AGENT, 'THIRD-PARTY.md');

/** [{name, version, source}] from Cargo.lock (registry crates only; the shell itself is skipped). */
export function lockedCrates(text) {
  const out = [];
  for (const block of text.split('[[package]]').slice(1)) {
    const get = (k) => new RegExp(`^${k} = "([^"]*)"`, 'm').exec(block)?.[1];
    if (get('source')?.startsWith('registry+')) out.push({ name: get('name'), version: get('version') });
  }
  return out;
}

/** The `license` (or `license-file`) of a crate's Cargo.toml text. */
export function crateLicence(toml) {
  const pkg = toml.split(/^\[(?!package\])/m)[0];
  return /^license\s*=\s*"([^"]+)"/m.exec(pkg)?.[1] ?? (/^license-file\s*=\s*"([^"]+)"/m.test(pkg) ? 'see its licence file' : 'not stated');
}

function registryDirs(home = os.homedir()) {
  const root = path.join(process.env.CARGO_HOME ?? path.join(home, '.cargo'), 'registry', 'src');
  try {
    return fs.readdirSync(root).map((d) => path.join(root, d));
  } catch {
    return [];
  }
}

/** Whether this machine has the crates to read licences from (a fresh clone may not). */
export function registryAvailable() {
  if (!fs.existsSync(LOCK)) return false;
  const regs = registryDirs();
  return regs.length > 0 && lockedCrates(fs.readFileSync(LOCK, 'utf8')).some((c) => regs.some((r) => fs.existsSync(path.join(r, `${c.name}-${c.version}`, 'Cargo.toml'))));
}

/** Licences npm lists for the dev tools, used when node_modules isn't installed. */
const KNOWN_NPM_LICENCES = { '@tauri-apps/cli': 'Apache-2.0 OR MIT' };

/**
 * The crate rows of the committed THIRD-PARTY.md, by "name version". A crate this machine
 * never downloaded (cargo fetches only what the platforms it built for need) keeps the
 * licence its row already has: that row was read from a registry that had the crate. So
 * the file doesn't depend on which crates happen to be cached here, and `--check` still
 * fails on a crate that has no row yet.
 */
function committedLicences() {
  const rows = new Map();
  let text = '';
  try {
    text = fs.readFileSync(OUT, 'utf8');
  } catch {
    return rows;
  }
  const table = text.split('| Crate | Version | Licence |')[1] ?? '';
  for (const m of table.matchAll(/^\| ([^|]+) \| ([^|]+) \| ([^|]+) \|$/gm)) rows.set(`${m[1]} ${m[2]}`, m[3]);
  return rows;
}

const NOT_DOWNLOADED = 'not downloaded on this machine (another platform\'s dependency)';

/** How many crates were not in the local registry on the last render. */
export let unverified = 0;

function render() {
  const regs = registryDirs();
  const committed = committedLicences();
  unverified = 0;
  const crates = lockedCrates(fs.readFileSync(LOCK, 'utf8')).map((c) => {
    const dir = regs.map((r) => path.join(r, `${c.name}-${c.version}`)).find((d) => fs.existsSync(path.join(d, 'Cargo.toml')));
    if (dir) return { ...c, licence: crateLicence(fs.readFileSync(path.join(dir, 'Cargo.toml'), 'utf8')) };
    unverified++;
    const kept = committed.get(`${c.name} ${c.version}`);
    return { ...c, licence: kept && kept !== NOT_DOWNLOADED ? kept : NOT_DOWNLOADED };
  });
  const npm = [];
  for (const [file, where] of [[path.join(AGENT, 'package.json'), AGENT], [path.join(AGENT, 'shell', 'package.json'), path.join(AGENT, 'shell')]]) {
    const pkg = JSON.parse(fs.readFileSync(file, 'utf8'));
    for (const [kind, deps] of [['dependency', pkg.dependencies], ['devDependency', pkg.devDependencies]]) {
      for (const [name, version] of Object.entries(deps ?? {})) {
        // The same line whether or not node_modules is installed (building the signed
        // app installs the Tauri CLI; a fresh clone doesn't), so --check agrees on both.
        let licence = KNOWN_NPM_LICENCES[name] ?? 'not installed here, so not read';
        try {
          licence = JSON.parse(fs.readFileSync(path.join(where, 'node_modules', name, 'package.json'), 'utf8')).license ?? 'not stated';
        } catch {}
        npm.push({ name, version, kind: `${path.relative(AGENT, file)} ${kind}`, licence });
      }
    }
  }
  const tally = {};
  for (const c of crates) tally[c.licence] = (tally[c.licence] ?? 0) + 1;
  const lines = [
    '# Third-party packages',
    '',
    'Generated by `node scripts/third-party.mjs` from `shell/src-tauri/Cargo.lock`, `package.json` and',
    '`shell/package.json`, reading licences from the local cargo registry and `node_modules` (no network).',
    'Regenerate it after changing a dependency; `--check` fails when it is out of date. See NOTICE for',
    'the assets and the reasoning.',
    '',
    `## npm (${npm.length})`,
    '',
    'The brain (`package.json`) has **no dependencies**. The shell has one devDependency, used only to',
    'bundle a `.app`; `cargo build` does not need it.',
    '',
    '| Package | Version | Where | Licence |',
    '|---|---|---|---|',
    ...npm.map((p) => `| ${p.name} | ${p.version} | ${p.kind} | ${p.licence} |`),
    '',
    `## Rust crates (${crates.length})`,
    '',
    '| Licence | Crates |',
    '|---|---|',
    ...Object.entries(tally).sort((a, b) => b[1] - a[1]).map(([l, n]) => `| ${l} | ${n} |`),
    '',
    '| Crate | Version | Licence |',
    '|---|---|---|',
    ...crates.map((c) => `| ${c.name} | ${c.version} | ${c.licence} |`),
    '',
  ];
  return lines.join('\n');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (!registryAvailable()) {
    // A fresh clone before its first `cargo build` has no registry to read from.
    process.stdout.write('skipped: no cargo registry (or Cargo.lock) on this machine to read licences from; build the shell once, then run this again\n');
    process.exit(0);
  }
  const text = render();
  if (process.argv.includes('--check')) {
    const now = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf8') : '';
    if (now !== text) {
      process.stderr.write('THIRD-PARTY.md is out of date: run `node scripts/third-party.mjs`\n');
      process.exit(1);
    }
    process.stdout.write(`THIRD-PARTY.md is up to date${unverified ? ` (${unverified} crates aren't in this machine's cargo registry, so their licences were taken from the file as committed; \`cargo fetch\` downloads every platform's crates)` : ''}\n`);
  } else {
    fs.writeFileSync(OUT, text);
    process.stdout.write(`wrote ${path.relative(process.cwd(), OUT)}\n`);
  }
}
