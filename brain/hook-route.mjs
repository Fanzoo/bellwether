import { createHash, timingSafeEqual } from 'node:crypto';

/**
 * The POST /hook handler: the only thing Claude Code ever talks to.
 *
 * A 2xx response with a JSON body is parsed by Claude Code as hook output, and on
 * PermissionRequest that output can approve or deny on the user's behalf. So this
 * module has exactly one way to answer, `answer()`, and it never sends a body: an
 * empty 204, or an empty 403 when the request fails a guard. A source-scan test
 * fails if anything in this file could write a body. Keep it that way: no
 * serialisation, no body-writing calls, nothing passed to the end call.
 *
 * Guards, in order:
 *   1. an Origin header       -> 403  (a browser; DNS rebinding)
 *   2. Host not 127.0.0.1:<port> or localhost:<port> -> 403
 *   3. Content-Type not application/json -> 403  (forces a CORS preflight, which
 *      the brain never answers)
 *   4. X-Bellwether-Token wrong or missing -> 403, compared in constant time
 *
 * Everything past the guards fails open and silent: a malformed body, an oversize
 * body, an unknown event or a throwing consumer all still get 204. The answer is
 * sent when the body has arrived and before it is parsed; parsing and the consumer
 * run on the next tick, so the state machine never adds latency to a tool call.
 */

export const MAX_BODY_BYTES = 8 * 1024 * 1024;

export function newCounters() {
  return {
    accepted: 0,
    rejected: { origin: 0, host: 0, contentType: 0, token: 0 },
    malformed: 0,
    oversize: 0,
    unknownEvent: 0,
    consumerErrors: 0,
  };
}

/**
 * @param {object} o
 * @param {number} o.port         the port actually bound, for the Host check
 * @param {string} o.token        BELLWETHER_HOOK_TOKEN
 * @param {(payload: object, headers: object, receivedAt: number) => void} o.onEvent
 * @param {Set<string>} o.knownEvents
 * @param {object} o.counters     from newCounters()
 * @param {(line: string) => void} o.log   content-free log lines only
 * @param {number} [o.delayMs]    debug (`serve --slow`): wait before answering
 * @param {boolean} [o.hang]      debug (`serve --hang`): accept and never answer
 * @param {() => number} [o.now]
 */
export function createHookRoute({ port, token, onEvent, knownEvents, counters, log, delayMs = 0, hang = false, now = Date.now }) {
  const allowedHosts = new Set([`127.0.0.1:${port}`, `localhost:${port}`]);
  const expected = digest(token);

  return function handleHook(req, res) {
    const reason = rejectionReason(req.headers, allowedHosts, expected);
    if (reason) {
      counters.rejected[reason]++;
      log(`rejected /hook: ${reason}`);
      answer(res, 403);
      req.resume();
      return;
    }

    const receivedAt = now();
    const chunks = [];
    let size = 0;
    let oversize = false;
    req.on('data', (chunk) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        oversize = true;
        chunks.length = 0;
      } else if (!oversize) {
        chunks.push(chunk);
      }
    });
    req.on('error', () => {});
    req.on('end', () => {
      if (hang) return; // debug only: the socket stays open until Claude Code gives up
      if (delayMs > 0) setTimeout(() => answer(res, 204), delayMs);
      else answer(res, 204);
      counters.accepted++;
      setImmediate(() => consume(oversize ? null : Buffer.concat(chunks), req.headers, receivedAt));
    });
  };

  function consume(body, headers, receivedAt) {
    if (body === null) {
      counters.oversize++;
      log(`dropped /hook body over ${MAX_BODY_BYTES} bytes`);
      return;
    }
    let payload;
    try {
      payload = JSON.parse(body.toString('utf8'));
    } catch {
      payload = null;
    }
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
      counters.malformed++;
      log('dropped /hook body: not a JSON object');
      return;
    }
    if (!knownEvents.has(payload.hook_event_name)) counters.unknownEvent++;
    try {
      onEvent(payload, headers, receivedAt);
    } catch (e) {
      counters.consumerErrors++;
      log(`event consumer threw: ${e?.name ?? 'Error'}`);
    }
  }
}

/** The single response path for /hook. Status only; never a body. */
function answer(res, status) {
  if (res.headersSent || res.destroyed) return;
  if (status === 204) res.writeHead(204);
  else res.writeHead(403, { 'Content-Length': '0' });
  res.end();
}

function rejectionReason(headers, allowedHosts, expected) {
  if (headers.origin !== undefined) return 'origin';
  if (!allowedHosts.has(String(headers.host ?? '').toLowerCase())) return 'host';
  const mediaType = String(headers['content-type'] ?? '').split(';')[0].trim().toLowerCase();
  if (mediaType !== 'application/json') return 'contentType';
  const given = headers['x-bellwether-token'];
  if (typeof given !== 'string' || !timingSafeEqual(digest(given), expected)) return 'token';
  return null;
}

/** Hashing first gives equal-length buffers, so the comparison leaks no length. */
function digest(value) {
  return createHash('sha256').update(String(value)).digest();
}
