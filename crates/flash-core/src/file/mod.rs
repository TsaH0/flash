//! Loading and saving files (PLAN Stage 6).
//!
//! The only module in `flash-core` that touches the file system. The
//! editor never calls it: commands queue requests that the frontend runs
//! (`editor::Request`), so replaying a trace cannot write a file.
//!
//! - [`load`] streams a file into a [`Text`], refusing invalid UTF-8 (no
//!   lossy conversion), stripping and remembering a BOM, detecting the
//!   dominant line ending, and recording size, mtime, and a hash.
//! - [`save`] writes atomically (temp file in the same directory, fsync,
//!   permissions, rename over the resolved target, fsync the directory),
//!   or in place behind a backup copy when the target has several hard
//!   links or its ownership cannot be kept. Before writing it checks that
//!   the file on disk is still the one loaded. A failed save leaves the
//!   original intact.
//!
//! This is atomic saving, not crash recovery.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::SystemTime;

use crate::document::LineEnding;
use crate::text::{Text, TextBuilder};

mod save;

pub use save::{Phase, SaveError, SaveMethod, Saved, save};

#[cfg(test)]
mod tests;

/// The UTF-8 byte order mark.
pub const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Bytes read per `read` call while loading.
const CHUNK: usize = 64 << 10;

/// What a file on disk looked like when it was loaded or saved, to detect
/// changes made by someone else before overwriting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskState {
    /// Size in bytes, BOM included.
    pub size: u64,
    /// Modification time, if the platform reports one.
    pub modified: Option<SystemTime>,
    /// FNV-1a 64 of the bytes on disk, BOM included.
    pub hash: u64,
}

/// A loaded file.
#[derive(Debug)]
pub struct Loaded {
    /// The content, without the BOM.
    pub text: Text,
    /// Whether the file started with a UTF-8 BOM (written back on save).
    pub bom: bool,
    /// The dominant line terminator (ties and none: LF).
    pub line_ending: LineEnding,
    pub disk: DiskState,
}

/// Why a file could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadError {
    /// The file does not exist (the caller may open an empty document).
    NotFound,
    /// Not a regular file: a directory, device, FIFO, or socket.
    NotAFile,
    /// The bytes are not UTF-8; `offset` is the first bad byte in the file.
    InvalidUtf8 { offset: u64 },
    /// Another I/O error (kind and message).
    Io(io::ErrorKind, String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such file"),
            Self::NotAFile => write!(f, "not a regular file"),
            Self::InvalidUtf8 { offset } => write!(
                f,
                "not valid UTF-8 (first bad byte at offset {offset}); refusing to open for editing"
            ),
            Self::Io(_, message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<io::Error> for LoadError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => LoadError::NotFound,
            kind => LoadError::Io(kind, e.to_string()),
        }
    }
}

/// FNV-1a 64, fed incrementally.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

/// Loads `path`. O(n); reads the file once.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() {
        return Err(LoadError::NotAFile);
    }
    let file = File::open(path)?;
    let mut loaded = load_from(file, CHUNK)?;
    // Size and mtime from the open file's metadata as read; a writer racing
    // with the read is caught at save time by the hash.
    loaded.disk.modified = meta.modified().ok();
    Ok(loaded)
}

/// Streams `input` in reads of `chunk` bytes. `disk.modified` is `None`.
pub(crate) fn load_from(mut input: impl Read, chunk: usize) -> Result<Loaded, LoadError> {
    let chunk = chunk.max(1);
    let mut buf = vec![0u8; chunk + 4];
    // Bytes at the front of `buf` carried from the last read: an
    // incomplete UTF-8 sequence, or a possible BOM prefix (at most 3).
    let mut carry = 0;
    // File offset of `buf[0]`.
    let mut start: u64 = 0;
    let mut hash = Fnv::new();
    let mut builder = TextBuilder::new();
    let mut bom = false;
    let mut at_start = true;
    let (mut lf, mut crlf, mut last) = (0usize, 0usize, 0u8);

    loop {
        let n = match input.read(&mut buf[carry..carry + chunk]) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        hash.update(&buf[carry..carry + n]);
        let end = carry + n;
        let eof = n == 0;
        let mut from = 0;
        if at_start {
            // Decide on the BOM once 3 bytes are in, or at EOF.
            if end < BOM.len() && !eof && BOM.starts_with(&buf[..end]) {
                carry = end;
                continue;
            }
            at_start = false;
            if buf[..end].starts_with(BOM) {
                bom = true;
                from = BOM.len();
            }
        }
        let data = &buf[from..end];
        let (valid, rest) = match std::str::from_utf8(data) {
            Ok(s) => (s, 0),
            Err(e) => {
                let good = e.valid_up_to();
                if e.error_len().is_some() || eof {
                    return Err(LoadError::InvalidUtf8 {
                        offset: start + (from + good) as u64,
                    });
                }
                // An incomplete sequence at the end of the read: keep it.
                let s = std::str::from_utf8(&data[..good]).unwrap_or_default();
                (s, data.len() - good)
            }
        };
        for (i, _) in valid.match_indices('\n') {
            let before = if i == 0 {
                last
            } else {
                valid.as_bytes()[i - 1]
            };
            if before == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
        if let Some(&b) = valid.as_bytes().last() {
            last = b;
        }
        builder.append(valid);
        if eof {
            break;
        }
        let consumed = end - rest;
        buf.copy_within(consumed..end, 0);
        start += consumed as u64;
        carry = rest;
    }
    let size = start + carry as u64;
    Ok(Loaded {
        text: builder.build(),
        bom,
        line_ending: if crlf > lf {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        },
        disk: DiskState {
            size,
            modified: None,
            hash: hash.finish(),
        },
    })
}

/// FNV-1a 64 of a file's bytes. O(n).
pub(crate) fn hash_file(path: &Path) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; CHUNK];
    let mut hash = Fnv::new();
    loop {
        match file.read(&mut buf) {
            Ok(0) => return Ok(hash.finish()),
            Ok(n) => hash.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}
