//! Frames: the views on screen with their selections, the separators
//! between split views, the status line or prompt, and the cursor.
//!
//! Each frame is composed into a [`Grid`] of cells and compared with the
//! grid the terminal shows; only what changed is written
//! ([`crate::emit`], DECISIONS 044). Styles come from the theme
//! ([`flash_core::theme`]).
//!
//! Columns: graphemes are measured with `unicode-width` per cluster
//! (`flash_core::width`), tabs expand to the view's tab width, and a
//! view's `left` column scrolls horizontally (soft wrap draws rows
//! instead). A wide grapheme cut by an edge is drawn as blanks. Terminals
//! disagree on the width of some emoji sequences; where one draws a
//! cluster wider or narrower than `unicode-width` says, the cluster itself
//! looks wrong (a known limitation), but the cursor is placed explicitly
//! after any cluster whose per-character width differs from its cluster
//! width, so the rest of the line stays in the right columns.

use std::io::{self, Write};
use std::num::NonZeroU8;
use std::ops::Range as Span;

use flash_core::search::Matcher;
use unicode_segmentation::UnicodeSegmentation;
use flash_core::theme::{Style, UiStyles};
use flash_core::width::grapheme_width;
use flash_core::{
    CursorShape, Direction, Document, Editor, Layout, OptionId, OptionValue, Prompt, Rect, Row,
    Selection, Text, ViewId, key_name,
};

use crate::emit::{CursorState, Emitter, Shape, Stats, Strategy};
use crate::grid::Grid;

/// The most bytes of one drawn line searched for matches to underline.
const HIGHLIGHT_MAX: usize = 64 << 10;

/// The most cells a frame composes. A hostile trace can resize to
/// 65535×65535; beyond this, rows past it are not drawn.
const MAX_CELLS: usize = 1 << 20;

/// Builds frames: composes a grid and writes what changed since the last
/// frame into a buffer it keeps.
#[derive(Debug)]
pub struct Renderer {
    buf: Vec<u8>,
    /// Use the views' [`flash_core::Layout`] caches (tests turn it off to
    /// check it changes nothing: every line is then laid out from its
    /// start).
    use_layout: bool,
    strategy: Strategy,
    /// The frame being composed, and the one the terminal shows (if
    /// known).
    next: Grid,
    shown: Grid,
    shown_valid: bool,
    emitter: Emitter,
    stats: Stats,
    /// Rows of the line being drawn (reused).
    rows: Vec<Row>,
}

impl Default for Renderer {
    fn default() -> Self {
        Renderer::new()
    }
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            buf: Vec::with_capacity(64 << 10),
            use_layout: true,
            strategy: Strategy::default(),
            next: Grid::default(),
            shown: Grid::default(),
            shown_valid: false,
            emitter: Emitter::default(),
            stats: Stats::default(),
            rows: Vec::new(),
        }
    }

    /// How frames are written (default [`Strategy::Cells`]).
    pub fn set_strategy(&mut self, strategy: Strategy) {
        self.strategy = strategy;
    }

    /// Forgets what the terminal shows: the next frame clears the screen
    /// and writes everything (after a suspend, or anything else that may
    /// have drawn on the terminal).
    pub fn invalidate(&mut self) {
        self.shown_valid = false;
        self.emitter.reset();
    }

    /// What the last frame wrote.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The bytes that bring the terminal from the last frame to `editor`'s
    /// state, with `latency_ns` on the status line if the `status-latency`
    /// option is on. Empty for a zero-sized screen. O(visible cells + the
    /// layout walks of the views' cursor lines).
    pub fn frame(&mut self, editor: &Editor, latency_ns: u64) -> &[u8] {
        self.buf.clear();
        let (width, height) = editor.screen();
        if width == 0 || height == 0 {
            self.shown_valid = false;
            self.stats = Stats::default();
            return &self.buf;
        }
        let rows = (MAX_CELLS / usize::from(width)).clamp(1, usize::from(u16::MAX));
        let height = height.min(rows as u16);
        let cursor = compose(
            &mut self.next,
            &mut self.rows,
            editor,
            (width, height),
            latency_ns,
            self.use_layout,
        );
        let prev = self.shown_valid.then_some(&self.shown);
        self.stats = self
            .emitter
            .frame(&mut self.buf, prev, &self.next, cursor, self.strategy);
        std::mem::swap(&mut self.next, &mut self.shown);
        self.shown_valid = true;
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

fn shape(s: CursorShape) -> Shape {
    match s {
        CursorShape::Block => Shape::Block,
        CursorShape::Bar => Shape::Bar,
        CursorShape::Underline => Shape::Underline,
    }
}

/// Composes `editor`'s screen into `grid`; returns where the cursor goes.
fn compose(
    grid: &mut Grid,
    rows: &mut Vec<Row>,
    editor: &Editor,
    (width, height): (u16, u16),
    latency_ns: u64,
    use_layout: bool,
) -> Option<CursorState> {
    let ui = *editor.theme().ui();
    grid.reset(width, height, ui.text);
    let focus = editor.focus();
    let mut cursor = None;
    for &(id, rect) in editor.tiles() {
        // Clipped to the grid (a capped huge screen).
        let rect = Rect {
            height: rect.height.min(height.saturating_sub(rect.y)),
            ..rect
        };
        let at = draw_view(grid, rows, editor, id, rect, use_layout, &ui);
        if focus == Some(id) {
            cursor = at;
        }
    }
    for sep in editor.separators() {
        let r = sep.rect;
        if r.y >= height {
            continue;
        }
        if sep.vertical {
            for y in r.y..(r.y + r.height).min(height) {
                grid.put(r.x, y, r.x + 1, "│", 1, ui.separator, false);
            }
        } else {
            let name = editor
                .view_document(sep.view)
                .map(|(_, d)| document_name(d))
                .unwrap_or_default();
            grid.fill(r.x, r.y, r.x + r.width, ui.separator);
            put_text(grid, r.x, r.y, r.width, &format!(" {name}"), 0, ui.separator);
        }
    }
    // The last row: the prompt, or the focused view's status line.
    let bottom = height - 1;
    if let Some(prompt) = editor.prompt() {
        let (line, at) = prompt_line(prompt);
        let cols = usize::from(width);
        let skip = if at >= cols { at + 1 - cols } else { 0 };
        grid.fill(0, bottom, width, ui.prompt);
        put_text(grid, 0, bottom, width, &line, skip, ui.prompt);
        return Some(CursorState {
            x: (at - skip).min(cols - 1) as u16,
            y: bottom,
            shape: shape(editor.cursor_shape(focus)),
        });
    }
    let focus = focus?;
    let (view, doc) = editor.view_document(focus)?;
    let selection = doc.selection(focus)?;
    let head = editor.cursor_position(focus);
    let (line, column) = view
        .cursor()
        .filter(|_| use_layout && view.layout().revision() == doc.revision())
        .filter(|c| Some(c.offset) == head)
        .copied()
        .or_else(|| Layout::locate_from_start(view.layout().config(), doc.text(), head?))
        .map_or((0, 0), |c| (c.line, c.column));
    let show_latency =
        editor.option(OptionId::STATUS_LATENCY, None) == Some(OptionValue::Bool(true));
    let status = status_line(
        editor,
        doc,
        selection,
        line,
        column,
        show_latency.then_some(latency_ns),
    );
    grid.fill(0, bottom, width, ui.statusline);
    put_text(grid, 0, bottom, width, &status, 0, ui.statusline);
    cursor.map(|(x, y)| CursorState {
        x,
        y,
        shape: shape(editor.cursor_shape(Some(focus))),
    })
}

/// Draws view `id` into `rect`; returns its primary cursor's cell if it is
/// inside the rectangle.
fn draw_view(
    grid: &mut Grid,
    rows: &mut Vec<Row>,
    editor: &Editor,
    id: ViewId,
    rect: Rect,
    use_layout: bool,
    ui: &UiStyles,
) -> Option<(u16, u16)> {
    let (view, doc) = editor.view_document(id)?;
    let selection = doc.selection(id)?;
    let text = doc.text();
    let tab = view.options().tab_width;
    let cols = usize::from(rect.width);
    let height = usize::from(rect.height);
    // The primary cursor: the block's grapheme in normal mode.
    let head = editor
        .cursor_position(id)
        .unwrap_or_else(|| selection.primary().head());
    // Without the cache (tests), a fresh layout lays every line out from
    // its start: its marks are never current.
    let fresh = Layout::new(view.layout().config());
    let layout = if use_layout { view.layout() } else { &fresh };
    let current = use_layout && layout.revision() == doc.revision();
    let cursor_at = match view.cursor().filter(|c| current && c.offset == head) {
        Some(c) => Some(*c),
        // O(length of the head's line).
        None => Layout::locate_from_start(layout.config(), text, head),
    };
    let highlight = editor.search_highlight();
    let span = |content, eol, row: usize, left, from| Span_ {
        content,
        eol,
        at: (rect.x, rect.y + row as u16),
        left,
        width: cols,
        tab,
        from,
    };
    let mut cell = None;
    if view.options().soft_wrap {
        let mut y = 0;
        let mut line = view.top();
        let mut first = view.top_row();
        while y < height && line < text.line_count() {
            rows.clear();
            rows.extend(layout.rows(doc, line, first).take(height - y));
            let matches = match (rows.first(), rows.last()) {
                (Some(a), Some(b)) => find(highlight, text, a.start, b.end),
                _ => Vec::new(),
            };
            for r in rows.iter() {
                if let Some(c) = cursor_at.filter(|c| c.line == line && c.row == r.index) {
                    let x = c.visual_column();
                    if x < cols {
                        cell = Some((rect.x + x as u16, rect.y + y as u16));
                    }
                }
                let s = span(r.start..r.end, r.last, y, r.column, (r.start, r.column));
                draw_span(grid, text, selection, &matches, &s, ui);
                y += 1;
            }
            line += 1;
            first = 0;
        }
    } else {
        for row in 0..height {
            let line = view.top() + row;
            let Ok(content) = text.line_range(line) else {
                break;
            };
            // Long lines start drawing at the layout's last mark before
            // the left column.
            let from = layout.start_before(doc, line, view.left());
            let to = content.end.min(from.0.saturating_add(HIGHLIGHT_MAX));
            let matches = find(highlight, text, from.0, to);
            let s = span(content, true, row, view.left(), from);
            draw_span(grid, text, selection, &matches, &s, ui);
        }
        if let Some(c) = cursor_at {
            let x = c.column.checked_sub(view.left()).filter(|&x| x < cols);
            let y = c.line.checked_sub(view.top()).filter(|&y| y < height);
            if let (Some(x), Some(y)) = (x, y) {
                cell = Some((rect.x + x as u16, rect.y + y as u16));
            }
        }
    }
    cell
}

/// The matches of `highlight` in `from..to` (`to` snapped back to a char
/// boundary).
fn find(highlight: Option<&Matcher>, text: &Text, from: usize, to: usize) -> Vec<Span<usize>> {
    let Some(m) = highlight else {
        return Vec::new();
    };
    let mut to = to.min(text.byte_len()).max(from);
    while to > from && !text.is_char_boundary(to) {
        to -= 1;
    }
    m.find_in(text, from..to).unwrap_or_default()
}

/// One span to draw: a line's content or one wrapped row.
struct Span_ {
    content: Span<usize>,
    /// The span ends the line: a secondary cursor at its end is drawn
    /// after it.
    eol: bool,
    /// The screen cell of the span's left edge (column `left`).
    at: (u16, u16),
    left: usize,
    width: usize,
    tab: NonZeroU8,
    /// A grapheme boundary in the span (or its start) at or before
    /// `left`, and its logical column: where walking starts.
    from: (usize, usize),
}

/// What a position is drawn as besides its text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Selection { primary: bool },
    Cursor,
}

/// Draws `s`: the cells from logical column `s.left`, `s.width` wide.
/// O(log n + log k + columns from `s.from` to `left + width`).
fn draw_span(
    grid: &mut Grid,
    text: &Text,
    selection: &Selection,
    matches: &[Span<usize>],
    s: &Span_,
    ui: &UiStyles,
) {
    let (x0, y) = s.at;
    let content = s.content.clone();
    let (from, from_column) = if content.contains(&s.from.0) || s.from.0 == content.end {
        s.from
    } else {
        (content.start, 0)
    };
    let Ok(graphemes) = text.graphemes(from, Direction::Forward) else {
        return;
    };
    let ranges = selection.ranges();
    let primary = selection.primary_index();
    // First range that can touch this span.
    let mut next = ranges.partition_point(|r| r.end() < content.start);
    let mut mark = |pos: usize| {
        while next < ranges.len() && ranges[next].end() < pos {
            next += 1;
        }
        // At most two ranges involve `pos`: one ending there, one starting
        // there.
        for (i, &r) in ranges.iter().enumerate().skip(next).take(2) {
            if !r.is_empty() && r.start() <= pos && pos < r.end() {
                return Mark::Selection {
                    primary: i == primary,
                };
            }
            if i != primary && r.head() == pos {
                return Mark::Cursor;
            }
        }
        Mark::None
    };
    let mut next_match = 0;
    let mut in_match = |pos: usize| {
        while next_match < matches.len() && matches[next_match].end <= pos {
            next_match += 1;
        }
        matches
            .get(next_match)
            .is_some_and(|m| m.start <= pos && pos < m.end)
    };
    let style_at = |m: Mark, matched: bool| {
        let base = if matched {
            ui.text.patch(ui.search)
        } else {
            ui.text
        };
        match m {
            Mark::None => base,
            Mark::Selection { primary: true } => base.patch(ui.primary_selection),
            Mark::Selection { primary: false } => base.patch(ui.selection),
            Mark::Cursor => base.patch(ui.cursor),
        }
    };
    let (left, tab) = (s.left, s.tab);
    let right = left.saturating_add(s.width);
    let x_right = x0 + s.width as u16;
    let (mut column, mut pos) = (from_column, from);
    for g in graphemes {
        if pos >= content.end || column >= right {
            break;
        }
        let cells = grapheme_width(&g, column, tab);
        let end = column + cells;
        if (end <= left && cells > 0) || (column < left && cells == 0) {
            column = end;
            pos += g.len();
            continue;
        }
        let style = style_at(mark(pos), in_match(pos));
        let x = x0 + (column.max(left) - left) as u16;
        let visible = end.min(right) - column.max(left);
        if g == "\t" || column < left || end > right {
            // A tab, or a grapheme cut by an edge: its visible cells as
            // blanks.
            grid.fill(x, y, x + visible as u16, style);
        } else if cells > 0 {
            put_grapheme(grid, x, y, x_right, &g, cells, style);
        }
        column = end;
        pos += g.len();
    }
    // A secondary cursor (or a selection) at the end of the line.
    if s.eol && pos == content.end && (left..right).contains(&column) {
        let m = mark(content.end);
        if m != Mark::None {
            let x = x0 + (column - left) as u16;
            grid.put(x, y, x_right, " ", 1, style_at(m, false), false);
        }
    }
}

/// Characters that reorder text on screen (bidirectional embeddings,
/// overrides, isolates, marks). Zero-width, so dropping them keeps columns.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Puts one grapheme of `cells` cells (never raw control characters).
fn put_grapheme(grid: &mut Grid, x: u16, y: u16, right: u16, g: &str, cells: usize, style: Style) {
    if g.chars().any(char::is_control) {
        // Every control grapheme other than tab (handled by the caller) is
        // one cell wide for unicode-width: draw it as one U+FFFD.
        grid.put(x, y, right, "\u{fffd}", 1, style, false);
        grid.fill(x + 1, y, (x + cells as u16).min(right), style);
    } else if g.chars().any(is_bidi_control) {
        let kept: String = g.chars().filter(|&c| !is_bidi_control(c)).collect();
        grid.put(x, y, right, &kept, cells, style, false);
    } else {
        // Terminals may disagree on this cluster's width: the cursor is
        // placed explicitly after it.
        grid.put(x, y, right, g, cells, style, disputed(g, cells));
    }
}

/// Whether terminals may draw cluster `g` other than `cells` wide: a
/// terminal that does not join clusters draws the sum of its chars'
/// widths; one that sizes a cluster by its first char draws that char's
/// width (a regional-indicator flag: 1, where `unicode-width` says 2).
fn disputed(g: &str, cells: usize) -> bool {
    let mut chars = g.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if chars.next().is_none() {
        return false;
    }
    let mut buf = [0; 4];
    let first = grapheme_width(first.encode_utf8(&mut buf), 0, NonZeroU8::MIN);
    per_char_width(g) != cells || first != cells
}

/// The width of `g` summed per char: what a terminal that does not join
/// clusters (ZWJ sequences, some emoji) draws.
fn per_char_width(g: &str) -> usize {
    let mut buf = [0; 4];
    g.chars()
        .map(|c| grapheme_width(c.encode_utf8(&mut buf), 0, NonZeroU8::MIN))
        .sum()
}

/// A document's name for the status and separator lines.
fn document_name(doc: &Document) -> String {
    let name = doc
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[scratch]".to_owned());
    let modified = if doc.is_modified() { " [+]" } else { "" };
    format!("{name}{modified}")
}

fn status_line(
    editor: &Editor,
    doc: &Document,
    selection: &Selection,
    line: usize,
    column: usize,
    latency_ns: Option<u64>,
) -> String {
    let n = selection.ranges().len();
    let cursors = if n == 1 {
        String::new()
    } else {
        format!("  {n} cursors")
    };
    // The input mode, and keys or a count typed so far.
    let mode = editor
        .modes()
        .info(editor.input_mode())
        .map_or_else(String::new, |m| m.name.to_uppercase());
    let mut pending = String::new();
    if let Some(count) = editor.count() {
        pending.push_str(&format!(" {count}"));
    }
    for &key in editor.keymap().pending() {
        pending.push(' ');
        pending.push_str(&key_name(key));
    }
    let latency = latency_ns.map_or_else(String::new, |ns| format!("  {} µs", ns / 1_000));
    format!(
        "{mode}{pending}  {}{cursors}  {}:{}{latency}  {}",
        document_name(doc),
        line + 1,
        column + 1,
        editor.message()
    )
}

/// The prompt's text and the display column of its cursor.
fn prompt_line(prompt: &Prompt) -> (String, usize) {
    let before = format!("{}{}", prompt.label(), &prompt.input()[..prompt.cursor()]);
    (
        format!("{}{}", prompt.label(), prompt.input()),
        text_width(&before),
    )
}

/// Puts `s` (without its first `skip` cells) at `(x, y)`, at most `width`
/// cells, by grapheme cluster: control characters as U+FFFD, bidi
/// controls dropped, disputed clusters marked as the text area's are.
fn put_text(grid: &mut Grid, x: u16, y: u16, width: u16, s: &str, skip: usize, style: Style) {
    let right = x + width;
    let mut column = 0usize;
    for g in s.graphemes(true) {
        let cells = text_cells(g);
        let end = column + cells;
        if end > skip + usize::from(width) {
            break;
        }
        if column >= skip && cells > 0 {
            let cx = x + (column - skip) as u16;
            put_grapheme(grid, cx, y, right, g, cells, style);
        }
        column = end;
    }
}

/// The cells grapheme `g` takes in status and prompt text: as in the text
/// area, a control grapheme one cell (drawn as U+FFFD).
fn text_cells(g: &str) -> usize {
    if g.chars().any(char::is_control) {
        1
    } else {
        grapheme_width(g, 0, NonZeroU8::MIN)
    }
}

/// Display width of `s` by grapheme cluster, as [`put_text`] draws it.
fn text_width(s: &str) -> usize {
    s.graphemes(true).map(text_cells).sum()
}

#[cfg(test)]
mod tests;
