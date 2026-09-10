//! Line diffs, for showing what an edit actually did.
//!
//! Small on purpose: an LCS over the changed region, capped so a huge file
//! falls back to trimming the common head and tail rather than burning time
//! and memory on a phone. Output follows the format pi uses, a sign and a
//! line number in front of each line, which the transcript colourises.

/// One rendered diff line.
pub struct DLine {
    /// ' ' unchanged, '-' removed, '+' added, '…' a gap.
    pub kind: char,
    pub line_no: usize,
    pub text: String,
}

impl DLine {
    /// The wire format: `+12 text`, `-4 text`, ` 7 text`.
    pub fn to_text(&self) -> String {
        match self.kind {
            '…' => "…".to_string(),
            k => format!("{k}{} {}", self.line_no, self.text),
        }
    }
}

/// How many lines of diff are worth keeping in a transcript.
pub const MAX_LINES: usize = 120;

/// Line-level diff with `context` unchanged lines around each change.
/// `body` is the number of changed lines shown before it gives up and
/// reports a gap instead.
pub fn unified(old: &str, new: &str, context: usize) -> Vec<DLine> {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();

    let ops = if a.len().max(b.len()) <= 600 {
        lcs_ops(&a, &b)
    } else {
        head_tail_ops(&a, &b)
    };

    // Keep only changes and their surroundings.
    let mut keep = vec![false; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        if op.0 != ' ' {
            let from = i.saturating_sub(context);
            let to = (i + context + 1).min(ops.len());
            for slot in keep.iter_mut().take(to).skip(from) {
                *slot = true;
            }
        }
    }

    let mut out: Vec<DLine> = Vec::new();
    let mut gap = false;
    for (i, (kind, ai, bi)) in ops.iter().enumerate() {
        if !keep[i] {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(DLine { kind: '…', line_no: 0, text: String::new() });
        }
        gap = false;
        let (line_no, text) = match kind {
            '-' => (*ai + 1, a[*ai]),
            '+' => (*bi + 1, b[*bi]),
            _ => (*ai + 1, a[*ai]),
        };
        out.push(DLine { kind: *kind, line_no, text: text.to_string() });
    }
    if out.len() > MAX_LINES {
        let dropped = out.len() - MAX_LINES;
        out.truncate(MAX_LINES);
        out.push(DLine { kind: '…', line_no: 0, text: format!("{dropped} more diff lines") });
    }
    out
}

/// Shortest common subsequence over lines, as (kind, a_index, b_index).
fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<(char, usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if a[i] == b[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((' ', i, j));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            ops.push(('-', i, j));
            i += 1;
        } else {
            ops.push(('+', i, j));
            j += 1;
        }
    }
    while i < n {
        ops.push(('-', i, j));
        i += 1;
    }
    while j < m {
        ops.push(('+', i, j));
        j += 1;
    }
    ops
}

/// Cheaper shape for very large files: common head, common tail, everything
/// between is a change.
fn head_tail_ops(a: &[&str], b: &[&str]) -> Vec<(char, usize, usize)> {
    let mut head = 0;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < a.len() - head && tail < b.len() - head && a[a.len() - 1 - tail] == b[b.len() - 1 - tail]
    {
        tail += 1;
    }
    let mut ops = Vec::new();
    for i in 0..head {
        ops.push((' ', i, i));
    }
    for i in head..a.len() - tail {
        ops.push(('-', i, head));
    }
    for j in head..b.len() - tail {
        ops.push(('+', a.len() - tail, j));
    }
    for k in 0..tail {
        ops.push((' ', head + k, b.len() - tail + k));
    }
    ops
}

/// `(+3 -1)` style counts, for the one-line summary above a diff.
pub fn counts(lines: &[DLine]) -> (usize, usize) {
    let added = lines.iter().filter(|l| l.kind == '+').count();
    let removed = lines.iter().filter(|l| l.kind == '-').count();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[DLine]) -> Vec<String> {
        lines.iter().map(|l| l.to_text()).collect()
    }

    #[test]
    fn a_single_changed_line_keeps_its_number() {
        let out = text(&unified("a\nb\nc\n", "a\nB\nc\n", 1));
        assert_eq!(out, vec![" 1 a", "-2 b", "+2 B", " 3 c"]);
    }

    #[test]
    fn added_lines_report_the_new_number() {
        let out = text(&unified("one\ntwo\n", "one\nmiddle\ntwo\n", 1));
        assert!(out.contains(&"+2 middle".to_string()), "{out:?}");
    }

    #[test]
    fn distant_changes_are_separated_by_a_gap() {
        let old = (1..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let new = old.replace("line 2\n", "line two\n").replace("line 35", "line thirty five");
        let out = unified(&old, &new, 1);
        assert!(out.iter().any(|l| l.kind == '…'), "there should be a gap between the hunks");
        assert!(out.iter().any(|l| l.kind == '+' && l.text == "line two"));
    }

    #[test]
    fn identical_text_has_no_changes() {
        let lines = unified("same\n", "same\n", 2);
        assert!(lines.iter().all(|l| l.kind == ' '), "{:?}", text(&lines));
        assert_eq!(counts(&lines), (0, 0));
    }

    #[test]
    fn a_whole_new_file_is_all_additions() {
        let lines = unified("", "a\nb\n", 1);
        assert_eq!(counts(&lines), (2, 0));
    }

    #[test]
    fn huge_files_skip_the_lcs() {
        let old: Vec<String> = (0..900).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new[500] = "changed".into();
        let lines = unified(&old.join("\n"), &new.join("\n"), 1);
        assert!(lines.iter().any(|l| l.kind == '+' && l.text == "changed"));
        assert!(lines.len() < 20, "only the change is reported: {}", lines.len());
    }

    #[test]
    fn long_diffs_are_capped() {
        let old = (0..400).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        let new = (0..400).map(|i| format!("x{i}")).collect::<Vec<_>>().join("\n");
        let lines = unified(&old, &new, 1);
        assert!(lines.len() <= MAX_LINES + 1);
        assert!(lines.last().unwrap().text.contains("more diff lines"));
    }
}
