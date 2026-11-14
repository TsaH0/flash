//! Constructors: a selection plus an intent becomes one change set and the
//! resulting selection, in one walk over the ranges. O(k + inserted bytes)
//! plus O(log n) per grapheme step.

use std::cmp::Ordering;

use smallvec::SmallVec;

use super::{Transaction, TransactionError};
use crate::changeset::{ChangeSetBuilder, ChangeSetError};
use crate::selection::{Range, Selection, normalise};
use crate::text::{Direction, Text};

impl Transaction {
    /// Inserts `s` at every range's head. A cursor ends after the inserted
    /// text; a forward range grows to include it; a backward range keeps
    /// its text, with the insertion before it. See the module docs for
    /// same-position inserts.
    pub fn insert(
        text: &Text,
        selection: &Selection,
        s: &str,
    ) -> Result<Transaction, TransactionError> {
        selection.validate(text)?;
        let mut walk = Walk::new(text, selection);
        for range in selection.ranges() {
            let (anchor, head) = (range.anchor(), range.head());
            let n = walk.edit(head, head, s)?;
            let after = n + s.len();
            walk.push(match anchor.cmp(&head) {
                Ordering::Equal => Range::point(after),
                Ordering::Less => Range::new(n - (head - anchor), after),
                Ordering::Greater => Range::new(after + (anchor - head), after),
            });
        }
        walk.finish(selection)
    }

    /// Deletes every range's text. Every range becomes a cursor where its
    /// text was.
    pub fn delete(text: &Text, selection: &Selection) -> Result<Transaction, TransactionError> {
        selection.validate(text)?;
        let mut walk = Walk::new(text, selection);
        for range in selection.ranges() {
            let n = walk.edit(range.start(), range.end(), "")?;
            walk.push(Range::point(n));
        }
        walk.finish(selection)
    }

    /// Replaces every range's text with `s`; each range then covers its
    /// replacement, keeping its direction (a cursor becomes a forward range).
    pub fn replace(
        text: &Text,
        selection: &Selection,
        s: &str,
    ) -> Result<Transaction, TransactionError> {
        selection.validate(text)?;
        let mut walk = Walk::new(text, selection);
        for range in selection.ranges() {
            let n = walk.edit(range.start(), range.end(), s)?;
            let end = n + s.len();
            walk.push(match range.direction() {
                Direction::Forward => Range::new(n, end),
                Direction::Backward => Range::new(end, n),
            });
        }
        walk.finish(selection)
    }

    /// Deletes every non-empty range's text, and the grapheme before every
    /// cursor. Overlapping deletions merge (module docs).
    pub fn backspace(text: &Text, selection: &Selection) -> Result<Transaction, TransactionError> {
        delete_around(text, selection, Direction::Backward)
    }

    /// Deletes every non-empty range's text, and the grapheme after every
    /// cursor. Overlapping deletions merge (module docs).
    pub fn delete_forward(
        text: &Text,
        selection: &Selection,
    ) -> Result<Transaction, TransactionError> {
        delete_around(text, selection, Direction::Forward)
    }
}

/// Backspace (`Backward`) or delete forward (`Forward`).
fn delete_around(
    text: &Text,
    selection: &Selection,
    direction: Direction,
) -> Result<Transaction, TransactionError> {
    selection.validate(text)?;
    // Merged deletions as (start, end, index of the first range in the
    // group). A group's ranges are contiguous in selection order, because a
    // new deletion only ever merges with the groups at the top of the stack.
    let mut groups: SmallVec<[(usize, usize, usize); 1]> = SmallVec::new();
    for (i, range) in selection.ranges().iter().enumerate() {
        let (mut start, mut end) = (range.start(), range.end());
        if range.is_empty() {
            let other = text.move_graphemes(range.head(), 1, direction)?;
            (start, end) = (start.min(other), end.max(other));
        }
        let mut first = i;
        while let Some(&(s, e, f)) = groups.last() {
            if start >= e {
                break;
            }
            groups.pop();
            (start, end, first) = (start.min(s), end.max(e), f);
        }
        groups.push((start, end, first));
    }

    let mut walk = Walk::new(text, selection);
    let count = selection.ranges().len();
    for (g, &(start, end, first)) in groups.iter().enumerate() {
        let n = walk.edit(start, end, "")?;
        let next_first = groups.get(g + 1).map_or(count, |&(_, _, f)| f);
        for _ in first..next_first {
            walk.push(Range::point(n));
        }
    }
    walk.finish(selection)
}

/// Emits edits in order through one builder and collects result ranges.
struct Walk {
    builder: ChangeSetBuilder,
    /// Old-text offset up to which ops have been emitted.
    old: usize,
    /// The new-text offset that `old` maps to.
    new: usize,
    ranges: SmallVec<[Range; 1]>,
}

impl Walk {
    fn new(text: &Text, selection: &Selection) -> Walk {
        let k = selection.ranges().len();
        Walk {
            builder: ChangeSetBuilder::with_capacity(text.byte_len(), 2 * k + 1),
            old: 0,
            new: 0,
            ranges: SmallVec::with_capacity(k),
        }
    }

    /// Replaces old `start..end` (with `start` at or after the previous
    /// edit's end) by `s`; returns where the edit starts in the new text.
    fn edit(&mut self, start: usize, end: usize, s: &str) -> Result<usize, ChangeSetError> {
        debug_assert!(self.old <= start && start <= end);
        self.builder
            .retain(start - self.old)
            .delete(end - start)
            .insert(s);
        let n = self
            .new
            .checked_add(start - self.old)
            .ok_or(ChangeSetError::Overflow)?;
        self.old = end;
        self.new = n.checked_add(s.len()).ok_or(ChangeSetError::Overflow)?;
        Ok(n)
    }

    fn push(&mut self, range: Range) {
        self.ranges.push(range);
    }

    /// Finishes the change set; the result ranges (one per input range, in
    /// order) keep the input's primary and are normalised.
    fn finish(mut self, selection: &Selection) -> Result<Transaction, TransactionError> {
        self.builder.retain_rest();
        let changes = self.builder.finish()?;
        let selection = normalise(self.ranges, selection.primary_index());
        Ok(Transaction::new(changes, Some(selection)))
    }
}
