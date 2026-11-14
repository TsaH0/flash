//! Naive `String` model for word and vertical motion: whole-string grapheme
//! segmentation and the `RefText` line model, no rope, no laziness.

use std::num::NonZeroU8;

use unicode_segmentation::UnicodeSegmentation;

use crate::text::Direction;
use crate::text::reference::RefText;
use crate::width::grapheme_width;

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Space,
    Word,
    Other,
}

fn kind(g: &str) -> Kind {
    let c = g.chars().next().expect("graphemes are non-empty");
    if c.is_whitespace() {
        Kind::Space
    } else if c.is_alphanumeric() || c == '_' {
        Kind::Word
    } else {
        Kind::Other
    }
}

/// Word motion from `head`, a grapheme boundary of `s`.
pub(crate) fn word_naive(s: &str, head: usize, count: usize, dir: Direction) -> usize {
    let gs: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
    let start_of = |i: usize| gs.get(i).map_or(s.len(), |g| g.0);
    // Index of the first grapheme at or after `head`.
    let mut i = gs.iter().position(|g| g.0 >= head).unwrap_or(gs.len());
    for _ in 0..count {
        match dir {
            Direction::Forward => {
                if i == gs.len() {
                    break;
                }
                let k = kind(gs[i].1);
                if k != Kind::Space {
                    while i < gs.len() && kind(gs[i].1) == k {
                        i += 1;
                    }
                }
                while i < gs.len() && kind(gs[i].1) == Kind::Space {
                    i += 1;
                }
            }
            Direction::Backward => {
                while i > 0 && kind(gs[i - 1].1) == Kind::Space {
                    i -= 1;
                }
                if i == 0 {
                    break;
                }
                let k = kind(gs[i - 1].1);
                while i > 0 && kind(gs[i - 1].1) == k {
                    i -= 1;
                }
            }
        }
    }
    start_of(i)
}

fn column_naive(s: &str, from: usize, to: usize, tab: NonZeroU8) -> usize {
    s[from..to]
        .graphemes(true)
        .fold(0, |col, g| col + grapheme_width(g, col, tab))
}

/// Vertical motion from `head` (a grapheme boundary): new head and goal.
pub(crate) fn vertical_naive(
    s: &str,
    head: usize,
    goal: Option<usize>,
    count: usize,
    dir: Direction,
    tab: NonZeroU8,
) -> (usize, usize) {
    let t = RefText::new(s);
    let line = t.line_of_offset(head).unwrap();
    let goal = goal.unwrap_or_else(|| column_naive(s, t.line_start(line).unwrap(), head, tab));
    let target = match dir {
        Direction::Forward => (line + count).min(t.line_count() - 1),
        Direction::Backward => line.saturating_sub(count),
    };
    let content = t.line_range(target).unwrap();
    let mut col = 0;
    for (off, g) in s[content.clone()].grapheme_indices(true) {
        let w = grapheme_width(g, col, tab);
        if w > 0 && col <= goal && goal < col + w {
            return (content.start + off, goal);
        }
        col += w;
    }
    (content.end, goal)
}
