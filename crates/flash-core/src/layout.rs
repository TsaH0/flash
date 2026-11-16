//! Display lines: where an offset is on screen, without walking long lines
//! (PLAN Stage 7b, revision 3; replaces the Stage 6 line anchor,
//! DECISIONS 025).
//!
//! # The layout
//!
//! A line's *logical column* of an offset is the summed width of the
//! graphemes before it ([`display_column`](crate::width::display_column);
//! tabs expand on it). Without wrapping a line is one row. With a wrap
//! width `W`, a line is cut into rows of at most `W` cells, greedily: a
//! row ends after its last space or tab if the next grapheme would not
//! fit, else before that grapheme; a grapheme wider than `W` gets a row
//! of its own. The cursor cell after a line's last grapheme counts as one
//! cell, so a full last row is followed by an empty row where the cursor
//! at the line's end is drawn. A row's *visual* column is its logical
//! column minus the logical column of the row's start.
//!
//! # The cache
//!
//! Laying out a line is O(line). Lines shorter than [`LONG`] bytes are
//! laid out from their start on every query. A long line keeps *marks*:
//! points at least [`SPACING`] bytes apart (row starts when wrapping,
//! grapheme boundaries otherwise) with their logical column and row, so a
//! query walks from the nearest mark: O(log marks + [`SPACING`] + the
//! row width). Marks cover a prefix of the line and are extended lazily,
//! as far as a query needs.
//!
//! Each mark also records how far the layout had *read* to place it
//! (`dep`): a row start depends on the grapheme that did not fit before
//! it, and every mark on the character right after it (whether it is a
//! grapheme boundary). After the document changes, a mark is kept only if
//! the revision log shows that nothing touched its line from the line
//! start up to `dep` ([`RevisionLog`](crate::document::RevisionLog)'s
//! `map_untouched`); then its offset just shifts with the line. So an edit
//! keeps every mark before it, and the walk after the edit starts from the
//! last mark before it.

use std::num::{NonZeroU8, NonZeroUsize};

use crate::document::{Document, Revision};
use crate::text::{Direction, Text};
use crate::width::grapheme_width;

/// Lines at least this long (bytes) keep marks. Small in tests, so short
/// random lines exercise the cache.
pub(crate) const LONG: usize = if cfg!(test) { 16 } else { 2048 };

/// The least distance in bytes between marks.
pub(crate) const SPACING: usize = if cfg!(test) { 6 } else { 512 };

/// The least distance in bytes of a mark left where a walk to a column
/// stopped (so the next walk to about that column is short).
const STOP_SPACING: usize = if cfg!(test) { 3 } else { 16 };

/// Long lines with marks, at most. The least recently used is evicted.
/// Enough for every visible line of a large terminal.
const MAX_LINES: usize = 128;

#[cfg(test)]
thread_local! {
    /// Graphemes visited by layout walks on this thread (tests bound it).
    pub(crate) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn walked() {
    #[cfg(test)]
    WALKED.with(|w| w.set(w.get() + 1));
}

/// How lines are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutConfig {
    pub tab_width: NonZeroU8,
    /// Cells per row when wrapping; `None`: a line is one row.
    pub wrap: Option<NonZeroUsize>,
}

/// Where an offset is: its line, logical column, and row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub offset: usize,
    pub line: usize,
    pub line_start: usize,
    /// Logical display column (from the line start). If `offset` is inside
    /// a cluster, the cluster is not counted.
    pub column: usize,
    /// The row of the line holding `offset` (0 without wrapping).
    pub row: usize,
    /// Where that row starts, and its logical column.
    pub row_start: usize,
    pub row_column: usize,
}

impl Location {
    /// The column within the row: where the offset is drawn.
    pub fn visual_column(&self) -> usize {
        self.column - self.row_column
    }
}

/// One row of a line: `start..end` (`end` is the next row's start, or the
/// line's content end for its last row), starting at logical column
/// `column`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub column: usize,
    pub last: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    offset: usize,
    /// Logical column at `offset` (with wrapping, the row's start column).
    column: usize,
    /// The row starting at `offset` (with wrapping), else 0.
    row: usize,
    /// The layout read up to here (exclusive) to place this mark.
    dep: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LongLine {
    /// The line's start offset (at the layout's revision).
    start: usize,
    /// Sorted by offset; `marks[0]` is the line start.
    marks: Vec<Mark>,
    used: u64,
}

/// A view's display-line cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    config: LayoutConfig,
    revision: Revision,
    lines: Vec<LongLine>,
    clock: u64,
    /// The marks of a short line (its start alone), reused so queries
    /// allocate nothing.
    scratch: Vec<Mark>,
    /// The last location found, as a starting point for the next query
    /// (kept across edits by the marks' rule).
    hot: Option<Hot>,
}

/// The last location [`Layout::locate`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hot {
    line_start: usize,
    offset: usize,
    column: usize,
    /// Whether `offset` is a grapheme boundary (else not a walk origin).
    boundary: bool,
    /// Its row's start, logical column, index, and what placing it read.
    row_start: usize,
    row_column: usize,
    row: usize,
    row_dep: usize,
    /// What the whole point depends on (the character after `offset`
    /// included).
    dep: usize,
}

impl Layout {
    pub fn new(config: LayoutConfig) -> Layout {
        Layout {
            config,
            revision: Revision::ZERO,
            lines: Vec::new(),
            clock: 0,
            scratch: Vec::new(),
            hot: None,
        }
    }

    pub fn config(&self) -> LayoutConfig {
        self.config
    }

    /// The revision the marks are valid for.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Changes the layout (tab width, wrap width): every mark is dropped.
    pub(crate) fn set_config(&mut self, config: LayoutConfig) {
        if config != self.config {
            self.config = config;
            self.lines.clear();
            self.hot = None;
        }
    }

    /// Brings the marks to `doc`'s revision: keeps, on each cached line,
    /// the marks no change since touched (shifted with the line), and drops
    /// the rest. O(cached lines · log marks · log entries since).
    pub(crate) fn sync(&mut self, doc: &Document) {
        let (from, to) = (self.revision, doc.revision());
        if from == to {
            return;
        }
        self.revision = to;
        let text = doc.text();
        self.hot = self.hot.and_then(|h| {
            let (start, _) = doc.log().map_untouched(from, to, h.line_start, h.dep)?;
            let shift = |o: usize| o - h.line_start + start;
            let offset = shift(h.offset);
            Some(Hot {
                line_start: start,
                offset,
                // An insert right at it may have joined it to a cluster.
                boundary: h.boundary && text.is_grapheme_boundary(offset),
                row_start: shift(h.row_start),
                row_dep: shift(h.row_dep),
                dep: shift(h.dep),
                ..h
            })
        });
        self.lines.retain_mut(|l| {
            let untouched = |m: &Mark| doc.log().map_untouched(from, to, l.start, m.dep);
            // Untouched is monotone in `dep` (a longer prefix fails if a
            // shorter one does), and `dep` grows with the marks.
            let kept = l.marks.partition_point(|m| untouched(m).is_some());
            if kept == 0 {
                return false;
            }
            let Some((start, _)) = untouched(&l.marks[kept - 1]) else {
                return false;
            };
            l.marks.truncate(kept);
            let old = l.start;
            for m in &mut l.marks {
                m.offset = m.offset - old + start;
                m.dep = m.dep - old + start;
            }
            l.start = start;
            // A mark depends on the text before it; whether it is still a
            // grapheme boundary also on the character at it, which only an
            // edit right at the last kept mark can have changed.
            while l.marks.len() > 1 && !text.is_grapheme_boundary(l.marks[l.marks.len() - 1].offset)
            {
                l.marks.pop();
            }
            true
        });
        // Anything else in the text may have changed length: nothing to do,
        // marks only describe their own line's prefix.
    }

    /// Where `offset` is, laying its line out from the start (no cache):
    /// O(line). For readers without a view's cache.
    pub fn locate_from_start(config: LayoutConfig, text: &Text, offset: usize) -> Option<Location> {
        Layout::new(config).locate(text, offset)
    }

    /// Where `offset` is, extending the marks of its line as far as it.
    /// `None` if `offset` is not a valid offset in `text`. The layout must
    /// be [`sync`](Self::sync)ed with the document of `text`.
    pub(crate) fn locate(&mut self, text: &Text, offset: usize) -> Option<Location> {
        let line = text.line_of_offset(offset).ok()?;
        let range = text.line_range(line).ok()?;
        let config = self.config;
        let (start, end) = (range.start, range.end);
        // The last location on this line, if any: usually next to this
        // one (typing, moving by a grapheme).
        let hot = self.hot.filter(|h| h.line_start == start);
        let marks = self.marks_mut(start, end);
        let (row_start, row_column, row, row_dep, column) = match config.wrap {
            Some(width) => {
                let i = marks.partition_point(|m| m.offset <= offset) - 1;
                let m = marks[i];
                let (mut s, mut c, mut row, mut dep) = (m.offset, m.column, m.row, m.dep);
                if let Some(h) = hot.filter(|h| h.row_start > s && h.row_start <= offset) {
                    (s, c, row, dep) = (h.row_start, h.row_column, h.row, h.row_dep);
                }
                loop {
                    let r = lay_row(text, end, s, c, config.tab_width, width.get())?;
                    if r.last || offset < r.end {
                        break;
                    }
                    extend(marks, r.end, r.end_col, row + 1, r.read);
                    (s, c, row, dep) = (r.end, r.end_col, row + 1, r.read);
                }
                let column = walk(text, (s, c), offset, config.tab_width, None)?;
                (s, c, row, dep, column)
            }
            None => {
                let i = marks.partition_point(|m| m.offset <= offset) - 1;
                let m = marks[i];
                let mut from = (m.offset, m.column);
                let mut back = None;
                if let Some(h) = hot.filter(|h| h.boundary) {
                    if h.offset <= offset && h.offset > from.0 {
                        from = (h.offset, h.column);
                    } else if h.offset > offset && h.offset - offset < offset - from.0 {
                        back = walk_back(text, (h.offset, h.column), offset);
                    }
                }
                let column = match back {
                    Some(column) => column,
                    None => walk(text, from, offset, config.tab_width, Some(marks))?,
                };
                (start, 0, 0, start, column)
            }
        };
        let boundary = text.is_grapheme_boundary(offset);
        self.hot = Some(Hot {
            line_start: start,
            offset,
            column,
            boundary,
            row_start,
            row_column,
            row,
            row_dep,
            // The text before it; its boundary status is checked again on
            // every sync.
            dep: row_dep.max(offset),
        });
        Some(Location {
            offset,
            line,
            line_start: start,
            column,
            row,
            row_start,
            row_column,
        })
    }

    /// Row `row` of `line` with wrapping (its last row if there are fewer),
    /// extending the marks as far as it.
    fn row_at(&mut self, text: &Text, line: usize, row: usize) -> Option<Row> {
        let width = self.config.wrap?;
        let range = text.line_range(line).ok()?;
        let tab = self.config.tab_width;
        let marks = self.marks_mut(range.start, range.end);
        let i = marks.partition_point(|m| m.row <= row) - 1;
        let m = marks[i];
        let (mut s, mut c, mut index) = (m.offset, m.column, m.row);
        loop {
            let r = lay_row(text, range.end, s, c, tab, width.get())?;
            if r.last || index == row {
                return Some(Row {
                    index,
                    start: s,
                    end: r.end,
                    column: c,
                    last: r.last,
                });
            }
            extend(marks, r.end, r.end_col, index + 1, r.read);
            (s, c, index) = (r.end, r.end_col, index + 1);
        }
    }

    /// The number of rows of `line` (1 without wrapping), extending its
    /// marks to the end. O(line) the first time for a long line.
    pub(crate) fn row_count(&mut self, text: &Text, line: usize) -> Option<usize> {
        let Some(width) = self.config.wrap else {
            return Some(1);
        };
        let range = text.line_range(line).ok()?;
        let tab = self.config.tab_width;
        let marks = self.marks_mut(range.start, range.end);
        let m = marks[marks.len() - 1];
        let (mut s, mut c, mut row) = (m.offset, m.column, m.row);
        loop {
            let r = lay_row(text, range.end, s, c, tab, width.get())?;
            if r.last {
                return Some(row + 1);
            }
            extend(marks, r.end, r.end_col, row + 1, r.read);
            (s, c, row) = (r.end, r.end_col, row + 1);
        }
    }

    /// Without wrapping: extends the marks of `line` (if long) up to
    /// logical column `left`, so [`start_before`](Self::start_before)
    /// finds a mark near it. O(1) once they reach it.
    pub(crate) fn extend_to_column(&mut self, text: &Text, line: usize, left: usize) {
        if self.config.wrap.is_some() {
            return;
        }
        let Ok(range) = text.line_range(line) else {
            return;
        };
        if range.end - range.start < LONG {
            return;
        }
        let tab = self.config.tab_width;
        let marks = self.marks_mut(range.start, range.end);
        let last = marks[marks.len() - 1];
        if last.column >= left {
            return;
        }
        let (mut pos, mut column) = (last.offset, last.column);
        let Ok(graphemes) = text.graphemes(pos, Direction::Forward) else {
            return;
        };
        for g in graphemes {
            if pos >= range.end || column >= left {
                break;
            }
            walked();
            column += grapheme_width(&g, column, tab);
            pos += g.len();
            extend(marks, pos, column, 0, pos);
        }
        // A mark where the walk stopped, so the next walk to about this
        // column (scrolled by a few columns) starts near it.
        if column >= left && pos >= marks[marks.len() - 1].offset + STOP_SPACING {
            marks.push(Mark {
                offset: pos,
                column,
                row: 0,
                dep: pos,
            });
        }
    }

    /// Without wrapping: [`extend_to_column`](Self::extend_to_column) for
    /// the long lines among `height` lines from `top`. Short lines are
    /// skipped by scanning their bytes for the newline (at most [`LONG`]
    /// each), not by an O(log n) line lookup.
    pub(crate) fn extend_visible(&mut self, text: &Text, top: usize, height: usize, left: usize) {
        if self.config.wrap.is_some() || left == 0 {
            return;
        }
        let Ok(mut pos) = text.line_start(top) else {
            return;
        };
        let len = text.byte_len();
        for line in top..top.saturating_add(height) {
            // A line already cached is long: no scan.
            let cached = self.lines.iter().any(|l| l.start == pos);
            match (!cached).then(|| newline_within(text, pos, LONG)).flatten() {
                Some(nl) => pos = nl + 1,
                None => {
                    self.extend_to_column(text, line, left);
                    match text.line_start(line + 1) {
                        Ok(next) => pos = next,
                        Err(_) => return,
                    }
                }
            }
            if pos > len {
                return;
            }
        }
    }

    /// The rows of `line` from row `from` on, without changing the cache
    /// (for renderers). Uses the marks if they are valid for `doc`'s
    /// revision, else lays the line out from its start.
    pub fn rows<'a>(&'a self, doc: &'a Document, line: usize, from: usize) -> Rows<'a> {
        let text = doc.text();
        let Ok(range) = text.line_range(line) else {
            return Rows::empty(text);
        };
        let wrap = self.config.wrap;
        let first = self
            .marks(doc, range.start)
            .filter(|_| wrap.is_some())
            .and_then(|marks| {
                let i = marks.partition_point(|m| m.row <= from);
                i.checked_sub(1).map(|i| marks[i])
            })
            .map_or((range.start, 0, 0), |m| (m.offset, m.column, m.row));
        Rows {
            text,
            end: range.end,
            tab: self.config.tab_width,
            wrap,
            next: Some(first),
            skip_to: from,
        }
    }

    /// Where drawing the unwrapped `line` from logical column `left` can
    /// start: a grapheme boundary of the line with a column before `left`
    /// (or the line start), and its column. Without changing the cache;
    /// the marks are used only if valid for `doc`'s revision.
    pub fn start_before(&self, doc: &Document, line: usize, left: usize) -> (usize, usize) {
        let Ok(start) = doc.text().line_start(line) else {
            return (0, 0);
        };
        if self.config.wrap.is_some() {
            return (start, 0);
        }
        self.marks(doc, start)
            .and_then(|marks| {
                let i = marks.partition_point(|m| m.column < left);
                i.checked_sub(1).map(|i| (marks[i].offset, marks[i].column))
            })
            .unwrap_or((start, 0))
    }

    /// The marks of the cached line starting at `start`, if current.
    fn marks(&self, doc: &Document, start: usize) -> Option<&[Mark]> {
        if self.revision != doc.revision() {
            return None;
        }
        self.lines
            .iter()
            .find(|l| l.start == start)
            .map(|l| &l.marks[..])
    }

    /// The marks of the line `start..end` to walk from and extend: the
    /// cached ones for a long line (cached on first use), else the scratch
    /// marks holding the line start alone.
    fn marks_mut(&mut self, start: usize, end: usize) -> &mut Vec<Mark> {
        let first = Mark {
            offset: start,
            column: 0,
            row: 0,
            dep: start,
        };
        if end - start < LONG {
            self.scratch.clear();
            self.scratch.push(first);
            return &mut self.scratch;
        }
        self.clock += 1;
        let i = match self.lines.iter().position(|l| l.start == start) {
            Some(i) => i,
            None => {
                if self.lines.len() >= MAX_LINES {
                    let oldest = (0..self.lines.len())
                        .min_by_key(|&i| self.lines[i].used)
                        .unwrap_or(0);
                    self.lines.swap_remove(oldest);
                }
                self.lines.push(LongLine {
                    start,
                    marks: vec![first],
                    used: 0,
                });
                self.lines.len() - 1
            }
        };
        let line = &mut self.lines[i];
        line.used = self.clock;
        &mut line.marks
    }
}

/// Column hints for vertical motion. The layout must be
/// [`sync`](Layout::sync)ed with the document of `text`.
impl crate::motion::Columns for Layout {
    fn column(&mut self, text: &Text, offset: usize) -> Option<usize> {
        self.locate(text, offset).map(|l| l.column)
    }

    fn before(&mut self, text: &Text, line: usize, goal: usize) -> Option<(usize, usize)> {
        self.extend_to_column(text, line, goal);
        let start = text.line_start(line).ok()?;
        let marks = &self.lines.iter().find(|l| l.start == start)?.marks;
        let i = marks.partition_point(|m| m.column <= goal);
        i.checked_sub(1).map(|i| (marks[i].offset, marks[i].column))
    }

    /// Wrapped rows: `count` rows from `head`'s, crossing lines, at the
    /// first non-zero-width grapheme of the target row covering the goal
    /// (a visual column), else the row's last grapheme (the line end on
    /// a line's last row). Stops at the first and last rows.
    fn row(
        &mut self,
        text: &Text,
        head: usize,
        count: usize,
        direction: Direction,
        goal: Option<usize>,
    ) -> Option<(usize, usize)> {
        self.config.wrap?;
        let loc = self.locate(text, head)?;
        let goal = goal.unwrap_or(loc.visual_column());
        let (mut line, mut row) = (loc.line, loc.row);
        for _ in 0..count {
            match direction {
                Direction::Forward => {
                    if row + 1 < self.row_count(text, line)? {
                        row += 1;
                    } else if line + 1 < text.line_count() {
                        (line, row) = (line + 1, 0);
                    } else {
                        break;
                    }
                }
                Direction::Backward => {
                    if row > 0 {
                        row -= 1;
                    } else if line > 0 {
                        line -= 1;
                        row = self.row_count(text, line)? - 1;
                    } else {
                        break;
                    }
                }
            }
        }
        let r = self.row_at(text, line, row)?;
        let tab = self.config.tab_width;
        let (mut pos, mut column) = (r.start, r.column);
        let mut last = r.start;
        for g in text.graphemes(r.start, Direction::Forward).ok()? {
            if pos >= r.end {
                break;
            }
            let w = grapheme_width(&g, column, tab);
            let visual = column - r.column;
            if w > 0 && visual <= goal && goal < visual + w {
                return Some((pos, goal));
            }
            last = pos;
            column += w;
            pos += g.len();
        }
        Some((if r.last { r.end } else { last }, goal))
    }
}

/// The first `\n` at or after `pos` and before `pos + limit`, if any.
fn newline_within(text: &Text, mut pos: usize, limit: usize) -> Option<usize> {
    let stop = pos.saturating_add(limit);
    while pos < stop {
        let (chunk, start) = text.chunk_containing(pos)?;
        let bytes = &chunk.as_bytes()[pos - start..];
        let n = bytes.len().min(stop - pos);
        if let Some(i) = bytes[..n].iter().position(|&b| b == b'\n') {
            return Some(pos + i);
        }
        pos += n;
    }
    None
}

/// Appends a mark at `offset` if it extends the marks by at least
/// [`SPACING`] bytes (`marks` has one entry at least).
fn extend(marks: &mut Vec<Mark>, offset: usize, column: usize, row: usize, read: usize) {
    let last = marks[marks.len() - 1].offset;
    if offset >= last + SPACING {
        marks.push(Mark {
            offset,
            column,
            row,
            // Whether it is still a grapheme boundary is checked on sync.
            dep: read,
        });
    }
}

/// The logical column of `to`, walking from `from` (a grapheme boundary
/// at or before `to` on its line, at that column). With `marks` (no
/// wrapping), appends marks passed beyond the last one.
fn walk(
    text: &Text,
    from: (usize, usize),
    to: usize,
    tab: NonZeroU8,
    mut marks: Option<&mut Vec<Mark>>,
) -> Option<usize> {
    let (mut pos, mut column) = from;
    if pos == to {
        return Some(column);
    }
    for g in text.graphemes(pos, Direction::Forward).ok()? {
        if pos + g.len() > to {
            break;
        }
        walked();
        column += grapheme_width(&g, column, tab);
        pos += g.len();
        if let Some(marks) = marks.as_deref_mut() {
            extend(marks, pos, column, 0, pos);
        }
    }
    Some(column)
}

/// The logical column of `to`, walking back from `from` (a grapheme
/// boundary after `to` on its line, at that column). `None` if a tab lies
/// between (its width depends on where it starts). If `to` is inside a
/// cluster, the cluster is not counted, as walking forward does.
fn walk_back(text: &Text, from: (usize, usize), to: usize) -> Option<usize> {
    let (mut pos, mut column) = from;
    let mut graphemes = text.graphemes(pos, Direction::Backward).ok()?;
    while pos > to {
        let g = graphemes.next()?;
        if g == "\t" {
            return None;
        }
        walked();
        column = column.checked_sub(grapheme_width(&g, 0, NonZeroU8::MIN))?;
        pos -= g.len();
    }
    Some(column)
}

/// The end of one row.
struct RowEnd {
    /// Where the next row starts (the line end for the last row).
    end: usize,
    end_col: usize,
    /// The layout read up to here (exclusive) to decide `end`.
    read: usize,
    last: bool,
}

/// Lays out the row starting at `start` (logical column `col0`) of a line
/// whose content ends at `line_end`: at most `width` cells.
fn lay_row(
    text: &Text,
    line_end: usize,
    start: usize,
    col0: usize,
    tab: NonZeroU8,
    width: usize,
) -> Option<RowEnd> {
    let (mut pos, mut col) = (start, col0);
    let mut brk: Option<(usize, usize)> = None;
    if start < line_end {
        for g in text.graphemes(start, Direction::Forward).ok()? {
            if pos >= line_end {
                break;
            }
            walked();
            let w = grapheme_width(&g, col, tab);
            if col + w - col0 > width && pos > start {
                let (end, end_col) = brk.unwrap_or((pos, col));
                return Some(RowEnd {
                    end,
                    end_col,
                    read: pos + g.len(),
                    last: false,
                });
            }
            pos += g.len();
            col += w;
            if g == " " || g == "\t" {
                brk = Some((pos, col));
            }
        }
    }
    // The cursor cell after the line's last grapheme.
    if col + 1 - col0 > width && pos > start {
        return Some(RowEnd {
            end: pos,
            end_col: col,
            // The character at the line end: any grapheme there would
            // overflow the full row too, so only an edit at it matters.
            read: pos + 1,
            last: false,
        });
    }
    Some(RowEnd {
        end: line_end,
        end_col: col,
        read: line_end,
        last: true,
    })
}

/// Rows of a line, from [`Layout::rows`].
pub struct Rows<'a> {
    text: &'a Text,
    end: usize,
    tab: NonZeroU8,
    wrap: Option<NonZeroUsize>,
    /// The next row to lay out: start, column, index.
    next: Option<(usize, usize, usize)>,
    /// Rows before this index are laid out but not yielded.
    skip_to: usize,
}

impl<'a> Rows<'a> {
    fn empty(text: &'a Text) -> Rows<'a> {
        Rows {
            text,
            end: 0,
            tab: NonZeroU8::MIN,
            wrap: None,
            next: None,
            skip_to: 0,
        }
    }
}

impl Iterator for Rows<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            let (start, column, index) = self.next.take()?;
            let row = match self.wrap {
                None => Row {
                    index,
                    start,
                    end: self.end,
                    column,
                    last: true,
                },
                Some(width) => {
                    let r = lay_row(self.text, self.end, start, column, self.tab, width.get())?;
                    if !r.last {
                        self.next = Some((r.end, r.end_col, index + 1));
                    }
                    Row {
                        index,
                        start,
                        end: r.end,
                        column,
                        last: r.last,
                    }
                }
            };
            if index >= self.skip_to {
                return Some(row);
            }
            if row.last {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
