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

    /// Calls `f` with each non-empty match starting at or after `from`, in
    /// order, until it returns `false`. The regex sees the whole text
    /// (look-behind before `from` works). Iterates as
    /// [`find_in`](Self::find_in) does. O(bytes searched), no allocation
    /// beyond the regex's cache.
    pub fn each_from(
        &self,
        text: &Text,
        from: usize,
        mut f: impl FnMut(Range<usize>) -> bool,
    ) -> Result<(), TextError> {
        text.check_offset(from)?;
        let total = text.byte_len();
        let mut pos = from;
        while pos < total {
            let Some(cursor) = TextCursor::at(text, pos) else {
                break;
            };
            let mut input = Input::new(cursor);
            input.set_start(pos);
            let Some(m) = self.regex.find(input) else {
                break;
            };
            if m.start() < m.end() {
                if !f(m.start()..m.end()) {
                    break;
                }
                pos = m.end();
            } else if m.end() < total {
                pos = m.end() + char_len(text, m.end());
            } else {
                break;
            }
        }
        Ok(())
    }

    /// The first match starting at or after `from`, else (wrapping) the
    /// first in the text. The flag says whether it wrapped.
    pub fn next_after(&self, text: &Text, from: usize) -> Option<(Range<usize>, bool)> {
        let mut found = None;
        self.each_from(text, from, |m| {
            found = Some(m);
            false
        })
        .ok()?;
        if let Some(m) = found {
            return Some((m, false));
        }
        self.each_from(text, 0, |m| {
            found = Some(m);
            false
        })
        .ok()?;
        found.filter(|m| m.start < from).map(|m| (m, true))
    }

    /// The last match starting before `before`, else (wrapping) the last
    /// in the text. O(bytes before the match found), since regexes search
    /// forward.
    pub fn previous_before(&self, text: &Text, before: usize) -> Option<(Range<usize>, bool)> {
        let (mut last_before, mut last) = (None, None);
        self.each_from(text, 0, |m| {
            if m.start < before {
                last_before = Some(m.clone());
            }
            last = Some(m);
            true
        })
        .ok()?;
        match last_before {
            Some(m) => Some((m, false)),
            None => last.map(|m| (m, true)),
        }
    }

    /// The edits replacing every non-empty match inside `range` (matches
    /// must lie within it; the regex sees the whole text) with
    /// `replacement`, where `$1`, `${name}`, and `$$` expand as in the
    /// `regex` crate.
    pub fn replacements(
        &self,
        text: &Text,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<Vec<(Range<usize>, String)>, TextError> {
        text.check_offset(range.start)?;
        text.check_offset(range.end)?;
        let mut edits = Vec::new();
        let mut caps = self.regex.create_captures();
        let info = self.regex.group_info().clone();
        let mut pos = range.start;
        while pos < range.end {
            let Some(cursor) = TextCursor::at(text, pos) else {
                break;
            };
            let mut input = Input::new(cursor);
            input.set_range(pos..range.end);
            self.regex.search_captures(input, &mut caps);
            let Some(m) = caps.get_match() else {
                break;
            };
            let span = m.range();
            if span.is_empty() {
                if span.end >= range.end {
                    break;
                }
                pos = span.end + char_len(text, span.end);
                continue;
            }
            let mut with = String::new();
            regex_cursor::regex_automata::util::interpolate::string(
                replacement,
                |i, dst| {
                    if let Some(g) = caps.get_group(i) {
                        dst.push_str(
                            &text
                                .slice(g.start..g.end)
                                .map(|s| s.to_string())
                                .unwrap_or_default(),
                        );
                    }
                },
                |name| info.to_index(regex_cursor::regex_automata::PatternID::ZERO, name),
                &mut with,
            );
            pos = span.end;
            edits.push((span, with));
        }
        Ok(edits)
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

/// The length of the char at `pos` (< the text's length).
fn char_len(text: &Text, pos: usize) -> usize {
    match text.chunk_containing(pos) {
        Some((chunk, start)) => match chunk.as_bytes()[pos - start] {
            b if b < 0x80 => 1,
            b if b < 0xE0 => 2,
            b if b < 0xF0 => 3,
            _ => 4,
        },
        None => 1,
    }
}

/// A cursor over the whole text, one rope chunk at a time
/// ([`Text::chunk_containing`], O(log n) per step, no allocation).
struct TextCursor<'a> {
    text: &'a Text,
    chunk: &'a str,
    offset: usize,
    total: usize,
}

impl<'a> TextCursor<'a> {
    /// On the chunk containing `pos`; `None` for an empty text.
    fn at(text: &'a Text, pos: usize) -> Option<TextCursor<'a>> {
        let total = text.byte_len();
        let (chunk, offset) = text.chunk_containing(pos.min(total.checked_sub(1)?))?;
        Some(TextCursor {
            text,
            chunk,
            offset,
            total,
        })
    }
}

impl Cursor for TextCursor<'_> {
    fn chunk(&self) -> &[u8] {
        self.chunk.as_bytes()
    }

    fn advance(&mut self) -> bool {
        match self.text.chunk_containing(self.offset + self.chunk.len()) {
            Some((chunk, offset)) => {
                (self.chunk, self.offset) = (chunk, offset);
                true
            }
            None => false,
        }
    }

    fn backtrack(&mut self) -> bool {
        let Some(before) = self.offset.checked_sub(1) else {
            return false;
        };
        match self.text.chunk_containing(before) {
            Some((chunk, offset)) => {
                (self.chunk, self.offset) = (chunk, offset);
                true
            }
            None => false,
        }
    }

    fn total_bytes(&self) -> Option<usize> {
        Some(self.total)
    }

    fn offset(&self) -> usize {
        self.offset
    }
}

/// `s` with every regex meta character escaped: a pattern matching `s`
/// literally (for searching the selected text).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
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

    /// The reference: regex-automata over the whole string, searching
    /// `from..`.
    fn reference_from(pat: &str, s: &str, from: usize) -> Vec<Range<usize>> {
        use regex_cursor::regex_automata::Input as RInput;
        regex_cursor::regex_automata::meta::Regex::new(pat)
            .unwrap()
            .find_iter(RInput::new(s.as_bytes()).range(from..))
            .filter(|m| m.start() < m.end())
            .map(|m| m.range())
            .collect()
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let t = Text::from("ab foo cd foo ef");
        let m = Matcher::new("foo").unwrap();
        assert_eq!(m.next_after(&t, 0), Some((3..6, false)));
        assert_eq!(m.next_after(&t, 4), Some((10..13, false)));
        assert_eq!(m.next_after(&t, 11), Some((3..6, true)));
        assert_eq!(m.previous_before(&t, 10), Some((3..6, false)));
        assert_eq!(m.previous_before(&t, 3), Some((10..13, true)));
        assert_eq!(Matcher::new("zz").unwrap().next_after(&t, 0), None);
        assert_eq!(
            Matcher::new("x").unwrap().next_after(&Text::from(""), 0),
            None
        );
        // The regex sees the text before the start: no word boundary
        // inside "afoo".
        let t = Text::from("afoo foo");
        let b = Matcher::new(r"\bfoo").unwrap();
        assert_eq!(b.next_after(&t, 1), Some((5..8, false)));
    }

    #[test]
    fn replacements_expand_captures() {
        let t = Text::from("a=1, b=22; c=3");
        let m = Matcher::new(r"(?P<k>\w)=(\d+)").unwrap();
        let edits = m.replacements(&t, 0..t.byte_len(), "$2:${k}$$").unwrap();
        assert_eq!(
            edits,
            [
                (0..3, "1:a$".to_owned()),
                (5..9, "22:b$".to_owned()),
                (11..14, "3:c$".to_owned())
            ]
        );
        // Only matches inside the range.
        let inside = m.replacements(&t, 4..10, "x").unwrap();
        assert_eq!(inside, [(5..9, "x".to_owned())]);
        assert_eq!(escape("a.b*(c)"), r"a\.b\*\(c\)");
        let lit = Matcher::new(&escape("1+1=2?")).unwrap();
        assert_eq!(
            lit.next_after(&Text::from("x 1+1=2? y"), 0),
            Some((2..8, false))
        );
    }

    proptest! {
        /// Matches from an offset, with the whole text as context, agree
        /// with regex-automata on the whole string.
        #[test]
        fn each_from_matches_the_reference(
            s in text_strategy(20),
            at in any::<prop::sample::Index>(),
            pat in prop::sample::select(vec![
                "a", r"\s+", r"\w+", "é|e\u{301}", r"\n", ".", r"\bxyz\b", "(?m)^a", "ab|ba|é",
            ]),
        ) {
            let from = crate::strategies::floor_boundary(&s, at.index(s.len() + 1));
            let t = Text::from(s.as_str());
            let m = Matcher::new(pat).unwrap();
            let mut got = Vec::new();
            m.each_from(&t, from, |r| {
                got.push(r);
                true
            })
            .unwrap();
            prop_assert_eq!(got, reference_from(pat, &s, from));
        }

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
