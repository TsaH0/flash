//! Layout: the wrap rule by hand, and the cache against laying each line
//! out from its start, under random edits.

use std::num::{NonZeroU8, NonZeroUsize};

use proptest::prelude::*;
use slotmap::SlotMap;

use super::*;
use crate::editor::ViewId;
use crate::strategies::{floor_boundary, piece, text_strategy};
use crate::width::display_column;
use crate::{ChangeSet, Text, Transaction};

fn config(tab: u8, wrap: Option<usize>) -> LayoutConfig {
    LayoutConfig {
        tab_width: NonZeroU8::new(tab).unwrap(),
        wrap: wrap.and_then(NonZeroUsize::new),
    }
}

/// Every row of `line`, laid out from its start (the reference).
fn rows_from_start(text: &Text, line: usize, c: LayoutConfig) -> Vec<(usize, usize, usize)> {
    let range = text.line_range(line).unwrap();
    let Some(width) = c.wrap else {
        return vec![(range.start, range.end, 0)];
    };
    let mut rows = Vec::new();
    let (mut s, mut col) = (range.start, 0);
    loop {
        let r = lay_row(text, range.end, s, col, c.tab_width, width.get()).unwrap();
        rows.push((s, r.end, col));
        if r.last {
            return rows;
        }
        (s, col) = (r.end, r.end_col);
    }
}

/// Where `offset` is, from the reference rows.
fn locate_from_start(text: &Text, offset: usize, c: LayoutConfig) -> Location {
    let line = text.line_of_offset(offset).unwrap();
    let rows = rows_from_start(text, line, c);
    let row = rows
        .iter()
        .position(|&(_, end, _)| offset < end)
        .unwrap_or(rows.len() - 1);
    let (row_start, _, row_column) = rows[row];
    Location {
        offset,
        line,
        line_start: text.line_start(line).unwrap(),
        column: display_column(text, offset, c.tab_width).unwrap(),
        row,
        row_start,
        row_column,
    }
}

fn rows_of(text: &str, tab: u8, width: usize) -> Vec<String> {
    let t = Text::from(text);
    rows_from_start(&t, 0, config(tab, Some(width)))
        .into_iter()
        .map(|(s, e, _)| text[s..e].to_owned())
        .collect()
}

#[test]
fn rows_break_after_spaces_else_at_graphemes() {
    assert_eq!(rows_of("hello world foo", 4, 11), ["hello ", "world foo"]);
    assert_eq!(rows_of("hello world foo", 4, 12), ["hello world ", "foo"]);
    // No space in the row: cut at the grapheme that does not fit.
    assert_eq!(rows_of("abcdefgh", 4, 3), ["abc", "def", "gh"]);
    // A wide grapheme that does not fit moves to the next row.
    assert_eq!(rows_of("ab老c", 4, 3), ["ab", "老c", ""]);
    assert_eq!(rows_of("ab老", 4, 3), ["ab", "老"]);
    // A grapheme wider than the row gets a row of its own.
    assert_eq!(rows_of("老虎", 4, 1), ["老", "虎", ""]);
    // A full last row: the cursor cell at the line end gets a new row.
    assert_eq!(rows_of("abc", 4, 3), ["abc", ""]);
    assert_eq!(rows_of("ab", 4, 3), ["ab"]);
    assert_eq!(rows_of("", 4, 3), [""]);
    // Tabs expand on the logical column, also on later rows.
    assert_eq!(rows_of("ab\tc", 4, 4), ["ab\t", "c"]);
    // Combining marks stay with their base.
    assert_eq!(rows_of("abe\u{301}d", 4, 3), ["abe\u{301}", "d"]);
}

#[test]
fn locations_on_wrapped_rows() {
    let t = Text::from("hello world foo\nx");
    let c = config(4, Some(11));
    let mut layout = Layout::new(c);
    let at = |layout: &mut Layout, o| layout.locate(&t, o).unwrap();
    let w = at(&mut layout, 6);
    assert_eq!(
        (w.row, w.row_start, w.column, w.visual_column()),
        (1, 6, 6, 0)
    );
    let f = at(&mut layout, 12);
    assert_eq!((f.row, f.visual_column()), (1, 6));
    let x = at(&mut layout, 16);
    assert_eq!((x.line, x.row, x.column), (1, 0, 0));
    assert_eq!(layout.row_count(&t, 0), Some(2));
    assert_eq!(layout.row_count(&t, 1), Some(1));
    assert_eq!(layout.locate(&t, 99), None);
}

/// `g j` / `g k`: by wrapped rows with hints, else exactly by lines.
#[test]
fn row_motion_moves_by_wrapped_rows() {
    use crate::motion::{Motion, Movement};
    use crate::selection::Range;
    let t = Text::from("hello world foo\nbar");
    let rows = |count, dir, from: usize, wrap| {
        let motion = Motion::Row {
            count,
            direction: dir,
            tab_width: NonZeroU8::new(4).unwrap(),
        };
        let mut layout = Layout::new(config(4, wrap));
        let r = motion
            .apply_range_with(&t, Range::point(from), Movement::Move, &mut layout)
            .unwrap();
        (r.head(), r.goal_column())
    };
    // "hello " / "world foo" / "bar" at width 11. From the f of foo
    // (row 1, visual column 6): up lands on the row's last grapheme,
    // the space; down goes to the next line's end.
    assert_eq!(rows(1, Direction::Backward, 12, Some(11)), (5, Some(6)));
    assert_eq!(rows(1, Direction::Forward, 12, Some(11)), (19, Some(6)));
    // From the w (row 1, column 0) up: the h.
    assert_eq!(rows(1, Direction::Backward, 6, Some(11)), (0, Some(0)));
    // Two rows down from the h: the b of bar.
    assert_eq!(rows(2, Direction::Forward, 0, Some(11)), (16, Some(0)));
    // Stops at the edges.
    assert_eq!(rows(5, Direction::Backward, 3, Some(11)), (3, Some(3)));
    // Without wrapping it is vertical motion.
    for from in 0..=t.byte_len() {
        for dir in [Direction::Forward, Direction::Backward] {
            let vertical = Motion::Vertical {
                count: 1,
                direction: dir,
                tab_width: NonZeroU8::new(4).unwrap(),
            }
            .apply_range(&t, Range::point(from), Movement::Move)
            .unwrap();
            assert_eq!(
                rows(1, dir, from, None),
                (vertical.head(), vertical.goal_column())
            );
        }
    }
}

/// A long line laid out once: later queries near the cursor walk only
/// near it, also after edits there.
#[test]
fn queries_on_a_long_line_walk_near_the_offset() {
    let line = "ab\tc老虎é😀 ".repeat(400);
    let mut doc = Document::new(Text::from(line.as_str()), None);
    let view = SlotMap::<ViewId, ()>::with_key().insert(());
    doc.add_view(view);
    for wrap in [None, Some(13)] {
        let mut layout = Layout::new(config(4, wrap));
        let middle = floor_boundary(&line, line.len() / 2);
        layout.locate(doc.text(), middle).unwrap();
        let walked = || WALKED.with(|w| w.get());
        for step in 0..40 {
            // Type at the middle, then look it up.
            let at = middle + step;
            let cs = ChangeSet::from_edits(doc.text().byte_len(), [(at..at, "x")]).unwrap();
            doc.commit(view, Transaction::new(cs, None)).unwrap();
            layout.sync(&doc);
            let before = walked();
            let got = layout.locate(doc.text(), at + 1).unwrap();
            let n = walked() - before;
            assert!(n <= 4 * SPACING + 2 * 13, "walked {n} for step {step}");
            assert_eq!(got, locate_from_start(doc.text(), at + 1, config(4, wrap)));
        }
    }
}

/// A mark depends on the character after it: an insert right at a mark
/// that joins two clusters (a ZWJ between emoji) makes it a non-boundary,
/// so it must not survive (random edits rarely hit this).
#[test]
fn an_insert_joining_clusters_at_a_mark_drops_it() {
    let line = "😀".repeat(40);
    let mut doc = Document::new(Text::from(line.as_str()), None);
    let view = SlotMap::<ViewId, ()>::with_key().insert(());
    doc.add_view(view);
    let c = config(4, None);
    let mut layout = Layout::new(c);
    let end = doc.text().byte_len();
    layout.locate(doc.text(), end).unwrap();
    // Marks are at boundaries at least SPACING apart: find one.
    let mark = layout.lines[0].marks[1].offset;
    let cs = ChangeSet::from_edits(end, [(mark..mark, "\u{200d}")]).unwrap();
    doc.commit(view, Transaction::new(cs, None)).unwrap();
    layout.sync(&doc);
    let end = doc.text().byte_len();
    assert_eq!(
        layout.locate(doc.text(), end).unwrap(),
        locate_from_start(doc.text(), end, c)
    );
}

#[derive(Debug, Clone)]
enum Op {
    Edit(prop::sample::Index, prop::sample::Index, String),
    Locate(prop::sample::Index),
    RowCount(prop::sample::Index),
    Rows(prop::sample::Index, usize),
    StartBefore(prop::sample::Index, usize),
    ExtendTo(prop::sample::Index, usize),
    Vertical(prop::sample::Index, usize, bool, Option<usize>),
    RowMotion(prop::sample::Index, usize, bool, Option<usize>),
    Configure(u8, Option<usize>),
}

fn op() -> impl Strategy<Value = Op> {
    let ix = any::<prop::sample::Index>;
    prop_oneof![
        3 => (ix(), ix(), prop::collection::vec(piece(), 0..3).prop_map(|v| v.concat()))
            .prop_map(|(a, b, s)| Op::Edit(a, b, s)),
        4 => ix().prop_map(Op::Locate),
        1 => ix().prop_map(Op::RowCount),
        2 => (ix(), 0..6usize).prop_map(|(l, r)| Op::Rows(l, r)),
        1 => (ix(), 0..60usize).prop_map(|(l, c)| Op::StartBefore(l, c)),
        1 => (ix(), 0..80usize).prop_map(|(l, c)| Op::ExtendTo(l, c)),
        2 => (ix(), 1..4usize, any::<bool>(), prop::option::of(0..80usize))
            .prop_map(|(o, n, down, goal)| Op::Vertical(o, n, down, goal)),
        2 => (ix(), 1..4usize, any::<bool>(), prop::option::of(0..20usize))
            .prop_map(|(o, n, down, goal)| Op::RowMotion(o, n, down, goal)),
        1 => (1..9u8, prop::option::of(1..14usize)).prop_map(|(t, w)| Op::Configure(t, w)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every query agrees with laying the line out from its start, after
    /// any sequence of edits, queries, and configuration changes.
    #[test]
    fn cache_matches_laying_out_from_the_start(
        s in text_strategy(24),
        tab in 1..9u8,
        wrap in prop::option::of(1..14usize),
        ops in prop::collection::vec(op(), 1..40),
    ) {
        let mut doc = Document::new(Text::from(s.as_str()), None);
        let view = SlotMap::<ViewId, ()>::with_key().insert(());
        doc.add_view(view);
        let mut c = config(tab, wrap);
        let mut layout = Layout::new(c);
        for op in ops {
            let text = doc.text().clone();
            let string = text.to_string();
            let lines = text.line_count();
            match op {
                Op::Edit(a, b, ins) => {
                    let a = floor_boundary(&string, a.index(string.len() + 1));
                    let b = floor_boundary(&string, b.index(string.len() + 1));
                    let cs = ChangeSet::from_edits(string.len(), [(a.min(b)..a.max(b), ins)])
                        .unwrap();
                    doc.commit(view, Transaction::new(cs, None)).unwrap();
                    layout.sync(&doc);
                }
                Op::Locate(i) => {
                    let o = floor_boundary(&string, i.index(string.len() + 1));
                    prop_assert_eq!(layout.locate(&text, o).unwrap(), locate_from_start(&text, o, c));
                }
                Op::RowCount(i) => {
                    let line = i.index(lines);
                    prop_assert_eq!(
                        layout.row_count(&text, line).unwrap(),
                        rows_from_start(&text, line, c).len()
                    );
                }
                Op::Rows(i, from) => {
                    let line = i.index(lines);
                    let got: Vec<(usize, usize, usize)> = layout
                        .rows(&doc, line, from)
                        .map(|r| (r.start, r.end, r.column))
                        .collect();
                    let want: Vec<_> = rows_from_start(&text, line, c).into_iter().skip(from).collect();
                    prop_assert_eq!(got, want);
                }
                Op::StartBefore(i, left) => {
                    let line = i.index(lines);
                    let (o, col) = layout.start_before(&doc, line, left);
                    let range = text.line_range(line).unwrap();
                    prop_assert!(range.contains(&o) || o == range.start || o == range.end);
                    prop_assert!(text.is_grapheme_boundary(o));
                    prop_assert_eq!(display_column(&text, o, c.tab_width).unwrap(), col);
                    prop_assert!(col < left || o == range.start, "{} at {} for left {}", o, col, left);
                }
                Op::Vertical(i, count, down, goal) => {
                    use crate::motion::{Motion, Movement};
                    use crate::selection::Range;
                    let o = floor_boundary(&string, i.index(string.len() + 1));
                    let range = Range::point(o);
                    let range = match goal {
                        Some(g) => range.with_goal_column(Some(g)),
                        None => range,
                    };
                    let motion = Motion::Vertical {
                        count,
                        direction: if down { Direction::Forward } else { Direction::Backward },
                        tab_width: c.tab_width,
                    };
                    let hinted = motion.apply_range_with(&text, range, Movement::Move, &mut layout);
                    prop_assert_eq!(hinted, motion.apply_range(&text, range, Movement::Move));
                }
                Op::RowMotion(i, count, down, goal) => {
                    use crate::motion::Columns;
                    let o = floor_boundary(&string, i.index(string.len() + 1));
                    let dir = if down { Direction::Forward } else { Direction::Backward };
                    let cached = layout.row(&text, o, count, dir, goal);
                    let fresh = Layout::new(c).row(&text, o, count, dir, goal);
                    prop_assert_eq!(cached, fresh);
                    if let Some((head, _)) = cached {
                        prop_assert!(text.is_grapheme_boundary(head));
                    }
                }
                Op::ExtendTo(i, left) => {
                    layout.extend_to_column(&text, i.index(lines), left);
                }
                Op::Configure(t, w) => {
                    c = config(t, w);
                    layout.set_config(c);
                }
            }
        }
    }
}
