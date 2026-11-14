//! A naive `String` model of change sets, written independently of the
//! canonical-form machinery so property tests can compare against it.

use super::Op;

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
