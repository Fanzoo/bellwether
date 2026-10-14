import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { EVENTS } from './hooks.mjs';
import { createHookRoute, newCounters } from './hook-route.mjs';
import { adoptLegacyHeaders, headersPresent, normalise, rawKeys } from './normalise.mjs';
import { BIND_HOST, CliError, DOCTOR_PING, EXIT, VERSION, clock } from './util.mjs';

/**
 * The brain's HTTP listener:
 *   POST /hook    Claude Code -> brain (hook-route.mjs; always an empty 204/403)
 *   GET  /health  {ok, version, pid, port, lastEventAt, counters}; no token, no content
 *   GET  /state, POST /control   from `routes` (state-routes.mjs), token required
 * Everything else is an empty 404, and OPTIONS is an empty 405 with no CORS headers,
 * so a browser preflight to /hook always fails.
 *
 * Debug modes for the spike: `hang` accepts /hook and never answers; `slowMs`
 * answers after a delay. Both still send nothing but an empty 204.
 */
export async function startServer({ port, token, onEvent = () => {}, log = () => {}, hang = false, slowMs = 0, makeRoutes = null }) {
  const counters = newCounters();
  const knownEvents = new Set(EVENTS);
  let lastEventAt = null;
  let boundPort = port;
  let hookRoute = null;
  let routes = null;

  const server = http.createServer((req, res) => {
    adoptLegacyHeaders(req.headers);
    const url = req.url ?? '/';
    const route = url.split('?')[0];
    if (route === '/hook' && req.method === 'POST') return hookRoute(req, res);
    if (routes && route !== '/hook' && routes(req, res, route, new URLSearchParams(url.split('?')[1] ?? ''))) return;
    if (route === '/health' && req.method === 'GET') {
      const body = Buffer.from(
        JSON.stringify({ ok: true, name: 'bellwether', version: VERSION, pid: process.pid, port: boundPort, lastEventAt, mode: hang ? 'hang' : slowMs ? `slow=${slowMs}` : 'normal', counters }),
      );
      res.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': body.length, 'Cache-Control': 'no-store' });
      res.end(body);
      return;
    }
    req.resume();
    res.writeHead(req.method === 'OPTIONS' ? 405 : 404, { 'Content-Length': '0' });
    res.end();
  });
  // Claude Code gives up after `timeout: 1`; nothing legitimate is slower than this.
  server.headersTimeout = 10_000;
  server.requestTimeout = hang ? 0 : 30_000;

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, BIND_HOST, () => {
      server.off('error', reject);
      resolve();
    });
  }).catch((e) => {
    if (e.code === 'EADDRINUSE') {
      throw new CliError(`port ${port} on ${BIND_HOST} is already in use. Run \`bellwether doctor\` to see what holds it.`, EXIT.NOT_RUNNING);
    }
    throw e;
  });
  boundPort = server.address().port;

  if (makeRoutes) routes = makeRoutes(boundPort);
  hookRoute = createHookRoute({
    port: boundPort,
    token,
    knownEvents,
    counters,
    log,
    hang,
    delayMs: slowMs,
    onEvent: (payload, headers, receivedAt) => {
      if (payload.hook_event_name === DOCTOR_PING) return;
      lastEventAt = receivedAt;
      onEvent(payload, headers, receivedAt);
    },
  });

  return {
    server,
    port: boundPort,
    counters,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/** One content-free line per event: time, event, short session id, agent, tool. */
export function describe(ev) {
  const sid = ev.sessionId ? ev.sessionId.slice(0, 8) : '--------';
  const who = ev.agentId ? `${ev.agentType ?? 'agent'}#${ev.agentId.slice(0, 6)}` : 'main';
  const what = [ev.toolName, ev.notificationType, ev.source, ev.reason, ev.trigger, ev.errorType].filter(Boolean).join(' ');
  return `${clock(ev.receivedAt)}  ${ev.event.padEnd(18)} s:${sid}  ${who.padEnd(16)} ${what}`;
}

/**
 * Appends one line per event to a recording: the normalised event, the field names
 * of the raw payload, and which X-Bellwether-* headers were non-empty. Raw values never
 * reach the file.
 */
export function createRecorder(file) {
  let fd;
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fd = fs.openSync(file, 'a');
  } catch (e) {
    // A directory that can't be written: one line and exit 3, not a stack trace.
    throw new CliError(`cannot open ${file}: ${e.code ?? e.message}`, EXIT.BAD_CONFIG);
  }
  return {
    file,
    write(payload, headers, receivedAt) {
      const line = { normalised: normalise(payload, headers, receivedAt), rawKeys: rawKeys(payload), headersPresent: headersPresent(headers) };
      fs.writeSync(fd, JSON.stringify(line) + '\n');
    },
    close: () => fs.closeSync(fd),
  };
}
