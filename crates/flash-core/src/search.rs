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
        Ok(self.find_in_chunks(&chunks, base))
    }

    /// The matches in the concatenation of `chunks` (each non-empty,
    /// each whole code points), offset by `base`.
    ///
    /// Not `find_iter`: regex-cursor 0.1.5's literal strategy (a pattern
    /// that is an alternation of literals, such as `é|e\u{301}`) reads
    /// ahead into the next chunk to confirm a match and never moves the
    /// cursor back, so `find_iter`'s next search starts in that chunk and
    /// misses a match beginning before it. Each search here starts from a
    /// fresh cursor on the chunk holding its start (O(log chunks)).
    /// Iteration follows `find_iter`: after a non-empty match the next
    /// search starts at its end; after an empty one, at the next char.
    fn find_in_chunks(&self, chunks: &[&str], base: usize) -> Vec<Range<usize>> {
        let mut starts = Vec::with_capacity(chunks.len());
        let mut total = 0;
        for c in chunks {
            starts.push(total);
            total += c.len();
        }
        let mut found = Vec::new();
        let mut pos = 0;
        // No non-empty match can start at the end.
        while pos < total {
            let index = starts.partition_point(|&s| s <= pos) - 1;
            let cursor = Chunks {
                chunks,
                index,
                offset: starts[index],
                total,
            };
            let mut input = Input::new(cursor);
            input.set_start(pos);
            let Some(m) = self.regex.find(input) else {
                break;
            };
            if m.start() < m.end() {
                found.push(base + m.start()..base + m.end());
                pos = m.end();
            } else if m.end() < total {
                pos = m.end() + char_len_at(chunks, &starts, m.end());
            } else {
                break;
            }
        }
        found
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

/// The length of the char starting at `pos` (< the total length), from its
/// leading byte.
fn char_len_at(chunks: &[&str], starts: &[usize], pos: usize) -> usize {
    let index = starts.partition_point(|&s| s <= pos) - 1;
    match chunks[index].as_bytes()[pos - starts[index]] {
        b if b < 0x80 => 1,
        b if b < 0xE0 => 2,
        b if b < 0xF0 => 3,
        _ => 4,
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

    fn regex_matches(pat: &str, s: &str) -> Vec<Range<usize>> {
        regex_cursor::regex_automata::meta::Regex::new(pat)
            .unwrap()
            .find_iter(s.as_bytes())
            .filter(|m| m.start() < m.end())
            .map(|m| m.range())
            .collect()
    }

    /// Every way to cut `s` into non-empty chunks at char boundaries.
    fn every_split(s: &str) -> Vec<Vec<&str>> {
        let cuts: Vec<usize> = s.char_indices().map(|(i, _)| i).skip(1).collect();
        (0u32..1 << cuts.len())
            .map(|mask| {
                let mut chunks = Vec::new();
                let mut from = 0;
                for (bit, &at) in cuts.iter().enumerate() {
                    if mask & (1 << bit) != 0 {
                        chunks.push(&s[from..at]);
                        from = at;
                    }
                }
                chunks.push(&s[from..]);
                chunks
            })
            .collect()
    }

    /// A match that straddles a chunk boundary is found, whatever the
    /// pattern compiles to (literal alternations use a prefilter).
    #[test]
    fn matches_across_every_chunk_split() {
        let cases = [
            ("ée\u{301}", "é|e\u{301}"),
            ("xxneedlexxneedle", "needle"),
            ("abcab", "abc|cab|bca"),
            ("foo bar", "foo|bar|o b"),
            ("aé😀b", "é😀|😀b|a"),
            ("a\r\nb", "\r\n|b"),
        ];
        for (s, pat) in cases {
            let m = Matcher::new(pat).unwrap();
            let want = regex_matches(pat, s);
            for chunks in every_split(s) {
                assert_eq!(
                    m.find_in_chunks(&chunks, 0),
                    want,
                    "pattern {pat:?} over chunks {chunks:?}"
                );
            }
        }
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

        /// The same over chunks cut at random char boundaries, whatever
        /// the rope's leaf size.
        #[test]
        fn matches_regex_over_random_chunks(
            s in text_strategy(20),
            cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
            pat in prop::sample::select(vec![
                "a", "xyz", r"\s+", r"\w+", "é|e\u{301}", r"\n", ".", r"[^a]+", "(?m)^",
                "ab|ba|é", r"\r\n|\n",
            ]),
        ) {
            let bounds: Vec<usize> = s.char_indices().map(|(i, _)| i).skip(1).collect();
            let mut at: Vec<usize> = if bounds.is_empty() {
                Vec::new()
            } else {
                cuts.iter().map(|c| bounds[c.index(bounds.len())]).collect()
            };
            at.sort_unstable();
            at.dedup();
            let mut chunks = Vec::new();
            let mut from = 0;
            for cut in at {
                chunks.push(&s[from..cut]);
                from = cut;
            }
            if from < s.len() {
                chunks.push(&s[from..]);
            }
            let m = Matcher::new(pat).unwrap();
            prop_assert_eq!(m.find_in_chunks(&chunks, 7), regex_matches(pat, &s)
                .into_iter()
                .map(|r| r.start + 7..r.end + 7)
                .collect::<Vec<_>>());
        }
    }
}
