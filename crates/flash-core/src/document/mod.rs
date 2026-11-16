//! Documents: text plus the revision state that tracks its changes.
//!
//! A [`Document`] owns its text, its revision counter, the bounded
//! [`RevisionLog`], and the selection of every view showing it.
//! [`Document::commit`] is the only way the text changes (PLAN 2.6, Stage
//! 4): it validates the transaction and every view's new selection first,
//! and only then applies the text, bumps the revision, and logs the change
//! set. Invariant: every view's selection is valid for the text
//! (DECISIONS 014).
//!
//! Every effective commit is also recorded in the document's branching
//! [`History`]. [`Document::undo`], [`Document::redo`], and
//! [`Document::goto`] commit the stored change sets through the same path,
//! so they bump the revision, enter the log, and map other views like any
//! edit (PLAN Stage 5).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use slotmap::SecondaryMap;
use smallvec::SmallVec;

use crate::changeset::{Assoc, ChangeSet, ChangeSetError};
use crate::editor::ViewId;
use crate::selection::Selection;
use crate::text::Text;
use crate::transaction::{Transaction, TransactionError};

mod history;
mod revision;

pub use history::{History, Node, NodeId, Record, Timestamp};
use history::{Path as HistoryPath, Plan};
pub use revision::{MapError, Revision, RevisionLog};

#[cfg(test)]
mod history_proptests;
#[cfg(test)]
mod history_tests;
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
    history: History,
}

/// What a commit does to history.
enum Step {
    /// Record the commit (an ordinary edit).
    Record(Record),
    /// Move the head along a path (undo, redo, jump); record nothing.
    Move(HistoryPath),
}

impl Document {
    /// A document at revision 0 with no views; detects the line ending.
    pub fn new(text: Text, path: Option<PathBuf>) -> Document {
        Document {
            line_ending: LineEnding::detect(&text),
            history: History::new(text.byte_len()),
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

    /// The undo tree.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Sets the longest pause between typing commits of one undo step.
    pub fn set_group_pause(&mut self, pause: Duration) {
        self.history.set_group_pause(pause);
    }

    /// Ends the open undo group: the next commit starts a new undo step.
    pub fn checkpoint(&mut self) {
        self.history.close_group();
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

    /// Replaces the selection of `view` if it is valid for the text. A
    /// selection change ends the open undo group.
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), CommitError> {
        let slot = self
            .selections
            .get_mut(view)
            .ok_or(CommitError::NoSuchView(view))?;
        selection
            .validate(&self.text)
            .map_err(TransactionError::from)?;
        *slot = selection;
        self.history.close_group();
        Ok(())
    }

    /// Commits `txn` on behalf of `view` as its own undo step, at the
    /// latest time history has seen. See [`commit_with`](Self::commit_with).
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, CommitError> {
        let time = self.history.latest_time();
        self.commit_with(view, txn, Record::step(time))
    }

    /// Commits `txn` on behalf of `view`: the only way the text changes.
    ///
    /// Validates the change set and `view`'s new selection (the
    /// transaction's, or the current one mapped), maps every other view's
    /// selection, and prepares the history change, before changing
    /// anything. Then applies the text, stores the selections, and, unless
    /// the change set is the identity, increments the revision, moves the
    /// change set into the log, and records it in history as `record` says
    /// (a new node, or merged into the open group). Returns the document's
    /// revision afterwards. On error nothing changes.
    ///
    /// An identity change set leaves other views' selections (and their goal
    /// columns) untouched and records nothing; it ends the open undo group
    /// unless `record.group` is set. O(ops · log n + Σ k over views), plus
    /// the group's ops when merging.
    pub fn commit_with(
        &mut self,
        view: ViewId,
        txn: Transaction,
        record: Record,
    ) -> Result<Revision, CommitError> {
        self.commit_step(view, txn, Step::Record(record))
    }

    /// Undoes the head node on behalf of `view`: commits its inverse, with
    /// `view` getting the selection stored before the node; other views are
    /// mapped through the inverse. Returns the revision afterwards, or
    /// `None` at the root (nothing changes).
    pub fn undo(&mut self, view: ViewId) -> Result<Option<Revision>, CommitError> {
        self.check_view(view)?;
        let head = self.history.head();
        let node = self.history.head_node();
        let Some(parent) = node.parent() else {
            return Ok(None);
        };
        let txn = Transaction::new(node.inverse().clone(), Some(node.before().clone()));
        let path = HistoryPath {
            up: vec![head],
            down: Vec::new(),
            target: parent,
        };
        self.commit_step(view, txn, Step::Move(path)).map(Some)
    }

    /// Redoes the head's last child (the branch last created or visited) on
    /// behalf of `view`, which gets the selection stored after it. `None`
    /// if the head has no children.
    pub fn redo(&mut self, view: ViewId) -> Result<Option<Revision>, CommitError> {
        self.check_view(view)?;
        let Some(child) = self.history.head_node().last_child() else {
            return Ok(None);
        };
        let node = &self.history.node_store()[child.index()];
        let txn = Transaction::new(node.forward().clone(), Some(node.after().clone()));
        let path = HistoryPath {
            up: Vec::new(),
            down: vec![child],
            target: child,
        };
        self.commit_step(view, txn, Step::Move(path)).map(Some)
    }

    /// Moves to node `target` on behalf of `view`: the inverses up to the
    /// common ancestor and the forwards down to `target`, composed into one
    /// change set and committed once (one revision; atomic). `view` gets
    /// the selection undo or redo of the last step would give it. `None` if
    /// `target` is the head.
    pub fn goto(&mut self, view: ViewId, target: NodeId) -> Result<Option<Revision>, CommitError> {
        self.check_view(view)?;
        if self.history.node(target).is_none() {
            return Err(CommitError::NoSuchNode(target));
        }
        if target == self.history.head() {
            return Ok(None);
        }
        let path = self.history.path_to(target);
        let nodes = self.history.node_store();
        let steps = path
            .up
            .iter()
            .map(|u| nodes[u.index()].inverse())
            .chain(path.down.iter().map(|d| nodes[d.index()].forward()));
        let mut composed: Option<ChangeSet> = None;
        for c in steps {
            composed = Some(match composed {
                None => c.clone(),
                Some(acc) => acc.compose(c).map_err(unreachable_algebra)?,
            });
        }
        let selection = match (path.down.last(), path.up.last()) {
            (Some(d), _) => nodes[d.index()].after(),
            (None, Some(u)) => nodes[u.index()].before(),
            (None, None) => return Ok(None),
        }
        .clone();
        let Some(changes) = composed else {
            return Ok(None);
        };
        let txn = Transaction::new(changes, Some(selection));
        self.commit_step(view, txn, Step::Move(path)).map(Some)
    }

    /// The commit machinery shared by edits and history moves.
    fn commit_step(
        &mut self,
        view: ViewId,
        txn: Transaction,
        step: Step,
    ) -> Result<Revision, CommitError> {
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
        let plan = match step {
            Step::Move(path) => Plan::Move(path),
            Step::Record(record) if identity => Plan::Nothing {
                keep_open: record.group,
            },
            Step::Record(record) => self.plan_record(view, current, &origin, &changes, record)?,
        };
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
        self.history.apply(plan);
        Ok(self.revision)
    }

    /// The history change for recording `changes` (valid for the current
    /// text, not the identity) from `view`, whose selection goes from
    /// `before` to `after`. Pure: computes the inverse against the text
    /// before it changes, and the compositions for a merge.
    fn plan_record(
        &self,
        view: ViewId,
        before: &Selection,
        after: &Selection,
        changes: &ChangeSet,
        record: Record,
    ) -> Result<Plan, CommitError> {
        let inverse = changes.invert(&self.text).map_err(unreachable_algebra)?;
        let time = self.history.clamp(record.time);
        if self.history.merges(view, record) {
            let head = self.history.head_node();
            let forward = head
                .forward()
                .compose(changes)
                .map_err(unreachable_algebra)?;
            let inverse = inverse
                .compose(head.inverse())
                .map_err(unreachable_algebra)?;
            Ok(Plan::Merge {
                forward,
                inverse,
                after: after.clone(),
                time,
            })
        } else {
            Ok(Plan::Push {
                forward: changes.clone(),
                inverse,
                before: before.clone(),
                after: after.clone(),
                time,
                view,
                group: record.group,
            })
        }
    }

    fn check_view(&self, view: ViewId) -> Result<(), CommitError> {
        if self.selections.contains_key(view) {
            Ok(())
        } else {
            Err(CommitError::NoSuchView(view))
        }
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
    pub(crate) fn add_view(&mut self, view: ViewId) {
        self.selections.insert(view, Selection::point(0));
    }

    /// Stops tracking `view`; `false` if it was not tracked. Ends the open
    /// undo group if it was `view`'s.
    pub(crate) fn remove_view(&mut self, view: ViewId) -> bool {
        if self.history.open_group() == Some(view) {
            self.history.close_group();
        }
        self.selections.remove(view).is_some()
    }
}

/// Converts an error that the history's construction rules out: inverting
/// a validated change set, or composing change sets whose lengths chain.
fn unreachable_algebra(e: ChangeSetError) -> CommitError {
    debug_assert!(false, "history algebra failed: {e}");
    CommitError::Transaction(TransactionError::ChangeSet(e))
}

/// Error returned by [`Document::commit`] and [`Document::set_selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitError {
    /// The view does not show this document.
    NoSuchView(ViewId),
    /// No history node has this ID.
    NoSuchNode(NodeId),
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
            Self::NoSuchNode(n) => write!(f, "no history node {}", n.index()),
            Self::Transaction(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transaction(e) => Some(e),
            Self::NoSuchView(_) | Self::NoSuchNode(_) => None,
        }
    }
}
