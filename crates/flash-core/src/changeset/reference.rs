//! A naive `String` model of change sets, written independently of the
//! canonical-form machinery so property tests can compare against it.

use super::{Assoc, Op};

/// Applies `ops` to `s` by walking them forward. Accepts any op sequence,
/// canonical or not. Panics if the ops do not fit `s` (test-only code).
pub(crate) fn apply_ops(ops: &[Op], s: &str) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for op in ops {
        match op {
            Op::Retain(n) => {
                out.push_str(&s[pos..pos + n]);
                pos += n;
            }
            Op::Delete(n) => pos += n,
            Op::Insert(text) => out.push_str(text),
        }
    }
    assert_eq!(pos, s.len(), "ops do not cover the text");
    out
}

/// Maps old position `pos` by the policy in the module docs, scanning from
/// the start for every call. An edit group is a maximal run of non-`Retain`
/// ops (in any order); it owns the closed old range it deletes. With a
/// deletion, its two old edges map to the replacement's two edges.
pub(crate) fn map_pos_naive(ops: &[Op], pos: usize, assoc: Assoc) -> usize {
    let (mut old, mut new) = (0, 0);
    let mut i = 0;
    while i < ops.len() {
        if let Op::Retain(k) = ops[i] {
            if pos < old + k {
                return new + (pos - old);
            }
            old += k;
            new += k;
            i += 1;
            continue;
        }
        let (mut deleted, mut inserted) = (0, 0);
        while let Some(op) = ops.get(i).filter(|op| !matches!(op, Op::Retain(_))) {
            match op {
                Op::Delete(d) => deleted += d,
                Op::Insert(s) => inserted += s.len(),
                Op::Retain(_) => unreachable!(),
            }
            i += 1;
        }
        if pos <= old + deleted {
            let at_left_edge = deleted > 0 && pos == old;
            let at_right_edge = deleted > 0 && pos == old + deleted;
            return if at_left_edge {
                new
            } else if at_right_edge || assoc == Assoc::After {
                new + inserted
            } else {
                new
            };
        }
        old += deleted;
        new += inserted;
    }
    new
}
