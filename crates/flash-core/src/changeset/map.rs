//! Mapping positions in the old text to positions in the new text.
//!
//! The policy is in the module docs of [`crate::changeset`].

use super::{Assoc, ChangeSet, ChangeSetError, Op};

impl ChangeSet {
    /// Maps `pos`, an offset in the text before this change, to the
    /// corresponding offset after it. O(ops).
    ///
    /// Fails with [`ChangeSetError::PositionOutOfBounds`] if
    /// `pos > len_before()`.
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> Result<usize, ChangeSetError> {
        let mut one = [pos];
        self.map_sorted(&mut one, assoc)?;
        Ok(one[0])
    }

    /// Maps every position in `positions` (sorted ascending, duplicates
    /// allowed) in place, in one pass over the ops: O(ops + positions), no
    /// allocation. Equal to calling [`map_pos`](Self::map_pos) on each, and
    /// the results stay sorted.
    ///
    /// Fails with [`ChangeSetError::UnsortedPositions`] or
    /// [`ChangeSetError::PositionOutOfBounds`] for the first offending
    /// element; then `positions` is unchanged.
    pub fn map_sorted(&self, positions: &mut [usize], assoc: Assoc) -> Result<(), ChangeSetError> {
        self.map_sorted_with(positions, |_| assoc)
    }

    /// Like [`map_sorted`](Self::map_sorted), but `assoc(i)` gives the
    /// [`Assoc`] for `positions[i]`. Same errors, same single pass, and
    /// `positions` is unchanged on error.
    pub fn map_sorted_with(
        &self,
        positions: &mut [usize],
        assoc: impl Fn(usize) -> Assoc,
    ) -> Result<(), ChangeSetError> {
        let mut prev = 0;
        for (index, &position) in positions.iter().enumerate() {
            if position > self.len_before {
                return Err(ChangeSetError::PositionOutOfBounds {
                    position,
                    len: self.len_before,
                });
            }
            if position < prev {
                return Err(ChangeSetError::UnsortedPositions { index });
            }
            prev = position;
        }
        self.map_sorted_unchecked(positions, assoc);
        Ok(())
    }

    /// `positions` must be sorted and at most `len_before`.
    fn map_sorted_unchecked(&self, positions: &mut [usize], assoc: impl Fn(usize) -> Assoc) {
        // `old`/`new`: where the op being visited starts in the old and the
        // new text.
        let (mut old, mut new) = (0, 0);
        let mut pending = positions.iter_mut().enumerate().peekable();
        let mut ops = self.ops.iter().peekable();
        while let Some(op) = ops.next() {
            let (deleted, inserted) = match op {
                Op::Retain(k) => {
                    // Strictly inside the run; its end belongs to whatever
                    // follows (an edit group or the end of the text).
                    while let Some((_, p)) = pending.next_if(|(_, p)| **p < old + k) {
                        *p = new + (*p - old);
                    }
                    old += k;
                    new += k;
                    continue;
                }
                Op::Delete(d) => match ops.next_if(|op| matches!(op, Op::Insert(_))) {
                    Some(Op::Insert(s)) => (*d, s.len()),
                    _ => (*d, 0),
                },
                Op::Insert(s) => (0, s.len()),
            };
            // An edit group covers the closed range [old, old + deleted].
            while let Some((i, p)) = pending.next_if(|(_, p)| **p <= old + deleted) {
                *p = group_image(*p - old, deleted, new, inserted, assoc(i));
            }
            old += deleted;
            new += inserted;
        }
        // Only `len_before` itself can remain, with no edit at the end.
        for (_, p) in pending {
            *p = new;
        }
    }
}

/// Image of a position `offset` bytes into an edit group that deletes
/// `deleted` bytes and inserts `inserted` bytes at new offset `new`
/// (`offset <= deleted`). With a deletion, the group's old edges map to the
/// replacement's edges and only interior positions follow `assoc`; a pure
/// insertion follows `assoc`.
fn group_image(offset: usize, deleted: usize, new: usize, inserted: usize, assoc: Assoc) -> usize {
    let left = new;
    let right = new + inserted;
    if deleted > 0 && offset == 0 {
        left
    } else if deleted > 0 && offset == deleted {
        right
    } else {
        match assoc {
            Assoc::Before => left,
            Assoc::After => right,
        }
    }
}
