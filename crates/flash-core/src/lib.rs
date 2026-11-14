//! Flash editing core.
//!
//! Text storage and the change set algebra now; documents, history, and the
//! headless editor arrive in later stages. This crate is independent of any terminal,
//! GUI, or plugin runtime.

#![forbid(unsafe_code)]

pub mod changeset;
pub mod motion;
pub mod selection;
pub mod text;
pub mod width;

#[cfg(test)]
mod strategies;

#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzzing;

#[cfg(feature = "internals")]
pub mod internals;

pub use changeset::{Assoc, ChangeSet, ChangeSetBuilder, ChangeSetError, Op};
pub use motion::{Motion, Movement};
pub use selection::{Range, Selection, SelectionError};
pub use text::{Direction, Text, TextBuilder, TextError, TextSlice};
