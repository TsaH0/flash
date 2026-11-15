//! Documents: text plus the revision state that tracks its changes.
//!
//! A [`Document`] owns its text, its revision counter, the bounded
//! [`RevisionLog`], and the selection of every view showing it.
//! [`Document::commit`] is the only way the text changes (PLAN 2.6, Stage
//! 4): it validates the transaction and every view's new selection first,
//! and only then applies the text, bumps the revision, and logs the change
//! set. Invariant: every view's selection is valid for the text
//! (DECISIONS 014).

use std::fmt;
use std::path::{Path, PathBuf};

use slotmap::SecondaryMap;
use smallvec::SmallVec;

use crate::changeset::Assoc;
use crate::editor::ViewId;
use crate::selection::Selection;
use crate::text::Text;
use crate::transaction::{Transaction, TransactionError};

mod revision;

pub use revision::{MapError, Revision, RevisionLog};

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

/// The line terminator a document uses for new lines. Detected at open;
/// the text itself is never normalised (DECISIONS 003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    /// The dominant terminator of `text`: `Crlf` if it has more `"\r\n"`
    /// than lone `"\n"`, otherwise `Lf` (also for text with no newline).
    /// One pass over the chunks; O(n).
    pub fn detect(text: &Text) -> LineEnding {
        let (mut lf, mut crlf) = (0usize, 0usize);
        let mut last = 0u8;
        for chunk in text.chunks() {
            let bytes = chunk.as_bytes();
            for (i, _) in chunk.match_indices('\n') {
                let before = if i == 0 { last } else { bytes[i - 1] };
                if before == b'\r' {
                    crlf += 1;
                } else {
                    lf += 1;
                }
            }
            if let Some(&b) = bytes.last() {
                last = b;
            }
        }
        if crlf > lf {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// One open text, its revision state, and the selections of its views.
#[derive(Debug, Clone)]
pub struct Document {
    text: Text,
    revision: Revision,
    log: RevisionLog,
    path: Option<PathBuf>,
    line_ending: LineEnding,
    selections: SecondaryMap<ViewId, Selection>,
}

impl Document {
    /// A document at revision 0 with no views; detects the line ending.
    pub fn new(text: Text, path: Option<PathBuf>) -> Document {
        Document {
            line_ending: LineEnding::detect(&text),
            text,
            revision: Revision::ZERO,
            log: RevisionLog::new(),
            path,
            selections: SecondaryMap::new(),
        }
    }

    /// Replaces the (empty) revision log, e.g. to change its bounds.
    pub fn with_log(mut self, log: RevisionLog) -> Document {
        self.log = log;
        self
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn log(&self) -> &RevisionLog {
        &self.log
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// The selection of `view`, or `None` if `view` does not show this
    /// document.
    pub fn selection(&self, view: ViewId) -> Option<&Selection> {
        self.selections.get(view)
    }

    /// The views showing this document.
    pub fn views(&self) -> impl Iterator<Item = ViewId> + '_ {
        self.selections.keys()
    }

    /// Replaces the selection of `view` if it is valid for the text.
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), CommitError> {
        let slot = self
            .selections
            .get_mut(view)
            .ok_or(CommitError::NoSuchView(view))?;
        selection
            .validate(&self.text)
            .map_err(TransactionError::from)?;
        *slot = selection;
        Ok(())
    }

    /// Commits `txn` on behalf of `view`: the only way the text changes.
    ///
    /// Validates the change set and `view`'s new selection (the
    /// transaction's, or the current one mapped), and maps every other
    /// view's selection, before changing anything. Then applies the text,
    /// stores the selections, and, unless the change set is the identity,
    /// increments the revision and moves the change set into the log.
    /// Returns the document's revision afterwards. On error nothing changes.
    ///
    /// An identity change set leaves other views' selections (and their goal
    /// columns) untouched. O(ops · log n + Σ k over views).
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, CommitError> {
        let current = self
            .selections
            .get(view)
            .ok_or(CommitError::NoSuchView(view))?;
        let origin = txn.prepare(&self.text, current)?;
        let changes = txn.into_changes();
        let identity = changes.is_identity();
        let mut mapped: SmallVec<[(ViewId, Selection); 4]> = SmallVec::new();
        if !identity {
            for (id, selection) in &self.selections {
                if id == view {
                    continue;
                }
                match selection.map(&changes) {
                    Ok(s) => mapped.push((id, s)),
                    Err(e) => {
                        // Selections are valid for the text and the change
                        // set fits it, so mapping cannot fail.
                        debug_assert!(false, "valid selection failed to map: {e}");
                        return Err(TransactionError::from(e).into());
                    }
                }
            }
        }
        if let Err(e) = changes.apply(&mut self.text) {
            debug_assert!(false, "change set failed after validation: {e}");
            return Err(TransactionError::from(e).into());
        }
        self.selections[view] = origin;
        for (id, selection) in mapped {
            self.selections[id] = selection;
        }
        if !identity {
            self.revision = self.revision.next();
            self.log.push(self.revision, changes);
        }
        Ok(self.revision)
    }

    /// Maps `positions` (sorted, offsets in the text at revision `from`) to
    /// the current text through the revision log
    /// ([`RevisionLog::map_from`]), and checks that every result is a valid
    /// offset in the current text: [`MapError::Text`] otherwise, which means
    /// an input was not a char boundary at `from`. On error `positions` is
    /// unchanged.
    pub fn map_from(
        &self,
        from: Revision,
        positions: &mut [usize],
        assoc: Assoc,
    ) -> Result<(), MapError> {
        let mut scratch: SmallVec<[usize; 8]> = SmallVec::from_slice(positions);
        self.log
            .map_from(from, self.revision, &mut scratch, assoc)?;
        for &p in &scratch {
            self.text.check_offset(p).map_err(MapError::Text)?;
        }
        positions.copy_from_slice(&scratch);
        Ok(())
    }

    /// Starts tracking a selection for `view`, at offset 0.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by Editor (next commit)"))]
    pub(crate) fn add_view(&mut self, view: ViewId) {
        self.selections.insert(view, Selection::point(0));
    }

    /// Stops tracking `view`; `false` if it was not tracked.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by Editor (next commit)"))]
    pub(crate) fn remove_view(&mut self, view: ViewId) -> bool {
        self.selections.remove(view).is_some()
    }
}

/// Error returned by [`Document::commit`] and [`Document::set_selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitError {
    /// The view does not show this document.
    NoSuchView(ViewId),
    /// The transaction or selection does not fit the text.
    Transaction(TransactionError),
}

impl From<TransactionError> for CommitError {
    fn from(e: TransactionError) -> Self {
        CommitError::Transaction(e)
    }
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchView(v) => write!(f, "view {v:?} does not show this document"),
            Self::Transaction(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transaction(e) => Some(e),
            Self::NoSuchView(_) => None,
        }
    }
}
