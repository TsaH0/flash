//! Display width: how many terminal cells a grapheme occupies.
//!
//! Rules (DECISIONS 012): a tab advances to the next multiple of the tab
//! width; every other grapheme takes `UnicodeWidthStr::width` cells
//! (unicode-width 0.2), so CJK and ZWJ emoji take 2, combining sequences 1,
//! zero-width spaces 0, and control characters 1. Renderers draw a control
//! character as U+FFFD in its one cell, never raw.

use std::num::NonZeroU8;

use unicode_width::UnicodeWidthStr;

use crate::text::{Direction, Text, TextError};

/// Cells taken by `grapheme` when it starts at display column `column`.
pub fn grapheme_width(grapheme: &str, column: usize, tab_width: NonZeroU8) -> usize {
    if grapheme == "\t" {
        let tab = usize::from(tab_width.get());
        tab - column % tab
    } else {
        grapheme.width()
    }
}

/// Display column of `offset` in its line: the summed widths of the
/// graphemes between the line's start and `offset`. If `offset` is inside a
/// cluster, that cluster is not counted. O(log n + column).
pub fn display_column(
    text: &Text,
    offset: usize,
    tab_width: NonZeroU8,
) -> Result<usize, TextError> {
    let line = text.line_of_offset(offset)?;
    let start = text.line_start(line)?;
    let (mut column, mut pos) = (0, start);
    for g in text.graphemes(start, Direction::Forward)? {
        if pos + g.len() > offset {
            break;
        }
        column += grapheme_width(&g, column, tab_width);
        pos += g.len();
    }
    Ok(column)
}
