//! A small line-based unified diff (LCS), ported line for line from brain/diff.mjs: the
//! `hooks install` preview is a contract, so the hunks, their headers and the choice
//! between equal-length alignments must be Node's exactly. Settings files are a few
//! hundred lines; the quadratic table is fine at that size.

/// One line of the edit script, with the 0-based positions in a and b where it applies.
struct Op<'a> {
    op: char,
    line: &'a str,
    ai: usize,
    bi: usize,
}

/// `unifiedDiff(oldText, newText, { oldName, newName, context })`: "" when nothing differs.
pub fn unified_diff(old: &str, new: &str, old_name: &str, new_name: &str, context: usize) -> String {
    let a = split_lines(old);
    let b = split_lines(new);
    let ops = diff_ops(&a, &b);
    if ops.iter().all(|o| o.op == ' ') {
        return String::new();
    }
    let mut out = vec![format!("--- {old_name}"), format!("+++ {new_name}")];
    let mut i = 0;
    while i < ops.len() {
        if ops[i].op == ' ' {
            i += 1;
            continue;
        }
        // Grow a hunk around this change, merging changes closer than 2 * context.
        let start = i.saturating_sub(context);
        let mut end = i;
        while end < ops.len() {
            if ops[end].op != ' ' {
                end += 1;
                continue;
            }
            let mut run = 0;
            while end + run < ops.len() && ops[end + run].op == ' ' {
                run += 1;
            }
            if end + run >= ops.len() || run > 2 * context {
                end = ops.len().min(end + context);
                break;
            }
            end += run;
        }
        let hunk = &ops[start..end];
        let first = &hunk[0];
        let old_count = hunk.iter().filter(|o| o.op != '+').count();
        let new_count = hunk.iter().filter(|o| o.op != '-').count();
        out.push(format!("@@ -{},{old_count} +{},{new_count} @@", first.ai + usize::from(old_count > 0), first.bi + usize::from(new_count > 0)));
        for o in hunk {
            out.push(format!("{}{}", o.op, o.line));
        }
        i = end;
    }
    out.join("\n") + "\n"
}

/// `diffStats(oldText, newText)`: lines added and removed.
pub struct Stats {
    pub added: usize,
    pub removed: usize,
    pub removed_lines: Vec<String>,
}

pub fn diff_stats(old: &str, new: &str) -> Stats {
    let a = split_lines(old);
    let b = split_lines(new);
    let ops = diff_ops(&a, &b);
    Stats {
        added: ops.iter().filter(|o| o.op == '+').count(),
        removed: ops.iter().filter(|o| o.op == '-').count(),
        removed_lines: ops.iter().filter(|o| o.op == '-').map(|o| o.line.to_string()).collect(),
    }
}

/// Splits on LF or CRLF (`text.split(/\r?\n/)`, a trailing empty line dropped), so a CRLF
/// file diffs line for line against its CRLF rewrite.
fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    // `\r` goes only with the `\n` after it: a lone CR at the very end stays in the line,
    // as `split(/\r?\n/)` keeps it (BR-3K's M4 finding D5b).
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.len() - 1;
    let mut lines: Vec<&str> = pieces.iter().enumerate().map(|(i, l)| if i < last { l.strip_suffix('\r').unwrap_or(l) } else { l }).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] = if a[i] == b[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push(Op { op: ' ', line: a[i], ai: i, bi: j });
            i += 1;
            j += 1;
        } else if i < n && (j >= m || lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
            ops.push(Op { op: '-', line: a[i], ai: i, bi: j });
            i += 1;
        } else {
            ops.push(Op { op: '+', line: b[j], ai: i, bi: j });
            j += 1;
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_diff() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n", "x", "y", 3), "");
        assert_eq!(unified_diff("a\nb\nc\n", "a\nB\nc\n", "x", "y", 3), "--- x\n+++ y\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
        assert_eq!(unified_diff("", "a\n", "x", "y", 3), "--- x\n+++ y\n@@ -0,0 +1,1 @@\n+a\n");
        assert_eq!(unified_diff("a\r", "a", "x", "y", 3), "--- x\n+++ y\n@@ -1,1 +1,1 @@\n-a\r\n+a\n");
        assert_eq!(split_lines("a\rb\r\n\r"), ["a\rb", "\r"]);
        let s = diff_stats("a\r\nb\r\n", "a\nc\n");
        assert_eq!((s.added, s.removed, s.removed_lines), (1, 1, vec!["b".to_string()]));
    }
}
