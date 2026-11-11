//! Benchmark-only access to operations that are otherwise crate-private.
//!
//! Compiled only with the `internals` feature. Editor code must not enable
//! it: document text changes go through transactions.

use std::ops::Range;

use crate::text::{Text, TextError};

pub fn text_insert(text: &mut Text, offset: usize, s: &str) -> Result<(), TextError> {
    text.insert(offset, s)
}

pub fn text_delete(text: &mut Text, range: Range<usize>) -> Result<(), TextError> {
    text.delete(range)
}

pub fn text_replace(text: &mut Text, range: Range<usize>, s: &str) -> Result<(), TextError> {
    text.replace(range, s)
}
