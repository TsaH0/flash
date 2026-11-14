//! Mapping a selection through a change set.

use smallvec::SmallVec;

use super::{Range, Selection, normalise};
use crate::changeset::{Assoc, ChangeSet, ChangeSetError};

impl Selection {
    /// Maps every range through `changes` (old text to new text), in one
    /// pass over the ops. O(ops + k), allocation-free for one range.
    ///
    /// Per endpoint (DECISIONS 011): a range's start maps with
    /// [`Assoc::After`]; its end maps with `After` if the range is empty and
    /// [`Assoc::Before`] otherwise. So a cursor moves past text inserted at
    /// it, and a non-empty range does not absorb text inserted at its edges.
    /// A range strictly inside a replaced span would come out inverted; its
    /// endpoints are reordered, so it covers the replacement. Directions are
    /// kept, goal columns are cleared, and the result is normalised (mapping
    /// can make ranges touch or coincide).
    ///
    /// Fails with [`ChangeSetError::PositionOutOfBounds`] if an endpoint is
    /// past `changes.len_before()`.
    pub fn map(&self, changes: &ChangeSet) -> Result<Selection, ChangeSetError> {
        let mut ends: SmallVec<[usize; 2]> = self
            .ranges
            .iter()
            .flat_map(|r| [r.start(), r.end()])
            .collect();
        changes.map_sorted_with(&mut ends, |i| {
            if i % 2 == 1 && !self.ranges[i / 2].is_empty() {
                Assoc::Before
            } else {
                Assoc::After
            }
        })?;
        let ranges = self
            .ranges
            .iter()
            .zip(ends.chunks_exact(2))
            .map(|(r, e)| Range::spanning(e[0].min(e[1]), e[0].max(e[1]), r.direction()))
            .collect();
        Ok(normalise(ranges, self.primary))
    }
}
