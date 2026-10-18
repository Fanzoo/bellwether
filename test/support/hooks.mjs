/**
 * Constants copied from the Node brain at v0.2.0-rc.3 (brain/hooks.mjs and
 * brain/defaults.mjs), for the tests that name them. Not the brain: no test checks them as
 * one; the Rust brain's own lists come through test/impl.mjs.
 */

/** The terminal headers a hook entry sends, and the environment variable each one names. */
export const ENV_HEADERS = {
  'X-Bellwether-Term-Program': 'TERM_PROGRAM',
  'X-Bellwether-Iterm-Session': 'ITERM_SESSION_ID',
  'X-Bellwether-Term-Session': 'TERM_SESSION_ID',
  'X-Bellwether-Wt-Session': 'WT_SESSION',
  'X-Bellwether-Tmux-Pane': 'TMUX_PANE',
  'X-Bellwether-Herdr-Pane': 'HERDR_PANE_ID',
  'X-Bellwether-Herdr-Socket': 'HERDR_SOCKET_PATH',
};

/** Marks a config whose palette withDefaults() upgraded from the old default (test/impl.mjs). */
export const PALETTE_UPGRADED = Symbol('palette upgraded');
