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

use std::convert::Infallible;
use std::fmt;
use std::path::PathBuf;

use slotmap::SlotMap;

use crate::document::{CommitError, Document, LineEnding, Revision};
use crate::motion::{Motion, Movement};
use crate::selection::{Range, Selection};
use crate::text::{Direction, Text, TextError};
use crate::transaction::{Transaction, TransactionError};

mod event;
mod ids;
mod keys;

pub use event::{Event, KeyCode, KeyEvent, Modifiers};
pub use ids::{DocumentId, ViewId};

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
    /// The editor should exit.
    pub quit: bool,
}

impl Effects {
    /// Whether anything visible changed.
    pub fn needs_redraw(&self) -> bool {
        self.text || self.selection || self.scroll || self.resize || self.message
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
}

impl Editor {
    /// An editor with no documents, for a `width` × `height` screen.
    pub fn new(width: u16, height: u16) -> Editor {
        Editor {
            documents: SlotMap::with_key(),
            views: SlotMap::with_key(),
            focus: None,
            screen: (width, height),
            message: String::new(),
            quit: false,
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

    /// Commits `txn` to the document of `view`, on its behalf
    /// ([`Document::commit`]). Returns the document's revision afterwards.
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, EditorError> {
        let revision = self
            .document_of_mut(view)?
            .commit(view, txn)
            .map_err(EditorError::Commit)?;
        if let Some(focus) = self.focus {
            self.scroll(focus);
        }
        Ok(revision)
    }

    /// The status message: the last command's error, or empty.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The screen size, `(width, height)`.
    pub fn screen(&self) -> (u16, u16) {
        self.screen
    }

    /// Applies one input event to the focused view (resizes and Ctrl+Q also
    /// without one) and scrolls the focused view so its primary cursor's
    /// line is visible. Never panics; a failed command changes nothing and
    /// sets the message.
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
            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers,
            }) if modifiers.ctrl => {
                self.quit = true;
                Effects {
                    quit: true,
                    ..Effects::default()
                }
            }
            Event::Key(key) => match self.focus {
                Some(view) => keys::handle_key(self, view, *key),
                None => Effects::default(),
            },
            Event::Paste { text } => match self.focus {
                Some(view) => self.edit(view, |t, sel, _| Transaction::insert(t, sel, text)),
                None => Effects::default(),
            },
        };
        if let Some(view) = self.focus {
            fx.scroll |= self.scroll(view);
        }
        fx
    }

    // ---- commands used by the key map ----

    /// Builds a transaction for `view` with `make` and commits it.
    pub(crate) fn edit(
        &mut self,
        view: ViewId,
        make: impl FnOnce(&Text, &Selection, LineEnding) -> Result<Transaction, TransactionError>,
    ) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        let before = doc.revision();
        let result = match doc.selection(view) {
            Some(sel) => make(doc.text(), sel, doc.line_ending())
                .map_err(CommitError::from)
                .and_then(|txn| doc.commit(view, txn)),
            None => Err(CommitError::NoSuchView(view)),
        };
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
        self.reselect(view, |text, sel| {
            let ranges = sel.ranges();
            let from = match direction {
                Direction::Forward => ranges[ranges.len() - 1],
                Direction::Backward => ranges[0],
            };
            let motion = Motion::Vertical {
                count: 1,
                direction,
                tab_width: keys::TAB,
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

    /// Replaces `view`'s selection with `make`'s result, validated.
    fn reselect<E: fmt::Display>(
        &mut self,
        view: ViewId,
        make: impl FnOnce(&Text, &Selection) -> Result<Selection, E>,
    ) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
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

    /// Focus the first remaining view if the focused one is gone.
    fn refocus(&mut self) {
        if self.focus.is_some_and(|v| !self.views.contains_key(v)) {
            self.focus = self.views.keys().next();
            if let Some(view) = self.focus {
                self.scroll(view);
            }
        }
    }

    /// Scrolls `view` by the least amount that shows its primary head's
    /// line. Returns whether `top` changed. O(log n).
    fn scroll(&mut self, view: ViewId) -> bool {
        let Some(v) = self.views.get(view) else {
            return false;
        };
        let line = self
            .documents
            .get(v.doc)
            .and_then(|d| Some((d, d.selection(view)?)))
            .and_then(|(d, sel)| d.text().line_of_offset(sel.primary().head()).ok());
        let (Some(line), Some(v)) = (line, self.views.get_mut(view)) else {
            return false;
        };
        let height = usize::from(v.height);
        let top = if height == 0 {
            v.top
        } else if line < v.top {
            line
        } else if line >= v.top + height {
            line + 1 - height
        } else {
            v.top
        };
        let changed = top != v.top;
        v.top = top;
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
