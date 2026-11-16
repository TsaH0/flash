//! Motions: moving or extending every range of a selection.
//!
//! A motion computes a new head from a range's head. [`Movement::Move`]
//! replaces the range with a cursor at the new head; [`Movement::Extend`]
//! keeps the anchor and moves the head. Only vertical motion sets a goal
//! column on its result; every other motion clears it.
//!
//! Every motion reads only the text it moves over (plus an O(log n)
//! descent), never the whole document. Grapheme clusters are those of the
//! whole text ([`Text::graphemes`]). Heads need not be grapheme
//! boundaries; results are always char boundaries, and grapheme boundaries
//! whenever the head was one.
//!
//! # Word classes
//!
//! A grapheme's class is that of its first char: whitespace
//! (`char::is_whitespace`, so `\n`, `\r\n` and `\t` included), word
//! (alphanumeric or `_`), or punctuation (everything else). Forward word
//! motion skips the run of the head grapheme's class unless it is
//! whitespace, then skips whitespace: it lands on the start of the next
//! word. Backward word motion skips whitespace, then the run before it, and
//! lands on that run's start. Both stop at the edges of the text.
//!
//! # Vertical motion
//!
//! The goal is the range's remembered goal column, or the head's display
//! column (see [`crate::width`]). On the target line (clamped to the text),
//! the result is the start of the first grapheme of non-zero width whose
//! cells cover the goal, or the line's content end if none does.

use std::iter::Peekable;
use std::num::NonZeroU8;

use smallvec::SmallVec;

use crate::selection::{Range, Selection, normalise};
use crate::text::{Direction, Text, TextError};
use crate::width::{display_column, grapheme_width};

#[cfg(test)]
pub(crate) mod reference;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

/// Whether a motion moves the whole range or only its head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    /// Replace the range with a cursor at the new head.
    Move,
    /// Keep the anchor; move the head.
    Extend,
}

/// Where a motion takes a head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// `count` extended grapheme clusters, stopping at the text's edges.
    Grapheme { count: usize, direction: Direction },
    /// `count` words (see the module docs).
    Word { count: usize, direction: Direction },
    /// Start of the head's line.
    LineStart,
    /// End of the head's line content, before its terminator.
    LineEnd,
    /// `count` lines up or down, keeping a goal display column.
    Vertical {
        count: usize,
        direction: Direction,
        tab_width: NonZeroU8,
    },
    /// Offset 0.
    DocumentStart,
    /// The end of the text.
    DocumentEnd,
}

/// Faster answers about display columns on long lines, for vertical
/// motion: a view's [`Layout`](crate::layout::Layout). Every answer is an
/// optimisation only; `None` means "walk from the line start", and the
/// motion's result is the same either way.
pub trait Columns {
    /// The display column of `offset` (a char boundary of `text`).
    fn column(&mut self, text: &Text, offset: usize) -> Option<usize>;
    /// A grapheme boundary on `line` whose display column is at most
    /// `goal`, and that column.
    fn before(&mut self, text: &Text, line: usize, goal: usize) -> Option<(usize, usize)>;
}

/// No hints: walk every line from its start.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoColumns;

impl Columns for NoColumns {
    fn column(&mut self, _: &Text, _: usize) -> Option<usize> {
        None
    }
    fn before(&mut self, _: &Text, _: usize, _: usize) -> Option<(usize, usize)> {
        None
    }
}

impl Motion {
    /// Applies this motion to every range of `selection` and normalises the
    /// result (ranges that meet merge; the primary survives).
    ///
    /// Fails with the first invalid endpoint if `selection` is not valid for
    /// `text`.
    pub fn apply(
        self,
        text: &Text,
        selection: &Selection,
        movement: Movement,
    ) -> Result<Selection, TextError> {
        self.apply_with(text, selection, movement, &mut NoColumns)
    }

    /// [`apply`](Self::apply), with `columns` answering column questions
    /// on long lines (same result, less walking).
    pub fn apply_with(
        self,
        text: &Text,
        selection: &Selection,
        movement: Movement,
        columns: &mut dyn Columns,
    ) -> Result<Selection, TextError> {
        selection.validate(text)?;
        let ranges = selection
            .ranges()
            .iter()
            .map(|r| self.move_range(text, *r, movement, columns))
            .collect::<Result<SmallVec<[Range; 1]>, TextError>>()?;
        Ok(normalise(ranges, selection.primary_index()))
    }

    /// Applies this motion to one range. Fails if the anchor or head is not
    /// a valid position in `text`.
    pub fn apply_range(
        self,
        text: &Text,
        range: Range,
        movement: Movement,
    ) -> Result<Range, TextError> {
        self.apply_range_with(text, range, movement, &mut NoColumns)
    }

    /// [`apply_range`](Self::apply_range) with column hints.
    pub fn apply_range_with(
        self,
        text: &Text,
        range: Range,
        movement: Movement,
        columns: &mut dyn Columns,
    ) -> Result<Range, TextError> {
        text.check_offset(range.anchor())?;
        text.check_offset(range.head())?;
        self.move_range(text, range, movement, columns)
    }

    /// `range` must be valid for `text`.
    fn move_range(
        self,
        text: &Text,
        range: Range,
        movement: Movement,
        columns: &mut dyn Columns,
    ) -> Result<Range, TextError> {
        let (head, goal) = self.target(text, range, columns)?;
        let moved = match movement {
            Movement::Move => Range::point(head),
            Movement::Extend => Range::new(range.anchor(), head),
        };
        Ok(moved.with_goal_column(goal))
    }

    /// The new head and, for vertical motion, the goal column.
    fn target(
        self,
        text: &Text,
        range: Range,
        columns: &mut dyn Columns,
    ) -> Result<(usize, Option<usize>), TextError> {
        let head = range.head();
        Ok(match self {
            Motion::Grapheme { count, direction } => {
                (text.move_graphemes(head, count, direction)?, None)
            }
            Motion::Word { count, direction } => (word(text, head, count, direction)?, None),
            Motion::LineStart => (text.line_start(text.line_of_offset(head)?)?, None),
            Motion::LineEnd => (text.line_range(text.line_of_offset(head)?)?.end, None),
            Motion::Vertical {
                count,
                direction,
                tab_width,
            } => {
                let (head, goal) = vertical(text, range, count, direction, tab_width, columns)?;
                (head, Some(goal))
            }
            Motion::DocumentStart => (0, None),
            Motion::DocumentEnd => (text.byte_len(), None),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Whitespace,
    Word,
    Punctuation,
}

/// The class of a grapheme: that of its first char.
pub(crate) fn class(grapheme: &str) -> Class {
    match grapheme.chars().next() {
        Some(c) if c.is_whitespace() => Class::Whitespace,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        _ => Class::Punctuation,
    }
}

/// Advances `graphemes` over the run of graphemes of class `class`,
/// returning the bytes skipped.
fn skip_class<'a, I>(graphemes: &mut Peekable<I>, class_of_run: Class) -> usize
where
    I: Iterator<Item = std::borrow::Cow<'a, str>>,
{
    let mut skipped = 0;
    while let Some(g) = graphemes.next_if(|g| class(g) == class_of_run) {
        skipped += g.len();
    }
    skipped
}

/// Word motion from `head`. O(log n + distance).
fn word(text: &Text, head: usize, count: usize, direction: Direction) -> Result<usize, TextError> {
    let mut pos = head;
    match direction {
        Direction::Forward => {
            let mut graphemes = text.graphemes(head, Direction::Forward)?.peekable();
            for _ in 0..count {
                let Some(first) = graphemes.peek() else { break };
                let first = class(first);
                if first != Class::Whitespace {
                    pos += skip_class(&mut graphemes, first);
                }
                pos += skip_class(&mut graphemes, Class::Whitespace);
            }
        }
        Direction::Backward => {
            let mut graphemes = text.graphemes(head, Direction::Backward)?.peekable();
            for _ in 0..count {
                pos -= skip_class(&mut graphemes, Class::Whitespace);
                let Some(last) = graphemes.peek() else { break };
                let last = class(last);
                pos -= skip_class(&mut graphemes, last);
            }
        }
    }
    Ok(pos)
}

/// Vertical motion: the new head and the goal column. O(log n + columns
/// walked on the source and target lines, from `columns`' hints).
fn vertical(
    text: &Text,
    range: Range,
    count: usize,
    direction: Direction,
    tab_width: NonZeroU8,
    columns: &mut dyn Columns,
) -> Result<(usize, usize), TextError> {
    let head = range.head();
    let goal = match range.goal_column() {
        Some(goal) => goal,
        None => match columns.column(text, head) {
            Some(column) => column,
            None => display_column(text, head, tab_width)?,
        },
    };
    let line = text.line_of_offset(head)?;
    let last = text.line_count() - 1;
    let target = match direction {
        Direction::Forward => line.saturating_add(count).min(last),
        Direction::Backward => line.saturating_sub(count),
    };
    let content = text.line_range(target)?;
    // A grapheme covering `goal` cannot start before a boundary whose
    // column is at most `goal`: the scan may start at one.
    let (mut column, mut pos) = columns
        .before(text, target, goal)
        .filter(|&(o, c)| c <= goal && (content.start..=content.end).contains(&o))
        .map_or((0, content.start), |(o, c)| (c, o));
    for g in text.graphemes(pos, Direction::Forward)? {
        if pos >= content.end {
            break;
        }
        let width = grapheme_width(&g, column, tab_width);
        if width > 0 && column <= goal && goal < column + width {
            return Ok((pos, goal));
        }
        column += width;
        pos += g.len();
    }
    Ok((content.end, goal))
}
