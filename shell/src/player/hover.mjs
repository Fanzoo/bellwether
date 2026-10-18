/**
 * The hover label: which sprite, if any, the cursor is over (deviation 136).
 *
 * It is worked out afresh on every frame, never kept from the last pointer event. The
 * strip window takes the mouse only while the shell's cursor poll finds the cursor on a
 * sprite's box (clickthrough.rs); the moment it doesn't, the window ignores the mouse and
 * the webview hears nothing more: no pointermove, no pointerleave. A hover kept from the
 * last event then stayed on for good, wherever the cursor had gone. So a sprite counts
 * as hovered only while both say so:
 *
 *   - the last pointer position the webview saw is on that sprite where it is now (a
 *     sheep that walks out from under a still cursor is no longer hovered), and
 *   - the shell's poll (`over`, the ids of this strip's boxes under the cursor) has it,
 *     which is the only word on the cursor once the window stops hearing the mouse.
 *
 * pointer   {x, y} in the strip's CSS pixels, or null when unknown
 * over      Set of sprite ids under the cursor, by the shell's poll (null: none)
 * hitAt     (x, y) => the id of the live sprite at that point, or null
 */
export function hoverTarget({ pointer, over, hitAt }) {
  if (!pointer || !Number.isFinite(pointer.x) || !Number.isFinite(pointer.y)) return null;
  const id = hitAt(pointer.x, pointer.y);
  if (id == null || !over || !over.has(id)) return null;
  return id;
}
