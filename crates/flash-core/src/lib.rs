//! Flash editing core.
//!
//! Text storage and the change set algebra now; documents, history, and the
//! headless editor arrive in later stages. This crate is independent of any terminal,
//! GUI, or plugin runtime.

#![forbid(unsafe_code)]

pub mod changeset;
pub mod document;
pub mod editor;
pub mod motion;
pub mod selection;
pub mod text;
pub mod trace;
pub mod transaction;
pub mod width;

#[cfg(test)]
mod strategies;

#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzzing;

#[cfg(feature = "internals")]
pub mod internals;

pub use changeset::{Assoc, ChangeSet, ChangeSetBuilder, ChangeSetError, Op};
pub use document::{CommitError, Document, LineEnding, MapError, Revision, RevisionLog};
pub use editor::{
    DocumentId, Editor, EditorError, Effects, Event, KeyCode, KeyEvent, Modifiers, View, ViewId,
};
pub use motion::{Motion, Movement};
pub use selection::{Range, Selection, SelectionError};
pub use text::{Direction, Text, TextBuilder, TextError, TextSlice};
pub use transaction::{Transaction, TransactionError};
