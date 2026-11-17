//! A screen of cells (PLAN Stage 7c): what a frame should show, built by
//! composing views, separators, and the status or prompt line, then
//! compared with the previous frame's grid to write only what changed
//! ([`crate::emit`]).

use compact_str::CompactString;
use flash_core::theme::Style;

/// One cell: the grapheme drawn there and its style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme drawn starting here, or `""` for the cell a wide
    /// grapheme to its left also covers.
    pub symbol: CompactString,
    pub style: Style,
    /// Terminals disagree on this grapheme's width (a cluster whose
    /// per-character widths differ from its cluster width): the cursor is
    /// placed explicitly after drawing it.
    pub resync: bool,
}

impl Cell {
    /// A space in `style`.
    pub fn blank(style: Style) -> Cell {
        Cell {
            symbol: CompactString::const_new(" "),
            style,
            resync: false,
        }
    }

    /// Whether this cell is covered by the wide grapheme to its left.
    pub fn is_continuation(&self) -> bool {
        self.symbol.is_empty()
    }
}

/// A `width` × `height` screen of cells, row-major.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Grid {
        let mut g = Grid::default();
        g.reset(width, height, Style::default());
        g
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Resizes to `width` × `height` and fills with blanks in `style`,
    /// keeping the allocation.
    pub fn reset(&mut self, width: u16, height: u16, style: Style) {
        self.width = width;
        self.height = height;
        let n = usize::from(width) * usize::from(height);
        self.cells.clear();
        self.cells.resize(n, Cell::blank(style));
    }

    pub fn cell(&self, x: u16, y: u16) -> &Cell {
        &self.cells[usize::from(y) * usize::from(self.width) + usize::from(x)]
    }

    /// Row `y`'s cells.
    pub fn row(&self, y: u16) -> &[Cell] {
        let w = usize::from(self.width);
        let start = usize::from(y) * w;
        &self.cells[start..start + w]
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }

    /// Draws `symbol` (`width` cells wide, 1 or 2) at `(x, y)`, clipped to
    /// `right` (exclusive) and the grid: a grapheme that does not fit is
    /// drawn as blanks in its style. Repairs any wide grapheme it cuts in
    /// half. Width 0 draws nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &mut self,
        x: u16,
        y: u16,
        right: u16,
        symbol: &str,
        width: usize,
        style: Style,
        resync: bool,
    ) {
        let right = right.min(self.width);
        if width == 0 || x >= right || y >= self.height {
            return;
        }
        let end = usize::from(x) + width;
        if end > usize::from(right) {
            for cx in x..right {
                self.set(cx, y, Cell::blank(style));
            }
            return;
        }
        self.set(
            x,
            y,
            Cell {
                symbol: CompactString::new(symbol),
                style,
                resync,
            },
        );
        for cx in (usize::from(x) + 1)..end {
            self.set(
                cx as u16,
                y,
                Cell {
                    symbol: CompactString::const_new(""),
                    style,
                    resync: false,
                },
            );
        }
    }

    /// Sets one cell, blanking the other half of a wide grapheme it
    /// overwrites part of.
    fn set(&mut self, x: u16, y: u16, cell: Cell) {
        let i = self.index(x, y);
        // Overwriting the right half of a wide grapheme: blank its left.
        if self.cells[i].is_continuation() && x > 0 {
            let style = self.cells[i - 1].style;
            self.cells[i - 1] = Cell::blank(style);
        }
        // Overwriting the left half: blank its right (unless this cell
        // is itself a continuation being written).
        if !cell.is_continuation()
            && x + 1 < self.width
            && self.cells[i + 1].is_continuation()
        {
            let style = self.cells[i + 1].style;
            self.cells[i + 1] = Cell::blank(style);
        }
        self.cells[i] = cell;
    }

    /// Fills `x..right` of row `y` with blanks in `style`.
    pub fn fill(&mut self, x: u16, y: u16, right: u16, style: Style) {
        if y >= self.height {
            return;
        }
        for cx in x..right.min(self.width) {
            self.set(cx, y, Cell::blank(style));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(g: &Grid, y: u16) -> String {
        g.row(y).iter().map(|c| c.symbol.as_str()).collect()
    }

    #[test]
    fn wide_graphemes_are_clipped_and_repaired() {
        let mut g = Grid::new(4, 1);
        g.put(0, 0, 4, "老", 2, Style::default(), false);
        g.put(2, 0, 4, "虎", 2, Style::default(), false);
        assert_eq!(text(&g, 0), "老虎");
        assert!(g.cell(1, 0).is_continuation());
        // Cut by the right edge: blanks.
        g.put(3, 0, 4, "漢", 2, Style::default(), false);
        assert_eq!(text(&g, 0), "老  ");
        // Overwriting half of one blanks the other half.
        g.put(1, 0, 4, "x", 1, Style::default(), false);
        assert_eq!(text(&g, 0), " x  ");
        g.put(0, 0, 4, "a", 1, Style::default(), false);
        g.put(2, 0, 4, "b", 1, Style::default(), false);
        g.put(1, 0, 4, "c", 1, Style::default(), false);
        assert_eq!(text(&g, 0), "acb ");
        // Out of bounds: nothing.
        g.put(9, 0, 4, "z", 1, Style::default(), false);
        g.put(0, 3, 4, "z", 1, Style::default(), false);
        assert_eq!(text(&g, 0), "acb ");
    }
}
