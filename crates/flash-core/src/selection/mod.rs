//! Selections: one or more ranges over a text, one of them primary.
//!
//! A [`Range`] is an `anchor` and a `head`, byte offsets *between*
//! characters. The head is the end that motions move; the anchor stays put
//! when a selection is extended. A range may be empty (`anchor == head`):
//! that is a plain cursor.
//!
//! # Normalisation
//!
//! A [`Selection`] always holds at least one range, sorted by
//! `(start, end)`, and is normalised by the merge rule (PLAN 2.5,
//! DECISIONS 011):
//!
//! - overlapping ranges merge;
//! - touching non-empty ranges stay separate, so adjacent words or lines
//!   remain distinct selections;
//! - an empty range touching or inside another range merges into it.
//!
//! A merged range is the union of its group. It takes its direction and
//! goal column from the *kept* range: the primary if the group contains it,
//! otherwise the group's first range in sorted order. The primary survives
//! merging: the output primary is the group that contains the input one.
//!
//! # Validity
//!
//! A selection does not know its text. [`Selection::validate`] checks that
//! every endpoint is a char boundary of a given text; every operation that
//! takes a selection and a text checks this first.

use std::fmt;

use smallvec::{SmallVec, smallvec};

use crate::text::{Direction, Text, TextError};

mod map;

#[cfg(test)]
pub(crate) mod proptests;
#[cfg(test)]
mod tests;

/// One selected range: an anchor and a head, plus the display column that
/// vertical motion aims for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    anchor: usize,
    head: usize,
    goal_column: Option<usize>,
}

impl Range {
    /// A range from `anchor` to `head`, with no goal column.
    pub fn new(anchor: usize, head: usize) -> Range {
        Range {
            anchor,
            head,
            goal_column: None,
        }
    }

    /// An empty range (a cursor) at `pos`.
    pub fn point(pos: usize) -> Range {
        Range::new(pos, pos)
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn head(&self) -> usize {
        self.head
    }

    /// The smaller endpoint.
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The larger endpoint.
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// `start()..end()`.
    pub fn span(&self) -> std::ops::Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// [`Direction::Backward`] if the head is before the anchor. An empty
    /// range counts as forward.
    pub fn direction(&self) -> Direction {
        if self.head < self.anchor {
            Direction::Backward
        } else {
            Direction::Forward
        }
    }

    /// The display column vertical motion aims for, if one is remembered.
    pub fn goal_column(&self) -> Option<usize> {
        self.goal_column
    }

    pub fn with_goal_column(self, goal: Option<usize>) -> Range {
        Range {
            goal_column: goal,
            ..self
        }
    }

    /// The range over `start..end` pointing in `direction`, with no goal
    /// column.
    fn spanning(start: usize, end: usize, direction: Direction) -> Range {
        match direction {
            Direction::Forward => Range::new(start, end),
            Direction::Backward => Range::new(end, start),
        }
    }
}

/// A non-empty, sorted, normalised set of ranges with a primary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    ranges: SmallVec<[Range; 1]>,
    primary: usize,
}

impl Selection {
    /// A single cursor at `pos`.
    pub fn point(pos: usize) -> Selection {
        Selection::single(Range::point(pos))
    }

    /// A selection of one range.
    pub fn single(range: Range) -> Selection {
        Selection {
            ranges: smallvec![range],
            primary: 0,
        }
    }

    /// A selection of `ranges`, in any order, with `ranges[primary]` as the
    /// primary. Normalises (see the module docs). O(k) for sorted input,
    /// O(k log k) otherwise.
    pub fn new(
        ranges: impl IntoIterator<Item = Range>,
        primary: usize,
    ) -> Result<Selection, SelectionError> {
        let ranges: SmallVec<[Range; 1]> = ranges.into_iter().collect();
        if ranges.is_empty() {
            return Err(SelectionError::Empty);
        }
        if primary >= ranges.len() {
            return Err(SelectionError::PrimaryOutOfBounds {
                primary,
                len: ranges.len(),
            });
        }
        Ok(normalise(ranges, primary))
    }

    /// The ranges, sorted by `(start, end)`. Never empty.
    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    pub fn primary(&self) -> &Range {
        &self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// This selection plus `range`, which becomes the primary.
    pub fn with_range(&self, range: Range) -> Selection {
        let mut ranges = self.ranges.clone();
        ranges.push(range);
        let primary = ranges.len() - 1;
        normalise(ranges, primary)
    }

    /// Only the primary range.
    pub fn primary_only(&self) -> Selection {
        Selection::single(*self.primary())
    }

    /// Heap bytes held (zero for one range, which is stored inline).
    pub(crate) fn heap_bytes(&self) -> usize {
        if self.ranges.spilled() {
            self.ranges.capacity() * size_of::<Range>()
        } else {
            0
        }
    }

    /// Checks that every endpoint is a valid position in `text` (in bounds
    /// and on a char boundary). Returns the first problem, checking each
    /// range's anchor then head, in order. O(k log n).
    pub fn validate(&self, text: &Text) -> Result<(), TextError> {
        for range in &self.ranges {
            text.check_offset(range.anchor)?;
            text.check_offset(range.head)?;
        }
        Ok(())
    }
}

/// Normalises `ranges` (non-empty, `primary` in bounds) by the merge rule.
pub(crate) fn normalise(mut ranges: SmallVec<[Range; 1]>, mut primary: usize) -> Selection {
    debug_assert!(primary < ranges.len());
    let key = |r: &Range| (r.start(), r.end());
    if !ranges.windows(2).all(|w| key(&w[0]) <= key(&w[1])) {
        let primary_range = ranges[primary];
        ranges.sort_by_key(key);
        // Any range equal to the primary has the primary's span, so it lands
        // in the same group; picking the first equal one is exact.
        primary = ranges.iter().position(|r| *r == primary_range).unwrap_or(0);
    }

    let mut out: SmallVec<[Range; 1]> = SmallVec::with_capacity(ranges.len());
    let mut out_primary = 0;
    let mut i = 0;
    while i < ranges.len() {
        let first = i;
        let (group_start, mut group_end) = (ranges[i].start(), ranges[i].end());
        let mut kept = first;
        if i == primary {
            out_primary = out.len();
        }
        i += 1;
        while let Some(r) = ranges.get(i) {
            let joins = r.start() < group_end
                || (r.start() == group_end && (r.is_empty() || group_start == group_end));
            if !joins {
                break;
            }
            group_end = group_end.max(r.end());
            if i == primary {
                kept = i;
                out_primary = out.len();
            }
            i += 1;
        }
        let k = ranges[kept];
        out.push(if i - first == 1 {
            k
        } else {
            Range::spanning(group_start, group_end, k.direction()).with_goal_column(k.goal_column)
        });
    }
    Selection {
        ranges: out,
        primary: out_primary,
    }
}

/// Error returned when constructing a [`Selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionError {
    /// No ranges were given.
    Empty,
    /// The primary index is not less than the number of ranges.
    PrimaryOutOfBounds { primary: usize, len: usize },
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => write!(f, "a selection needs at least one range"),
            Self::PrimaryOutOfBounds { primary, len } => {
                write!(f, "primary index {primary} is out of bounds ({len} ranges)")
            }
        }
    }
}

impl std::error::Error for SelectionError {}
