//! The headless editor: documents and views behind generational IDs.
//!
//! [`Editor`] owns every open [`Document`] and [`View`]. Frontends feed it
//! raw input through [`Editor::handle`], which returns the [`Effects`] the
//! frontend must act on (redraw, quit); everything else is read through
//! accessors. Text changes only through [`Document::commit`] (via
//! [`Editor::commit`] or the key map), so every view of a document stays
//! valid (DECISIONS 014).
//!
//! The layout is the skeleton's, and temporary: each view fills the
//! screen except its last row, which the frontend uses for a status line.
//! Only the focused view is shown and scrolled.
//!
//! Time is an input ([`Editor::handle_at`]): the editor never reads a
//! clock for state, so replaying a trace with its recorded times groups
//! undo steps exactly as the session did. Typing keys commit as one undo
//! group until a pause, a motion, a paste, or an undo (PLAN Stage 5).
//!
//! The editor performs no I/O. Saving, opening, the system clipboard, and
//! suspending are [`Request`]s the frontend carries out (Stage 6); a
//! one-line [`Prompt`] asks for file names and confirmations.

use std::convert::Infallible;
use std::fmt;
use std::num::NonZeroU8;
use std::path::PathBuf;
use std::time::Instant;

use slotmap::SlotMap;

use crate::document::{CommitError, Document, LineEnding, NodeId, Record, Revision, Timestamp};
use crate::motion::{Motion, Movement};
use crate::selection::{Range, Selection};
use crate::text::{Direction, Text, TextError};
use crate::transaction::{Transaction, TransactionError};

mod column;
mod commands;
mod event;
mod ids;
mod keys;
mod prompt;

pub use column::LineAnchor;
pub use commands::Request;
pub use event::{Event, KeyCode, KeyEvent, Modifiers};
pub use ids::{DocumentId, ViewId};
pub use prompt::{Prompt, PromptKind};

#[cfg(test)]
mod commands_tests;
#[cfg(test)]
pub(crate) mod proptests;
#[cfg(test)]
mod tests;

/// A window onto one document. Its selection lives in the document
/// (DECISIONS 014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    doc: DocumentId,
    /// First visible line.
    top: usize,
    /// First visible display column (horizontal scroll).
    left: usize,
    anchor: Option<LineAnchor>,
    /// Text area in cells (the screen minus the status row).
    width: u16,
    height: u16,
}

impl View {
    pub fn doc(&self) -> DocumentId {
        self.doc
    }

    /// The first visible line.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The first visible display column.
    pub fn left(&self) -> usize {
        self.left
    }

    /// The primary head's column and where its line's visible part starts,
    /// as of the last scroll (check its fields before use).
    pub fn anchor(&self) -> Option<&LineAnchor> {
        self.anchor.as_ref()
    }

    /// The text area, `(width, height)` in cells.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

/// What handling one event changed, so the frontend knows what to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects {
    /// The focused view's document text changed.
    pub text: bool,
    /// The focused view's selection changed.
    pub selection: bool,
    /// The focused view scrolled.
    pub scroll: bool,
    /// The screen size changed.
    pub resize: bool,
    /// The status message changed.
    pub message: bool,
    /// The prompt opened, changed, or closed.
    pub prompt: bool,
    /// Requests are waiting ([`Editor::take_requests`]).
    pub requests: bool,
    /// The editor should exit.
    pub quit: bool,
}

impl Effects {
    /// Whether anything visible changed.
    pub fn needs_redraw(&self) -> bool {
        self.text || self.selection || self.scroll || self.resize || self.message || self.prompt
    }
}

/// All editing state: open documents, views, focus, screen size.
#[derive(Debug, Clone)]
pub struct Editor {
    documents: SlotMap<DocumentId, Document>,
    views: SlotMap<ViewId, View>,
    focus: Option<ViewId>,
    screen: (u16, u16),
    message: String,
    quit: bool,
    /// The time of the event being handled (the last one given).
    now: Timestamp,
    prompt: Option<Prompt>,
    /// Copied text, one piece per range.
    register: Vec<String>,
    requests: Vec<Request>,
    tab_width: NonZeroU8,
    /// Wall time of the last commit, for instrumentation only.
    last_commit_ns: u64,
}

impl Editor {
    /// Columns per tab stop unless set otherwise (an option from Stage 7).
    pub const DEFAULT_TAB_WIDTH: NonZeroU8 = NonZeroU8::new(4).unwrap();

    /// An editor with no documents, for a `width` × `height` screen.
    pub fn new(width: u16, height: u16) -> Editor {
        Editor {
            documents: SlotMap::with_key(),
            views: SlotMap::with_key(),
            focus: None,
            screen: (width, height),
            message: String::new(),
            quit: false,
            now: Timestamp::ZERO,
            prompt: None,
            register: Vec::new(),
            requests: Vec::new(),
            tab_width: Self::DEFAULT_TAB_WIDTH,
            last_commit_ns: 0,
        }
    }

    /// Opens `text` as a new document with no views.
    pub fn open(&mut self, text: Text, path: Option<PathBuf>) -> DocumentId {
        self.documents.insert(Document::new(text, path))
    }

    /// Opens a view of `doc` with a cursor at 0 and focuses it.
    pub fn open_view(&mut self, doc: DocumentId) -> Result<ViewId, EditorError> {
        let (width, height) = self.view_area();
        let document = self
            .documents
            .get_mut(doc)
            .ok_or(EditorError::NoSuchDocument(doc))?;
        let view = self.views.insert(View {
            doc,
            top: 0,
            left: 0,
            anchor: None,
            width,
            height,
        });
        document.add_view(view);
        self.focus = Some(view);
        Ok(view)
    }

    /// Closes `view`. If it was focused, focus moves to another view, or to
    /// none. Its document stays open.
    pub fn close_view(&mut self, view: ViewId) -> Result<(), EditorError> {
        let removed = self
            .views
            .remove(view)
            .ok_or(EditorError::NoSuchView(view))?;
        if let Some(doc) = self.documents.get_mut(removed.doc) {
            doc.remove_view(view);
        }
        self.refocus();
        Ok(())
    }

    /// Closes `doc` and every view of it.
    pub fn close_document(&mut self, doc: DocumentId) -> Result<(), EditorError> {
        let removed = self
            .documents
            .remove(doc)
            .ok_or(EditorError::NoSuchDocument(doc))?;
        for view in removed.views() {
            self.views.remove(view);
        }
        self.refocus();
        Ok(())
    }

    /// Focuses `view` and scrolls it to its primary cursor.
    pub fn set_focus(&mut self, view: ViewId) -> Result<(), EditorError> {
        if !self.views.contains_key(view) {
            return Err(EditorError::NoSuchView(view));
        }
        self.focus = Some(view);
        self.scroll(view);
        Ok(())
    }

    pub fn focus(&self) -> Option<ViewId> {
        self.focus
    }

    pub fn document(&self, doc: DocumentId) -> Option<&Document> {
        self.documents.get(doc)
    }

    pub fn documents(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.documents.iter()
    }

    pub fn view(&self, view: ViewId) -> Option<&View> {
        self.views.get(view)
    }

    pub fn views(&self) -> impl Iterator<Item = (ViewId, &View)> {
        self.views.iter()
    }

    /// `view` and the document it shows.
    pub fn view_document(&self, view: ViewId) -> Option<(&View, &Document)> {
        let v = self.views.get(view)?;
        Some((v, self.documents.get(v.doc)?))
    }

    /// The selection of `view`.
    pub fn selection(&self, view: ViewId) -> Option<&Selection> {
        self.view_document(view)?.1.selection(view)
    }

    /// Replaces the selection of `view` if it is valid for its text.
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), EditorError> {
        self.document_of_mut(view)?
            .set_selection(view, selection)
            .map_err(EditorError::Commit)?;
        self.scroll(view);
        Ok(())
    }

    /// Commits `txn` to the document of `view`, on its behalf, as its own
    /// undo step at the current time ([`Document::commit_with`]). Returns
    /// the document's revision afterwards.
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, EditorError> {
        let record = Record::step(self.now);
        let revision = self
            .document_of_mut(view)?
            .commit_with(view, txn, record)
            .map_err(EditorError::Commit)?;
        self.scroll_focus();
        Ok(revision)
    }

    /// Undoes the last change to `view`'s document, on its behalf
    /// ([`Document::undo`]). `None` if there is nothing to undo.
    pub fn undo(&mut self, view: ViewId) -> Result<Option<Revision>, EditorError> {
        let result = self.document_of_mut(view)?.undo(view);
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Redoes the change last undone in `view`'s document
    /// ([`Document::redo`]). `None` if there is nothing to redo.
    pub fn redo(&mut self, view: ViewId) -> Result<Option<Revision>, EditorError> {
        let result = self.document_of_mut(view)?.redo(view);
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Moves `view`'s document to history node `node` ([`Document::goto`]).
    pub fn goto(&mut self, view: ViewId, node: NodeId) -> Result<Option<Revision>, EditorError> {
        let result = self.document_of_mut(view)?.goto(view, node);
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Ends the open undo group of `view`'s document.
    pub fn checkpoint(&mut self, view: ViewId) -> Result<(), EditorError> {
        self.document_of_mut(view)?.checkpoint();
        Ok(())
    }

    /// The time of the event being handled: the last one given to
    /// [`handle_at`](Self::handle_at).
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// The open prompt, if any.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The register: the pieces last copied or cut.
    pub fn register(&self) -> &[String] {
        &self.register
    }

    pub fn tab_width(&self) -> NonZeroU8 {
        self.tab_width
    }

    /// Sets the tab width used by motions, scrolling, and rendering.
    pub fn set_tab_width(&mut self, width: NonZeroU8) {
        self.tab_width = width;
        if let Some(view) = self.focus {
            self.scroll(view);
        }
    }

    /// Nanoseconds the last key-driven commit took (instrumentation; not
    /// editor state, and not reproduced by replay).
    pub fn last_commit_ns(&self) -> u64 {
        self.last_commit_ns
    }

    /// The status message: the last command's error, or empty.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Shows `message` on the status line (the frontend reporting on a
    /// request, e.g. a copy too large for the system clipboard).
    pub fn notify(&mut self, message: impl fmt::Display) -> Effects {
        Effects {
            message: self.report(Some(message)),
            ..Effects::default()
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The screen size, `(width, height)`.
    pub fn screen(&self) -> (u16, u16) {
        self.screen
    }

    /// [`handle`](Self::handle)s `event` as happening at `now` (nanoseconds
    /// since an epoch the frontend chooses; a trace's `t_ns`). Undo groups
    /// end when typing pauses for longer than the document's group pause.
    pub fn handle_at(&mut self, event: &Event, now: Timestamp) -> Effects {
        self.now = now;
        self.handle(event)
    }

    /// Applies one input event to the focused view (resizes and Ctrl+Q also
    /// without one) and scrolls the focused view so its primary cursor's
    /// line is visible. Never panics; a failed command changes nothing and
    /// sets the message. The event happens at the last time given to
    /// [`handle_at`](Self::handle_at).
    pub fn handle(&mut self, event: &Event) -> Effects {
        let mut fx = match event {
            Event::Resize { width, height } => {
                self.screen = (*width, *height);
                let (w, h) = self.view_area();
                for view in self.views.values_mut() {
                    (view.width, view.height) = (w, h);
                }
                Effects {
                    resize: true,
                    ..Effects::default()
                }
            }
            _ if self.prompt.is_some() => self.prompt_event(event),
            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers,
            }) if modifiers.ctrl => self.quit_command(),
            Event::Key(key) => match self.focus {
                Some(view) => keys::handle_key(self, view, *key),
                None => Effects::default(),
            },
            // A paste is one transaction and its own undo step (PLAN 2.9).
            Event::Paste { text } => match self.focus {
                Some(view) => self.edit(view, false, |t, sel, _| Transaction::insert(t, sel, text)),
                None => Effects::default(),
            },
        };
        if let Some(view) = self.focus {
            fx.scroll |= self.scroll(view);
        }
        fx.requests = !self.requests.is_empty();
        fx
    }

    // ---- commands used by the key map ----

    /// Builds a transaction for `view` with `make` and commits it at the
    /// current time; `group`: as typing, which may join the open undo group.
    pub(crate) fn edit(
        &mut self,
        view: ViewId,
        group: bool,
        make: impl FnOnce(&Text, &Selection, LineEnding) -> Result<Transaction, TransactionError>,
    ) -> Effects {
        let record = Record {
            time: self.now,
            group,
        };
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        let before = doc.revision();
        let mut commit_ns = None;
        let result = match doc.selection(view) {
            Some(sel) => make(doc.text(), sel, doc.line_ending())
                .map_err(CommitError::from)
                .and_then(|txn| {
                    let start = Instant::now();
                    let r = doc.commit_with(view, txn, record);
                    commit_ns = Some(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    r
                }),
            None => Err(CommitError::NoSuchView(view)),
        };
        if let Some(ns) = commit_ns {
            self.last_commit_ns = ns;
        }
        let changed = result.as_ref().is_ok_and(|&after| after != before);
        Effects {
            text: changed,
            selection: changed,
            message: self.report(result.err()),
            ..Effects::default()
        }
    }

    /// Applies `motion` to every range of `view`'s selection.
    pub(crate) fn motion(&mut self, view: ViewId, motion: Motion, movement: Movement) -> Effects {
        self.reselect(view, |text, sel| motion.apply(text, sel, movement))
    }

    /// Adds a cursor one line below the last range (or above the first), at
    /// its goal column, as the new primary.
    pub(crate) fn add_cursor(&mut self, view: ViewId, direction: Direction) -> Effects {
        let tab_width = self.tab_width;
        self.reselect(view, |text, sel| {
            let ranges = sel.ranges();
            let from = match direction {
                Direction::Forward => ranges[ranges.len() - 1],
                Direction::Backward => ranges[0],
            };
            let motion = Motion::Vertical {
                count: 1,
                direction,
                tab_width,
            };
            let start = Range::point(from.head()).with_goal_column(from.goal_column());
            let range = motion.apply_range(text, start, Movement::Move)?;
            Ok::<_, TextError>(sel.with_range(range))
        })
    }

    /// Keeps only a cursor at the primary head.
    pub(crate) fn collapse_to_primary(&mut self, view: ViewId) -> Effects {
        self.reselect(view, |_, sel| {
            Ok::<_, Infallible>(Selection::point(sel.primary().head()))
        })
    }

    /// Undo (`redo` false) or redo for the key map: reports a change, or
    /// that there is nothing to do, in the message.
    pub(crate) fn history_step(&mut self, view: ViewId, redo: bool) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        let before = doc.revision();
        let result = if redo { doc.redo(view) } else { doc.undo(view) };
        let (moved, changed) = match result {
            Ok(Some(after)) => (true, after != before),
            _ => (false, false),
        };
        let message = match result {
            Ok(Some(_)) => None,
            Ok(None) if redo => Some("already at the newest change".to_owned()),
            Ok(None) => Some("already at the oldest change".to_owned()),
            Err(e) => Some(e.to_string()),
        };
        Effects {
            text: changed,
            selection: moved,
            message: self.report(message),
            ..Effects::default()
        }
    }

    /// Replaces `view`'s selection with `make`'s result, validated. Ends
    /// the open undo group even if the selection does not change: a motion
    /// is a boundary.
    fn reselect<E: fmt::Display>(
        &mut self,
        view: ViewId,
        make: impl FnOnce(&Text, &Selection) -> Result<Selection, E>,
    ) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        doc.checkpoint();
        let Some(current) = doc.selection(view) else {
            return Effects::default();
        };
        let result = match make(doc.text(), current) {
            Ok(next) if next == *current => Ok(false),
            Ok(next) => doc
                .set_selection(view, next)
                .map(|()| true)
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let changed = result.as_ref().is_ok_and(|&c| c);
        Effects {
            selection: changed,
            message: self.report(result.err()),
            ..Effects::default()
        }
    }

    /// Sets the message to `error`, or clears it. Returns whether it changed.
    fn report(&mut self, error: Option<impl fmt::Display>) -> bool {
        let next = error.map(|e| e.to_string()).unwrap_or_default();
        let changed = next != self.message;
        self.message = next;
        changed
    }

    // ---- internals ----

    fn document_of_mut(&mut self, view: ViewId) -> Result<&mut Document, EditorError> {
        let doc = self
            .views
            .get(view)
            .ok_or(EditorError::NoSuchView(view))?
            .doc;
        self.documents
            .get_mut(doc)
            .ok_or(EditorError::NoSuchDocument(doc))
    }

    /// The text area of a view on the current screen: all but the status row.
    fn view_area(&self) -> (u16, u16) {
        (self.screen.0, self.screen.1.saturating_sub(1))
    }

    fn scroll_focus(&mut self) {
        if let Some(focus) = self.focus {
            self.scroll(focus);
        }
    }

    /// Focus the first remaining view if the focused one is gone.
    fn refocus(&mut self) {
        if self.focus.is_some_and(|v| !self.views.contains_key(v)) {
            self.focus = self.views.keys().next();
            if let Some(view) = self.focus {
                self.scroll(view);
            }
        }
    }

    /// Scrolls `view` by the least amount that shows its primary head
    /// (line and column), and records the view's [`LineAnchor`]. Returns
    /// whether `top` or `left` changed. O(log n + view width + distance
    /// from the nearest still-valid checkpoint of the previous anchor, or
    /// from the line start; see the `column` module).
    fn scroll(&mut self, view: ViewId) -> bool {
        let tab = self.tab_width;
        let Some(v) = self.views.get(view) else {
            return false;
        };
        let Some(doc) = self.documents.get(v.doc) else {
            return false;
        };
        let Some(head) = doc.selection(view).map(|s| s.primary().head()) else {
            return false;
        };
        let Some(column::Located {
            line,
            line_start,
            column,
            rear,
            old_start,
        }) = column::locate(doc, v.anchor.as_ref(), head, tab)
        else {
            return false;
        };
        let (width, height) = (usize::from(v.width), usize::from(v.height));
        let left = if width == 0 {
            v.left
        } else if column < v.left {
            column
        } else if column >= v.left + width {
            column + 1 - width
        } else {
            v.left
        };
        let top = if height == 0 {
            v.top
        } else if line < v.top {
            line
        } else if line >= v.top + height {
            line + 1 - height
        } else {
            v.top
        };
        let anchor = LineAnchor {
            revision: doc.revision(),
            head,
            line,
            line_start,
            head_column: column,
            head_on_boundary: doc.text().is_grapheme_boundary(head),
            tab_width: tab,
            // Unscrolled (most lines): the visible part starts the line.
            // Scrolled by less than a view width: forward from the
            // previous start, if still valid. Else back from the head.
            start: if left == 0 {
                Some((line_start, 0))
            } else if let Some(s) = old_start
                .filter(|&(o, c)| o <= head && left.checked_sub(c).is_some_and(|d| d <= width))
            {
                column::line_start_from(doc.text(), s, head, left, tab)
            } else {
                column::line_start_at(doc.text(), head, column, left)
            },
            rear,
        };
        let Some(v) = self.views.get_mut(view) else {
            return false;
        };
        let changed = top != v.top || left != v.left;
        v.anchor = Some(anchor);
        v.top = top;
        v.left = left;
        changed
    }
}

/// Error returned by [`Editor`] methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditorError {
    /// No open document has this ID (it was closed, or never existed).
    NoSuchDocument(DocumentId),
    /// No open view has this ID.
    NoSuchView(ViewId),
    /// The commit or selection change was rejected.
    Commit(CommitError),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchDocument(d) => write!(f, "no such document: {d:?}"),
            Self::NoSuchView(v) => write!(f, "no such view: {v:?}"),
            Self::Commit(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for EditorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Commit(e) => Some(e),
            _ => None,
        }
    }
}
