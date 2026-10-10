/**
 * A small line-based unified diff (LCS), so the installer can show exactly what it
 * would change without shelling out to `diff`, which Windows lacks. Settings files
 * are a few hundred lines; the quadratic table is fine at that size.
 */
export function unifiedDiff(oldText, newText, { oldName = 'a', newName = 'b', context = 3 } = {}) {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  const ops = diffOps(a, b);
  if (ops.every((o) => o.op === ' ')) return '';

  const out = [`--- ${oldName}`, `+++ ${newName}`];
  let i = 0;
  while (i < ops.length) {
    if (ops[i].op === ' ') {
      i++;
      continue;
    }
    // Grow a hunk around this change, merging changes closer than 2 * context.
    let start = Math.max(0, i - context);
    let end = i;
    while (end < ops.length) {
      if (ops[end].op !== ' ') {
        end++;
        continue;
      }
      let run = 0;
      while (end + run < ops.length && ops[end + run].op === ' ') run++;
      if (end + run >= ops.length || run > 2 * context) {
        end = Math.min(ops.length, end + context);
        break;
      }
      end += run;
    }
    const hunk = ops.slice(start, end);
    const first = hunk[0];
    const oldCount = hunk.filter((o) => o.op !== '+').length;
    const newCount = hunk.filter((o) => o.op !== '-').length;
    out.push(`@@ -${first.ai + (oldCount ? 1 : 0)},${oldCount} +${first.bi + (newCount ? 1 : 0)},${newCount} @@`);
    for (const o of hunk) out.push(o.op + o.line);
    i = end;
  }
  return out.join('\n') + '\n';
}

/** Counts of added and removed lines, for summaries and tests. */
export function diffStats(oldText, newText) {
  const ops = diffOps(splitLines(oldText), splitLines(newText));
  return {
    added: ops.filter((o) => o.op === '+').length,
    removed: ops.filter((o) => o.op === '-').length,
    removedLines: ops.filter((o) => o.op === '-').map((o) => o.line),
  };
}

/** Splits on LF or CRLF, so a CRLF file diffs line for line against its CRLF rewrite. */
function splitLines(text) {
  if (text === '') return [];
  const lines = text.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  return lines;
}

/** Each op carries the 0-based positions in a and b where it applies. */
function diffOps(a, b) {
  const n = a.length;
  const m = b.length;
  const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      ops.push({ op: ' ', line: a[i], ai: i, bi: j });
      i++;
      j++;
    } else if (i < n && (j >= m || lcs[i + 1][j] >= lcs[i][j + 1])) {
      ops.push({ op: '-', line: a[i], ai: i, bi: j });
      i++;
    } else {
      ops.push({ op: '+', line: b[j], ai: i, bi: j });
      j++;
    }
  }
  return ops;
}
