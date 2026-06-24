//! Minimal unified line diff for the external-change "show diff" action.
//!
//! Dependency-free LCS over lines. Produces a unified-ish listing (` `
//! context, `-` editor-only, `+` disk-only) so the conflict banner's "show
//! diff" can open a real comparison instead of a line-count summary.
//!
//! Bounded: above [`MAX_DIFF_LINES`] on either side it falls back to a
//! summary, because a full LCS table is O(n·m) and a notes editor's
//! conflict diff does not need to scale to huge generated files.

/// Per-side line cap. The LCS table is `(n+1)·(m+1)` `u32`s; 2000² ≈ 16 MB,
/// an acceptable one-shot allocation for an explicit user action.
const MAX_DIFF_LINES: usize = 2_000;

/// Build a unified line diff between `editor` (the in-memory buffer) and
/// `disk` (the current file bytes). `name` labels the header.
pub(crate) fn unified_line_diff(name: &str, editor: &str, disk: &str) -> String {
    let a: Vec<&str> = editor.lines().collect();
    let b: Vec<&str> = disk.lines().collect();
    let mut out =
        format!("Diff for {name}\n  (- = your editor buffer, + = current file on disk)\n\n");
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        out.push_str(&format!(
            "File too large for an inline diff: editor {} lines, disk {} lines.\n",
            a.len(),
            b.len()
        ));
        return out;
    }
    let (mut added, mut removed) = (0usize, 0usize);
    for op in diff_ops(&a, &b) {
        match op {
            DiffOp::Equal(line) => {
                out.push_str("  ");
                out.push_str(line);
            }
            DiffOp::Removed(line) => {
                removed += 1;
                out.push_str("- ");
                out.push_str(line);
            }
            DiffOp::Added(line) => {
                added += 1;
                out.push_str("+ ");
                out.push_str(line);
            }
        }
        out.push('\n');
    }
    if added == 0 && removed == 0 {
        out.push_str("\n(no line differences — only whitespace / line-ending changes)\n");
    } else {
        out.push_str(&format!(
            "\n{removed} line(s) only in editor, {added} line(s) only on disk.\n"
        ));
    }
    out
}

enum DiffOp<'a> {
    Equal(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// LCS-backtracked op list. `Removed` = present only in `a` (editor),
/// `Added` = present only in `b` (disk).
fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<DiffOp<'a>> {
    let n = a.len();
    let m = b.len();
    let stride = m + 1;
    // Flat suffix-LCS table: lcs[i*stride + j] = LCS length of a[i..], b[j..].
    let mut lcs = vec![0u32; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * stride + j] = if a[i] == b[j] {
                lcs[(i + 1) * stride + (j + 1)] + 1
            } else {
                lcs[(i + 1) * stride + j].max(lcs[i * stride + (j + 1)])
            };
        }
    }
    let mut ops = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(DiffOp::Equal(a[i]));
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * stride + j] >= lcs[i * stride + (j + 1)] {
            ops.push(DiffOp::Removed(a[i]));
            i += 1;
        } else {
            ops.push(DiffOp::Added(b[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push(DiffOp::Removed(a[i]));
        i += 1;
    }
    while j < m {
        ops.push(DiffOp::Added(b[j]));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_reports_no_line_differences() {
        let text = "one\ntwo\nthree\n";
        let diff = unified_line_diff("note.md", text, text);
        assert!(diff.contains("no line differences"));
        assert!(!diff.contains("\n- "));
        assert!(!diff.contains("\n+ "));
    }

    #[test]
    fn added_disk_line_is_marked_plus() {
        let editor = "one\ntwo\n";
        let disk = "one\ntwo\nthree\n";
        let diff = unified_line_diff("note.md", editor, disk);
        assert!(diff.contains("+ three"));
        assert!(diff.contains("1 line(s) only on disk"));
        assert!(diff.contains("0 line(s) only in editor"));
    }

    #[test]
    fn removed_editor_line_is_marked_minus() {
        let editor = "one\ntwo\nthree\n";
        let disk = "one\nthree\n";
        let diff = unified_line_diff("note.md", editor, disk);
        assert!(diff.contains("- two"));
        assert!(diff.contains("1 line(s) only in editor"));
    }

    #[test]
    fn mixed_changes_count_both_sides() {
        let editor = "alpha\nbeta\ngamma\n";
        let disk = "alpha\nbeta-changed\ngamma\ndelta\n";
        let diff = unified_line_diff("note.md", editor, disk);
        // `beta` removed, `beta-changed` + `delta` added.
        assert!(diff.contains("- beta"));
        assert!(diff.contains("+ beta-changed"));
        assert!(diff.contains("+ delta"));
        assert!(diff.contains("  alpha"));
        assert!(diff.contains("  gamma"));
        assert!(diff.contains("1 line(s) only in editor, 2 line(s) only on disk"));
    }
}
