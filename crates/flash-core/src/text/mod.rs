//! Document text storage: a validating wrapper around [`crop::Rope`].
//!
//! # Coordinates
//!
//! Every position is a UTF-8 **byte offset** in `0..=byte_len()`. An offset
//! names a position *between* bytes, like a cursor, not a byte: `"hi"` has
//! positions 0 (before `h`), 1, and 2 (end of text), so `byte_len()` itself is
//! a valid offset.
//!
//! An offset is *valid* when it is in bounds and lies on a `char` boundary.
//! Every method that takes an offset or a range validates it and returns
//! [`TextError`] instead of panicking; crop itself panics on bad offsets, so
//! no crop call in this module is reachable with an unvalidated offset.
//!
//! # Line model
//!
//! Lines are *cursor rows*, which differs from crop's `line_len()`:
//!
//! | text      | `line_count()` | rows                         |
//! |-----------|----------------|------------------------------|
//! | `""`      | 1              | `""`                         |
//! | `"a"`     | 1              | `"a"`                        |
//! | `"a\n"`   | 2              | `"a"`, `""`                  |
//! | `"a\r\nb"`| 2              | `"a"`, `"b"`                 |
//! | `"a\rb"`  | 1              | `"a\rb"` (lone CR is content) |
//!
//! * `line_count() == number of '\n' bytes + 1`, so it is never zero.
//! * Line `i` starts immediately after the `i`-th `'\n'` (line 0 at offset 0).
//! * A line's terminator is `"\r\n"` or `"\n"`. A lone `'\r'` is content.
//! * The last line never has a terminator.
//! * Text is stored byte-for-byte as given; line endings are never normalised.
//!
//! # Mutation
//!
//! [`Text::insert`], [`Text::delete`], and [`Text::replace`] are
//! `pub(crate)`: outside this crate, text changes only through the document's
//! transaction mechanism (docs/PLAN.md 2.2, 2.6).

use std::fmt;
use std::ops::Range;

use crop::{Rope, RopeBuilder, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

#[cfg(test)]
mod proptests;
#[cfg(test)]
pub(crate) mod reference;
#[cfg(test)]
mod tests;

/// Error returned when an offset, range, or line index is not valid for a
/// [`Text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// The offset is greater than the text's byte length.
    OffsetOutOfBounds { offset: usize, len: usize },
    /// The offset is in bounds but falls inside a multi-byte character.
    NotCharBoundary { offset: usize },
    /// The range's start is greater than its end.
    InvertedRange { start: usize, end: usize },
    /// The line index is not less than [`Text::line_count`].
    LineOutOfBounds { line: usize, line_count: usize },
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::OffsetOutOfBounds { offset, len } => {
                write!(
                    f,
                    "byte offset {offset} is out of bounds (text length {len})"
                )
            }
            Self::NotCharBoundary { offset } => {
                write!(
                    f,
                    "byte offset {offset} is not on a UTF-8 character boundary"
                )
            }
            Self::InvertedRange { start, end } => {
                write!(f, "range start {start} is greater than range end {end}")
            }
            Self::LineOutOfBounds { line, line_count } => {
                write!(f, "line {line} is out of bounds (line count {line_count})")
            }
        }
    }
}

impl std::error::Error for TextError {}

/// Direction of a motion through a [`Text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Towards the end of the text (increasing offsets).
    Forward,
    /// Towards the start of the text (decreasing offsets).
    Backward,
}

/// A contiguous, non-empty piece of the rope and the offset where it starts.
/// Chunks tile the text: the one containing byte `b` ends where the one
/// containing the next byte begins.
#[derive(Clone, Copy)]
struct Chunk<'a> {
    text: &'a str,
    start: usize,
}

impl Chunk<'_> {
    fn end(&self) -> usize {
        self.start + self.text.len()
    }
}

/// Document text.
///
/// Cloning is O(1): the clone shares the rope's tree, and later edits to
/// either copy only the path they touch. Use [`Text::snapshot`] to make that
/// intent explicit when handing text to another thread.
#[derive(Clone, Default)]
pub struct Text {
    rope: Rope,
}

impl Text {
    /// Creates an empty text.
    pub fn new() -> Self {
        Self { rope: Rope::new() }
    }

    /// Byte length. O(1).
    pub fn byte_len(&self) -> usize {
        self.rope.byte_len()
    }

    /// Whether the text is empty. O(1).
    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    /// An immutable, independent copy of the current text. O(1).
    pub fn snapshot(&self) -> Text {
        self.clone()
    }

    // ----------------------------------------------------------------------
    // Validation
    // ----------------------------------------------------------------------

    /// Whether `offset` is a valid position: in bounds and on a `char`
    /// boundary. Never panics. O(log n).
    pub fn is_char_boundary(&self, offset: usize) -> bool {
        offset <= self.byte_len() && self.rope.is_char_boundary(offset)
    }

    /// Returns `Ok(())` if `offset` is a valid position. O(log n).
    pub fn check_offset(&self, offset: usize) -> Result<(), TextError> {
        let len = self.byte_len();
        if offset > len {
            return Err(TextError::OffsetOutOfBounds { offset, len });
        }
        if !self.rope.is_char_boundary(offset) {
            return Err(TextError::NotCharBoundary { offset });
        }
        Ok(())
    }

    /// Returns `Ok(())` if both ends of `range` are valid positions and
    /// `start <= end`. O(log n).
    pub fn check_range(&self, range: &Range<usize>) -> Result<(), TextError> {
        if range.start > range.end {
            return Err(TextError::InvertedRange {
                start: range.start,
                end: range.end,
            });
        }
        // Check the end first so an out-of-bounds range reports the end.
        self.check_offset(range.end)?;
        self.check_offset(range.start)
    }

    /// Whether `offset` lies on an extended grapheme cluster boundary.
    /// Returns `false` for invalid offsets. Never panics. Never allocates.
    ///
    /// Cost is O(log n) plus the grapheme context needed, which is a few
    /// bytes except in pathological runs (e.g. thousands of consecutive
    /// regional-indicator symbols).
    ///
    /// This deliberately does not call crop's `Rope::is_grapheme_boundary`:
    /// in crop 0.4.3 that walks chunks backwards from the end of the rope to
    /// the offset, which is O(document length).
    pub fn is_grapheme_boundary(&self, offset: usize) -> bool {
        if !self.is_char_boundary(offset) {
            return false;
        }
        let len = self.byte_len();
        if offset == 0 || offset == len {
            return true;
        }

        let mut cursor = GraphemeCursor::new(offset, len, true);
        let found = self.chunk_at(offset).and_then(|mut chunk| {
            self.run_grapheme_query(&mut cursor, &mut chunk, GraphemeCursor::is_boundary)
        });
        debug_assert!(found.is_some(), "grapheme query left the text");
        found.unwrap_or(true)
    }

    /// Moves `count` extended grapheme clusters from `offset` in
    /// `direction` and returns the new offset, stopping early at the start
    /// or end of the text. `count == 0` returns `offset` unchanged.
    ///
    /// `offset` must be a valid position (see [`Text::check_offset`]). It
    /// need not be a grapheme boundary: the first step then lands on the
    /// nearest boundary in `direction`. Every returned offset is a grapheme
    /// boundary. Never panics. Never allocates.
    ///
    /// Cost is O(log n) to start, plus O(1) per step within a rope chunk and
    /// O(log n) per chunk crossed: the grapheme cursor and its chunk are
    /// reused between steps, so a counted motion is cheaper than `count`
    /// separate one-step calls.
    pub fn move_graphemes(
        &self,
        offset: usize,
        count: usize,
        direction: Direction,
    ) -> Result<usize, TextError> {
        self.check_offset(offset)?;
        let len = self.byte_len();
        let forward = direction == Direction::Forward;
        let at_edge = |p: usize| if forward { p == len } else { p == 0 };

        let mut position = offset;
        // Cursor and chunk reused by consecutive steps that stay inside the
        // chunk. `None` means the next step starts fresh at `position`.
        let mut reuse: Option<(GraphemeCursor, Chunk<'_>)> = None;
        for _ in 0..count {
            if at_edge(position) {
                break;
            }
            let fast = match &mut reuse {
                Some(state) => Some(state),
                None => self
                    .step_chunk(position, direction)
                    .map(|chunk| reuse.insert((GraphemeCursor::new(position, len, true), chunk))),
            };
            let step = fast.map(|(cursor, chunk)| match direction {
                Direction::Forward => cursor.next_boundary(chunk.text, chunk.start),
                Direction::Backward => cursor.prev_boundary(chunk.text, chunk.start),
            });
            let next = match step {
                Some(Ok(next)) => next,
                // The step needs bytes outside the current chunk. Redo it
                // from `position` with a cursor that fetches what it asks
                // for. Partial cursor state is not carried across chunks
                // (DECISIONS 008).
                Some(Err(_)) | None => {
                    reuse = None;
                    self.grapheme_step(position, direction)
                }
            };
            let Some(next) = next else {
                debug_assert!(false, "grapheme step from {position} found no boundary");
                break;
            };
            position = next;
            // At a chunk edge, the next step starts with the adjacent chunk.
            if let Some((_, chunk)) = &reuse {
                if position == chunk.start || position == chunk.end() {
                    reuse = None;
                }
            }
        }
        Ok(position)
    }

    /// The chunk containing byte `byte`, or `None` if `byte >= byte_len()`.
    /// O(log n), no allocation.
    fn chunk_at(&self, byte: usize) -> Option<Chunk<'_>> {
        self.rope
            .chunk_at_byte(byte)
            .map(|(text, start)| Chunk { text, start })
    }

    /// The chunk a grapheme step from `position` reads first: the one after
    /// it going forward, the one before it going backward.
    fn step_chunk(&self, position: usize, direction: Direction) -> Option<Chunk<'_>> {
        match direction {
            Direction::Forward => self.chunk_at(position),
            Direction::Backward => self.chunk_at(position.checked_sub(1)?),
        }
    }

    /// One grapheme step from `position`, which must not be at the edge of
    /// the text in `direction`, with a fresh cursor. `None` only if the
    /// grapheme cursor misbehaves.
    fn grapheme_step(&self, position: usize, direction: Direction) -> Option<usize> {
        let mut cursor = GraphemeCursor::new(position, self.byte_len(), true);
        let mut chunk = self.step_chunk(position, direction)?;
        let found = match direction {
            Direction::Forward => {
                self.run_grapheme_query(&mut cursor, &mut chunk, GraphemeCursor::next_boundary)
            }
            Direction::Backward => {
                self.run_grapheme_query(&mut cursor, &mut chunk, GraphemeCursor::prev_boundary)
            }
        };
        found.unwrap_or(None)
    }

    /// Runs one `GraphemeCursor` query to completion, fetching the chunks and
    /// preceding context the cursor asks for. `chunk` must hold the bytes
    /// the cursor reads first; afterwards it is the chunk the query ended in.
    ///
    /// Returns `None` only if the cursor asks for bytes outside the text,
    /// which its contract rules out: it requests the next chunk only below
    /// `byte_len()`, and earlier bytes only above 0.
    fn run_grapheme_query<'a, T>(
        &'a self,
        cursor: &mut GraphemeCursor,
        chunk: &mut Chunk<'a>,
        mut query: impl FnMut(&mut GraphemeCursor, &str, usize) -> Result<T, GraphemeIncomplete>,
    ) -> Option<T> {
        loop {
            match query(cursor, chunk.text, chunk.start) {
                Ok(found) => return Some(found),
                Err(GraphemeIncomplete::NextChunk) => *chunk = self.chunk_at(chunk.end())?,
                Err(GraphemeIncomplete::PrevChunk) => {
                    *chunk = self.chunk_at(chunk.start.checked_sub(1)?)?;
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let context = self.chunk_at(end.checked_sub(1)?)?;
                    cursor.provide_context(&context.text[..end - context.start], context.start);
                }
                Err(GraphemeIncomplete::InvalidOffset) => return None,
            }
        }
    }

    // ----------------------------------------------------------------------
    // Lines
    // ----------------------------------------------------------------------

    /// Number of cursor rows: `'\n'` count + 1. Never zero. O(log n).
    pub fn line_count(&self) -> usize {
        self.newline_count() + 1
    }

    fn newline_count(&self) -> usize {
        // crop's raw line metric at an offset counts the '\n' bytes before it.
        self.rope.line_of_byte(self.byte_len())
    }

    /// The line containing `offset`: the number of `'\n'` bytes before it.
    /// An offset between `'\r'` and `'\n'` belongs to the line the pair
    /// terminates. O(log n).
    pub fn line_of_offset(&self, offset: usize) -> Result<usize, TextError> {
        self.check_offset(offset)?;
        Ok(self.rope.line_of_byte(offset))
    }

    /// Byte offset where `line` starts. O(log n).
    pub fn line_start(&self, line: usize) -> Result<usize, TextError> {
        self.check_line(line)?;
        // Invariant: line <= newline_count <= crop's line_len, so crop's
        // byte_of_line cannot panic, and for line <= newline_count it returns
        // the offset just after the line-th '\n'.
        Ok(self.rope.byte_of_line(line))
    }

    /// Byte range of `line`'s content, excluding its terminator. O(log n).
    pub fn line_range(&self, line: usize) -> Result<Range<usize>, TextError> {
        let full = self.line_range_with_terminator(line)?;
        let end = full.end - self.terminator_len_at(full.clone());
        Ok(full.start..end)
    }

    /// Byte range of `line` including its terminator, if any. The ranges of
    /// all lines tile `0..byte_len()` without gaps. O(log n).
    pub fn line_range_with_terminator(&self, line: usize) -> Result<Range<usize>, TextError> {
        let start = self.line_start(line)?;
        let end = if line + 1 < self.line_count() {
            self.rope.byte_of_line(line + 1)
        } else {
            self.byte_len()
        };
        Ok(start..end)
    }

    /// `line`'s content, excluding its terminator.
    pub fn line(&self, line: usize) -> Result<TextSlice<'_>, TextError> {
        let range = self.line_range(line)?;
        Ok(TextSlice {
            slice: self.rope.byte_slice(range),
        })
    }

    /// `line` including its terminator, if any.
    pub fn line_with_terminator(&self, line: usize) -> Result<TextSlice<'_>, TextError> {
        let range = self.line_range_with_terminator(line)?;
        Ok(TextSlice {
            slice: self.rope.byte_slice(range),
        })
    }

    fn check_line(&self, line: usize) -> Result<(), TextError> {
        let line_count = self.line_count();
        if line >= line_count {
            return Err(TextError::LineOutOfBounds { line, line_count });
        }
        Ok(())
    }

    /// Length of the terminator ending `full` (0, 1, or 2). `full` must be a
    /// line range with terminator produced by this text.
    fn terminator_len_at(&self, full: Range<usize>) -> usize {
        if full.is_empty() || self.rope.byte(full.end - 1) != b'\n' {
            return 0;
        }
        if full.end - full.start >= 2 && self.rope.byte(full.end - 2) == b'\r' {
            2
        } else {
            1
        }
    }

    // ----------------------------------------------------------------------
    // Content access
    // ----------------------------------------------------------------------

    /// A borrowed view of `range`. O(log n).
    pub fn slice(&self, range: Range<usize>) -> Result<TextSlice<'_>, TextError> {
        self.check_range(&range)?;
        Ok(TextSlice {
            slice: self.rope.byte_slice(range),
        })
    }

    /// The whole text as a slice.
    pub fn as_slice(&self) -> TextSlice<'_> {
        TextSlice {
            slice: self.rope.byte_slice(..),
        }
    }

    /// Iterates the text's chunks in order. Never yields an empty string.
    pub fn chunks(&self) -> Chunks<'_> {
        Chunks {
            inner: self.rope.chunks(),
        }
    }

    // ----------------------------------------------------------------------
    // Mutation (crate-private)
    // ----------------------------------------------------------------------

    /// Inserts `s` at `offset`. O(log n + |s|). On error the text is unchanged.
    #[allow(dead_code, reason = "first caller is ChangeSet application (Stage 2)")]
    pub(crate) fn insert(&mut self, offset: usize, s: &str) -> Result<(), TextError> {
        self.check_offset(offset)?;
        if !s.is_empty() {
            self.rope.insert(offset, s);
        }
        Ok(())
    }

    /// Deletes `range`. O(log n). On error the text is unchanged.
    #[allow(dead_code, reason = "first caller is ChangeSet application (Stage 2)")]
    pub(crate) fn delete(&mut self, range: Range<usize>) -> Result<(), TextError> {
        self.check_range(&range)?;
        if !range.is_empty() {
            self.rope.delete(range);
        }
        Ok(())
    }

    /// Replaces `range` with `s`. O(log n + |s|). On error the text is
    /// unchanged.
    #[allow(dead_code, reason = "first caller is ChangeSet application (Stage 2)")]
    pub(crate) fn replace(&mut self, range: Range<usize>, s: &str) -> Result<(), TextError> {
        self.check_range(&range)?;
        if !range.is_empty() || !s.is_empty() {
            self.rope.replace(range, s);
        }
        Ok(())
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Self {
            rope: Rope::from(s),
        }
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Self {
            rope: Rope::from(s),
        }
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.rope, f)
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.rope, f)
    }
}

/// Content equality. O(n).
impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.rope == other.rope
    }
}

impl Eq for Text {}

impl PartialEq<str> for Text {
    fn eq(&self, other: &str) -> bool {
        self.rope == *other
    }
}

impl PartialEq<&str> for Text {
    fn eq(&self, other: &&str) -> bool {
        self.rope == **other
    }
}

/// Builds a [`Text`] by appending pieces, e.g. while streaming a file.
#[derive(Default)]
pub struct TextBuilder {
    builder: RopeBuilder,
}

impl TextBuilder {
    pub fn new() -> Self {
        Self {
            builder: RopeBuilder::new(),
        }
    }

    pub fn append(&mut self, s: &str) -> &mut Self {
        self.builder.append(s);
        self
    }

    pub fn build(self) -> Text {
        Text {
            rope: self.builder.build(),
        }
    }
}

/// A borrowed range of a [`Text`].
#[derive(Clone, Copy)]
pub struct TextSlice<'a> {
    slice: RopeSlice<'a>,
}

impl<'a> TextSlice<'a> {
    pub fn len(&self) -> usize {
        self.slice.byte_len()
    }

    pub fn is_empty(&self) -> bool {
        self.slice.is_empty()
    }

    /// Iterates the slice's chunks in order. Never yields an empty string.
    pub fn chunks(&self) -> Chunks<'a> {
        Chunks {
            inner: self.slice.chunks(),
        }
    }
}

impl fmt::Display for TextSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.slice, f)
    }
}

impl fmt::Debug for TextSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.slice, f)
    }
}

impl PartialEq<str> for TextSlice<'_> {
    fn eq(&self, other: &str) -> bool {
        self.slice == *other
    }
}

impl PartialEq<&str> for TextSlice<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.slice == **other
    }
}

/// Iterator over a text's chunks. Double-ended; never yields `""`.
#[derive(Clone)]
pub struct Chunks<'a> {
    inner: crop::iter::Chunks<'a>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.find(|c| !c.is_empty())
    }
}

impl DoubleEndedIterator for Chunks<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.by_ref().rev().find(|c| !c.is_empty())
    }
}
