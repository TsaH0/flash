//! Validating a change set against a text, applying it, and inverting it.
//!
//! Every operation here validates the change set against the text first
//! ([`ChangeSet::validate`]) and touches nothing if that fails, so a failed
//! application leaves the text unchanged.

use super::{ChangeSet, ChangeSetBuilder, ChangeSetError, Op};
use crate::text::{Text, TextError};

impl ChangeSet {
    /// Checks that this change set can be applied to `text`: the lengths
    /// match and every op boundary is a char boundary of `text`.
    ///
    /// Returns the first problem found. O(ops · log n).
    pub fn validate(&self, text: &Text) -> Result<(), ChangeSetError> {
        let len = text.byte_len();
        if len != self.len_before {
            return Err(ChangeSetError::LengthMismatch {
                expected: self.len_before,
                actual: len,
            });
        }
        let mut pos = 0;
        for op in &self.ops {
            if let Op::Retain(n) | Op::Delete(n) = op {
                // Cannot overflow: the lengths sum to len_before.
                pos += n;
                if !text.is_char_boundary(pos) {
                    return Err(ChangeSetError::NotCharBoundary { offset: pos });
                }
            }
        }
        Ok(())
    }

    /// Applies this change set to `text`. On error the text is unchanged.
    ///
    /// One rope edit per `(Delete, Insert)` group, back to front:
    /// O(groups · log n). Rebuilding the rope instead was slower at every
    /// measured edit count (DECISIONS 010).
    pub(crate) fn apply(&self, text: &mut Text) -> Result<(), ChangeSetError> {
        self.validate(text)?;
        self.edit_back_to_front(text)
    }

    /// Walks the ops from the end so that each edit leaves the offsets of
    /// the ones still to apply unchanged. `self` must be valid for `text`.
    fn edit_back_to_front(&self, text: &mut Text) -> Result<(), ChangeSetError> {
        // Old-text offset where the op being visited ends.
        let mut end = self.len_before;
        let mut ops = self.ops.iter().rev().peekable();
        while let Some(op) = ops.next() {
            match op {
                Op::Retain(n) => end -= n,
                Op::Delete(n) => {
                    let start = end - n;
                    text.delete(start..end).map_err(after_validation)?;
                    end = start;
                }
                Op::Insert(s) => {
                    // Canonical form puts a group's Delete just before its
                    // Insert, so walking backwards it comes just after.
                    if let Some(&&Op::Delete(d)) = ops.peek() {
                        ops.next();
                        let start = end - d;
                        text.replace(start..end, s).map_err(after_validation)?;
                        end = start;
                    } else {
                        text.insert(end, s).map_err(after_validation)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The change set that undoes this one: applied to the result of
    /// applying `self` to `original`, it gives back `original`.
    ///
    /// Fails if `self` is not valid for `original`. O(ops · log n + deleted).
    pub fn invert(&self, original: &Text) -> Result<ChangeSet, ChangeSetError> {
        self.validate(original)?;
        let mut builder = ChangeSetBuilder::with_capacity(self.len_after, self.ops.len());
        let mut pos = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    builder.retain(*n);
                    pos += n;
                }
                Op::Delete(n) => {
                    let deleted = original.slice(pos..pos + n).map_err(after_validation)?;
                    for chunk in deleted.chunks() {
                        builder.insert(chunk);
                    }
                    pos += n;
                }
                Op::Insert(s) => {
                    builder.delete(s.len());
                }
            }
        }
        builder.finish()
    }
}

/// Converts a `Text` error that a successful [`ChangeSet::validate`] rules
/// out. Reaching this is a bug, caught in debug builds.
fn after_validation(e: TextError) -> ChangeSetError {
    debug_assert!(false, "text edit failed after validation: {e}");
    match e {
        TextError::NotCharBoundary { offset } => ChangeSetError::NotCharBoundary { offset },
        TextError::OffsetOutOfBounds { offset, len } => ChangeSetError::LengthMismatch {
            expected: offset,
            actual: len,
        },
        TextError::InvertedRange { start, end } => ChangeSetError::LengthMismatch {
            expected: start,
            actual: end,
        },
        TextError::LineOutOfBounds { line, line_count } => ChangeSetError::LengthMismatch {
            expected: line,
            actual: line_count,
        },
    }
}
