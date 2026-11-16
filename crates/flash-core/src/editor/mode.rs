//! Modes (PLAN 2.5, Stage 7). Task 5 fills this in; the ID exists so
//! hooks can name modes.

/// A registered mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModeId(pub(crate) u32);
