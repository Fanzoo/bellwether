import { spawnSync } from 'node:child_process';
import { BIND_HOST } from './util.mjs';

/**
 * Who holds the brain's port, read-only: nothing, a brain (bellwether's or pasture's,
 * by its /health answer, with its pid), or something else. For something else, the
 * listening pid and its command line come from `lsof` and `ps`; nothing is signalled.
 * Synchronous (a short child process), so `migrate` can stay a plain function.
 *
 * @returns {{state: 'free'} | {state: 'brain', name: string, pid: number|null} | {state: 'other', pid: number|null, command: string|null}}
 */
export function portHolder(port, { timeoutMs = 1500 } = {}) {
  const script = `
    const http = require('node:http');
    const req = http.get({ host: ${JSON.stringify(BIND_HOST)}, port: ${Number(port)}, path: '/health', timeout: ${timeoutMs} }, (res) => {
      let b = ''; res.setEncoding('utf8'); res.on('data', (c) => { if (b.length < 4096) b += c; });
      res.on('end', () => { let j = null; try { j = JSON.parse(b); } catch {} process.stdout.write(JSON.stringify(j && (j.name === 'bellwether' || j.name === 'pasture') ? { state: 'brain', name: j.name, pid: Number.isInteger(j.pid) ? j.pid : null } : { state: 'other' })); });
    });
    req.on('timeout', () => { req.destroy(); process.stdout.write(JSON.stringify({ state: 'other' })); });
    req.on('error', (e) => process.stdout.write(JSON.stringify(e.code === 'ECONNREFUSED' ? { state: 'free' } : { state: 'other' })));`;
  const r = spawnSync(process.execPath, ['-e', script], { encoding: 'utf8', timeout: timeoutMs + 3000 });
  let v;
  try {
    v = JSON.parse(r.stdout);
  } catch {
    v = { state: 'other' };
  }
  if (v.state !== 'other') return v;
  return { state: 'other', ...listener(port) };
}

/** The pid listening on 127.0.0.1:<port> and its command line (lsof, ps; read-only). */
export function listener(port) {
  const l = spawnSync('lsof', ['-nP', `-iTCP@${BIND_HOST}:${Number(port)}`, '-sTCP:LISTEN', '-Fp'], { encoding: 'utf8', timeout: 5000 });
  const pid = Number(/^p(\d+)$/m.exec(l.stdout ?? '')?.[1]) || null;
  if (!pid) return { pid: null, command: null };
  const ps = spawnSync('ps', ['-o', 'command=', '-p', String(pid)], { encoding: 'utf8', timeout: 5000 });
  return { pid, command: (ps.stdout ?? '').trim() || null };
}
