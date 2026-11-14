//! Transactions: one change set plus the selection that goes with it.
//!
//! All text changes go through a [`Transaction`] (PLAN 2.6). The
//! constructors in this module turn a selection and an intent (insert,
//! delete, replace, backspace, delete forward) into one [`ChangeSet`] and the
//! resulting selection. [`Transaction::apply`] validates everything first and
//! only then changes the text, so a failed transaction changes nothing
//! (DECISIONS 010, 013).
//!
//! # Collision rules
//!
//! Ranges in a normalised selection never overlap, but two can touch and so
//! share a head, and a cursor's grapheme can reach into a neighbour:
//!
//! - *Same-position inserts:* each range inserts its own copy, in selection
//!   order, and each head lands right after its own copy.
//! - *Overlapping deletes* (backspace and delete forward only): deletions
//!   that overlap merge into their union, and every range whose deletion
//!   merged collapses to the union's start. Touching deletions stay
//!   separate edits.

use std::fmt;

use smallvec::SmallVec;

use crate::changeset::{ChangeSet, ChangeSetError, Op};
use crate::selection::Selection;
use crate::text::{Text, TextError};

mod build;

#[cfg(test)]
pub(crate) mod reference;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

/// A change to a text and, optionally, the selection to set afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    changes: ChangeSet,
    selection: Option<Selection>,
}

impl Transaction {
    /// A transaction of `changes`. With `selection` `None`, applying it maps
    /// the current selection through `changes`.
    pub fn new(changes: ChangeSet, selection: Option<Selection>) -> Transaction {
        Transaction { changes, selection }
    }

    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }

    /// The selection to set after applying, if the transaction carries one.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Applies this transaction to `text` and updates `selection`: to the
    /// transaction's selection if it has one, otherwise to `selection`
    /// mapped through the changes.
    ///
    /// Validates first, then mutates: the change set must fit `text`, and
    /// the transaction's selection must be valid for the text it produces
    /// (checked without building it), or the current selection valid for
    /// `text`. On error neither argument changes. O(ops · log n + k log n).
    ///
    /// Public until `Document::commit` (Stage 4) becomes the only caller;
    /// then `pub(crate)`.
    pub fn apply(
        &self,
        text: &mut Text,
        selection: &mut Selection,
    ) -> Result<(), TransactionError> {
        self.changes.validate(text)?;
        let next = match &self.selection {
            Some(next) => {
                check_after(&self.changes, text, &endpoints(next))?;
                next.clone()
            }
            None => {
                selection.validate(text)?;
                selection.map(&self.changes)?
            }
        };
        if let Err(e) = self.changes.apply(text) {
            debug_assert!(false, "change set failed after validation: {e}");
            return Err(e.into());
        }
        *selection = next;
        Ok(())
    }
}

/// Every range's start and end, in order. Sorted, because selections are
/// normalised.
fn endpoints(selection: &Selection) -> SmallVec<[usize; 2]> {
    selection
        .ranges()
        .iter()
        .flat_map(|r| [r.start(), r.end()])
        .collect()
}

/// Checks that every one of `positions` (sorted) is a valid position in the
/// text `changes` produces from `old`, without building that text. `changes`
/// must be valid for `old`. Errors are in new-text offsets.
///
/// One pass over the ops: a position in a retained run is a char boundary
/// iff its old offset is; a position in an inserted string iff it is one of
/// that string's; op edges are always boundaries. O(ops + k log n).
fn check_after(changes: &ChangeSet, old: &Text, positions: &[usize]) -> Result<(), TextError> {
    let mut pending = positions.iter().copied().peekable();
    let (mut old_pos, mut new_pos) = (0, 0);
    for op in changes.ops() {
        match op {
            Op::Retain(k) => {
                while let Some(p) = pending.next_if(|&p| p < new_pos + k) {
                    if !old.is_char_boundary(old_pos + (p - new_pos)) {
                        return Err(TextError::NotCharBoundary { offset: p });
                    }
                }
                old_pos += k;
                new_pos += k;
            }
            Op::Delete(d) => old_pos += d,
            Op::Insert(s) => {
                while let Some(p) = pending.next_if(|&p| p < new_pos + s.len()) {
                    if !s.is_char_boundary(p - new_pos) {
                        return Err(TextError::NotCharBoundary { offset: p });
                    }
                }
                new_pos += s.len();
            }
        }
    }
    match pending.find(|&p| p > new_pos) {
        Some(offset) => Err(TextError::OffsetOutOfBounds {
            offset,
            len: new_pos,
        }),
        None => Ok(()),
    }
}

/// Error returned by building or applying a [`Transaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionError {
    /// The change set does not fit the text.
    ChangeSet(ChangeSetError),
    /// A selection endpoint is out of bounds or inside a character: the
    /// selection given to a constructor, the current selection, or the
    /// transaction's selection against the text it produces.
    Selection(TextError),
}

impl From<ChangeSetError> for TransactionError {
    fn from(e: ChangeSetError) -> Self {
        TransactionError::ChangeSet(e)
    }
}

impl From<TextError> for TransactionError {
    fn from(e: TextError) -> Self {
        TransactionError::Selection(e)
    }
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChangeSet(e) => write!(f, "change set does not fit the text: {e}"),
            Self::Selection(e) => write!(f, "selection is not valid for the text: {e}"),
        }
    }
}

impl std::error::Error for TransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ChangeSet(e) => Some(e),
            Self::Selection(e) => Some(e),
        }
    }
}
