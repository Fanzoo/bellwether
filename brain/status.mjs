/**
 * `pasture status`: the flock as text. Also the terminal fallback when there's no
 * overlay (Wayland, or before the shell exists).
 *
 *   meadow          #7fb3ff  busy·running    ALERT  Bash: git push origin main?
 *     └ Explore     lamb     busy·reading
 */
export function renderStatus(snap, { palette = [], now = Date.now(), color = false } = {}) {
  const lines = [];
  const head = `pasture  rev ${snap.rev}  ${snap.flock.length} sheep${snap.muted ? '  MUTED' : ''}`;
  lines.push(head);
  if (!snap.flock.length) lines.push('  (no sessions yet — start or prompt a Claude Code session)');
  for (const s of snap.flock) {
    const colour = palette[s.colourIndex] ?? `#${s.colourIndex}`;
    lines.push(row(s.label, colour, s, now, color));
    for (const l of s.lambs) lines.push(row(`  └ ${l.label}`, 'lamb', l, now, color));
    if (s.hiddenLambCount) lines.push(`  └ +${s.hiddenLambCount} more lamb${s.hiddenLambCount === 1 ? '' : 's'}`);
  }
  return lines.join('\n') + '\n';
}

function row(label, colour, a, now, color) {
  const state = a.flavour ? `${a.state}·${a.flavour}` : a.state;
  let alert = '';
  if (a.alert) {
    alert = `ALERT  ${a.alert.text}${a.alert.confirmed ? '  (!)' : ''}`;
    if (color) alert = `\x1b[1;33m${alert}\x1b[0m`;
  } else if (a.lambAlert) {
    alert = 'a lamb needs you';
  }
  const effect = a.effect ? `  *${a.effect.name}*` : '';
  return `${fit(label, 16)} ${fit(colour, 8)} ${fit(state, 16)} ${alert}${effect}`.trimEnd();
}

function fit(s, n) {
  const chars = [...String(s)];
  return chars.length > n ? chars.slice(0, n - 1).join('') + '…' : chars.join('').padEnd(n);
}
