//! Change sets: the editing algebra.
//!
//! A [`ChangeSet`] describes one change to a text of a known length as a
//! sequence of [`Op`]s that walk the old text from start to end: `Retain(n)`
//! keeps the next `n` bytes, `Delete(n)` removes them, and `Insert(s)` adds
//! `s` at the current position. Lengths are UTF-8 bytes.
//!
//! # Canonical form
//!
//! The only way to make a change set is [`ChangeSetBuilder`] (directly or
//! through [`ChangeSet::from_edits`]), and it always produces canonical form,
//! so two change sets with the same effect built from the same edits compare
//! equal:
//!
//! 1. No op is empty (`Retain(0)`, `Delete(0)`, `Insert("")`).
//! 2. No two adjacent ops have the same kind.
//! 3. A `Delete` never directly follows an `Insert`: between two `Retain`s
//!    there is at most one `Delete`, then at most one `Insert`.
//! 4. `len_before` is the sum of `Retain` and `Delete` lengths, and
//!    `len_after` the sum of `Retain` lengths and inserted bytes.
//!
//! A change set is structurally valid by construction. Whether it is valid
//! *for a given text* (right length, every op boundary on a char boundary) is
//! checked separately against that text.

use std::fmt;

use compact_str::CompactString;

mod builder;

#[cfg(test)]
pub(crate) mod reference;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

pub use builder::ChangeSetBuilder;

/// One step of a [`ChangeSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Keep the next `n` bytes of the old text.
    Retain(usize),
    /// Remove the next `n` bytes of the old text.
    Delete(usize),
    /// Insert this text at the current position.
    Insert(CompactString),
}

/// Which side of an insertion a mapped position ends up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Assoc {
    /// Stay before text inserted at the position.
    Before,
    /// Move after text inserted at the position.
    After,
}

/// A change to a text of length [`len_before`](ChangeSet::len_before),
/// producing a text of length [`len_after`](ChangeSet::len_after).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}

impl ChangeSet {
    /// The change set that keeps all `len` bytes.
    pub fn identity(len: usize) -> ChangeSet {
        let ops = if len == 0 {
            Vec::new()
        } else {
            vec![Op::Retain(len)]
        };
        ChangeSet {
            ops,
            len_before: len,
            len_after: len,
        }
    }

    /// Builds a change set from edits `(range, replacement)` on a text of
    /// `len_before` bytes.
    ///
    /// Edits must be sorted by start and must not overlap. Touching edits
    /// are allowed, and several inserts at one position are concatenated in
    /// input order. Edit `i` is rejected with
    /// [`ChangeSetError::InvalidEdit`] if its range is inverted, ends past
    /// `len_before`, or starts before the previous edit ends.
    pub fn from_edits<I, S>(len_before: usize, edits: I) -> Result<ChangeSet, ChangeSetError>
    where
        I: IntoIterator<Item = (std::ops::Range<usize>, S)>,
        S: AsRef<str>,
    {
        let mut builder = ChangeSetBuilder::new(len_before);
        let mut prev_end = 0;
        for (index, (range, s)) in edits.into_iter().enumerate() {
            if range.start > range.end || range.end > len_before || range.start < prev_end {
                return Err(ChangeSetError::InvalidEdit { index });
            }
            builder
                .retain(range.start - prev_end)
                .delete(range.end - range.start)
                .insert(s.as_ref());
            prev_end = range.end;
        }
        builder.retain_rest();
        builder.finish()
    }

    /// Length of the text this change set applies to.
    pub fn len_before(&self) -> usize {
        self.len_before
    }

    /// Length of the text this change set produces.
    pub fn len_after(&self) -> usize {
        self.len_after
    }

    /// The ops, in canonical form.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Whether this change set leaves every text unchanged (no deletes or
    /// inserts).
    pub fn is_identity(&self) -> bool {
        self.ops.iter().all(|op| matches!(op, Op::Retain(_)))
    }
}

/// Error returned by change set construction, validation, and algebra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeSetError {
    /// A length does not match: the ops against the builder's `len_before`,
    /// a change set against a text, or two change sets being composed.
    LengthMismatch { expected: usize, actual: usize },
    /// A length sum exceeds `usize::MAX`.
    Overflow,
    /// An op boundary falls inside a multi-byte character. `offset` is in
    /// the text the change set is applied to.
    NotCharBoundary { offset: usize },
    /// Edit `index` passed to [`ChangeSet::from_edits`] is inverted, out of
    /// order, overlapping, or out of bounds.
    InvalidEdit { index: usize },
    /// A position to map is past the change set's `len_before`.
    PositionOutOfBounds { position: usize, len: usize },
    /// Positions passed for sorted mapping are not sorted: element `index`
    /// is smaller than the one before it.
    UnsortedPositions { index: usize },
}

impl fmt::Display for ChangeSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::LengthMismatch { expected, actual } => {
                write!(
                    f,
                    "length mismatch: expected {expected} bytes, got {actual}"
                )
            }
            Self::Overflow => write!(f, "change set length overflows usize"),
            Self::NotCharBoundary { offset } => {
                write!(
                    f,
                    "byte offset {offset} is not on a UTF-8 character boundary"
                )
            }
            Self::InvalidEdit { index } => {
                write!(
                    f,
                    "edit {index} is inverted, out of order, or out of bounds"
                )
            }
            Self::PositionOutOfBounds { position, len } => {
                write!(f, "position {position} is out of bounds (length {len})")
            }
            Self::UnsortedPositions { index } => {
                write!(
                    f,
                    "position at index {index} is smaller than the one before it"
                )
            }
        }
    }
}

impl std::error::Error for ChangeSetError {}
