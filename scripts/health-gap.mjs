#!/usr/bin/env node
/**
 * Measures the brain's update gap (distribution plan D5, M9 "Done when"): polls
 * `GET /health` every 50 ms and prints each change of version or pid, and every stretch
 * with no answer and how long it lasted. Read only: it sends nothing but /health (which
 * needs no token). Stops after --seconds, or on Ctrl-C, with a summary.
 *
 *   node scripts/health-gap.mjs [--port 47417] [--seconds 300]
 *
 * Start it, then choose "Check for Updates…" > Install and Relaunch in the app. Expected:
 * one gap under 1000 ms, and the new version reported within 2 s of the relaunch.
 */
function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? Number(process.argv[i + 1]) : fallback;
}

const port = arg('port', 47417);
const seconds = arg('seconds', 300);
if (!Number.isInteger(port) || port < 1024 || port > 65535 || !(seconds > 0)) {
  process.stderr.write('usage: node scripts/health-gap.mjs [--port <1024-65535>] [--seconds <n>]\n');
  process.exit(3);
}

const t0 = Date.now();
const gaps = [];
let last = null;
let gapStart = null;
let stop = false;
process.on('SIGINT', () => {
  stop = true;
});
const at = () => `+${String(Date.now() - t0).padStart(6)} ms`;
console.log(`polling http://127.0.0.1:${port}/health every 50 ms for ${seconds} s (Ctrl-C stops)`);
while (!stop && Date.now() - t0 < seconds * 1000) {
  const started = Date.now();
  let h = null;
  try {
    const r = await fetch(`http://127.0.0.1:${port}/health`, { signal: AbortSignal.timeout(500) });
    if (r.ok) h = await r.json();
  } catch {
    // no answer
  }
  if (h) {
    if (gapStart !== null) {
      const ms = started - gapStart;
      gaps.push(ms);
      console.log(`${at()}  answering again after a ${ms} ms gap`);
      gapStart = null;
    }
    const key = `${h.version} ${h.pid}`;
    if (key !== last) console.log(`${at()}  version ${h.version}, pid ${h.pid}`);
    last = key;
  } else if (gapStart === null) {
    gapStart = started;
    console.log(`${at()}  no answer`);
  }
  await new Promise((r) => setTimeout(r, Math.max(0, 50 - (Date.now() - started))));
}
console.log(`gaps: ${gaps.length ? gaps.map((g) => `${g} ms`).join(', ') : 'none'}${gapStart !== null ? ' (and no answer at the end)' : ''}`);
