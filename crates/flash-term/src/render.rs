//! Frames: the focused view's visible lines, selections, cursor, and the
//! status line or prompt.
//!
//! A full redraw per frame into a reused buffer, wrapped in synchronized-
//! update escapes so the terminal shows it at once; the caller writes it
//! with one `write` and flushes. Stage 7 benchmarks diffing against this.
//!
//! Columns: graphemes are measured with `unicode-width` per cluster
//! (`flash_core::width`), tabs expand to the editor's tab width, and the
//! view's `left` column scrolls horizontally. A wide grapheme cut by the
//! left or right edge is drawn as spaces. Terminals disagree on the width
//! of some emoji sequences; where one draws a cluster wider than
//! `unicode-width` says (or narrower), the cluster itself looks wrong (a
//! known limitation), but the cursor is re-positioned after any cluster
//! whose per-character width differs from its cluster width, so the rest
//! of the line stays in the right columns.

use std::io::{self, Write};
use std::num::NonZeroU8;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use flash_core::width::{display_column, grapheme_width};
use flash_core::{Direction, Document, Editor, Prompt, Selection, View};

/// Builds frames into a buffer it keeps between frames.
#[derive(Debug, Default)]
pub struct Renderer {
    buf: Vec<u8>,
    /// Use the view's [`flash_core::LineAnchor`] (tests turn it off to
    /// check it changes nothing).
    anchors: bool,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            buf: Vec::with_capacity(64 << 10),
            anchors: true,
        }
    }

    /// The frame for `editor` (its focused view, or a cleared screen when
    /// none is focused), with `latency_ns` shown on the status line. Empty
    /// for a zero-sized screen. O(visible cells + the head's line).
    pub fn frame(&mut self, editor: &Editor, latency_ns: u64) -> &[u8] {
        self.buf.clear();
        // Writing into a Vec cannot fail.
        let result = frame(&mut self.buf, editor, latency_ns, self.anchors);
        debug_assert!(result.is_ok());
        &self.buf
    }

    /// Writes the frame for `editor` to `out` (the caller flushes).
    pub fn render(
        &mut self,
        editor: &Editor,
        latency_ns: u64,
        out: &mut impl Write,
    ) -> io::Result<()> {
        out.write_all(self.frame(editor, latency_ns))
    }
}

fn frame(out: &mut Vec<u8>, editor: &Editor, latency_ns: u64, anchors: bool) -> io::Result<()> {
    let (width, height) = editor.screen();
    if width == 0 || height == 0 {
        return Ok(());
    }
    queue!(out, BeginSynchronizedUpdate, Hide)?;
    let focused = editor.focus().and_then(|focus| {
        let (view, doc) = editor.view_document(focus)?;
        Some((focus, view, doc, doc.selection(focus)?))
    });
    let Some((focus, view, doc, selection)) = focused else {
        queue!(out, Clear(ClearType::All))?;
        if let Some(prompt) = editor.prompt() {
            bottom_line(out, prompt_line(prompt), width, height)?;
            queue!(out, Show)?;
        }
        return queue!(out, EndSynchronizedUpdate);
    };
    let tab = view.options().tab_width;
    let cols = usize::from(width);
    let rows = usize::from(height - 1);
    let text = doc.text();
    // The primary cursor: the block's grapheme in normal mode.
    let head = editor
        .cursor_position(focus)
        .unwrap_or_else(|| selection.primary().head());
    // The anchor the view recorded when it scrolled, if still current:
    // the head's column and where its line's visible part starts, so a
    // long line is not walked again.
    let anchor = view.anchor().filter(|a| {
        anchors && a.revision == doc.revision() && a.head == head && a.tab_width == tab
    });
    let head_line = match anchor {
        Some(a) => a.line,
        None => text.line_of_offset(head).unwrap_or(0),
    };
    for row in 0..rows {
        queue!(out, MoveTo(0, row as u16))?;
        let line = view.top() + row;
        if line < text.line_count() {
            let start = anchor
                .filter(|a| a.line == line)
                .and_then(|a| a.start)
                .filter(|&(_, column)| column <= view.left());
            render_line(
                out,
                doc,
                selection,
                line,
                row as u16,
                view.left(),
                cols,
                tab,
                start,
            )?;
        }
        queue!(
            out,
            SetAttribute(Attribute::Reset),
            Clear(ClearType::UntilNewLine)
        )?;
    }

    let column = match anchor {
        Some(a) => a.head_column,
        // O(length of the head's line).
        None => display_column(text, head, tab).unwrap_or(0),
    };
    let cursor = match editor.prompt() {
        Some(prompt) => bottom_line(out, prompt_line(prompt), width, height)?,
        None => {
            let status = status_line(editor, doc, selection, head_line, column, latency_ns);
            bottom_line(out, (status, None), width, height)?;
            cursor_in_view(view, head_line, column, cols, rows)
        }
    };
    if let Some((x, y)) = cursor {
        queue!(out, MoveTo(x, y), Show)?;
    }
    queue!(out, EndSynchronizedUpdate)
}

/// The primary cursor's cell, if it is inside the text area.
fn cursor_in_view(
    view: &View,
    line: usize,
    column: usize,
    cols: usize,
    rows: usize,
) -> Option<(u16, u16)> {
    let x = column.checked_sub(view.left()).filter(|&x| x < cols)?;
    let y = line.checked_sub(view.top()).filter(|&y| y < rows)?;
    Some((x as u16, y as u16))
}

fn status_line(
    editor: &Editor,
    doc: &Document,
    selection: &Selection,
    line: usize,
    column: usize,
    latency_ns: u64,
) -> String {
    let name = doc
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[scratch]".to_owned());
    let modified = if doc.is_modified() { " [+]" } else { "" };
    let n = selection.ranges().len();
    let cursors = if n == 1 {
        String::new()
    } else {
        format!("  {n} cursors")
    };
    format!(
        "{name}{modified}{cursors}  {}:{}  {} µs  {}",
        line + 1,
        column + 1,
        latency_ns / 1_000,
        editor.message()
    )
}

/// The prompt's text and the display column of its cursor.
fn prompt_line(prompt: &Prompt) -> (String, Option<usize>) {
    let before = format!("{}{}", prompt.label(), &prompt.input()[..prompt.cursor()]);
    let cursor = text_width(&before);
    (
        format!("{}{}", prompt.label(), prompt.input()),
        Some(cursor),
    )
}

/// Draws the last row: `line` reversed when it is the status line. With a
/// cursor column (the prompt), scrolls the line so the cursor is visible
/// and returns the cursor cell.
fn bottom_line(
    out: &mut Vec<u8>,
    (line, cursor): (String, Option<usize>),
    width: u16,
    height: u16,
) -> io::Result<Option<(u16, u16)>> {
    let cols = usize::from(width);
    let skip = match cursor {
        Some(c) if c >= cols => c + 1 - cols,
        _ => 0,
    };
    queue!(out, MoveTo(0, height - 1))?;
    if cursor.is_none() {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    queue!(
        out,
        Print(fit(&line, skip, cols)),
        SetAttribute(Attribute::Reset),
        Clear(ClearType::UntilNewLine)
    )?;
    Ok(cursor.map(|c| ((c - skip).min(cols - 1) as u16, height - 1)))
}

/// Characters that reorder text on screen (bidirectional embeddings,
/// overrides, isolates, marks). Zero-width, so dropping them keeps columns.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Prints one grapheme of `cells` cells (never raw control characters).
fn print_grapheme(out: &mut Vec<u8>, g: &str, cells: usize) -> io::Result<()> {
    if g.chars().any(char::is_control) {
        // Every control grapheme other than tab (handled by the caller) is
        // one cell wide for unicode-width: draw it as one U+FFFD.
        queue!(out, Print('\u{fffd}'))?;
        for _ in 1..cells {
            queue!(out, Print(' '))?;
        }
        Ok(())
    } else if g.chars().any(is_bidi_control) {
        let kept: String = g.chars().filter(|&c| !is_bidi_control(c)).collect();
        queue!(out, Print(kept))
    } else {
        queue!(out, Print(g))
    }
}

/// The width of `g` summed per char: what a terminal that does not join
/// clusters (ZWJ sequences, some emoji) draws.
fn per_char_width(g: &str) -> usize {
    let mut buf = [0; 4];
    g.chars()
        .map(|c| grapheme_width(c.encode_utf8(&mut buf), 0, NonZeroU8::MIN))
        .sum()
}

/// Draws one line's content (on screen row `row`) from display column
/// `left`, `width` cells wide. Cells inside a non-empty range, and
/// secondary cursors, are drawn reversed. Walks from the line start, or
/// from `start` (a grapheme boundary on the line and its display column,
/// at or before `left`). O(log n + log k + columns from the start to
/// `left + width`).
#[allow(clippy::too_many_arguments)]
fn render_line(
    out: &mut Vec<u8>,
    doc: &Document,
    selection: &Selection,
    line: usize,
    row: u16,
    left: usize,
    width: usize,
    tab: NonZeroU8,
    start: Option<(usize, usize)>,
) -> io::Result<()> {
    let text = doc.text();
    let Ok(content) = text.line_range(line) else {
        return Ok(());
    };
    let (from, from_column) = start
        .filter(|&(offset, _)| content.contains(&offset) || offset == content.end)
        .unwrap_or((content.start, 0));
    let Ok(graphemes) = text.graphemes(from, Direction::Forward) else {
        return Ok(());
    };
    let ranges = selection.ranges();
    let primary = selection.primary_index();
    // First range that can touch this line.
    let mut next = ranges.partition_point(|r| r.end() < content.start);
    let mut marked = |pos: usize| {
        while next < ranges.len() && ranges[next].end() < pos {
            next += 1;
        }
        // At most two ranges involve `pos`: one ending there, one starting
        // there.
        (next..ranges.len().min(next + 2)).any(|i| {
            let r = ranges[i];
            (!r.is_empty() && r.start() <= pos && pos < r.end())
                || (i != primary && r.head() == pos)
        })
    };

    let right = left.saturating_add(width);
    let (mut column, mut pos, mut reversed) = (from_column, from, false);
    for g in graphemes {
        if pos >= content.end || column >= right {
            break;
        }
        let cells = grapheme_width(&g, column, tab);
        let end = column + cells;
        if end <= left && cells > 0 {
            column = end;
            pos += g.len();
            continue;
        }
        if column < left && cells == 0 {
            pos += g.len();
            continue;
        }
        let mark = marked(pos);
        if mark != reversed {
            let attribute = if mark {
                Attribute::Reverse
            } else {
                Attribute::NoReverse
            };
            queue!(out, SetAttribute(attribute))?;
            reversed = mark;
        }
        let visible = end.min(right) - column.max(left);
        if g == "\t" || column < left || end > right {
            // A tab, or a grapheme cut by an edge: its visible cells as
            // spaces.
            queue!(out, Print(" ".repeat(visible)))?;
        } else {
            print_grapheme(out, &g, cells)?;
            if g.chars().nth(1).is_some() && per_char_width(&g) != cells {
                // Terminals disagree on this cluster's width: put the
                // cursor where the next grapheme belongs.
                queue!(out, MoveTo((end - left) as u16, row))?;
            }
        }
        column = end;
        pos += g.len();
    }
    // A secondary cursor at the end of the line.
    if pos == content.end && (left..right).contains(&column) && marked(content.end) {
        queue!(out, SetAttribute(Attribute::Reverse), Print(' '))?;
    }
    Ok(())
}

/// Display width of `s` measured per char (status and prompt text, where
/// clusters do not matter), control characters counting as one.
fn text_width(s: &str) -> usize {
    let tab = NonZeroU8::MIN;
    let mut column = 0;
    for c in s.chars() {
        let mut buf = [0; 4];
        column += grapheme_width(c.encode_utf8(&mut buf), column, tab);
    }
    column
}

/// `s` without its first `skip` cells, cut to at most `width` cells, with
/// control characters replaced by U+FFFD and bidi controls dropped.
fn fit(s: &str, skip: usize, width: usize) -> String {
    let tab = NonZeroU8::MIN;
    let mut out = String::new();
    let mut column = 0;
    for c in s.chars() {
        if is_bidi_control(c) {
            continue;
        }
        let c = if c.is_control() { '\u{fffd}' } else { c };
        let mut buf = [0; 4];
        let cells = grapheme_width(c.encode_utf8(&mut buf), column, tab);
        let end = column + cells;
        if end > skip + width {
            break;
        }
        if column >= skip {
            out.push(c);
        } else if end > skip {
            // A wide char cut by the left edge.
            out.extend(std::iter::repeat_n(' ', end - skip));
        }
        column = end;
    }
    out
}

#[cfg(test)]
mod tests;
