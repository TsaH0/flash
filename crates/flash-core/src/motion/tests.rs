//! Hand-written motion cases, Unicode-heavy.

use std::num::NonZeroU8;

use super::*;
use crate::width::{display_column, grapheme_width};
use crate::{Range, Selection, Text, TextError};

const T4: NonZeroU8 = NonZeroU8::new(4).unwrap();
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";

fn r(anchor: usize, head: usize) -> Range {
    Range::new(anchor, head)
}

/// Head after moving a cursor at `head` in `s`.
fn go(s: &str, head: usize, motion: Motion) -> usize {
    motion
        .apply_range(&Text::from(s), Range::point(head), Movement::Move)
        .unwrap()
        .head()
}

fn right(count: usize) -> Motion {
    Motion::Grapheme {
        count,
        direction: Direction::Forward,
    }
}

fn left(count: usize) -> Motion {
    Motion::Grapheme {
        count,
        direction: Direction::Backward,
    }
}

fn word(count: usize, direction: Direction) -> Motion {
    Motion::Word { count, direction }
}

fn vertical(count: usize, direction: Direction) -> Motion {
    Motion::Vertical {
        count,
        direction,
        tab_width: T4,
    }
}

fn all_motions() -> Vec<Motion> {
    let mut v = vec![
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocumentStart,
        Motion::DocumentEnd,
    ];
    for d in [Direction::Forward, Direction::Backward] {
        v.push(Motion::Grapheme {
            count: 1,
            direction: d,
        });
        v.push(Motion::Word {
            count: 2,
            direction: d,
        });
        v.push(vertical(1, d));
    }
    v
}

#[test]
fn grapheme_motion_over_clusters() {
    assert_eq!(go("e\u{301}x", 0, right(1)), 3);
    assert_eq!(go("e\u{301}x", 4, left(1)), 3);
    assert_eq!(go("e\u{301}x", 0, right(5)), 4);
    let fam = format!("{FAMILY}a");
    assert_eq!(go(&fam, 0, right(1)), 18);
    assert_eq!(go("a\r\nb", 1, right(1)), 3);
}

#[test]
fn move_collapses_extend_keeps_anchor() {
    let t = Text::from("abc");
    assert_eq!(
        right(1).apply_range(&t, r(0, 2), Movement::Move),
        Ok(Range::point(3))
    );
    assert_eq!(
        right(2).apply_range(&t, Range::point(0), Movement::Extend),
        Ok(r(0, 2))
    );
    assert_eq!(
        left(1).apply_range(&t, r(1, 3), Movement::Extend),
        Ok(r(1, 2))
    );
}

#[test]
fn line_start_and_end() {
    assert_eq!(go("ab\r\ncd", 5, Motion::LineStart), 4);
    assert_eq!(go("ab\r\ncd", 5, Motion::LineEnd), 6);
    assert_eq!(go("ab\r\ncd", 1, Motion::LineEnd), 2);
    assert_eq!(go("a\n", 0, Motion::LineEnd), 1);
    assert_eq!(go("a\n", 2, Motion::LineEnd), 2);
}

#[test]
fn document_start_and_end() {
    assert_eq!(go("ab\ncd", 3, Motion::DocumentStart), 0);
    assert_eq!(go("ab\ncd", 1, Motion::DocumentEnd), 5);
}

#[test]
fn word_forward() {
    let f = |s, h| go(s, h, word(1, Direction::Forward));
    assert_eq!(f("foo bar", 0), 4);
    assert_eq!(f("foo.bar", 0), 3);
    assert_eq!(f("foo.bar", 3), 4);
    assert_eq!(f("  foo", 0), 2);
    assert_eq!(f("a\r\nb", 0), 3);
    assert_eq!(f("foo bar", 4), 7);
    assert_eq!(go("foo bar baz", 0, word(2, Direction::Forward)), 8);
}

#[test]
fn word_backward() {
    let b = |s, h| go(s, h, word(1, Direction::Backward));
    assert_eq!(b("foo bar", 7), 4);
    assert_eq!(b("foo bar", 4), 0);
    assert_eq!(b("foo bar", 5), 4);
    assert_eq!(b("foo  ", 5), 0);
    assert_eq!(b("foo", 0), 0);
}

#[test]
fn word_over_combining_marks() {
    let s = "e\u{301}e\u{301} x";
    assert_eq!(go(s, 0, word(1, Direction::Forward)), 7);
    assert_eq!(go(s, 8, word(1, Direction::Backward)), 7);
    assert_eq!(go(s, 7, word(1, Direction::Backward)), 0);
}

#[test]
fn vertical_remembers_goal() {
    let t = Text::from("abcdef\nab\nabcdef");
    let down = vertical(1, Direction::Forward);
    let once = down
        .apply_range(&t, Range::point(5), Movement::Move)
        .unwrap();
    assert_eq!(once, Range::point(9).with_goal_column(Some(5)));
    let twice = down.apply_range(&t, once, Movement::Move).unwrap();
    assert_eq!(twice, Range::point(15).with_goal_column(Some(5)));
    let after = right(1).apply_range(&t, once, Movement::Move).unwrap();
    assert_eq!(after.goal_column(), None);
}

#[test]
fn vertical_expands_tabs() {
    assert_eq!(go("\tx\nabcdefg", 1, vertical(1, Direction::Forward)), 7);
}

#[test]
fn vertical_wide_chars() {
    assert_eq!(go("abcd\n漢字", 3, vertical(1, Direction::Forward)), 8);
}

#[test]
fn vertical_clamps_at_edges() {
    assert_eq!(go("abcd", 3, vertical(1, Direction::Backward)), 3);
    assert_eq!(go("a\nb\nc", 0, vertical(100, Direction::Forward)), 4);
}

#[test]
fn vertical_extend_keeps_anchor() {
    let t = Text::from("ab\ncd");
    let got = vertical(1, Direction::Forward)
        .apply_range(&t, Range::point(1), Movement::Extend)
        .unwrap();
    assert_eq!(got, r(1, 4).with_goal_column(Some(1)));
}

#[test]
fn motions_on_empty_text() {
    let t = Text::new();
    for m in all_motions() {
        assert_eq!(
            m.apply_range(&t, Range::point(0), Movement::Extend)
                .unwrap()
                .span(),
            0..0
        );
        let s = m.apply(&t, &Selection::point(0), Movement::Move).unwrap();
        assert_eq!(s.ranges().len(), 1);
        assert_eq!(s.primary().span(), 0..0, "{m:?}");
    }
}

#[test]
fn motions_from_inside_cluster() {
    assert_eq!(go("a\r\nb", 2, Motion::LineEnd), 1);
    assert_eq!(go("a\r\nb", 2, right(1)), 3);
    assert_eq!(go("a\r\nb", 2, word(1, Direction::Forward)), 3);
    let t = Text::from("e\u{301}");
    for m in all_motions() {
        for movement in [Movement::Move, Movement::Extend] {
            let got = m.apply_range(&t, Range::point(1), movement).unwrap();
            assert!(t.is_char_boundary(got.head()), "{m:?} gave {got:?}");
        }
    }
}

#[test]
fn motion_rejects_invalid_ranges() {
    assert_eq!(
        Motion::LineEnd.apply_range(&Text::from("ab"), Range::point(9), Movement::Move),
        Err(TextError::OffsetOutOfBounds { offset: 9, len: 2 })
    );
    assert_eq!(
        right(1).apply(&Text::from("é"), &Selection::point(1), Movement::Move),
        Err(TextError::NotCharBoundary { offset: 1 })
    );
    // The anchor is checked too.
    assert_eq!(
        right(1).apply_range(&Text::from("é"), r(1, 0), Movement::Extend),
        Err(TextError::NotCharBoundary { offset: 1 })
    );
}

#[test]
fn converging_cursors_merge() {
    let t = Text::from("abcdef");
    let s = Selection::new([Range::point(2), Range::point(5)], 1).unwrap();
    let got = Motion::DocumentStart.apply(&t, &s, Movement::Move).unwrap();
    assert_eq!(got, Selection::point(0));
    assert_eq!(got.primary_index(), 0);
}

#[test]
fn width_rules() {
    assert_eq!(grapheme_width("\t", 1, T4), 3);
    assert_eq!(grapheme_width("\t", 4, T4), 4);
    assert_eq!(grapheme_width("漢", 0, T4), 2);
    assert_eq!(grapheme_width("e\u{301}", 0, T4), 1);
    assert_eq!(grapheme_width(FAMILY, 0, T4), 2);
    assert_eq!(grapheme_width("\u{1b}", 0, T4), 1);
    assert_eq!(grapheme_width("\u{200b}", 0, T4), 0);
    assert_eq!(display_column(&Text::from("a\tb"), 2, T4), Ok(4));
    assert_eq!(display_column(&Text::from("ab\n漢x"), 6, T4), Ok(2));
    assert_eq!(
        display_column(&Text::from("ab"), 3, T4),
        Err(TextError::OffsetOutOfBounds { offset: 3, len: 2 })
    );
}
