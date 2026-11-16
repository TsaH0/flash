//! Flash editing core.
//!
//! Text storage, the change set algebra, selections and transactions,
//! documents with branching history, and the headless editor. This crate is
//! independent of any terminal, GUI, or plugin runtime.

#![forbid(unsafe_code)]

pub mod changeset;
pub mod document;
pub mod editor;
pub mod file;
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
pub use document::{
    CommitError, Document, History, LineEnding, MapError, Node, NodeId, Record, Revision,
    RevisionLog, Timestamp,
};
pub use editor::{
    CommandError, CommandId, CommandInfo, Commands, Context, DocumentId, Editor, EditorError,
    Effects, Event, KeyCode, KeyEvent, LineAnchor, Modifiers, OptionError, OptionId, OptionInfo,
    OptionKind, OptionValue, Options, Owner, Prompt, PromptKind, Request, Scope, View, ViewId,
    ViewOptions,
};
pub use motion::{Motion, Movement};
pub use selection::{Range, Selection, SelectionError};
pub use text::{Direction, Text, TextBuilder, TextError, TextSlice};
pub use transaction::{Transaction, TransactionError};
