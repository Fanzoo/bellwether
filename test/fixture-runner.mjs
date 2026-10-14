import fs from 'node:fs';
// The brain under test: brain/flock.mjs, or under BELLWETHER_IMPL=rust the Rust reducer,
// called through test/impl.mjs, so this same loop drives both.
import { Flock, withDefaults } from './impl.mjs';

/**
 * Replays a fixture through the reducer with a fake clock: each event is applied at
 * its own receivedAt, and the flock is ticked every `stepMs` from the first event to
 * `tailMs` after the last. Returns the transitions: one line whenever an actor's
 * shown state, flavour or alert changes, with times relative to the first event.
 */
export function runFixture(lines, { config = withDefaults({ port: 47417 }), stepMs = 250, tailMs = 3000 } = {}) {
  const events = lines.map((l) => l.normalised);
  const t0 = events[0].receivedAt;
  const flock = new Flock({ config, t: t0 });
  const last = new Map();
  const out = [];
  const snapshots = [];
  const record = (t) => {
    const snap = flock.snapshot();
    snapshots.push(snap);
    const seen = new Set();
    const note = (actor, a) => {
      seen.add(actor);
      const view = { state: a.state };
      if (a.flavour) view.flavour = a.flavour;
      if (a.alert) view.alert = `${a.alert.kind}: ${a.alert.text}${a.alert.confirmed ? ' (confirmed)' : ''}`;
      const key = JSON.stringify(view);
      if (last.get(actor) !== key) {
        last.set(actor, key);
        out.push({ atMs: t - t0, actor, ...view });
      }
    };
    for (const s of snap.flock) {
      note(`sheep:${s.identityKey}`, s);
      for (const l of s.lambs) note(`lamb:${l.agentType}:${l.id}`, l);
    }
    for (const actor of last.keys()) {
      if (!seen.has(actor) && last.get(actor) !== 'gone') {
        last.set(actor, 'gone');
        out.push({ atMs: t - t0, actor, state: 'gone' });
      }
    }
  };
  let i = 0;
  const end = events.at(-1).receivedAt + tailMs;
  for (let t = t0; t <= end; t += stepMs) {
    while (i < events.length && events[i].receivedAt <= t) {
      flock.apply(events[i]);
      record(events[i].receivedAt);
      i++;
    }
    flock.tick(t);
    record(t);
  }
  return { transitions: out, snapshots, flock };
}

export function readJsonl(file) {
  return fs.readFileSync(file, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l));
}
