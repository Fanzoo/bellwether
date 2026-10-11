import fs from 'node:fs';
import http from 'node:http';
import { envAssignments } from './envfile.mjs';
import { BIND_HOST, DOCTOR_PING } from './util.mjs';

/**
 * `node brain/probe.mjs --port <n> --env <file> [--timeout <s>]`: waits until a
 * bellwether brain answers on the port (/health says "bellwether") and accepts the
 * token in the .env (a doctor ping to /hook gets 204). Exit 0 when it does, 1 with the
 * reason when the time runs out. `migrate` runs this after loading the new brain,
 * before it rewrites any hook. The token is read from the file, never from argv, and
 * never printed.
 */
const arg = (k) => {
  const i = process.argv.indexOf(`--${k}`);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const port = Number(arg('port'));
const envFile = arg('env');
const deadline = Date.now() + 1000 * (Number(arg('timeout')) || 15);
let token = null;
try {
  const text = fs.readFileSync(envFile, 'utf8');
  token = envAssignments(text, 'BELLWETHER_HOOK_TOKEN').at(-1)?.value || null;
} catch {}
if (!Number.isInteger(port) || !token) {
  process.stdout.write(!token ? `no BELLWETHER_HOOK_TOKEN in ${envFile}\n` : 'usage: probe.mjs --port <n> --env <file>\n');
  process.exit(1);
}

const get = () =>
  new Promise((resolve) => {
    const req = http.get({ host: BIND_HOST, port, path: '/health', timeout: 1000 }, (res) => {
      let b = '';
      res.setEncoding('utf8');
      res.on('data', (c) => (b += c));
      res.on('end', () => {
        try {
          resolve(JSON.parse(b));
        } catch {
          resolve(null);
        }
      });
    });
    req.on('timeout', () => (req.destroy(), resolve(null)));
    req.on('error', () => resolve(null));
  });
const ping = () =>
  new Promise((resolve) => {
    const body = JSON.stringify({ hook_event_name: DOCTOR_PING });
    const req = http.request({ host: BIND_HOST, port, path: '/hook', method: 'POST', timeout: 1000, headers: { 'Content-Type': 'application/json', 'X-Bellwether-Token': token, 'Content-Length': Buffer.byteLength(body) } }, (res) => {
      res.resume();
      res.on('end', () => resolve(res.statusCode));
    });
    req.on('timeout', () => (req.destroy(), resolve(null)));
    req.on('error', () => resolve(null));
    req.end(body);
  });

let last = 'nothing answered';
while (Date.now() < deadline) {
  const h = await get();
  if (h?.name === 'bellwether') {
    const status = await ping();
    if (status === 204) {
      process.stdout.write(`the bellwether brain answers on port ${port} (pid ${h.pid}) and accepts the token\n`);
      process.exit(0);
    }
    last = `the bellwether brain (pid ${h.pid}) refused the token (${status ?? 'no answer'})`;
  } else if (h?.name) last = `a ${h.name} brain (pid ${h.pid}) still holds port ${port}`;
  await new Promise((r) => setTimeout(r, 300));
}
process.stdout.write(`${last} after waiting\n`);
process.exit(1);
