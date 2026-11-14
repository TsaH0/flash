//! Hand-written cases for selection normalisation and mapping.

use super::*;
use crate::{ChangeSet, TextError};

fn r(anchor: usize, head: usize) -> Range {
    Range::new(anchor, head)
}

fn sel(ranges: &[Range], primary: usize) -> Selection {
    Selection::new(ranges.iter().copied(), primary).unwrap()
}

#[test]
fn range_accessors() {
    let b = r(5, 2);
    assert_eq!((b.anchor(), b.head(), b.start(), b.end()), (5, 2, 2, 5));
    assert_eq!(b.span(), 2..5);
    assert_eq!(b.direction(), Direction::Backward);
    assert_eq!(r(2, 5).direction(), Direction::Forward);
    assert_eq!(Range::point(3).direction(), Direction::Forward);
    assert!(Range::point(3).is_empty());
    assert!(!b.is_empty());
    assert_eq!(b.goal_column(), None);
    assert_eq!(b.with_goal_column(Some(7)).goal_column(), Some(7));
}

#[test]
fn normalise_sorts_and_tracks_primary() {
    let s = sel(&[r(10, 12), r(0, 2), r(5, 6)], 0);
    assert_eq!(s.ranges(), &[r(0, 2), r(5, 6), r(10, 12)]);
    assert_eq!(s.primary_index(), 2);
    assert_eq!(*s.primary(), r(10, 12));
}

#[test]
fn overlapping_ranges_merge() {
    assert_eq!(sel(&[r(0, 5), r(3, 8)], 0).ranges(), &[r(0, 8)]);
}

#[test]
fn touching_non_empty_ranges_stay_separate() {
    let s = sel(&[r(0, 3), r(3, 6)], 1);
    assert_eq!(s.ranges(), &[r(0, 3), r(3, 6)]);
    assert_eq!(s.primary_index(), 1);
}

#[test]
fn empty_touching_or_inside_merges() {
    assert_eq!(sel(&[r(0, 3), r(3, 3)], 0).ranges(), &[r(0, 3)]);
    assert_eq!(sel(&[r(3, 3), r(3, 6)], 0).ranges(), &[r(3, 6)]);
    assert_eq!(sel(&[r(2, 2), r(0, 5)], 0).ranges(), &[r(0, 5)]);
    assert_eq!(sel(&[r(4, 4), r(4, 4)], 1).ranges(), &[r(4, 4)]);
}

#[test]
fn merged_direction_follows_primary() {
    assert_eq!(sel(&[r(0, 5), r(8, 3)], 1).ranges(), &[r(8, 0)]);
    assert_eq!(sel(&[r(0, 5), r(8, 3)], 0).ranges(), &[r(0, 8)]);
}

#[test]
fn merged_direction_without_primary_follows_first() {
    let s = sel(&[r(5, 0), r(3, 8), r(20, 21)], 2);
    assert_eq!(s.ranges(), &[r(8, 0), r(20, 21)]);
    assert_eq!(s.primary_index(), 1);
}

#[test]
fn merged_range_keeps_kept_goal_column() {
    let a = r(0, 5).with_goal_column(Some(9));
    let s = sel(&[a, r(3, 8)], 0);
    assert_eq!(s.ranges(), &[r(0, 8).with_goal_column(Some(9))]);
}

#[test]
fn new_rejects_empty_and_bad_primary() {
    assert_eq!(Selection::new([], 0), Err(SelectionError::Empty));
    assert_eq!(
        Selection::new([r(0, 0)], 1),
        Err(SelectionError::PrimaryOutOfBounds { primary: 1, len: 1 })
    );
}

#[test]
fn validate_reports_bad_endpoints() {
    assert_eq!(
        Selection::point(1).validate(&Text::from("é")),
        Err(TextError::NotCharBoundary { offset: 1 })
    );
    assert_eq!(
        Selection::point(3).validate(&Text::from("ab")),
        Err(TextError::OffsetOutOfBounds { offset: 3, len: 2 })
    );
    assert_eq!(
        Selection::single(r(2, 0)).validate(&Text::from("ab")),
        Ok(())
    );
}

#[test]
fn with_range_and_primary_only() {
    let s = Selection::point(0).with_range(r(4, 4));
    assert_eq!(s.ranges(), &[r(0, 0), r(4, 4)]);
    assert_eq!(s.primary_index(), 1);
    assert_eq!(s.primary_only(), Selection::point(4));
}

#[test]
fn ten_thousand_cursors_merge() {
    let s = Selection::new((0..10_000).map(|_| r(0, 0)), 9_999).unwrap();
    assert_eq!(s, Selection::point(0));
}

fn edits(len: usize, e: &[(std::ops::Range<usize>, &str)]) -> ChangeSet {
    ChangeSet::from_edits(len, e.iter().cloned()).unwrap()
}

#[test]
fn map_cursor_moves_past_insert() {
    let c = edits(2, &[(1..1, "XY")]);
    assert_eq!(Selection::point(1).map(&c), Ok(Selection::point(3)));
}

#[test]
fn map_range_does_not_absorb_edge_inserts() {
    let c = edits(4, &[(1..1, "X"), (3..3, "Y")]);
    assert_eq!(
        Selection::single(r(1, 3)).map(&c),
        Ok(Selection::single(r(2, 4)))
    );
    assert_eq!(
        Selection::single(r(3, 1)).map(&c),
        Ok(Selection::single(r(4, 2)))
    );
}

#[test]
fn map_deleted_range_collapses() {
    let c = edits(4, &[(0..4, "")]);
    assert_eq!(Selection::single(r(1, 3)).map(&c), Ok(Selection::point(0)));
}

#[test]
fn map_touching_after_delete() {
    let c = edits(3, &[(1..2, "")]);
    assert_eq!(
        sel(&[r(0, 1), r(2, 3)], 0).map(&c).unwrap().ranges(),
        &[r(0, 1), r(1, 2)]
    );
    let merged = sel(&[r(0, 1), r(2, 2)], 1).map(&c).unwrap();
    assert_eq!(merged.ranges(), &[r(0, 1)]);
    assert_eq!(merged.primary_index(), 0);
}

#[test]
fn map_clears_goal_column() {
    let c = ChangeSet::identity(3);
    let s = Selection::single(r(0, 2).with_goal_column(Some(4)));
    assert_eq!(s.map(&c), Ok(Selection::single(r(0, 2))));
}

#[test]
fn map_on_empty_document() {
    let c = edits(0, &[(0..0, "hi")]);
    assert_eq!(Selection::point(0).map(&c), Ok(Selection::point(2)));
}

#[test]
fn map_rejects_out_of_bounds() {
    let c = ChangeSet::identity(2);
    assert_eq!(
        Selection::point(3).map(&c),
        Err(crate::ChangeSetError::PositionOutOfBounds {
            position: 3,
            len: 2
        })
    );
}

#[test]
fn map_range_inside_replacement_covers_it() {
    let c = edits(10, &[(0..10, "XYZ")]);
    assert_eq!(
        Selection::single(r(3, 7)).map(&c),
        Ok(Selection::single(r(0, 3)))
    );
    assert_eq!(
        Selection::single(r(7, 3)).map(&c),
        Ok(Selection::single(r(3, 0)))
    );
    let gone = edits(10, &[(0..10, "")]);
    assert_eq!(
        Selection::single(r(3, 7)).map(&gone),
        Ok(Selection::point(0))
    );
}
