//! Shared benchmark fixtures and helpers.
//!
//! Fixtures are generated in memory from a fixed seed, so every run on every
//! machine measures the same bytes. Nothing is read from disk.

#![allow(dead_code, reason = "each bench target uses a subset")]

use flash_core::Text;

/// SplitMix64: tiny, deterministic, good enough for picking offsets.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "fn",
    "let",
    "mut",
    "self",
    "impl",
    "struct",
    "return",
    "match",
    "Some",
    "None",
    "Ok",
    "Err",
    "text",
    "offset",
    "len",
    "range",
    "line",
    "chunk",
    "rope",
    "insert",
    "delete",
    "usize",
    "selection",
    "document",
    "view",
    "editor",
    "=>",
    "==",
    "&&",
    "+",
    "-",
    "(",
    ")",
    "{",
    "}",
    ";",
    ",",
    "0",
    "1",
    "42",
    "\"str\"",
    "// comment",
];

/// Source-code-like ASCII: indented lines of words, ~40 bytes per line.
pub fn source_like(target_len: usize, seed: u64) -> String {
    let mut rng = Rng::new(seed);
    let mut s = String::with_capacity(target_len + 128);
    while s.len() < target_len {
        let indent = rng.below(4) * 4;
        s.extend(std::iter::repeat_n(' ', indent));
        let words = 1 + rng.below(8);
        for w in 0..words {
            if w > 0 {
                s.push(' ');
            }
            s.push_str(WORDS[rng.below(WORDS.len())]);
        }
        s.push('\n');
    }
    s
}

/// Unicode-heavy text: CJK, combining marks, emoji, ZWJ sequences, flags,
/// with CRLF line endings.
pub fn unicode_like(target_len: usize, seed: u64) -> String {
    const PIECES: &[&str] = &[
        "老虎",
        "日本語",
        "한국어",
        "e\u{301}",
        "ñ",
        "😀",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "\u{1F1FA}\u{1F1F8}",
        "abc",
        " ",
        "Ωμέγα",
        "\t",
    ];
    let mut rng = Rng::new(seed);
    let mut s = String::with_capacity(target_len + 128);
    while s.len() < target_len {
        for _ in 0..(1 + rng.below(12)) {
            s.push_str(PIECES[rng.below(PIECES.len())]);
        }
        s.push_str("\r\n");
    }
    s
}

pub struct Fixture {
    pub name: &'static str,
    pub content: String,
}

pub const KB: usize = 1024;
pub const MB: usize = 1024 * 1024;

pub fn small() -> Fixture {
    Fixture {
        name: "small",
        content: source_like(4 * KB, 1),
    }
}

pub fn medium() -> Fixture {
    Fixture {
        name: "medium",
        content: source_like(MB, 2),
    }
}

/// 100 MB of source-like text (generated, then repeated, to keep setup fast).
pub fn large() -> Fixture {
    let block = source_like(MB, 3);
    Fixture {
        name: "large",
        content: block.repeat(100),
    }
}

/// 1 MB with no newline: stresses anything that is O(line length).
pub fn one_line() -> Fixture {
    let mut s = source_like(MB, 4);
    s = s.replace('\n', " ");
    Fixture {
        name: "one-line",
        content: s,
    }
}

pub fn unicode() -> Fixture {
    Fixture {
        name: "unicode",
        content: unicode_like(MB, 5),
    }
}

pub fn empty() -> Fixture {
    Fixture {
        name: "empty",
        content: String::new(),
    }
}

/// `n` valid offsets (char boundaries) spread over `text`.
pub fn random_offsets(text: &Text, n: usize, seed: u64) -> Vec<usize> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let mut o = rng.below(text.len() + 1);
            while !text.is_char_boundary(o) {
                o -= 1;
            }
            o
        })
        .collect()
}

/// `n` valid ranges of at most `max_len` bytes.
pub fn random_ranges(
    text: &Text,
    n: usize,
    max_len: usize,
    seed: u64,
) -> Vec<std::ops::Range<usize>> {
    let mut rng = Rng::new(seed);
    random_offsets(text, n, seed ^ 0xABCD)
        .into_iter()
        .map(|start| {
            let mut end = (start + rng.below(max_len + 1)).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            start..end.max(start)
        })
        .collect()
}
