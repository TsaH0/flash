//! `String`-backed reference model for [`Text`](super::Text).
//!
//! Deliberately naive: every query scans the whole string. Its only job is to
//! be obviously correct so the rope-backed implementation can be checked
//! against it. It implements the same documented line model and returns the
//! same errors, so results can be compared with `==`.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::{Direction, TextError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RefText {
    pub(crate) s: String,
}

impl RefText {
    pub(crate) fn new(s: &str) -> Self {
        Self { s: s.to_owned() }
    }

    pub(crate) fn len(&self) -> usize {
        self.s.len()
    }

    pub(crate) fn is_char_boundary(&self, offset: usize) -> bool {
        offset <= self.s.len() && self.s.is_char_boundary(offset)
    }

    pub(crate) fn check_offset(&self, offset: usize) -> Result<(), TextError> {
        if offset > self.s.len() {
            return Err(TextError::OffsetOutOfBounds {
                offset,
                len: self.s.len(),
            });
        }
        if !self.s.is_char_boundary(offset) {
            return Err(TextError::NotCharBoundary { offset });
        }
        Ok(())
    }

    pub(crate) fn check_range(&self, range: &Range<usize>) -> Result<(), TextError> {
        if range.start > range.end {
            return Err(TextError::InvertedRange {
                start: range.start,
                end: range.end,
            });
        }
        self.check_offset(range.end)?;
        self.check_offset(range.start)
    }

    pub(crate) fn is_grapheme_boundary(&self, offset: usize) -> bool {
        if !self.is_char_boundary(offset) {
            return false;
        }
        offset == self.s.len() || self.s.grapheme_indices(true).any(|(i, _)| i == offset)
    }

    /// Every grapheme boundary in increasing order: each cluster start, then `len`.
    pub(crate) fn grapheme_boundaries(&self) -> Vec<usize> {
        let mut bounds: Vec<usize> = self.s.grapheme_indices(true).map(|(i, _)| i).collect();
        bounds.push(self.s.len());
        bounds
    }

    pub(crate) fn move_graphemes(
        &self,
        offset: usize,
        count: usize,
        direction: Direction,
    ) -> Result<usize, TextError> {
        self.check_offset(offset)?;
        Ok(move_over_boundaries(
            &self.grapheme_boundaries(),
            offset,
            count,
            direction,
        ))
    }

    /// Start offset of every line: 0, then one past each '\n'.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(self.s.match_indices('\n').map(|(i, _)| i + 1));
        starts
    }

    pub(crate) fn line_count(&self) -> usize {
        self.line_starts().len()
    }

    pub(crate) fn line_of_offset(&self, offset: usize) -> Result<usize, TextError> {
        self.check_offset(offset)?;
        Ok(self.s[..offset].matches('\n').count())
    }

    pub(crate) fn line_start(&self, line: usize) -> Result<usize, TextError> {
        let starts = self.line_starts();
        starts.get(line).copied().ok_or(TextError::LineOutOfBounds {
            line,
            line_count: starts.len(),
        })
    }

    pub(crate) fn line_range_with_terminator(
        &self,
        line: usize,
    ) -> Result<Range<usize>, TextError> {
        let starts = self.line_starts();
        let start = self.line_start(line)?;
        let end = starts.get(line + 1).copied().unwrap_or(self.s.len());
        Ok(start..end)
    }

    pub(crate) fn line_range(&self, line: usize) -> Result<Range<usize>, TextError> {
        let full = self.line_range_with_terminator(line)?;
        let text = &self.s[full.clone()];
        let content_len = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text)
            .len();
        Ok(full.start..full.start + content_len)
    }

    pub(crate) fn insert(&mut self, offset: usize, s: &str) -> Result<(), TextError> {
        self.check_offset(offset)?;
        self.s.insert_str(offset, s);
        Ok(())
    }

    pub(crate) fn delete(&mut self, range: Range<usize>) -> Result<(), TextError> {
        self.check_range(&range)?;
        self.s.replace_range(range, "");
        Ok(())
    }

    pub(crate) fn replace(&mut self, range: Range<usize>, s: &str) -> Result<(), TextError> {
        self.check_range(&range)?;
        self.s.replace_range(range, s);
        Ok(())
    }
}

/// Steps `count` times to the nearest boundary strictly after (or before)
/// the current offset, stopping when there is none. `bounds` is sorted.
pub(crate) fn move_over_boundaries(
    bounds: &[usize],
    offset: usize,
    count: usize,
    direction: Direction,
) -> usize {
    let mut p = offset;
    for _ in 0..count {
        let next = match direction {
            Direction::Forward => bounds.get(bounds.partition_point(|&b| b <= p)),
            Direction::Backward => bounds[..bounds.partition_point(|&b| b < p)].last(),
        };
        match next {
            Some(&b) => p = b,
            None => break,
        }
    }
    p
}
