//! Document revisions and the bounded revision log (PLAN 2.7).
//!
//! A [`Revision`] counts effective text changes. The [`RevisionLog`] keeps
//! the change sets of the most recent ones, so a position computed against
//! an older revision (by a background worker, an LSP server, a plugin) can
//! be mapped to the current text, or rejected once its revision has fallen
//! out of the log.

use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;

use crate::changeset::{Assoc, ChangeSet, ChangeSetError};
use crate::text::TextError;

/// The number of effective text changes a document has seen. Strictly
/// increases on every commit that changes the text (including, from Stage
/// 5, undo and redo); never on a selection-only commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(pub(crate) u64);

impl Revision {
    /// The revision of a freshly opened document.
    pub const ZERO: Revision = Revision(0);

    pub fn get(self) -> u64 {
        self.0
    }

    /// The revision after this one. Saturates; 2^64 commits do not happen.
    pub(crate) fn next(self) -> Revision {
        Revision(self.0.saturating_add(1))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// The change sets of the most recent revisions. Entry `(r, c)` turns the
/// text at revision `r - 1` into the text at `r`; entries are consecutive.
///
/// Bounded by entry count and by total op count: one 10,000-cursor edit
/// holds about 20,000 ops, so an entry bound alone could keep hundreds of
/// megabytes. The oldest entries are evicted first; the newest is always
/// kept, even if it alone exceeds the op bound.
#[derive(Debug, Clone)]
pub struct RevisionLog {
    entries: VecDeque<(Revision, ChangeSet)>,
    ops: usize,
    max_entries: usize,
    max_ops: usize,
}

impl RevisionLog {
    pub const DEFAULT_MAX_ENTRIES: usize = 1024;
    pub const DEFAULT_MAX_OPS: usize = 1 << 20;

    /// An empty log with the default bounds.
    pub fn new() -> RevisionLog {
        RevisionLog {
            entries: VecDeque::new(),
            ops: 0,
            max_entries: Self::DEFAULT_MAX_ENTRIES,
            max_ops: Self::DEFAULT_MAX_OPS,
        }
    }

    /// An empty log keeping at most `max_entries` entries and, beyond the
    /// newest entry, at most `max_ops` ops in total.
    pub fn with_limits(max_entries: NonZeroUsize, max_ops: usize) -> RevisionLog {
        RevisionLog {
            max_entries: max_entries.get(),
            max_ops,
            ..RevisionLog::new()
        }
    }

    /// Appends the change set that produced `revision`, which must follow
    /// the newest entry, then evicts the oldest entries over the bounds.
    pub(crate) fn push(&mut self, revision: Revision, changes: ChangeSet) {
        debug_assert!(
            self.entries
                .back()
                .is_none_or(|&(last, _)| revision == last.next()),
            "revision log entries must be consecutive"
        );
        self.ops += changes.ops().len();
        self.entries.push_back((revision, changes));
        while self.entries.len() > 1
            && (self.entries.len() > self.max_entries || self.ops > self.max_ops)
        {
            if let Some((_, old)) = self.entries.pop_front() {
                self.ops -= old.ops().len();
            }
        }
    }

    /// Number of entries held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The oldest revision positions can be mapped from, given that the
    /// document is at `current`: the revision before the first entry, or
    /// `current` itself when the log is empty.
    pub fn oldest(&self, current: Revision) -> Revision {
        match self.entries.front() {
            Some(&(first, _)) => Revision(first.0 - 1),
            None => current,
        }
    }

    /// Maps `positions` (sorted ascending, offsets in the text at revision
    /// `from`) to offsets in the text at `current`, through every logged
    /// change set after `from`, in order. Never through a composed change
    /// set (DECISIONS 009). `from == current` leaves them unchanged.
    ///
    /// Fails with [`MapError::Future`] if `from > current`,
    /// [`MapError::Evicted`] if `from` is older than [`oldest`](Self::oldest),
    /// or [`MapError::ChangeSet`] if the positions are unsorted or past the
    /// text's length at `from`. On error `positions` is unchanged.
    ///
    /// Cannot detect a position that was not a char boundary at `from`; use
    /// `Document::map_from`, which validates the results.
    /// O(Σ ops + k · entries after `from`).
    pub fn map_from(
        &self,
        from: Revision,
        current: Revision,
        positions: &mut [usize],
        assoc: Assoc,
    ) -> Result<(), MapError> {
        if from > current {
            return Err(MapError::Future { from, current });
        }
        let oldest = self.oldest(current);
        if from < oldest {
            return Err(MapError::Evicted { from, oldest });
        }
        let skip = (from.0 - oldest.0) as usize;
        let mut chain = self.entries.iter().skip(skip).map(|(_, c)| c);
        // The first change set checks the positions; the rest cannot fail,
        // because entries are consecutive (each `len_before` is the
        // previous `len_after`) and mapping keeps positions sorted and
        // within the new length.
        if let Some(first) = chain.next() {
            first.map_sorted(positions, assoc)?;
        }
        for changes in chain {
            if let Err(e) = changes.map_sorted(positions, assoc) {
                debug_assert!(false, "consecutive log entries failed to map: {e}");
                return Err(e.into());
            }
        }
        Ok(())
    }
}

impl Default for RevisionLog {
    fn default() -> Self {
        RevisionLog::new()
    }
}

/// Error returned by mapping positions from an older revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MapError {
    /// `from` is older than the log reaches.
    Evicted { from: Revision, oldest: Revision },
    /// `from` is newer than the document.
    Future { from: Revision, current: Revision },
    /// The positions are unsorted or out of bounds for the text at `from`.
    ChangeSet(ChangeSetError),
    /// A mapped position is not valid in the current text: the input was
    /// not a char boundary at `from`.
    Text(TextError),
}

impl From<ChangeSetError> for MapError {
    fn from(e: ChangeSetError) -> Self {
        MapError::ChangeSet(e)
    }
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evicted { from, oldest } => {
                write!(
                    f,
                    "revision {from} is older than the log's oldest, {oldest}"
                )
            }
            Self::Future { from, current } => {
                write!(f, "revision {from} is newer than the document's, {current}")
            }
            Self::ChangeSet(e) => write!(f, "positions do not fit the old text: {e}"),
            Self::Text(e) => write!(f, "mapped position is not valid: {e}"),
        }
    }
}

impl std::error::Error for MapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ChangeSet(e) => Some(e),
            Self::Text(e) => Some(e),
            _ => None,
        }
    }
}
