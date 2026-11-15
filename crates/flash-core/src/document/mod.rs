//! Documents: text plus the revision state that tracks its changes.

mod revision;

pub use revision::{MapError, Revision, RevisionLog};

#[cfg(test)]
mod tests;
