import { identityOf, sheepId } from './identity.mjs';
import { AGENT_TYPE_CHARS } from './normalise.mjs';
import { cleanLabel, truncate } from './redact.mjs';
import { setOwn } from './util.mjs';

/**
 * The flock: sheep (sessions) and lambs (subagents), their calm states, and their
 * alerts. A pure reducer: nothing here reads the clock. `apply(ev)` takes the time
 * from `ev.receivedAt`, and `tick(t)` re-evaluates states at time `t`, so a test can
 * replay a fixture with a fake clock and get the same transitions every time.
 *
 * Calm states: arriving, busy (flavour running | digging | reading | thinking), done,
 * idle (grazing), waiting, sleeping, compacting, error (flavour = error kind),
 * leaving. Each is held for at least behaviour.minDwellMs before another replaces
 * it (hysteresis). Alerts are an overlay with absolute priority and never wait.
 *
 * Alert matching and clearing rules, including this one: UserPromptSubmit, Stop and StopFailure clear only the
 * sheep's own alerts, because a lamb can still be waiting on a prompt after the
 * main thread's turn ended.
 */

const PENDING_TTL_MS = 15 * 60_000;
const PENDING_CAP = 256;
const PROGRESS_GUARD_MS = 2000;
const NOTIFY_AFTER_CLEAR_GUARD_MS = 2000;
const EFFECT_MS = 1500;
const IDENTITY_TTL_MS = 7 * 24 * 3600_000;
// Bounds on what a token holder (or a hand-edited state.json) can make the brain keep.
export const LIMITS = {
  identities: 2000, // most recently seen kept
  monitorsPerIdentity: 16,
  monitorNameChars: 4096,
  identityKeyChars: 1024,
  labelChars: 200,
  lambsPerSheep: 64, // beyond this the oldest lamb without an alert is dropped
  sheep: 200, // present at once; beyond this the least recently seen without an alert leaves
};

const CATEGORY = {
  Bash: 'running',
  BashOutput: 'running',
  KillShell: 'running',
  Edit: 'digging',
  MultiEdit: 'digging',
  Write: 'digging',
  NotebookEdit: 'digging',
  Read: 'reading',
  Grep: 'reading',
  Glob: 'reading',
  LS: 'reading',
  WebFetch: 'reading',
  WebSearch: 'reading',
};
const categoryOf = (toolName) => CATEGORY[toolName] ?? 'thinking';

export class Flock {
  /**
   * @param {object} o
   * @param {object} o.config   merged config (defaults.mjs)
   * @param {object} [o.persisted]  data/state.json contents: {muted, identities}
   * @param {number} [o.t]      time of construction, for pruning identities
   */
  constructor({ config, persisted = null, t = 0 }) {
    const b = config.behaviour;
    this.cfg = {
      busyWindowMs: b.busyWindowSec * 1000,
      grazeAfterMs: b.grazeAfterSec * 1000,
      sleepAfterMs: b.sleepAfterMin * 60_000,
      minDwellMs: b.minDwellMs,
      reapAfterMs: b.reapAfterHours * 3600_000,
      lambTimeoutMs: b.lambTimeoutMin * 60_000,
      toolTimeoutMs: b.toolTimeoutMin * 60_000,
      thinkingTimeoutMs: b.thinkingTimeoutSec * 1000,
      maxLambsShown: b.maxLambsShown,
      paletteSize: config.pets.palette.length,
      detail: config.bubble.detail,
      maxChars: config.bubble.maxChars,
    };
    this.sheep = new Map(); // identityKey -> sheep
    this.sessions = new Map(); // session_id -> identityKey
    // data/state.json is ours, but a hand edit or a bad write must not reach the shell:
    // every field is checked for type and range, and anything else is dropped.
    this.muted = persisted?.muted === true;
    this.identities = new Map();
    const raw = persisted && typeof persisted.identities === 'object' && !Array.isArray(persisted.identities) ? persisted.identities : {};
    const loaded = [];
    for (const [k, v] of Object.entries(raw)) {
      const ident = sanitiseIdentity(k, v, t, this.cfg.paletteSize);
      if (ident) loaded.push([k, ident]);
    }
    loaded.sort((a, b) => b[1].lastSeen - a[1].lastSeen);
    for (const [k, ident] of loaded.slice(0, LIMITS.identities)) this.identities.set(k, ident);
    this.persistDirty = false;
  }

  // ---------------------------------------------------------------- events

  /** Applies one normalised HookEvent. Returns true if anything may have changed. */
  apply(ev) {
    const t = ev.receivedAt;
    if (!ev.sessionId || ev.event === 'unknown') return false;
    const handler = HANDLERS[ev.event];
    if (!handler) return false;
    // A session that ends before we ever saw it (brain started late) gets no sheep:
    // there's nothing to show but a sheep leaving.
    if (ev.event === 'SessionEnd' && !this.sessions.has(ev.sessionId)) return false;
    const sheep = this.#sheepFor(ev, t);
    handler.call(this, sheep, ev, t);
    this.#evaluate(t);
    return true;
  }

  /** Re-evaluates every actor at time t: states, effects, leaving, reaping. */
  tick(t) {
    for (const [key, s] of this.sheep) {
      if (t - s.lastEventAt > this.cfg.reapAfterMs) {
        this.#forget(key, s);
        continue;
      }
      s.pending = s.pending.filter((p) => t - p.at < PENDING_TTL_MS);
      // A tool whose PostToolUse never came, and a lamb whose SubagentStop never came,
      // can't linger: they time out (behaviour.toolTimeoutMin, lambTimeoutMin). An
      // alert keeps a lamb, though: there is no timeout on alerts.
      for (const a of [s, ...s.lambs.values()]) {
        for (const [id, f] of a.inFlight) if (t - f.at >= this.cfg.toolTimeoutMs) a.inFlight.delete(id);
      }
      for (const l of s.lambs.values()) {
        if (l.leavingSince == null && !l.alert && t - l.lastEventAt >= this.cfg.lambTimeoutMs) {
          l.alive = false;
          l.inFlight.clear();
          l.leavingSince = t;
        }
      }
    }
    this.#evaluate(t);
  }

  /** `/control` actions from the shell or the CLI. */
  control(msg, t) {
    switch (msg?.action) {
      case 'mute':
      case 'unmute':
        this.muted = msg.action === 'mute';
        this.persistDirty = true;
        return { ok: true };
      case 'dismiss': {
        const actor = this.#actorById(msg.actorId);
        if (!actor) return { ok: false, error: 'no such actor' };
        actor.alert = null;
        return { ok: true };
      }
      case 'placed': {
        const ident = typeof msg.identityKey === 'string' ? this.identities.get(msg.identityKey) : null;
        if (!ident || typeof msg.monitor !== 'string' || msg.monitor === '' || !Number.isInteger(msg.slot) || msg.slot < 0 || msg.slot > 10_000) return { ok: false, error: 'placed needs a known identityKey, a monitor name and a slot from 0 to 10000' };
        if (msg.monitor.length > LIMITS.monitorNameChars) return { ok: false, error: `monitor name longer than ${LIMITS.monitorNameChars} characters` };
        if (!Object.hasOwn(ident.slots, msg.monitor) && Object.keys(ident.slots).length >= LIMITS.monitorsPerIdentity) return { ok: false, error: `at most ${LIMITS.monitorsPerIdentity} monitors per sheep` };
        setOwn(ident.slots, msg.monitor, msg.slot);
        ident.lastSeen = t;
        this.persistDirty = true;
        return { ok: true };
      }
      default:
        return { ok: false, error: 'unknown action' };
    }
  }

  // -------------------------------------------------------------- snapshot

  /** The shell's view: already truncated and redacted text only. */
  /**
   * One mark per open alert and whether it is confirmed. A mark that wasn't there
   * before an event means a new or newly confirmed alert, which the brain publishes
   * without waiting for the throttle.
   */
  alertMarks() {
    const marks = new Set();
    for (const [key, s] of this.sheep) {
      if (s.alert) marks.add(`${key}\0\0${s.alert.kind}\0${s.alert.since}\0${s.alert.confirmed}`);
      for (const [id, l] of s.lambs) if (l.alert) marks.add(`${key}\0${id}\0${l.alert.kind}\0${l.alert.since}\0${l.alert.confirmed}`);
    }
    return marks;
  }

  snapshot() {
    const flock = [...this.sheep.values()]
      .sort((a, b) => a.slot - b.slot || a.created - b.created)
      .map((s) => {
        const lambs = [...s.lambs.values()].sort((a, b) => (b.alert ? 1 : 0) - (a.alert ? 1 : 0) || a.created - b.created);
        const shown = lambs.slice(0, this.cfg.maxLambsShown);
        return clean({
          id: s.id,
          identityKey: s.identityKey,
          label: s.label,
          colourIndex: s.colourIndex,
          slot: s.slot,
          // Per monitor, as the shell placed this identity before (`placed`); the shell
          // uses `slot` on the primary monitor and these on the others (Milestone 8).
          slots: { ...(this.identities.get(s.identityKey)?.slots ?? {}) },
          state: s.shown.state,
          flavour: s.shown.flavour,
          since: s.shown.since,
          alert: alertView(s.alert),
          effect: s.effect ? { ...s.effect } : undefined,
          terminal: { ...s.terminal },
          lambAlert: lambs.some((l) => l.alert) || undefined,
          lambs: shown.map((l) =>
            clean({
              id: l.id,
              agentType: l.agentType,
              label: l.agentType,
              state: l.shown.state,
              flavour: l.shown.flavour,
              alert: alertView(l.alert),
              effect: l.effect ? { ...l.effect } : undefined,
            }),
          ),
          hiddenLambCount: lambs.length - shown.length,
        });
      });
    return { muted: this.muted, flock };
  }

  /** What data/state.json keeps between runs. */
  persistable() {
    return { version: 1, muted: this.muted, identities: Object.fromEntries(this.identities) };
  }

  // -------------------------------------------------------------- internals

  #sheepFor(ev, t) {
    const { key, terminalBased } = identityOf(ev);
    const previousKey = this.sessions.get(ev.sessionId);
    if (previousKey !== undefined && previousKey !== key) {
      // The same session now reports another terminal (resumed in another tab).
      const old = this.sheep.get(previousKey);
      if (old && old.sessionId === ev.sessionId) this.#leave(old, t);
    }
    let s = this.sheep.get(key);
    if (!s) {
      this.#capSheep();
      s = this.#newSheep(key, terminalBased, t);
      this.sheep.set(key, s);
    }
    if (s.sessionId !== ev.sessionId) {
      // A newer session in the same pane takes the sheep over (after /clear, or a new
      // `claude` in the same tab). Nothing of the old session's alerts or lambs survives.
      if (s.sessionId) this.sessions.delete(s.sessionId);
      s.sessionId = ev.sessionId;
      s.alert = null;
      s.lambs.clear();
      s.pending = [];
      resetTurn(s);
      s.leavingSince = null;
    }
    this.sessions.set(ev.sessionId, key);
    s.lastEventAt = t;
    const label = cleanLabel(ev.projectLabel, LIMITS.labelChars);
    if (label) s.label = label;
    if (Object.keys(ev.terminal ?? {}).length) s.terminal = { ...ev.terminal };
    const ident = this.identities.get(key);
    ident.lastSeen = t;
    ident.label = typeof s.label === 'string' ? s.label.slice(0, LIMITS.labelChars) : undefined;
    this.persistDirty = true;
    return s;
  }

  /**
   * Keeps at most LIMITS.sheep present: the least recently seen sheep without an alert
   * is forgotten to make room (its identity, colour and slot stay saved). Without this
   * cap, thousands of terminal-less sessions would all be "on screen" and the
   * identity cap below could never apply.
   */
  #capSheep() {
    if (this.sheep.size < LIMITS.sheep) return;
    const all = [...this.sheep].sort((a, b) => a[1].lastEventAt - b[1].lastEventAt);
    const victim = all.find(([, s]) => !s.alert && ![...s.lambs.values()].some((l) => l.alert)) ?? all[0];
    this.#forget(victim[0], victim[1]);
  }

  /** Keeps at most LIMITS.identities, dropping the least recently seen that isn't present. */
  #capIdentities(keep) {
    if (this.identities.size <= LIMITS.identities) return;
    const candidates = [...this.identities].filter(([k]) => k !== keep && !this.sheep.has(k)).sort((a, b) => a[1].lastSeen - b[1].lastSeen);
    for (const [k] of candidates.slice(0, this.identities.size - LIMITS.identities)) this.identities.delete(k);
  }

  #newSheep(key, terminalBased, t) {
    let ident = this.identities.get(key);
    if (!ident) {
      ident = { colourIndex: undefined, slots: {}, label: undefined, lastSeen: t };
      this.identities.set(key, ident);
      this.#capIdentities(key);
    }
    const present = [...this.sheep.values()];
    const usedColours = new Set(present.map((s) => s.colourIndex));
    let colourIndex = ident.colourIndex;
    if (!Number.isInteger(colourIndex) || colourIndex >= this.cfg.paletteSize || usedColours.has(colourIndex)) {
      colourIndex = lowestFree(usedColours, this.cfg.paletteSize) ?? present.length % this.cfg.paletteSize;
    }
    ident.colourIndex = colourIndex;
    const usedSlots = new Set(present.map((s) => s.slot));
    let slot = ident.slots.primary;
    if (!Number.isInteger(slot) || usedSlots.has(slot)) slot = lowestFree(usedSlots, Infinity);
    ident.slots.primary = slot;
    return {
      ...newActor('sheep', sheepId(key), t),
      identityKey: key,
      terminalBased,
      sessionId: null,
      label: ident.label ?? key,
      terminal: {},
      lambs: new Map(),
      pending: [],
      colourIndex,
      slot,
      compacting: false,
      lastPermissionClearAt: -Infinity,
    };
  }

  /** The actor an event belongs to: its lamb, or the sheep. `null` means ignore it. */
  #actorFor(s, ev, t, { create = true } = {}) {
    if (!ev.agentId) return s;
    const lamb = s.lambs.get(ev.agentId);
    if (lamb) {
      lamb.lastEventAt = t;
      return lamb;
    }
    // A lamb first seen mid-flight appears on its first event, but only with a real
    // agent type. Claude Code's internal helper agent has agent_type "" and must
    // never become a lamb (Milestone 1 Q3).
    if (!create || !ev.agentType) return null;
    if (s.lambs.size >= LIMITS.lambsPerSheep) {
      const oldest = [...s.lambs.values()].filter((x) => !x.alert).sort((a, b) => a.lastEventAt - b.lastEventAt)[0];
      if (!oldest) return null;
      s.lambs.delete(oldest.agentId);
    }
    // Session-qualified: agent ids are unique within a session, not across sessions.
    // The type is cut again here for events that didn't come through normalise.
    const l = { ...newActor('lamb', lambId(s, ev.agentId), t), agentId: ev.agentId, agentType: truncate(ev.agentType, AGENT_TYPE_CHARS), alive: true };
    s.lambs.set(ev.agentId, l);
    return l;
  }

  /** An actor of this sheep (itself or one of its lambs): events never reach across sessions. */
  #actorIn(s, id) {
    if (s.id === id) return s;
    for (const l of s.lambs.values()) if (l.id === id) return l;
    return null;
  }

  /** Any actor by id (/control). Ids are unique: lamb ids carry their sheep's id. */
  #actorById(id) {
    for (const s of this.sheep.values()) {
      if (s.id === id) return s;
      for (const l of s.lambs.values()) if (l.id === id) return l;
    }
    return null;
  }

  #leave(s, t) {
    if (s.leavingSince == null) s.leavingSince = t;
    s.alert = null;
    for (const l of s.lambs.values()) {
      l.alert = null;
      if (l.leavingSince == null) l.leavingSince = t;
    }
  }

  #forget(key, s) {
    this.sheep.delete(key);
    if (s.sessionId && this.sessions.get(s.sessionId) === key) this.sessions.delete(s.sessionId);
  }

  #evaluate(t) {
    for (const [key, s] of this.sheep) {
      for (const [id, l] of s.lambs) {
        if (l.leavingSince != null && t - l.leavingSince >= this.cfg.minDwellMs) {
          s.lambs.delete(id);
          continue;
        }
        settle(l, this.#target(l, t), t, this.cfg.minDwellMs);
        if (l.effect && t - l.effect.at >= EFFECT_MS) l.effect = null;
      }
      if (s.leavingSince != null && t - s.leavingSince >= this.cfg.minDwellMs) {
        this.#forget(key, s);
        continue;
      }
      settle(s, this.#target(s, t), t, this.cfg.minDwellMs);
      if (s.effect && t - s.effect.at >= EFFECT_MS) s.effect = null;
    }
  }

  #target(a, t) {
    const c = this.cfg;
    if (a.leavingSince != null) return { state: 'leaving' };
    if (t - a.created < c.minDwellMs) return { state: 'arriving' };
    if (a.error) return { state: 'error', flavour: a.error };
    if (a.compacting) return { state: 'compacting' };
    if (a.inFlight.size > 0) return { state: 'busy', flavour: categoryOf([...a.inFlight.values()].at(-1).toolName) };
    const recentTool = t - a.lastToolAt < c.busyWindowMs ? categoryOf(a.lastToolName) : 'thinking';
    if (a.kind === 'lamb' && a.alive) return { state: 'busy', flavour: recentTool };
    if (a.turnActive && t - a.lastEventAt < c.thinkingTimeoutMs) return { state: 'busy', flavour: recentTool };
    if (a.waitingSince != null) return { state: t - a.waitingSince >= c.sleepAfterMs ? 'sleeping' : 'waiting' };
    if (a.stoppedAt != null && t - a.stoppedAt < c.grazeAfterMs) return { state: 'done' };
    const quiet = t - Math.max(a.lastActivityAt, a.stoppedAt ?? -Infinity);
    return { state: quiet >= c.sleepAfterMs ? 'sleeping' : 'idle' };
  }

  #bubble(toolName, summary) {
    if (!toolName) return truncate('Permission needed?', this.cfg.maxChars);
    const text = this.cfg.detail === 'full' && summary ? `${toolName}: ${summary}?` : `${toolName}?`;
    return truncate(text, this.cfg.maxChars);
  }

  #alertsInSession(s) {
    return [s, ...s.lambs.values()].filter((a) => a.alert);
  }

  #clear(actor, s, t) {
    if (actor.alert?.kind === 'permission') s.lastPermissionClearAt = t;
    actor.alert = null;
  }

  // Handlers, bound below. `this` is the Flock.
  static handlers = {
    UserPromptSubmit(s, ev, t) {
      this.#clear(s, s, t);
      // A new turn: whatever the last one left in flight (a denied or interrupted
      // tool never sends PostToolUse) is over.
      s.inFlight.clear();
      s.turnActive = true;
      s.stoppedAt = null;
      s.error = null;
      s.waitingSince = null;
      s.lastActivityAt = t;
    },

    PreToolUse(s, ev, t) {
      const actor = this.#actorFor(s, ev, t);
      if (!actor) return;
      // Progress means the prompt was answered: a different tool call from the same
      // actor, more than 2 s after the alert (a parallel batch's PreToolUse events can
      // arrive around the PermissionRequest, so not before).
      if (actor.alert && t - actor.alert.since > PROGRESS_GUARD_MS && ev.toolUseId !== actor.alert.toolUseId) this.#clear(actor, s, t);
      s.pending.push({ toolUseId: ev.toolUseId, fingerprint: ev.toolFingerprint, agentId: ev.agentId, actorId: actor.id, at: t, toolName: ev.toolName, toolSummary: ev.toolSummary });
      if (s.pending.length > PENDING_CAP) s.pending.shift();
      actor.inFlight.set(ev.toolUseId ?? `anon-${t}`, { toolName: ev.toolName, at: t });
      actor.lastToolName = ev.toolName;
      actor.lastToolAt = t;
      actor.lastActivityAt = t;
      actor.waitingSince = null;
      if (actor === s) {
        s.turnActive = true;
        s.stoppedAt = null;
      }
    },

    PermissionRequest(s, ev, t) {
      // Correlate with the PreToolUse it belongs to: same fingerprint, same agent if
      // the request names one, most recent first, and not already alerted.
      const alerted = new Set(this.#alertsInSession(s).map((a) => a.alert.toolUseId).filter(Boolean));
      const match = [...s.pending].reverse().find((p) => p.fingerprint && p.fingerprint === ev.toolFingerprint && (!ev.agentId || p.agentId === ev.agentId) && !alerted.has(p.toolUseId));
      let actor = match ? (this.#actorIn(s, match.actorId) ?? null) : null;
      if (!actor) actor = this.#actorFor(s, ev, t) ?? s;
      actor.alert = {
        kind: 'permission',
        text: this.#bubble(ev.toolName ?? match?.toolName, ev.toolSummary ?? match?.toolSummary),
        since: t,
        toolUseId: match?.toolUseId,
        fingerprint: ev.toolFingerprint,
        confirmed: false,
      };
      actor.waitingSince = null;
    },

    Notification(s, ev, t) {
      switch (ev.notificationType) {
        case 'permission_prompt': {
          const open = this.#alertsInSession(s).filter((a) => a.alert.kind === 'permission');
          const unconfirmed = open.filter((a) => !a.alert.confirmed).sort((a, b) => a.alert.since - b.alert.since);
          if (unconfirmed.length) unconfirmed[0].alert.confirmed = true;
          else if (!open.length && t - s.lastPermissionClearAt > NOTIFY_AFTER_CLEAR_GUARD_MS) {
            // No PermissionRequest came first (sandbox network prompts): the
            // notification is the only signal, so it raises the alert itself.
            s.alert = { kind: 'permission', text: truncate(ev.message ?? 'Permission needed?', this.cfg.maxChars), since: t, confirmed: true };
          }
          return;
        }
        case 'idle_prompt':
          s.turnActive = false;
          s.waitingSince = t;
          return;
        case 'elicitation_dialog':
        case 'elicitation_url_dialog': {
          const actor = this.#actorFor(s, ev, t) ?? s;
          if (actor.alert?.kind === 'elicitation') actor.alert.confirmed = true;
          else actor.alert = { kind: 'elicitation', text: truncate(ev.message ?? 'Input requested', this.cfg.maxChars), since: t, confirmed: true };
          return;
        }
        case 'elicitation_complete':
        case 'elicitation_response':
          for (const a of this.#alertsInSession(s)) if (a.alert.kind === 'elicitation') this.#clear(a, s, t);
          return;
        case 'agent_needs_input':
          s.alert = { kind: 'input', text: truncate(ev.message ?? 'Needs your input', this.cfg.maxChars), since: t, confirmed: false };
          return;
        default:
      }
    },

    Elicitation(s, ev, t) {
      const actor = this.#actorFor(s, ev, t) ?? s;
      const text = [ev.mcpServer, ev.message].filter(Boolean).join(': ') || 'Input requested';
      actor.alert = { kind: 'elicitation', text: truncate(text, this.cfg.maxChars), since: t, confirmed: false };
    },

    ElicitationResult(s, ev, t) {
      for (const a of this.#alertsInSession(s)) if (a.alert.kind === 'elicitation') this.#clear(a, s, t);
    },

    PostToolUse(s, ev, t) {
      this.#toolDone(s, ev, t);
    },

    PostToolUseFailure(s, ev, t) {
      const actor = this.#toolDone(s, ev, t);
      if (actor) actor.effect = { name: 'trip', at: t };
    },

    PermissionDenied(s, ev, t) {
      const actor = this.#toolDone(s, ev, t);
      if (actor) actor.waitingSince = t;
    },

    Stop(s, ev, t) {
      if (ev.agentId) return;
      this.#clear(s, s, t);
      s.turnActive = false;
      s.waitingSince = null;
      s.stoppedAt = t;
      s.inFlight.clear();
      s.lastActivityAt = t;
    },

    StopFailure(s, ev, t) {
      if (ev.agentId) return;
      this.#clear(s, s, t);
      s.turnActive = false;
      s.stoppedAt = t;
      s.inFlight.clear();
      s.error = ev.errorType ?? 'error';
    },

    SubagentStart(s, ev, t) {
      const lamb = this.#actorFor(s, ev, t);
      if (lamb && lamb !== s) lamb.lastActivityAt = t;
    },

    SubagentStop(s, ev, t) {
      // Only a lamb we already know can stop. An unknown agent (in particular the
      // internal helper with agent_type "") is ignored, never created.
      const lamb = this.#actorFor(s, ev, t, { create: false });
      if (!lamb || lamb === s) return;
      this.#clear(lamb, s, t);
      lamb.alive = false;
      lamb.inFlight.clear();
      lamb.leavingSince = t;
    },

    PreCompact(s) {
      s.compacting = true;
    },

    PostCompact(s, ev, t) {
      s.compacting = false;
      s.lastActivityAt = t;
    },

    SessionEnd(s, ev, t) {
      // Every alert in the session goes, lambs included (Decision 14), and the lambs
      // leave with the session.
      for (const a of this.#alertsInSession(s)) this.#clear(a, s, t);
      for (const l of s.lambs.values()) if (l.leavingSince == null) l.leavingSince = t;
      this.sessions.delete(s.sessionId);
      if ((ev.reason === 'clear' || ev.reason === 'resume') && s.terminalBased) {
        // Same tab or pane, new session next (Decision 8): the sheep stays.
        resetTurn(s);
        s.stoppedAt = t;
        s.sessionId = null;
      } else {
        this.#leave(s, t);
      }
    },
  };

  #toolDone(s, ev, t) {
    // Clear by tool_use_id wherever the alert sits (the PreToolUse decided the actor).
    if (ev.toolUseId) for (const a of this.#alertsInSession(s)) if (a.alert.toolUseId === ev.toolUseId) this.#clear(a, s, t);
    s.pending = s.pending.filter((p) => p.toolUseId !== ev.toolUseId);
    const actor = this.#actorFor(s, ev, t, { create: false });
    if (!actor) return null;
    actor.inFlight.delete(ev.toolUseId);
    actor.lastActivityAt = t;
    return actor;
  }
}

const HANDLERS = Flock.handlers;

function newActor(kind, id, t) {
  return {
    kind,
    id,
    created: t,
    lastEventAt: t,
    lastActivityAt: t,
    lastToolAt: -Infinity,
    lastToolName: undefined,
    turnActive: false,
    inFlight: new Map(),
    stoppedAt: null,
    waitingSince: null,
    error: null,
    leavingSince: null,
    alert: null,
    effect: null,
    shown: { state: 'arriving', flavour: undefined, since: t },
  };
}

function resetTurn(s) {
  s.turnActive = false;
  s.inFlight.clear();
  s.stoppedAt = null;
  s.waitingSince = null;
  s.error = null;
  s.compacting = false;
}

/** Hysteresis: a calm state changes only after minDwellMs, except into `leaving`. */
function settle(a, target, t, minDwellMs) {
  if (a.shown.state === target.state && a.shown.flavour === target.flavour) return;
  if (target.state !== 'leaving' && t - a.shown.since < minDwellMs) return;
  a.shown = { state: target.state, flavour: target.flavour, since: t };
}

function lowestFree(used, limit) {
  for (let i = 0; i < limit; i++) if (!used.has(i)) return i;
  return undefined;
}

function alertView(alert) {
  return alert ? { kind: alert.kind, text: alert.text, since: alert.since, confirmed: alert.confirmed } : undefined;
}

/** Drops undefined fields so the snapshot JSON stays small and stable. */
function clean(o) {
  for (const k of Object.keys(o)) if (o[k] === undefined) delete o[k];
  return o;
}

/** One persisted identity, checked field by field; null if it can't be used at all. */
function sanitiseIdentity(key, v, t, paletteSize) {
  if (typeof key !== 'string' || key === '' || key.length > LIMITS.identityKeyChars) return null;
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const lastSeen = typeof v.lastSeen === 'number' && Number.isFinite(v.lastSeen) && v.lastSeen <= t + 86_400_000 ? v.lastSeen : t;
  if (t - lastSeen >= IDENTITY_TTL_MS) return null;
  const colourIndex = Number.isInteger(v.colourIndex) && v.colourIndex >= 0 && v.colourIndex < paletteSize ? v.colourIndex : undefined;
  const slots = {};
  if (v.slots && typeof v.slots === 'object' && !Array.isArray(v.slots)) {
    for (const [m, slot] of Object.entries(v.slots)) {
      if (Object.keys(slots).length >= LIMITS.monitorsPerIdentity) break;
      if (m !== '' && m.length <= LIMITS.monitorNameChars && Number.isInteger(slot) && slot >= 0 && slot <= 10_000) setOwn(slots, m, slot);
    }
  }
  // Cleaned like bubble text (control, bidi and zero-width characters out), then capped.
  const label = typeof v.label === 'string' ? cleanLabel(v.label.slice(0, 4 * LIMITS.labelChars), LIMITS.labelChars) : undefined;
  return { colourIndex, slots, label, lastSeen };
}

/** A lamb's id: its sheep's id and its agent id, so two sessions' lambs never share one. */
function lambId(s, agentId) {
  return `l-${s.id.replace(/^s-/, '')}-${agentId}`;
}
