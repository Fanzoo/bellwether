import path from 'node:path';
import { Flock } from './flock.mjs';
import { normalise } from './normalise.mjs';
import { loadState, saveState } from './persist.mjs';
import { createPublisher } from './publisher.mjs';
import { makeTextCleaner } from './redact.mjs';
import { createRecorder, describe, startServer } from './server.mjs';
import { createStateRoutes } from './state-routes.mjs';

/**
 * The running brain: listener + normaliser + flock + publisher + persistence.
 * `serve` and `record` use it; tests start it on port 0 with a temp data dir.
 *
 * Events are normalised with the text cleaner (redaction, truncation) before they
 * reach the flock. The recorder, when there is one, sees the raw payload but writes
 * only field names and the cleaner-free normalised event (no free text).
 */
export async function startBrain({ config, token, dataDir, log = () => {}, hang = false, slowMs = 0, recordTo = null, tickMs = 1000, persistMs = 2000, publishMs = 250 }) {
  const clean = makeTextCleaner(config.bubble);
  const stateFile = path.join(dataDir, 'state.json');
  const flock = new Flock({ config, persisted: loadState(stateFile, (w) => log(`warning: ${w}`)), t: Date.now() });
  const publisher = createPublisher({ build: () => flock.snapshot(), intervalMs: publishMs });
  const recorder = recordTo ? createRecorder(recordTo) : null;

  const server = await startServer({
    port: config.port,
    token,
    hang,
    slowMs,
    log,
    onEvent: (payload, headers, receivedAt) => {
      recorder?.write(payload, headers, receivedAt);
      const ev = normalise(payload, headers, receivedAt, { clean });
      log(describe(ev));
      const before = flock.alertMarks();
      if (flock.apply(ev)) publisher.markDirty(isRaised(before, flock.alertMarks()));
    },
    makeRoutes: (boundPort) =>
      createStateRoutes({
        port: boundPort,
        token,
        publisher,
        log,
        control: (msg) => {
          const r = flock.control(msg, Date.now());
          // Someone asked (the tray, `bellwether mute`): show it now, not after the throttle.
          if (r.ok) publisher.markDirty(true);
          return r;
        },
      }),
  });

  const save = () => {
    if (!flock.persistDirty) return;
    flock.persistDirty = false;
    try {
      saveState(stateFile, flock.persistable());
    } catch (e) {
      log(`warning: could not save ${stateFile} (${e.code ?? e.message})`);
    }
  };
  const ticker = setInterval(() => {
    flock.tick(Date.now());
    publisher.markDirty();
  }, tickMs);
  const saver = setInterval(save, persistMs);

  return {
    port: server.port,
    counters: server.counters,
    flock,
    publisher,
    recorder,
    async close() {
      clearInterval(ticker);
      clearInterval(saver);
      publisher.close();
      save();
      await server.close();
      recorder?.close();
    },
  };
}

/** True if `after` has an alert mark `before` didn't: a new alert, or one just confirmed. */
function isRaised(before, after) {
  for (const m of after) if (!before.has(m)) return true;
  return false;
}
