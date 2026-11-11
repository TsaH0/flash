//! Flash editing core.
//!
//! Text storage now; the editing algebra, documents, history, and headless
//! editor arrive in later stages. This crate is independent of any terminal,
//! GUI, or plugin runtime.

#![forbid(unsafe_code)]

pub mod text;

pub use text::{Text, TextBuilder, TextError, TextSlice};
