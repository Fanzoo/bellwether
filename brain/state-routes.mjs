import { createHash, timingSafeEqual } from 'node:crypto';

/**
 * GET /state and POST /control, for the shell and `bellwether status`. Same guards as
 * /hook: no Origin header, Host must be 127.0.0.1:<port> or localhost:<port>, and
 * X-Bellwether-Token must match. They never send CORS headers, so a browser can't read
 * them even by accident.
 *
 *   GET  /state?after=<rev>[&wait=<ms>]  the Snapshot as soon as rev > after, or the
 *        current one after `wait` (default and maximum 25 s; 0 answers at once)
 *   POST /control  {action: "mute" | "unmute" | "dismiss" | "placed", ...}
 */
const LONG_POLL_MS = 25_000;
const MAX_CONTROL_BYTES = 16 * 1024;

export function createStateRoutes({ port, token, publisher, control, log = () => {} }) {
  const allowedHosts = new Set([`127.0.0.1:${port}`, `localhost:${port}`]);
  const expected = digest(token);

  return function route(req, res, path, query) {
    if (path !== '/state' && path !== '/control') return false;
    const reason = rejectionReason(req.headers, allowedHosts, expected);
    if (reason) {
      log(`rejected ${path}: ${reason}`);
      req.resume();
      res.writeHead(403, { 'Content-Length': '0' });
      res.end();
      return true;
    }
    if (path === '/state' && req.method === 'GET') {
      req.resume();
      const after = Number.parseInt(query.get('after') ?? '-1', 10);
      const wait = Math.min(LONG_POLL_MS, Math.max(0, Number.parseInt(query.get('wait') ?? String(LONG_POLL_MS), 10) || 0));
      let closed = false;
      res.on('close', () => (closed = true));
      publisher.waitFor(Number.isFinite(after) ? after : -1, wait).then((snap) => {
        if (!closed) json(res, 200, snap);
      });
      return true;
    }
    if (path === '/control' && req.method === 'POST') {
      const type = String(req.headers['content-type'] ?? '').split(';')[0].trim().toLowerCase();
      if (type !== 'application/json') {
        req.resume();
        json(res, 415, { ok: false, error: 'Content-Type must be application/json' });
        return true;
      }
      const chunks = [];
      let size = 0;
      req.on('data', (c) => {
        size += c.length;
        if (size <= MAX_CONTROL_BYTES) chunks.push(c);
      });
      req.on('end', () => {
        if (size > MAX_CONTROL_BYTES) return json(res, 413, { ok: false, error: 'body too large' });
        let msg;
        try {
          msg = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        } catch {
          return json(res, 400, { ok: false, error: 'not JSON' });
        }
        const result = control(msg);
        json(res, result.ok ? 200 : 400, result);
      });
      return true;
    }
    req.resume();
    res.writeHead(405, { 'Content-Length': '0' });
    res.end();
    return true;
  };
}

function json(res, status, value) {
  const body = Buffer.from(JSON.stringify(value));
  res.writeHead(status, { 'Content-Type': 'application/json', 'Content-Length': body.length, 'Cache-Control': 'no-store' });
  res.end(body);
}

function rejectionReason(headers, allowedHosts, expected) {
  if (headers.origin !== undefined) return 'origin';
  if (!allowedHosts.has(String(headers.host ?? '').toLowerCase())) return 'host';
  const given = headers['x-bellwether-token'];
  if (typeof given !== 'string' || !timingSafeEqual(digest(given), expected)) return 'token';
  return null;
}

function digest(value) {
  return createHash('sha256').update(String(value)).digest();
}
