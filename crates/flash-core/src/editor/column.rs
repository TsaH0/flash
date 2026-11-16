//! The primary head's display column, kept without walking long lines
//! (PLAN 4: `one-line` must be slow-but-bounded; Stage 5 handled a key on
//! it in microseconds).
//!
//! A column is the summed width of the graphemes from the line's start
//! ([`display_column`](crate::width::display_column)), so the obvious
//! computation is O(line). Each view instead keeps a [`LineAnchor`] from
//! its last scroll, with up to three checkpoints (a grapheme boundary and
//! its column): the head; the start of the line's visible part; and a
//! *rear* checkpoint between [`REAR`] and `2 * REAR` bytes behind the
//! head, kept on lines longer than that. A checkpoint stays valid across
//! edits that leave its line's prefix alone
//! ([`RevisionLog::map_untouched`](crate::document::RevisionLog)) if it was
//! a grapheme boundary then and still is, and the new column is a walk
//! from the nearest valid checkpoint: forward (exact, tabs included) or
//! backward (only over non-tab graphemes, whose width does not depend on
//! where they start).
//!
//! So a key that types, deletes, moves, undoes or redoes near the head
//! walks at most about `2 * REAR` bytes, and moving the rear checkpoint up
//! is paid for by the distance the head moved. Only a jump along a long
//! line, or an edit at least `REAR` bytes behind the head, walks from the
//! line start.

use std::num::NonZeroU8;

use crate::document::{Document, Revision};
use crate::text::{Direction, Text};
use crate::width::grapheme_width;

/// How far behind the head, in bytes, the rear checkpoint is placed.
/// Small in tests, so random short lines exercise it.
pub(crate) const REAR: usize = if cfg!(test) { 16 } else { 1024 };

/// What a view knows about its primary head's line after it scrolled, so
/// neither the next scroll nor a renderer walks a long line again. Valid
/// only while `revision`, `head`, and `tab_width` still match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineAnchor {
    pub revision: Revision,
    /// The primary head, its line, and where that line starts.
    pub head: usize,
    pub line: usize,
    pub line_start: usize,
    /// The head's display column.
    pub head_column: usize,
    /// Whether the head is a grapheme boundary. If not, its column leaves
    /// out the cluster around it, so it is no checkpoint for later walks.
    pub head_on_boundary: bool,
    pub tab_width: NonZeroU8,
    /// Where drawing `line` can start: `(offset, column)`, the first
    /// grapheme boundary at or before the view's `left` column. `None` if
    /// the head is inside a cluster or a tab lies between the two (then a
    /// renderer walks from the line start).
    pub start: Option<(usize, usize)>,
    /// A grapheme boundary on `line` between [`REAR`] and `2 * REAR`
    /// bytes behind the head, and its column: where the next column walk
    /// restarts after an edit just behind the head. `None` on a line
    /// whose start is that close.
    pub rear: Option<(usize, usize)>,
}

/// The head's line, line start, and column, and the new rear checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Located {
    pub(crate) line: usize,
    pub(crate) line_start: usize,
    pub(crate) column: usize,
    pub(crate) rear: Option<(usize, usize)>,
    /// The previous anchor's left-edge checkpoint, if still valid (mapped
    /// to the current text): reusable while the view's `left` is the same.
    pub(crate) old_start: Option<(usize, usize)>,
}

#[cfg(test)]
thread_local! {
    /// Graphemes visited by the column walks on this thread (tests check
    /// that local edits do not walk the line).
    pub(crate) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn walked() {
    #[cfg(test)]
    WALKED.with(|w| w.set(w.get() + 1));
}

/// Locates `head`, walking from the nearest checkpoint of `old` (the
/// view's previous anchor) that is still valid, else from the line start.
pub(crate) fn locate(
    doc: &Document,
    old: Option<&LineAnchor>,
    head: usize,
    tab: NonZeroU8,
) -> Option<Located> {
    let text = doc.text();
    let line = text.line_of_offset(head).ok()?;
    let line_start = text.line_start(line).ok()?;
    // The nearest known column at or before `head`, and after it; and the
    // old rear checkpoint, if still valid and not after `head`.
    let mut before = (line_start, 0);
    let mut after: Option<(usize, usize)> = None;
    let (mut old_rear, mut old_start) = (None, None);
    for (a, kind, (offset, column)) in old
        .filter(|a| a.tab_width == tab)
        .into_iter()
        .flat_map(|a| {
            let head = a.head_on_boundary.then_some((a.head, a.head_column));
            [
                (head, Mark::Head),
                (a.start, Mark::Start),
                (a.rear, Mark::Rear),
            ]
            .map(|(m, kind)| m.map(|m| (a, kind, m)))
        })
        .flatten()
    {
        let Some((start, offset)) =
            doc.log()
                .map_untouched(a.revision, doc.revision(), a.line_start, offset)
        else {
            continue;
        };
        // A boundary then and now: the column counts the same graphemes.
        if start != line_start || !text.is_grapheme_boundary(offset) {
            continue;
        }
        if kind == Mark::Start {
            old_start = Some((offset, column));
        }
        if offset <= head {
            if kind == Mark::Rear {
                old_rear = Some((offset, column));
            }
            if offset > before.0 {
                before = (offset, column);
            }
        } else if after.is_none_or(|(o, _)| offset < o) {
            after = Some((offset, column));
        }
    }
    // Where the rear checkpoint goes: the last boundary at or before
    // `limit`, unless the old one is still close enough.
    let limit = head.saturating_sub(REAR);
    let backward = after
        .filter(|&(o, _)| o - head < head - before.0)
        .and_then(|(o, c)| walk_back(text, o, c, head));
    let (column, passed) = match backward {
        Some(column) => (column, None),
        None => walk_forward(text, before, head, tab, limit)?,
    };
    let rear = if head - line_start <= REAR {
        None
    } else if let Some(r) = old_rear.filter(|r| head - r.0 <= 2 * REAR) {
        Some(r)
    } else if passed.is_some() {
        passed
    } else {
        // Move the rear checkpoint up from the old one: as far as the head
        // moved since it was placed.
        let from = old_rear.unwrap_or((line_start, 0));
        walk_forward(text, from, limit, tab, limit)?.1
    };
    Some(Located {
        line,
        line_start,
        column,
        rear,
        old_start,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Head,
    Start,
    Rear,
}

/// The display column of `to`, given that `from.0` (a grapheme boundary at
/// or before `to` on the same line) is at column `from.1`. Matches
/// [`display_column`](crate::width::display_column): a cluster containing
/// `to` is not counted. Also returns the last grapheme boundary passed at
/// or before `limit`, with its column, if any.
fn walk_forward(
    text: &Text,
    from: (usize, usize),
    to: usize,
    tab: NonZeroU8,
    limit: usize,
) -> Option<(usize, Option<(usize, usize)>)> {
    let (mut pos, mut column) = from;
    let mut passed = None;
    for g in text.graphemes(pos, Direction::Forward).ok()? {
        if pos <= limit {
            passed = Some((pos, column));
        }
        if pos + g.len() > to {
            break;
        }
        walked();
        column += grapheme_width(&g, column, tab);
        pos += g.len();
    }
    if pos <= limit {
        passed = Some((pos, column));
    }
    Some((column, passed))
}

/// Like [`walk_forward`], from a grapheme boundary `from` after `to`, at
/// `column`. `None` if a tab lies between (its width depends on where it
/// starts).
fn walk_back(text: &Text, from: usize, column: usize, to: usize) -> Option<usize> {
    let (mut column, mut pos) = (column, from);
    let mut graphemes = text.graphemes(from, Direction::Backward).ok()?;
    // Stops at or before `to`; before it if `to` is inside a cluster,
    // which is then not counted, as walking forward does.
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

/// The same point as [`line_start_at`], found by walking forward from
/// `from` (a grapheme boundary on the head's line at or before `left`, at
/// or before `head`): the last boundary at or before `left`, or the first
/// one at exactly `left` (zero-width graphemes there are drawn, so the
/// start goes before them, even when `from` is already at `left`).
/// O(distance from `from` to `left`, plus zero-width graphemes before it).
pub(crate) fn line_start_from(
    text: &Text,
    from: (usize, usize),
    head: usize,
    left: usize,
    tab: NonZeroU8,
) -> Option<(usize, usize)> {
    let (mut pos, mut column) = from;
    if column == left {
        // Already at `left`: zero-width graphemes just before also sit
        // there and are drawn, so the start goes before them (as
        // `line_start_at` does). `from` is on a line, so this stops at its
        // start at the latest (a line break is not zero-width here).
        for g in text.graphemes(pos, Direction::Backward).ok()? {
            if g.ends_with('\n') || g == "\t" || grapheme_width(&g, 0, NonZeroU8::MIN) != 0 {
                break;
            }
            walked();
            pos -= g.len();
        }
        return Some((pos, column));
    }
    for g in text.graphemes(pos, Direction::Forward).ok()? {
        let width = grapheme_width(&g, column, tab);
        if column == left || column + width > left {
            break;
        }
        walked();
        column += width;
        pos += g.len();
    }
    // The head's column is at least `left`, so the walk stops by then.
    debug_assert!(pos <= head, "line start {pos} past the head {head}");
    Some((pos, column))
}

/// The first grapheme boundary at or before display column `left` on the
/// line of `head` (a grapheme boundary at display column `head_column`),
/// found by walking back from the head: `left` is within one view width
/// of the head, so this is O(width). `None` (the renderer then walks from
/// the line start) if `head` is inside a cluster or a tab is in the way
/// (a tab's width depends on where it starts, which walking back does not
/// know).
pub(crate) fn line_start_at(
    text: &Text,
    head: usize,
    head_column: usize,
    left: usize,
) -> Option<(usize, usize)> {
    if !text.is_grapheme_boundary(head) {
        return None;
    }
    let width = |g: &str| grapheme_width(g, 0, NonZeroU8::MIN);
    let (mut offset, mut column) = (head, head_column);
    let mut graphemes = text.graphemes(head, Direction::Backward).ok()?.peekable();
    loop {
        if column <= left {
            if column == left {
                // Zero-width graphemes just before also sit at `left`,
                // and a walk from the line start draws them: include them.
                while let Some(g) =
                    graphemes.next_if(|g| !g.ends_with('\n') && g != "\t" && width(g) == 0)
                {
                    offset -= g.len();
                }
            }
            return Some((offset, column));
        }
        let g = graphemes.next()?;
        if g == "\t" || g.ends_with('\n') {
            return None;
        }
        // Not a tab: the width does not depend on the column.
        column = column.checked_sub(width(&g))?;
        offset -= g.len();
    }
}
