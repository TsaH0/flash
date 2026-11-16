//! Regular expressions over the rope, without copying it (`regex-cursor`,
//! PLAN Stage 7).
//!
//! A [`Matcher`] runs over the chunks of a range of a [`Text`]; matches are
//! absolute byte ranges. Chunks never split a code point, so every regex
//! feature works (Unicode classes, word boundaries). Matching is linear in
//! the searched bytes (the `regex` engines' guarantee), whatever the
//! pattern.

use std::fmt;
use std::ops::Range;

use regex_cursor::engines::meta::Regex;
use regex_cursor::{Cursor, Input};

use crate::text::{Text, TextError};

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Matcher {
    regex: Regex,
}

/// The pattern did not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad pattern: {}", self.0)
    }
}

impl std::error::Error for PatternError {}

impl Matcher {
    pub fn new(pattern: &str) -> Result<Matcher, PatternError> {
        Regex::new(pattern)
            .map(|regex| Matcher { regex })
            .map_err(|e| PatternError(e.to_string()))
    }

    /// Every non-overlapping match inside `range` of `text`, in order,
    /// leftmost-first. Empty matches are left out.
    pub fn find_in(
        &self,
        text: &Text,
        range: Range<usize>,
    ) -> Result<Vec<Range<usize>>, TextError> {
        let base = range.start;
        let slice = text.slice(range)?;
        let chunks: Vec<&str> = slice.chunks().collect();
        let total = slice.len();
        let cursor = Chunks {
            chunks: &chunks,
            index: 0,
            offset: 0,
            total,
        };
        Ok(self
            .regex
            .find_iter(Input::new(cursor))
            .filter(|m| m.start() < m.end())
            .map(|m| base + m.start()..base + m.end())
            .collect())
    }

    /// Whether `range` of `text` contains a match.
    pub fn is_match_in(&self, text: &Text, range: Range<usize>) -> Result<bool, TextError> {
        let slice = text.slice(range)?;
        let chunks: Vec<&str> = slice.chunks().collect();
        let cursor = Chunks {
            chunks: &chunks,
            index: 0,
            offset: 0,
            total: slice.len(),
        };
        Ok(self.regex.find(Input::new(cursor)).is_some())
    }
}

/// A cursor over a list of non-empty chunks.
struct Chunks<'a> {
    chunks: &'a [&'a str],
    index: usize,
    /// Offset of `chunks[index]` from the start.
    offset: usize,
    total: usize,
}

impl Cursor for Chunks<'_> {
    fn chunk(&self) -> &[u8] {
        self.chunks.get(self.index).map_or(&[], |c| c.as_bytes())
    }

    fn advance(&mut self) -> bool {
        if self.index + 1 < self.chunks.len() {
            self.offset += self.chunks[self.index].len();
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn backtrack(&mut self) -> bool {
        if self.index > 0 {
            self.index -= 1;
            self.offset -= self.chunks[self.index].len();
            true
        } else {
            false
        }
    }

    fn total_bytes(&self) -> Option<usize> {
        Some(self.total)
    }

    fn offset(&self) -> usize {
        self.offset
    }
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "expected matches are lists of byte ranges"
)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::strategies::text_strategy;

    #[test]
    fn finds_inside_a_range_across_chunks() {
        // Long enough to span several rope chunks.
        let s = format!("{}needle é needle{}", "x".repeat(5000), "y".repeat(5000));
        let t = Text::from(s.as_str());
        let m = Matcher::new("needle").unwrap();
        let found = m.find_in(&t, 0..t.byte_len()).unwrap();
        assert_eq!(found, [5000..5006, 5010..5016]);
        assert_eq!(
            m.find_in(&t, 5003..5013).unwrap(),
            Vec::<Range<usize>>::new()
        );
        assert_eq!(m.find_in(&t, 5000..5013).unwrap(), [5000..5006]);
        let word = Matcher::new(r"\bé\b").unwrap();
        assert_eq!(word.find_in(&t, 0..t.byte_len()).unwrap(), [5007..5009]);
        assert!(Matcher::new("(").is_err());
        assert!(m.find_in(&t, 0..t.byte_len() + 1).is_err());
        assert!(
            Matcher::new("a*")
                .unwrap()
                .find_in(&t, 0..5)
                .unwrap()
                .is_empty(),
            "no a: only empty matches"
        );
    }

    proptest! {
        /// Same matches as the `regex` crate on the whole string.
        #[test]
        fn matches_regex_on_a_string(s in text_strategy(20), pat in prop::sample::select(vec![
            "a", "xyz", r"\s+", r"\w+", "é|e\u{301}", r"\n", ".", r"[^a]+", "(?m)^",
        ])) {
            let t = Text::from(s.as_str());
            let m = Matcher::new(pat).unwrap();
            let got = m.find_in(&t, 0..t.byte_len()).unwrap();
            let want: Vec<Range<usize>> = regex_cursor::regex_automata::meta::Regex::new(pat)
                .unwrap()
                .find_iter(s.as_bytes())
                .filter(|m| m.start() < m.end())
                .map(|m| m.range())
                .collect();
            prop_assert_eq!(got, want);
        }
    }
}
