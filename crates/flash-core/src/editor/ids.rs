//! Generational IDs for documents and views.
//!
//! A key from a closed document or view never matches a later one, even
//! when its slot is reused, so a stale ID is rejected rather than aliased.

slotmap::new_key_type! {
    /// Identifies an open document in an [`Editor`](super::Editor).
    pub struct DocumentId;
    /// Identifies an open view in an [`Editor`](super::Editor).
    pub struct ViewId;
}
