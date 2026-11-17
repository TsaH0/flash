//! The terminal frontend over the headless editor (PLAN 2.2, 2.9).
//!
//! - [`input`]: crossterm events to core [`flash_core::Event`]s.
//! - [`render`]: frames of the focused view, status line, and prompt.
//! - [`osc52`]: the system clipboard escape.
//! - [`terminal`]: raw mode, alternate screen, bracketed paste, and their
//!   restoration (on drop, from a panic hook, around a suspend).
//!
//! Text from a file is never written to the terminal raw: control
//! characters are drawn as U+FFFD and bidirectional overrides are dropped
//! (DECISIONS 012), so a file cannot inject escape sequences.

#![forbid(unsafe_code)]

pub mod emit;
pub mod grid;
pub mod input;
pub mod osc52;
pub mod render;
pub mod terminal;

pub use input::convert;
pub use emit::Strategy;
pub use render::Renderer;
