//! Writing a grid to the terminal (PLAN Stage 7c): what changed since the
//! previous frame, as few bytes as practical.
//!
//! Three strategies, compared by bytes and cells per frame
//! (DECISIONS 044):
//!
//! - [`Strategy::Full`]: every row, every frame (the Stage 6 renderer).
//! - [`Strategy::Rows`]: every row that changed, whole.
//! - [`Strategy::Cells`]: the changed runs of cells in each row. Runs
//!   separated by a few unchanged cells are joined (writing them is
//!   cheaper than moving the cursor); a run reaching blank cells to the
//!   end of its row ends with "erase to end of line".
//!
//! Styles are written only when they change (one SGR sequence with every
//! attribute, so no state is assumed). The cursor is moved only where the
//! next write is not where the terminal's cursor already is; after a
//! grapheme whose width terminals disagree on, its position is unknown.
//! A frame that changes cells is wrapped in synchronized-update escapes
//! with the cursor hidden; a frame that only moves the cursor writes just
//! that.

use std::io::Write;

use flash_core::theme::{Attributes, Color, Style};

use crate::grid::{Cell, Grid};

/// How a frame is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    Full,
    Rows,
    #[default]
    Cells,
}

impl Strategy {
    /// `full`, `rows`, or `cells`.
    pub fn parse(s: &str) -> Option<Strategy> {
        match s {
            "full" => Some(Strategy::Full),
            "rows" => Some(Strategy::Rows),
            "cells" => Some(Strategy::Cells),
            _ => None,
        }
    }
}

/// The terminal cursor's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Block,
    Bar,
    Underline,
}

/// Where the cursor is shown, if anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub x: u16,
    pub y: u16,
    pub shape: Shape,
}

/// What one frame wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub bytes: usize,
    /// Cells written (each grapheme counts once; erased cells count).
    pub cells: usize,
}

/// Unchanged cells worth writing to join two runs rather than moving the
/// cursor (a move is 6–8 bytes).
const JOIN_GAP: usize = 4;

/// The writer's knowledge of the terminal between frames.
#[derive(Debug, Default)]
pub struct Emitter {
    /// The cursor as last shown (`None`: hidden or unknown).
    cursor: Option<CursorState>,
    /// The shape last set.
    shape: Option<Shape>,
}

impl Emitter {
    /// Forgets the terminal's state (after a suspend or resize).
    pub fn reset(&mut self) {
        *self = Emitter::default();
    }

    /// Writes `next` to `out` given that the terminal shows `prev` (`None`:
    /// unknown, so everything is written after clearing the screen), then
    /// shows the cursor at `cursor` (or hides it).
    pub fn frame(
        &mut self,
        out: &mut Vec<u8>,
        prev: Option<&Grid>,
        next: &Grid,
        cursor: Option<CursorState>,
        strategy: Strategy,
    ) -> Stats {
        let start = out.len();
        let prev = prev.filter(|p| p.width() == next.width() && p.height() == next.height());
        let mut w = Writer {
            out,
            pen: Pen::default(),
            cells: 0,
        };
        let changed = match prev {
            None => true,
            Some(p) => (0..next.height()).any(|y| p.row(y) != next.row(y)),
        };
        if changed {
            w.raw(b"\x1b[?2026h\x1b[?25l");
            if prev.is_none() {
                w.raw(b"\x1b[0m\x1b[2J");
                w.pen.style = Some(Style::default());
            }
            for y in 0..next.height() {
                let row = next.row(y);
                let old = prev.map(|p| p.row(y));
                match (strategy, old) {
                    (Strategy::Cells, Some(old)) => w.row_cells(y, old, row),
                    (Strategy::Rows, Some(old)) if old == row => {}
                    _ => w.row_full(y, row),
                }
            }
        }
        // The cursor: hidden for the update, so placed and shown again;
        // else moved only if it moved.
        match cursor {
            Some(c) => {
                if changed || self.cursor.map(|o| (o.x, o.y)) != Some((c.x, c.y)) {
                    w.cup(c.x, c.y);
                }
                if self.shape != Some(c.shape) {
                    w.raw(match c.shape {
                        Shape::Block => b"\x1b[2 q",
                        Shape::Bar => b"\x1b[6 q",
                        Shape::Underline => b"\x1b[4 q",
                    });
                    self.shape = Some(c.shape);
                }
                if changed || self.cursor.is_none() {
                    w.raw(b"\x1b[?25h");
                }
            }
            None => {
                if !changed && self.cursor.is_some() {
                    w.raw(b"\x1b[?25l");
                }
            }
        }
        if changed {
            w.raw(b"\x1b[?2026l");
        }
        self.cursor = cursor;
        let cells = w.cells;
        Stats {
            bytes: out.len() - start,
            cells,
        }
    }
}

/// The terminal's cursor position and style while writing.
#[derive(Debug, Default)]
struct Pen {
    pos: Option<(u16, u16)>,
    style: Option<Style>,
}

struct Writer<'a> {
    out: &'a mut Vec<u8>,
    pen: Pen,
    cells: usize,
}

impl Writer<'_> {
    fn raw(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn cup(&mut self, x: u16, y: u16) {
        // Writing into a Vec cannot fail.
        let _ = write!(self.out, "\x1b[{};{}H", y + 1, x + 1);
        self.pen.pos = Some((x, y));
    }

    fn style(&mut self, style: Style) {
        if self.pen.style == Some(style) {
            return;
        }
        self.out.extend_from_slice(b"\x1b[0");
        let a = style.attributes;
        for (attr, code) in [
            (Attributes::BOLD, &b";1"[..]),
            (Attributes::DIM, b";2"),
            (Attributes::ITALIC, b";3"),
            (Attributes::UNDERLINE, b";4"),
            (Attributes::REVERSE, b";7"),
            (Attributes::STRIKE, b";9"),
        ] {
            if a.contains(attr) {
                self.out.extend_from_slice(code);
            }
        }
        color(self.out, style.fg, 30);
        color(self.out, style.bg, 40);
        self.out.push(b'm');
        self.pen.style = Some(style);
    }

    /// Writes the grapheme starting at cell `x` of row `y`, `cells` wide.
    fn cell(&mut self, x: u16, y: u16, cell: &Cell, cells: u16, width: u16) {
        if self.pen.pos != Some((x, y)) {
            self.cup(x, y);
        }
        self.style(cell.style);
        if cell.resync {
            // Terminals disagree on this cluster's width: blank every cell
            // it is given first, so one that draws it narrower leaves no
            // stale character beside it.
            for _ in 0..cells {
                self.out.push(b' ');
            }
            self.cup(x, y);
        }
        self.out.extend_from_slice(cell.symbol.as_bytes());
        self.cells += 1;
        let next = x + cells;
        // At the right edge the terminal is in its pending-wrap state; a
        // disputed width leaves the column unknown.
        self.pen.pos = (next < width && !cell.resync).then_some((next, y));
    }

    /// Erases from `x` to the end of row `y` (the default style's blank).
    fn erase(&mut self, x: u16, y: u16, cells: usize) {
        if self.pen.pos != Some((x, y)) {
            self.cup(x, y);
        }
        self.style(Style::default());
        self.raw(b"\x1b[K");
        self.cells += cells;
    }

    fn row_full(&mut self, y: u16, row: &[Cell]) {
        let tail = blank_tail(row);
        self.run(y, row, 0, tail);
        if tail < row.len() {
            self.erase(tail as u16, y, row.len() - tail);
        }
    }

    /// Writes the graphemes starting in cells `from..to` of `row`.
    fn run(&mut self, y: u16, row: &[Cell], from: usize, to: usize) {
        let width = row.len() as u16;
        for x in from..to {
            let cell = &row[x];
            if cell.is_continuation() {
                continue;
            }
            let cells = if x + 1 < row.len() && row[x + 1].is_continuation() {
                2
            } else {
                1
            };
            self.cell(x as u16, y, cell, cells, width);
        }
    }

    fn row_cells(&mut self, y: u16, old: &[Cell], row: &[Cell]) {
        let n = row.len();
        // Where the row's trailing default blanks start.
        let tail = blank_tail(row);
        let mut x = 0;
        while x < n {
            if old[x] == row[x] {
                x += 1;
                continue;
            }
            // A run starts at a grapheme's first cell.
            let mut start = x;
            while start > 0 && row[start].is_continuation() {
                start -= 1;
            }
            // Through the changed cells, and a short unchanged gap when
            // more changes follow it.
            let mut end = x + 1;
            loop {
                while end < n && old[end] != row[end] {
                    end += 1;
                }
                match (end..(end + JOIN_GAP).min(n)).find(|&i| old[i] != row[i]) {
                    Some(i) => end = i,
                    None => break,
                }
            }
            // Whole graphemes: through any continuation cells.
            while end < n && row[end].is_continuation() {
                end += 1;
            }
            if end > tail {
                // The run reaches the trailing blanks: write up to them
                // and erase the rest of the line.
                let from = start.max(tail);
                self.run(y, row, start, from);
                self.erase(from as u16, y, n - from);
                return;
            }
            self.run(y, row, start, end);
            x = end;
        }
    }
}

/// The index of the first cell of `row`'s trailing run of default blanks
/// (`row.len()` if it ends otherwise).
fn blank_tail(row: &[Cell]) -> usize {
    let blank = Cell::blank(Style::default());
    row.iter().rposition(|c| *c != blank).map_or(0, |i| i + 1)
}

/// The SGR parameters for `c` as a foreground (`base` 30) or background
/// (`base` 40) color.
fn color(out: &mut Vec<u8>, c: Color, base: u8) {
    let _ = match c {
        Color::Reset => Ok(()),
        Color::Ansi(i) if i < 8 => write!(out, ";{}", base + i),
        Color::Ansi(i) => write!(out, ";{}", base + 60 + (i - 8) % 8),
        Color::Indexed(i) => write!(out, ";{};5;{i}", base + 8),
        Color::Rgb(r, g, b) => write!(out, ";{};2;{r};{g};{b}", base + 8),
    };
}
