/**
 * Press, click or drag (D10, milestone 12): a pure state machine for one pointer gesture
 * on a sprite. No DOM: main.js feeds it the canvas's pointer events and carries out the
 * actions it returns, and `--demo-drag` feeds it a scripted gesture the same way.
 *
 * - A primary-button press on a sprite is a press: nothing visible yet (no hop). The shell
 *   is told (`press`), so the strip window keeps the mouse for the whole gesture.
 * - Moving `DRAG_THRESHOLD_PX` or more from the press point while pressed lifts the sheep
 *   (`lift`, with the grab offset: cursor x − sprite left).
 * - Released before that, however long the press, it is a click: the hop and the focus,
 *   as before, only now on release (a press can't know yet whether it will be a drag).
 * - Released after the lift, it is a drop. A cancel (pointercancel, Quiet, Hide, the
 *   session ending, the shell's fail-safe) ends it with nothing recorded.
 * - A right or middle press does what any press did before: it clicks at once. Only the
 *   button that started the gesture ends it: another button going down or up meanwhile
 *   (a right click while the left is held) is ignored.
 * - A lamb is never dragged: within the threshold its release is a click (its parent's
 *   terminal comes forward); past it, nothing happens and the log says so.
 *
 * Actions: {do: 'press'|'lift'|'carry'|'click'|'release'|'drop'|'cancel'|'log', …}.
 * `release` ends the shell's hold after a click (or an ignored lamb drag); `drop` is the
 * release of a lifted sheep; `cancel` says whether the sheep had been lifted.
 */

/** How far (logical px) the pointer must move from the press point to start a drag. */
export const DRAG_THRESHOLD_PX = 4;

/**
 * Whether the button that started the gesture has come up. With `buttons` (the bitmask of
 * buttons still held, as pointer events give it), by its bit: under pointer-event chording a
 * second button's release can arrive as a pointerup, or the first's as a pointermove. Without
 * it, by `button`, the one that changed.
 */
function released(s, ev) {
  if (Number.isInteger(ev.buttons)) return (ev.buttons & 1) === 0; // the primary's bit
  return (ev.button ?? 0) === (s.button ?? 0);
}

export function idleDrag() {
  return { phase: 'idle' };
}

/** One pointer event: returns the next state and what to do. Never throws. */
export function dragEvent(state, ev) {
  const s = state ?? idleDrag();
  const none = { state: s, actions: [] };
  switch (ev?.type) {
    case 'down': {
      if (s.phase !== 'idle' || ev.id == null) return none;
      // Other buttons keep their old meaning: a click, at once.
      if ((ev.button ?? 0) !== 0) return { state: s, actions: [{ do: 'click', id: ev.id }] };
      return {
        state: { phase: 'pressed', id: ev.id, lamb: !!ev.lamb, x0: ev.x, y0: ev.y, grabDx: ev.x - (ev.left ?? ev.x), button: 0 },
        actions: [{ do: 'press', id: ev.id }],
      };
    }
    case 'move': {
      // The pressing button came up without a pointerup (chorded with another): that ends it.
      if (s.phase !== 'idle' && Number.isInteger(ev.buttons) && released(s, ev)) return dragEvent(s, { ...ev, type: 'up' });
      if (s.phase === 'pressed') {
        if (Math.hypot(ev.x - s.x0, ev.y - s.y0) < DRAG_THRESHOLD_PX) return none;
        if (s.lamb) return { state: { phase: 'ignored', id: s.id, button: s.button }, actions: [{ do: 'log', line: 'lamb drag ignored' }] };
        return { state: { phase: 'carrying', id: s.id, grabDx: s.grabDx, button: s.button }, actions: [{ do: 'lift', id: s.id, grabDx: s.grabDx }, { do: 'carry', id: s.id, x: ev.x, y: ev.y }] };
      }
      if (s.phase === 'carrying') return { state: s, actions: [{ do: 'carry', id: s.id, x: ev.x, y: ev.y }] };
      return none;
    }
    case 'up': {
      // Another button coming up mid-gesture doesn't end it.
      if (s.phase !== 'idle' && !released(s, ev)) return none;
      if (s.phase === 'pressed') return { state: idleDrag(), actions: [{ do: 'click', id: s.id }, { do: 'release', id: s.id }] };
      if (s.phase === 'carrying') return { state: idleDrag(), actions: [{ do: 'drop', id: s.id, x: ev.x, y: ev.y }] };
      if (s.phase === 'ignored') return { state: idleDrag(), actions: [{ do: 'release', id: s.id }] };
      return none;
    }
    case 'cancel': {
      if (s.phase === 'idle') return none;
      return { state: idleDrag(), actions: [{ do: 'cancel', id: s.id, reason: String(ev.reason ?? 'cancelled'), lifted: s.phase === 'carrying' }] };
    }
    default:
      return none;
  }
}
