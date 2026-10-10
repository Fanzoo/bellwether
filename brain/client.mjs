import http from 'node:http';
import { BIND_HOST, CliError, EXIT } from './util.mjs';

/**
 * A small HTTP client for the brain's own API (status, replay, doctor).
 *
 * Every request uses its own connection (`agent: false`). With Node's default
 * keep-alive agent, a request can be written to a pooled socket at the moment the
 * server's idle timeout closes it, which fails with ECONNRESET. A GET that fails
 * that way is retried once on a fresh connection; a POST is not, because /hook
 * events must not be delivered twice.
 */
export async function call(port, opts) {
  try {
    return await once(port, opts);
  } catch (e) {
    if ((opts.method ?? 'GET') === 'GET' && (e.code === 'ECONNRESET' || e.code === 'EPIPE')) return once(port, opts);
    throw e;
  }
}

function once(port, { method = 'GET', path, token, body = null, headers = {}, timeoutMs = 30_000 }) {
  return new Promise((resolve, reject) => {
    const data = body === null ? null : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
    const req = http.request(
      {
        host: BIND_HOST,
        port,
        method,
        agent: false,
        path,
        timeout: timeoutMs,
        headers: { ...(token ? { 'X-Pasture-Token': token } : {}), ...(data ? { 'Content-Type': 'application/json', 'Content-Length': data.length } : {}), ...headers },
      },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          const text = Buffer.concat(chunks).toString('utf8');
          let json = null;
          try {
            json = text ? JSON.parse(text) : null;
          } catch {}
          resolve({ status: res.statusCode, json, text });
        });
      },
    );
    req.on('timeout', () => req.destroy(Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' })));
    req.on('error', reject);
    req.end(data ?? undefined);
  });
}

/** GET /state, translating the usual failures into CLI errors with a cause. */
export async function getState(port, token, { after = -1, waitMs = 0 } = {}) {
  let r;
  try {
    r = await call(port, { path: `/state?after=${after}&wait=${waitMs}`, token, timeoutMs: waitMs + 10_000 });
  } catch (e) {
    if (e.code === 'ECONNREFUSED') throw new CliError(`brain not running — run \`pasture serve\` (nothing listening on ${BIND_HOST}:${port})`, EXIT.NOT_RUNNING);
    throw new CliError(`could not reach the brain on port ${port}: ${e.code ?? e.message}`, EXIT.NOT_RUNNING);
  }
  if (r.status === 403) throw new CliError('the brain refused the token in .env (403); it was started with a different PASTURE_HOOK_TOKEN. Restart `pasture serve`', EXIT.BAD_CONFIG);
  if (r.status !== 200 || !r.json) throw new CliError(`unexpected answer from /state: HTTP ${r.status}`, EXIT.UNEXPECTED);
  return r.json;
}

/** POST /control (mute, unmute, …), with the same failure messages as getState. */
export async function postControl(port, token, msg) {
  let r;
  try {
    r = await call(port, { method: 'POST', path: '/control', token, body: msg, timeoutMs: 5000 });
  } catch (e) {
    if (e.code === 'ECONNREFUSED') throw new CliError(`brain not running — run \`pasture serve\` (nothing listening on ${BIND_HOST}:${port})`, EXIT.NOT_RUNNING);
    throw new CliError(`could not reach the brain on port ${port}: ${e.code ?? e.message}`, EXIT.NOT_RUNNING);
  }
  if (r.status === 403) throw new CliError('the brain refused the token in .env (403); it was started with a different PASTURE_HOOK_TOKEN. Restart `pasture serve`', EXIT.BAD_CONFIG);
  if (r.status !== 200 || !r.json?.ok) throw new CliError(`the brain refused ${msg.action}: ${r.json?.error ?? `HTTP ${r.status}`}`, EXIT.UNEXPECTED);
  return r.json;
}

