//! Benchmark-only access to operations that are otherwise crate-private.
//!
//! Compiled only with the `internals` feature. Editor code must not enable
//! it: document text changes go through transactions.

use std::ops::Range;

use crate::changeset::{ChangeSet, ChangeSetError};
use crate::selection::Selection;
use crate::text::{Text, TextError};
use crate::transaction::{Transaction, TransactionError};

pub fn text_insert(text: &mut Text, offset: usize, s: &str) -> Result<(), TextError> {
    text.insert(offset, s)
}

pub fn text_delete(text: &mut Text, range: Range<usize>) -> Result<(), TextError> {
    text.delete(range)
}

pub fn text_replace(text: &mut Text, range: Range<usize>, s: &str) -> Result<(), TextError> {
    text.replace(range, s)
}

/// Validates and applies `c` (`ChangeSet::apply`).
pub fn changeset_apply(c: &ChangeSet, text: &mut Text) -> Result<(), ChangeSetError> {
    c.apply(text)
}

/// Validates and applies `txn` to a bare text and selection
/// (`Transaction::apply`), for benchmarks comparable with Stage 3.
pub fn transaction_apply(
    txn: &Transaction,
    text: &mut Text,
    selection: &mut Selection,
) -> Result<(), TransactionError> {
    txn.apply(text, selection)
}
