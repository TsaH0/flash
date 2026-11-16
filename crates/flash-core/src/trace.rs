//! Trace record and replay (PLAN Stage 4, DECISIONS 016).
//!
//! A trace is the raw input of an editing session, as JSON Lines. The first
//! line is a [`TraceHeader`] naming the document and its length and
//! content hash, the screen size, and the starting cursor. Every later line
//! is a [`TraceEntry`]: an [`Event`] and the time it arrived. Replaying the
//! events into an editor started from the same header reproduces the
//! session exactly, which makes a trace both an integration test and a
//! latency benchmark driver.
//!
//! Traces are untrusted input: reading never panics, rejects lines longer
//! than a limit before allocating past it, and stops at the first error.

use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::document::Timestamp;
use crate::editor::{Editor, Event, ViewId};
use crate::selection::Selection;
use crate::text::{Text, TextError};

/// The trace format version this build reads and writes.
pub const TRACE_VERSION: u32 = 1;

/// Default longest line [`TraceReader`] accepts: room for large pastes.
pub const DEFAULT_MAX_LINE: usize = 16 << 20;

/// Where a trace's starting document comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSource {
    /// A file, as given on the command line.
    Path(String),
    /// A named benchmark fixture.
    Fixture(String),
}

/// The first line of a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceHeader {
    /// Always [`TRACE_VERSION`].
    pub flash_trace: u32,
    pub document: TraceSource,
    /// Byte length of the starting document.
    pub len: u64,
    /// [`content_hash`] of the starting document.
    pub hash: u64,
    /// Screen size, `(width, height)`.
    pub size: (u16, u16),
    /// Starting cursor offset.
    pub cursor: u64,
}

/// One recorded event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Nanoseconds since recording started: the time the editor handles the
    /// event at ([`Editor::handle_at`]), which decides undo grouping. Not
    /// required to be monotonic (history clamps). Stage 6 also uses it to
    /// replay coalescing.
    pub t_ns: u64,
    pub event: Event,
}

/// FNV-1a (64-bit) of the text's bytes. O(n).
pub fn content_hash(text: &Text) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in text.chunks() {
        for &b in chunk.as_bytes() {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Writes a trace: the header on creation, then one line per entry.
pub struct TraceWriter<W: Write> {
    out: W,
}

impl<W: Write> TraceWriter<W> {
    pub fn new(mut out: W, header: &TraceHeader) -> Result<Self, TraceError> {
        write_line(&mut out, header)?;
        Ok(TraceWriter { out })
    }

    pub fn write(&mut self, entry: &TraceEntry) -> Result<(), TraceError> {
        write_line(&mut self.out, entry)
    }

    /// Flushes and returns the writer.
    pub fn into_inner(mut self) -> Result<W, TraceError> {
        self.out.flush()?;
        Ok(self.out)
    }
}

fn write_line(out: &mut impl Write, value: &impl Serialize) -> Result<(), TraceError> {
    serde_json::to_writer(&mut *out, value).map_err(|e| TraceError::Json {
        line: 0,
        message: e.to_string(),
    })?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Reads a trace: the header on creation, then entries as an iterator,
/// which ends after the first error.
pub struct TraceReader<R: BufRead> {
    input: R,
    header: TraceHeader,
    max_line: usize,
    /// 1-based number of the last line read.
    line: usize,
    buf: Vec<u8>,
    done: bool,
}

impl<R: BufRead> TraceReader<R> {
    /// Reads and checks the header. Lines longer than `max_line` bytes
    /// (excluding the newline) are errors.
    pub fn new(input: R, max_line: usize) -> Result<Self, TraceError> {
        let mut reader = TraceReader {
            input,
            header: TraceHeader {
                flash_trace: 0,
                document: TraceSource::Fixture(String::new()),
                len: 0,
                hash: 0,
                size: (0, 0),
                cursor: 0,
            },
            max_line,
            line: 0,
            buf: Vec::new(),
            done: false,
        };
        if !reader.next_line()? {
            return Err(TraceError::MissingHeader);
        }
        let header: TraceHeader = reader.parse()?;
        if header.flash_trace != TRACE_VERSION {
            return Err(TraceError::Version(header.flash_trace));
        }
        reader.header = header;
        Ok(reader)
    }

    pub fn header(&self) -> &TraceHeader {
        &self.header
    }

    /// Reads the next non-blank line into `buf` (without its terminator).
    /// `false` at end of input.
    fn next_line(&mut self) -> Result<bool, TraceError> {
        loop {
            self.buf.clear();
            let limit = u64::try_from(self.max_line)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let n = (&mut self.input)
                .take(limit)
                .read_until(b'\n', &mut self.buf)?;
            if n == 0 {
                return Ok(false);
            }
            self.line += 1;
            if self.buf.last() == Some(&b'\n') {
                self.buf.pop();
                if self.buf.last() == Some(&b'\r') {
                    self.buf.pop();
                }
            } else if self.buf.len() > self.max_line {
                return Err(TraceError::LineTooLong {
                    line: self.line,
                    max: self.max_line,
                });
            }
            if !self.buf.iter().all(u8::is_ascii_whitespace) {
                return Ok(true);
            }
        }
    }

    fn parse<'a, T: Deserialize<'a>>(&'a self) -> Result<T, TraceError> {
        serde_json::from_slice(&self.buf).map_err(|e| TraceError::Json {
            line: self.line,
            message: e.to_string(),
        })
    }

    fn next_entry(&mut self) -> Result<Option<TraceEntry>, TraceError> {
        if !self.next_line()? {
            return Ok(None);
        }
        self.parse().map(Some)
    }
}

impl<R: BufRead> Iterator for TraceReader<R> {
    type Item = Result<TraceEntry, TraceError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let item = self.next_entry().transpose();
        if !matches!(item, Some(Ok(_))) {
            self.done = true;
        }
        item
    }
}

/// An editor started as `header` describes: `text` (which must match the
/// header's length and hash) opened in one view, focused, with a cursor at
/// `header.cursor`, on a screen of `header.size`.
pub fn start(header: &TraceHeader, text: Text) -> Result<(Editor, ViewId), TraceError> {
    if u64::try_from(text.byte_len()).ok() != Some(header.len) {
        return Err(TraceError::Mismatch { what: "len" });
    }
    if content_hash(&text) != header.hash {
        return Err(TraceError::Mismatch { what: "hash" });
    }
    let cursor = usize::try_from(header.cursor).unwrap_or(usize::MAX);
    text.check_offset(cursor).map_err(TraceError::Cursor)?;
    let path = match &header.document {
        TraceSource::Path(p) => Some(PathBuf::from(p)),
        TraceSource::Fixture(_) => None,
    };
    let (width, height) = header.size;
    let mut editor = Editor::new(width, height);
    let doc = editor.open(text, path);
    let view = editor
        .open_view(doc)
        .map_err(|e| TraceError::Io(io::ErrorKind::Other, e.to_string()))?;
    editor
        .set_selection(view, Selection::point(cursor))
        .map_err(|e| TraceError::Io(io::ErrorKind::Other, e.to_string()))?;
    Ok((editor, view))
}

/// Feeds every remaining entry of `reader` to `editor`, each at its
/// recorded time. Returns the number of events replayed, or the first read
/// error.
pub fn replay<R: BufRead>(
    reader: &mut TraceReader<R>,
    editor: &mut Editor,
) -> Result<usize, TraceError> {
    let mut count = 0;
    for entry in reader {
        let entry = entry?;
        editor.handle_at(&entry.event, Timestamp::from_nanos(entry.t_ns));
        count += 1;
    }
    Ok(count)
}

/// Error reading, writing, or starting a trace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraceError {
    /// An I/O error (kind and message).
    Io(io::ErrorKind, String),
    /// The input has no header line.
    MissingHeader,
    /// Line `line` (1-based; 0 when writing) is not a valid header or entry.
    Json { line: usize, message: String },
    /// Line `line` is longer than `max` bytes.
    LineTooLong { line: usize, max: usize },
    /// The header's version is not [`TRACE_VERSION`].
    Version(u32),
    /// The document does not match the header's `len` or `hash`.
    Mismatch { what: &'static str },
    /// The header's cursor is not a valid offset in the document.
    Cursor(TextError),
}

impl From<io::Error> for TraceError {
    fn from(e: io::Error) -> Self {
        TraceError::Io(e.kind(), e.to_string())
    }
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_, message) => write!(f, "trace I/O error: {message}"),
            Self::MissingHeader => write!(f, "trace has no header line"),
            Self::Json { line, message } => write!(f, "trace line {line}: {message}"),
            Self::LineTooLong { line, max } => {
                write!(f, "trace line {line} is longer than {max} bytes")
            }
            Self::Version(v) => write!(
                f,
                "trace version {v} is not supported (expected {TRACE_VERSION})"
            ),
            Self::Mismatch { what } => {
                write!(f, "document does not match the trace header ({what})")
            }
            Self::Cursor(e) => write!(f, "trace header cursor is not valid: {e}"),
        }
    }
}

impl std::error::Error for TraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cursor(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
