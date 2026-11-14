//! The Stage 3 walking skeleton: a crude editor over the real core APIs.
//!
//! Throwaway by design (PLAN Stage 3): hard-coded keys (deleted in Stage 7),
//! a full redraw every frame (replaced in Stage 6), no save. Its job is to
//! exercise selections, motions, and transactions for real and to give an
//! end-to-end latency number from this stage on.
//!
//! Text from the file is never written to the terminal raw: a control
//! character other than tab is drawn as U+FFFD (DECISIONS 012), so a file
//! cannot inject escape sequences.

use std::io::{self, Write};
use std::num::NonZeroU8;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType};
use flash_core::width::{display_column, grapheme_width};
use flash_core::{
    Direction, Motion, Movement, Range, Selection, Text, TextError, Transaction, TransactionError,
};

const TAB: NonZeroU8 = NonZeroU8::new(4).unwrap();

/// Editor state for one file in one terminal.
pub struct Skeleton {
    name: String,
    text: Text,
    selection: Selection,
    /// First visible line.
    top: usize,
    width: u16,
    height: u16,
    last_latency_ns: u64,
    message: String,
    quit: bool,
}

impl Skeleton {
    /// A skeleton editing `text`, with a cursor at 0.
    pub fn new(name: String, text: Text, width: u16, height: u16) -> Skeleton {
        Skeleton {
            name,
            text,
            selection: Selection::point(0),
            top: 0,
            width,
            height,
            last_latency_ns: 0,
            message: String::new(),
            quit: false,
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The status-line message: the last error, or empty.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Replaces the selection if it is valid for the text.
    pub fn set_selection(&mut self, selection: Selection) -> Result<(), TextError> {
        selection.validate(&self.text)?;
        self.selection = selection;
        Ok(())
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The latency shown on the status line, in nanoseconds.
    pub fn set_last_latency(&mut self, nanos: u64) {
        self.last_latency_ns = nanos;
    }

    /// Applies one input event. Never panics; a failed core call leaves the
    /// state unchanged and sets the status message.
    pub fn handle(&mut self, event: &Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(*key),
            Event::Paste(s) => self.edit(|t, sel| Transaction::insert(t, sel, s)),
            Event::Resize(width, height) => (self.width, self.height) = (*width, *height),
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let movement = if key.modifiers.contains(KeyModifiers::SHIFT) {
            Movement::Extend
        } else {
            Movement::Move
        };
        let grapheme = |direction| Motion::Grapheme {
            count: 1,
            direction,
        };
        let word = |direction| Motion::Word {
            count: 1,
            direction,
        };
        let vertical = |direction| Motion::Vertical {
            count: 1,
            direction,
            tab_width: TAB,
        };
        use Direction::{Backward, Forward};
        match key.code {
            KeyCode::Char('q') if ctrl => self.quit = true,
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut buf = [0; 4];
                let s: &str = c.encode_utf8(&mut buf);
                self.edit(|t, sel| Transaction::insert(t, sel, s));
            }
            KeyCode::Enter => self.edit(|t, sel| Transaction::insert(t, sel, "\n")),
            KeyCode::Tab => self.edit(|t, sel| Transaction::insert(t, sel, "\t")),
            KeyCode::Backspace => self.edit(Transaction::backspace),
            KeyCode::Delete => self.edit(Transaction::delete_forward),
            KeyCode::Left if ctrl => self.motion(word(Backward), movement),
            KeyCode::Right if ctrl => self.motion(word(Forward), movement),
            KeyCode::Left => self.motion(grapheme(Backward), movement),
            KeyCode::Right => self.motion(grapheme(Forward), movement),
            KeyCode::Up if alt => self.add_cursor(Backward),
            KeyCode::Down if alt => self.add_cursor(Forward),
            KeyCode::Up => self.motion(vertical(Backward), movement),
            KeyCode::Down => self.motion(vertical(Forward), movement),
            KeyCode::Home if ctrl => self.motion(Motion::DocumentStart, movement),
            KeyCode::End if ctrl => self.motion(Motion::DocumentEnd, movement),
            KeyCode::Home => self.motion(Motion::LineStart, movement),
            KeyCode::End => self.motion(Motion::LineEnd, movement),
            KeyCode::Esc => self.selection = Selection::point(self.selection.primary().head()),
            _ => {}
        }
    }

    fn edit(
        &mut self,
        make: impl FnOnce(&Text, &Selection) -> Result<Transaction, TransactionError>,
    ) {
        let result = make(&self.text, &self.selection)
            .and_then(|txn| txn.apply(&mut self.text, &mut self.selection));
        self.report(result.err());
    }

    fn motion(&mut self, motion: Motion, movement: Movement) {
        match motion.apply(&self.text, &self.selection, movement) {
            Ok(selection) => {
                self.selection = selection;
                self.report(None::<TextError>);
            }
            Err(e) => self.report(Some(e)),
        }
    }

    /// Adds a cursor one line below the last range (or above the first),
    /// at its goal column, as the new primary.
    fn add_cursor(&mut self, direction: Direction) {
        let ranges = self.selection.ranges();
        let from = match direction {
            Direction::Forward => ranges[ranges.len() - 1],
            Direction::Backward => ranges[0],
        };
        let motion = Motion::Vertical {
            count: 1,
            direction,
            tab_width: TAB,
        };
        let start = Range::point(from.head()).with_goal_column(from.goal_column());
        match motion.apply_range(&self.text, start, Movement::Move) {
            Ok(range) => {
                self.selection = self.selection.with_range(range);
                self.report(None::<TextError>);
            }
            Err(e) => self.report(Some(e)),
        }
    }

    fn report(&mut self, error: Option<impl std::fmt::Display>) {
        self.message = error.map(|e| e.to_string()).unwrap_or_default();
    }

    /// Queues a full frame into `out` (the caller flushes). Scrolls so the
    /// primary head is visible. Queues nothing for a zero-sized terminal.
    pub fn render(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        let width = usize::from(self.width);
        let rows = usize::from(self.height - 1);
        let head = self.selection.primary().head();
        let head_line = self.text.line_of_offset(head).unwrap_or(0);
        if head_line < self.top {
            self.top = head_line;
        } else if rows > 0 && head_line >= self.top + rows {
            self.top = head_line + 1 - rows;
        }

        queue!(out, Hide)?;
        for row in 0..rows {
            queue!(out, MoveTo(0, row as u16))?;
            let line = self.top + row;
            if line < self.text.line_count() {
                self.render_line(out, line, width)?;
            }
            queue!(
                out,
                SetAttribute(Attribute::Reset),
                Clear(ClearType::UntilNewLine)
            )?;
        }

        let column = display_column(&self.text, head, TAB).unwrap_or(0);
        let status = format!(
            "{}  {} cursors  {}:{}  {} µs  {}",
            self.name,
            self.selection.ranges().len(),
            head_line + 1,
            column + 1,
            self.last_latency_ns / 1_000,
            self.message
        );
        queue!(
            out,
            MoveTo(0, self.height - 1),
            SetAttribute(Attribute::Reverse),
            Print(fit(&status, width)),
            SetAttribute(Attribute::Reset),
            Clear(ClearType::UntilNewLine)
        )?;

        let x = column.min(width - 1) as u16;
        let y = if rows == 0 {
            0
        } else {
            (head_line - self.top) as u16
        };
        queue!(out, MoveTo(x, y), Show)
    }

    /// Draws one line's content, cut at `width` cells. Cells inside a
    /// non-empty range, and secondary cursors, are drawn reversed.
    /// O(log n + log k + visible cells).
    fn render_line(&self, out: &mut impl Write, line: usize, width: usize) -> io::Result<()> {
        let (Ok(content), Ok(graphemes)) = (
            self.text.line_range(line),
            self.text
                .line_start(line)
                .and_then(|start| self.text.graphemes(start, Direction::Forward)),
        ) else {
            return Ok(());
        };
        let ranges = self.selection.ranges();
        let primary = self.selection.primary_index();
        // First range that can touch this line.
        let mut next = ranges.partition_point(|r| r.end() < content.start);
        let mut marked = |pos: usize| {
            while next < ranges.len() && ranges[next].end() < pos {
                next += 1;
            }
            // At most two ranges involve `pos`: one ending there, one
            // starting there.
            (next..ranges.len().min(next + 2)).any(|i| {
                let r = ranges[i];
                (!r.is_empty() && r.start() <= pos && pos < r.end())
                    || (i != primary && r.head() == pos)
            })
        };

        let (mut column, mut pos, mut reversed) = (0, content.start, false);
        for g in graphemes {
            if pos >= content.end {
                break;
            }
            let cells = grapheme_width(&g, column, TAB);
            if column + cells > width {
                return Ok(());
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
            if g == "\t" {
                queue!(out, Print(" ".repeat(cells)))?;
            } else if g.chars().any(char::is_control) {
                queue!(out, Print('\u{fffd}'))?;
            } else {
                queue!(out, Print(&*g))?;
            }
            column += cells;
            pos += g.len();
        }
        // A secondary cursor at the end of the line.
        if column < width && marked(content.end) && pos == content.end {
            queue!(out, SetAttribute(Attribute::Reverse), Print(' '))?;
        }
        Ok(())
    }
}

/// `s` cut to at most `width` cells, with control characters replaced by
/// U+FFFD.
fn fit(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut column = 0;
    for c in s.chars() {
        let c = if c.is_control() { '\u{fffd}' } else { c };
        let mut buf = [0; 4];
        let cells = grapheme_width(c.encode_utf8(&mut buf), column, TAB);
        if column + cells > width {
            break;
        }
        out.push(c);
        column += cells;
    }
    out
}

#[cfg(test)]
#[path = "skeleton_tests.rs"]
mod tests;
