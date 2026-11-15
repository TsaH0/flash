//! The walking skeleton's frontend: crossterm → core events, and a crude
//! full-redraw renderer over the headless editor.
//!
//! Throwaway by design (PLAN Stage 3): a full redraw every frame (replaced
//! in Stage 6). Editing state, keys, and scrolling live in
//! [`flash_core::Editor`]; this module only reads it.
//!
//! Text from the file is never written to the terminal raw: a control
//! character other than tab is drawn as U+FFFD (DECISIONS 012), so a file
//! cannot inject escape sequences.

use std::io::{self, Write};
use std::num::NonZeroU8;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self as ct, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType};
use flash_core::width::{display_column, grapheme_width};
use flash_core::{
    Direction, Document, Editor, Event, KeyCode, KeyEvent, Modifiers, Selection, View,
};

const TAB: NonZeroU8 = NonZeroU8::new(4).unwrap();

/// Converts a crossterm event to a core event. `None` for key releases and
/// for events the editor does not use (mouse, focus, unmapped keys).
pub fn convert(event: &ct::Event) -> Option<Event> {
    match event {
        ct::Event::Key(key) if key.kind != KeyEventKind::Release => {
            let mut modifiers = Modifiers {
                shift: key.modifiers.contains(KeyModifiers::SHIFT),
                ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
                alt: key.modifiers.contains(KeyModifiers::ALT),
            };
            let code = match key.code {
                ct::KeyCode::Char(c) => KeyCode::Char(c),
                ct::KeyCode::Enter => KeyCode::Enter,
                ct::KeyCode::Tab => KeyCode::Tab,
                ct::KeyCode::BackTab => {
                    modifiers.shift = true;
                    KeyCode::Tab
                }
                ct::KeyCode::Backspace => KeyCode::Backspace,
                ct::KeyCode::Delete => KeyCode::Delete,
                ct::KeyCode::Left => KeyCode::Left,
                ct::KeyCode::Right => KeyCode::Right,
                ct::KeyCode::Up => KeyCode::Up,
                ct::KeyCode::Down => KeyCode::Down,
                ct::KeyCode::Home => KeyCode::Home,
                ct::KeyCode::End => KeyCode::End,
                ct::KeyCode::PageUp => KeyCode::PageUp,
                ct::KeyCode::PageDown => KeyCode::PageDown,
                ct::KeyCode::Esc => KeyCode::Esc,
                ct::KeyCode::F(n) => KeyCode::F(n),
                _ => return None,
            };
            Some(Event::Key(KeyEvent::new(code, modifiers)))
        }
        ct::Event::Paste(text) => Some(Event::Paste { text: text.clone() }),
        ct::Event::Resize(width, height) => Some(Event::Resize {
            width: *width,
            height: *height,
        }),
        _ => None,
    }
}

/// Frontend state that is not editing state: the file name and the last
/// frame's latency, both shown on the status line.
pub struct Skeleton {
    name: String,
    last_latency_ns: u64,
}

impl Skeleton {
    pub fn new(name: String) -> Skeleton {
        Skeleton {
            name,
            last_latency_ns: 0,
        }
    }

    /// The latency shown on the status line, in nanoseconds.
    pub fn set_last_latency(&mut self, nanos: u64) {
        self.last_latency_ns = nanos;
    }

    /// Queues a full frame of `editor`'s focused view into `out` (the
    /// caller flushes). Queues nothing for a zero-sized screen; clears the
    /// screen if no view is focused.
    pub fn render(&self, editor: &Editor, out: &mut impl Write) -> io::Result<()> {
        let (width, height) = editor.screen();
        if width == 0 || height == 0 {
            return Ok(());
        }
        let Some((view, doc, selection)) = focused(editor) else {
            return queue!(out, Clear(ClearType::All));
        };
        let width = usize::from(width);
        let rows = usize::from(height - 1);
        let text = doc.text();
        let head = selection.primary().head();
        let head_line = text.line_of_offset(head).unwrap_or(0);

        queue!(out, Hide)?;
        for row in 0..rows {
            queue!(out, MoveTo(0, row as u16))?;
            let line = view.top() + row;
            if line < text.line_count() {
                render_line(out, doc, selection, line, width)?;
            }
            queue!(
                out,
                SetAttribute(Attribute::Reset),
                Clear(ClearType::UntilNewLine)
            )?;
        }

        let column = display_column(text, head, TAB).unwrap_or(0);
        let status = format!(
            "{}  {} cursors  {}:{}  {} µs  {}",
            self.name,
            selection.ranges().len(),
            head_line + 1,
            column + 1,
            self.last_latency_ns / 1_000,
            editor.message()
        );
        queue!(
            out,
            MoveTo(0, height - 1),
            SetAttribute(Attribute::Reverse),
            Print(fit(&status, width)),
            SetAttribute(Attribute::Reset),
            Clear(ClearType::UntilNewLine)
        )?;

        let x = column.min(width - 1) as u16;
        let y = head_line
            .saturating_sub(view.top())
            .min(rows.saturating_sub(1)) as u16;
        queue!(out, MoveTo(x, y), Show)
    }
}

/// The focused view, its document, and its selection.
fn focused(editor: &Editor) -> Option<(&View, &Document, &Selection)> {
    let focus = editor.focus()?;
    let (view, doc) = editor.view_document(focus)?;
    Some((view, doc, doc.selection(focus)?))
}

/// Draws one line's content, cut at `width` cells. Cells inside a non-empty
/// range, and secondary cursors, are drawn reversed.
/// O(log n + log k + visible cells).
fn render_line(
    out: &mut impl Write,
    doc: &Document,
    selection: &Selection,
    line: usize,
    width: usize,
) -> io::Result<()> {
    let text = doc.text();
    let (Ok(content), Ok(graphemes)) = (
        text.line_range(line),
        text.line_start(line)
            .and_then(|start| text.graphemes(start, Direction::Forward)),
    ) else {
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
