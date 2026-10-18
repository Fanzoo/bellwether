/**
 * A test driver, copied from the Node brain at v0.2.0-rc.3 (brain/client.mjs): a small
 * HTTP client for a running brain's API. Not the brain: no test checks it as one.
 *
 * Every request uses its own connection (`agent: false`). With Node's default
 * keep-alive agent, a request can be written to a pooled socket at the moment the
 * server's idle timeout closes it, which fails with ECONNRESET. A GET that fails
 * that way is retried once on a fresh connection; a POST is not, because /hook
 * events must not be delivered twice.
 */
import http from 'node:http';

const BIND_HOST = '127.0.0.1';

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
        headers: { ...(token ? { 'X-Bellwether-Token': token } : {}), ...(data ? { 'Content-Type': 'application/json', 'Content-Length': data.length } : {}), ...headers },
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
