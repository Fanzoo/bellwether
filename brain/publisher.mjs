/**
 * Turns the flock into numbered snapshots for `/state`, coalesced: at most one
 * snapshot per `intervalMs` (250 ms), and a new `rev` only when the content actually
 * changed. The first change after a quiet spell publishes at once.
 *
 * Urgent changes (a new alert, or an alert being confirmed) skip the interval and
 * publish at once, and the interval then restarts from them. What keeps the rate
 * honest is a second, hard rule that urgent publishes obey too: never more than
 * `perWindow` (4) snapshots in any `windowMs` (1 s).
 */
export function createPublisher({ build, intervalMs = 250, now = Date.now, perWindow = 4, windowMs = 1000 }) {
  const first = build();
  let lastBody = JSON.stringify(first);
  let current = { rev: 0, at: now(), ...first };
  let lastPublishAt = -Infinity;
  let timer = null;
  let timerAt = Infinity;
  let urgent = false;
  let published = 0;
  const recent = []; // times of the last perWindow snapshots that went out
  const waiters = new Set();

  /** The earliest time the next snapshot may go out. */
  function earliest() {
    const byWindow = recent.length >= perWindow ? recent[recent.length - perWindow] + windowMs : -Infinity;
    return Math.max(byWindow, urgent ? -Infinity : lastPublishAt + intervalMs);
  }

  function schedule() {
    const at = earliest();
    if (timer && timerAt <= at) return;
    clearTimeout(timer);
    timerAt = at;
    timer = setTimeout(publish, Math.max(0, at - now()));
  }

  function publish() {
    clearTimeout(timer);
    timer = null;
    timerAt = Infinity;
    // Timers can fire a millisecond early; re-check, or five snapshots could fit in a second.
    if (earliest() > now()) return schedule();
    urgent = false;
    const snap = build();
    const body = JSON.stringify(snap);
    lastPublishAt = now();
    if (body === lastBody) return;
    lastBody = body;
    published++;
    recent.push(lastPublishAt);
    if (recent.length > perWindow) recent.shift();
    current = { rev: current.rev + 1, at: lastPublishAt, ...snap };
    for (const w of [...waiters]) w();
  }

  return {
    /**
     * Something may have changed; publish when the throttle allows. `urgent`: a new or
     * newly confirmed alert, published at once unless the 4-a-second cap is reached.
     */
    markDirty(isUrgent = false) {
      if (isUrgent) {
        urgent = true;
        if (earliest() <= now()) return publish();
      }
      schedule();
    },
    current: () => current,
    /** Number of snapshots with new content so far (for tests and `replay`). */
    published: () => published,
    /** Resolves with the snapshot as soon as rev > after, or with the current one after timeoutMs. */
    waitFor(after, timeoutMs) {
      if (current.rev > after || timeoutMs <= 0) return Promise.resolve(current);
      return new Promise((resolve) => {
        const done = () => {
          clearTimeout(t);
          waiters.delete(done);
          resolve(current);
        };
        const t = setTimeout(done, timeoutMs);
        waiters.add(done);
      });
    },
    close() {
      clearTimeout(timer);
      timer = null;
      timerAt = Infinity;
      for (const w of [...waiters]) w();
    },
  };
}
