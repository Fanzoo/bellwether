import { createHash } from 'node:crypto';

/**
 * The sheep identity key: what keeps a sheep's colour and slot across /clear (which
 * changes session_id) and across brain restarts. The innermost multiplexer wins,
 * because every pane inside herdr or tmux inherits the host terminal's ids (measured
 * in Milestone 1 Q6: five herdr panes, one ITERM_SESSION_ID).
 *
 *   1. herdr pane      herdr:<socket>#<HERDR_PANE_ID>
 *   2. tmux pane       tmux:<TMUX_PANE>
 *   3. iTerm2 tab      iterm:<uuid part of ITERM_SESSION_ID>   (the wNtNpN prefix moves with the tab)
 *   4. Terminal.app    term:<TERM_SESSION_ID>, only when TERM_PROGRAM=Apple_Terminal
 *                      (iTerm2 sets TERM_SESSION_ID too, to its own id)
 *   5. Windows Terminal wt:<WT_SESSION>
 *   6. otherwise       session:<session_id>
 *
 * `terminalBased` tells the reducer whether a /clear keeps the sheep: with only the
 * session to go on, a new session_id is a new sheep.
 */
export function identityOf(ev) {
  const t = ev.terminal ?? {};
  if (t.herdrPane) return { key: `herdr:${t.herdrSocket ?? ''}#${t.herdrPane}`, terminalBased: true };
  if (t.tmuxPane) return { key: `tmux:${t.tmuxPane}`, terminalBased: true };
  if (t.itermSession) return { key: `iterm:${t.itermSession.replace(/^w\d+t\d+p\d+:/, '')}`, terminalBased: true };
  if (t.termSession && t.program === 'Apple_Terminal') return { key: `term:${t.termSession}`, terminalBased: true };
  if (t.wtSession) return { key: `wt:${t.wtSession}`, terminalBased: true };
  return { key: `session:${ev.sessionId ?? 'unknown'}`, terminalBased: false };
}

/** A short, stable, opaque actor id for the snapshot. */
export function sheepId(identityKey) {
  return 's-' + createHash('sha256').update(identityKey).digest('hex').slice(0, 10);
}
