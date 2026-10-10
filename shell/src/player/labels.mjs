/**
 * Labels that tell same-project sheep apart (Milestone 10). Two sessions in one project
 * both say "my-project"; when two or more sheep on the strip share a label, each
 * gets a short distinguisher from its terminal: the herdr pane ("p4"), else the iTerm
 * tab ("tab 2"), else the tmux pane ("%3"), else its place in the strip ("#2"). A sheep
 * alone in its project keeps the plain label.
 *
 * The distinguisher is cut to letters, digits, `#` and `%`, and the whole label is
 * capped at MAX_LABEL characters, like other labels.
 */
export const MAX_LABEL = 40;

/** A short tag for one terminal, or null if its headers give nothing usable. */
export function terminalTag(t = {}) {
  const clean = (s) => String(s).replace(/[^A-Za-z0-9#%]/g, '').slice(0, 8);
  if (typeof t.herdrPane === 'string') {
    const pane = clean(t.herdrPane.split(':').pop());
    if (pane) return pane;
  }
  if (typeof t.itermSession === 'string') {
    const m = /^w\d+t(\d+)p\d+:/.exec(t.itermSession);
    if (m) return `tab ${Number(m[1]) + 1}`;
  }
  if (typeof t.tmuxPane === 'string' && clean(t.tmuxPane)) return clean(t.tmuxPane);
  return null;
}

function cap(s, max = MAX_LABEL) {
  const chars = [...s];
  return chars.length <= max ? s : chars.slice(0, max - 1).join('') + '…';
}

/**
 * @param {Array<{id, label, terminal?, slot?}>} sheep  the sheep on this strip
 * @returns {Map<id, string>} the label to show for each
 */
export function displayLabels(sheep) {
  const out = new Map();
  const groups = new Map();
  // Grouped on what is shown: two long names that look the same once capped are the
  // same label to the eye, so they get distinguishers too (verification D3).
  for (const s of sheep) {
    const shown = cap(typeof s.label === 'string' ? s.label : '');
    if (!groups.has(shown)) groups.set(shown, []);
    groups.get(shown).push(s);
  }
  for (const [shown, group] of groups) {
    if (group.length === 1) {
      out.set(group[0].id, shown);
      continue;
    }
    const ordered = [...group].sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0) || String(a.id).localeCompare(String(b.id)));
    const tags = ordered.map((s) => terminalTag(s.terminal));
    // Tags must tell them apart; if any is missing or repeated, number them instead.
    const usable = tags.every((t) => t) && new Set(tags).size === tags.length;
    ordered.forEach((s, i) => {
      const tag = ` · ${usable ? tags[i] : `#${i + 1}`}`;
      // The name gives way to the tag, never the other way round.
      const label = typeof s.label === 'string' ? s.label : '';
      out.set(s.id, cap(label, MAX_LABEL - [...tag].length) + tag);
    });
  }
  return out;
}
