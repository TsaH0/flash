//! Hand-written cases for the documented behaviour of [`ChangeSet`].

#![allow(
    clippy::reversed_empty_ranges,
    reason = "inverted ranges are deliberate invalid inputs"
)]

use super::*;
use crate::Text;

fn ins(s: &str) -> Op {
    Op::Insert(s.into())
}

#[test]
fn builder_canonicalises() {
    let mut b = ChangeSetBuilder::new(9);
    b.retain(2)
        .retain(0)
        .retain(3)
        .insert("a")
        .insert("")
        .delete(1)
        .insert("b")
        .delete(2)
        .retain(1);
    let c = b.finish().unwrap();
    assert_eq!(
        c.ops(),
        &[Op::Retain(5), Op::Delete(3), ins("ab"), Op::Retain(1)]
    );
    assert_eq!(c.len_before(), 9);
    assert_eq!(c.len_after(), 8);
}

#[test]
fn builder_rejects_length_mismatch() {
    let mut b = ChangeSetBuilder::new(5);
    b.retain(3);
    assert_eq!(
        b.finish(),
        Err(ChangeSetError::LengthMismatch {
            expected: 5,
            actual: 3
        })
    );
    let mut b = ChangeSetBuilder::new(5);
    b.retain(6);
    assert_eq!(
        b.finish(),
        Err(ChangeSetError::LengthMismatch {
            expected: 5,
            actual: 6
        })
    );
}

#[test]
fn builder_overflow_is_an_error() {
    let mut b = ChangeSetBuilder::new(usize::MAX);
    b.retain(usize::MAX).retain(1);
    assert_eq!(b.finish(), Err(ChangeSetError::Overflow));

    let mut b = ChangeSetBuilder::new(usize::MAX);
    b.retain(usize::MAX).insert("a");
    assert_eq!(b.finish(), Err(ChangeSetError::Overflow));

    let c = ChangeSet::from_edits(usize::MAX, [(usize::MAX - 1..usize::MAX, "")]).unwrap();
    assert_eq!(c.ops(), &[Op::Retain(usize::MAX - 1), Op::Delete(1)]);
}

#[test]
fn from_edits_cases() {
    // Inserts at one position concatenate in input order.
    let c = ChangeSet::from_edits(4, [(2..2, "a"), (2..2, "b")]).unwrap();
    assert_eq!(c.ops(), &[Op::Retain(2), ins("ab"), Op::Retain(2)]);
    // Touching ranges are allowed.
    let c = ChangeSet::from_edits(6, [(1..3, "x"), (3..3, "y"), (3..5, "")]).unwrap();
    assert_eq!(
        c.ops(),
        &[Op::Retain(1), Op::Delete(4), ins("xy"), Op::Retain(1)]
    );
    assert_eq!(c.len_after(), 4);

    let err = |e: Result<ChangeSet, ChangeSetError>| e.unwrap_err();
    assert_eq!(
        err(ChangeSet::from_edits(10, [(3..5, ""), (4..6, "")])),
        ChangeSetError::InvalidEdit { index: 1 }
    );
    assert_eq!(
        err(ChangeSet::from_edits(10, [(5..3, "")])),
        ChangeSetError::InvalidEdit { index: 0 }
    );
    assert_eq!(
        err(ChangeSet::from_edits(10, [(0..11, "")])),
        ChangeSetError::InvalidEdit { index: 0 }
    );
    // Unsorted starts are rejected too.
    assert_eq!(
        err(ChangeSet::from_edits(10, [(6..7, ""), (2..3, "")])),
        ChangeSetError::InvalidEdit { index: 1 }
    );
}

#[test]
fn empty_document_changesets() {
    let id = ChangeSet::identity(0);
    assert!(id.ops().is_empty());
    assert!(id.is_identity());
    assert_eq!((id.len_before(), id.len_after()), (0, 0));

    let c = ChangeSet::from_edits(0, [(0..0, "hi")]).unwrap();
    assert_eq!(c.ops(), &[ins("hi")]);
    assert_eq!(c.len_after(), 2);
    assert!(!c.is_identity());

    assert_eq!(ChangeSet::identity(7).ops(), &[Op::Retain(7)]);
    assert!(ChangeSet::identity(7).is_identity());
}

#[test]
fn errors_display() {
    assert_eq!(
        ChangeSetError::LengthMismatch {
            expected: 5,
            actual: 3
        }
        .to_string(),
        "length mismatch: expected 5 bytes, got 3"
    );
    assert_eq!(
        ChangeSetError::Overflow.to_string(),
        "change set length overflows usize"
    );
}

fn hello_edits() -> ChangeSet {
    ChangeSet::from_edits(11, [(0..5, "goodbye"), (11..11, "!")]).unwrap()
}

#[test]
fn apply_basic() {
    let mut t = Text::from("hello world");
    hello_edits().apply(&mut t).unwrap();
    assert_eq!(t, "goodbye world!");
}

#[test]
fn apply_rejects_wrong_length() {
    let mut t = Text::from("ab");
    assert_eq!(
        ChangeSet::identity(3).apply(&mut t),
        Err(ChangeSetError::LengthMismatch {
            expected: 3,
            actual: 2
        })
    );
    assert_eq!(t, "ab");
}

#[test]
fn apply_rejects_boundary_inside_char() {
    // Valid for any 4-byte text whose byte 1 is a char boundary, not for "😀".
    let c = ChangeSet::from_edits(4, [(1..2, "")]).unwrap();
    let mut t = Text::from("😀");
    let want = Err(ChangeSetError::NotCharBoundary { offset: 1 });
    assert_eq!(c.validate(&t), want);
    assert_eq!(c.apply(&mut t), want);
    assert_eq!(t, "😀");
    assert_eq!(c.invert(&t).map(|_| ()), want);
    // The same change set is fine for a text where byte 1 is a boundary.
    let mut ok = Text::from("abcd");
    c.apply(&mut ok).unwrap();
    assert_eq!(ok, "acd");
}

#[test]
fn invert_basic() {
    let original = Text::from("hello world");
    let c = hello_edits();
    let inv = c.invert(&original).unwrap();
    assert_eq!((inv.len_before(), inv.len_after()), (14, 11));
    let mut t = Text::from("goodbye world!");
    inv.apply(&mut t).unwrap();
    assert_eq!(t, "hello world");
}

#[test]
fn apply_on_multi_leaf_text_matches_reference() {
    let s = "line\n".repeat(5_000);
    let edits: Vec<_> = (0..50)
        .map(|i| {
            let at = i * 499;
            (at..at + 3, if i % 2 == 0 { "é😀" } else { "" })
        })
        .collect();
    let c = ChangeSet::from_edits(s.len(), edits).unwrap();
    let mut t = Text::from(s.as_str());
    assert!(t.chunks().count() > 1, "fixture should span several chunks");
    c.apply(&mut t).unwrap();
    assert_eq!(t, super::reference::apply_ops(c.ops(), &s).as_str());
}

#[test]
fn compose_basic() {
    let a = ChangeSet::from_edits(3, [(1..1, "XY")]).unwrap(); // abc -> aXYbc
    let b = ChangeSet::from_edits(5, [(2..4, "")]).unwrap(); // aXYbc -> aXc
    let ab = a.compose(&b).unwrap();
    assert_eq!((ab.len_before(), ab.len_after()), (3, 3));
    let mut t = Text::from("abc");
    ab.apply(&mut t).unwrap();
    assert_eq!(t, "aXc");
    assert_eq!(
        ab.ops(),
        &[Op::Retain(1), Op::Delete(1), ins("X"), Op::Retain(1)]
    );
}

#[test]
fn compose_rejects_length_mismatch() {
    let a = ChangeSet::from_edits(3, [(1..1, "XY")]).unwrap();
    assert_eq!(
        a.compose(&ChangeSet::identity(4)),
        Err(ChangeSetError::LengthMismatch {
            expected: 5,
            actual: 4
        })
    );
}

#[test]
fn compose_rejects_split_inside_inserted_char() {
    let a = ChangeSet::from_edits(0, [(0..0, "é")]).unwrap(); // 2 bytes
    let delete_half = ChangeSet::from_edits(2, [(0..1, "")]).unwrap();
    assert_eq!(
        a.compose(&delete_half),
        Err(ChangeSetError::NotCharBoundary { offset: 1 })
    );
    let retain_half = ChangeSet::from_edits(2, [(1..2, "")]).unwrap();
    assert_eq!(
        a.compose(&retain_half),
        Err(ChangeSetError::NotCharBoundary { offset: 1 })
    );
}

use Assoc::{After, Before};

#[test]
fn map_insert_assoc() {
    let c = ChangeSet::from_edits(2, [(1..1, "XY")]).unwrap(); // ab -> aXYb
    assert_eq!(c.map_pos(1, Before), Ok(1));
    assert_eq!(c.map_pos(1, After), Ok(3));
    assert_eq!(c.map_pos(0, After), Ok(0));
    assert_eq!(c.map_pos(2, Before), Ok(4));
}

#[test]
fn map_delete_collapses() {
    let c = ChangeSet::from_edits(4, [(1..3, "")]).unwrap(); // abcd -> ad
    for q in 1..=3 {
        assert_eq!(c.map_pos(q, Before), Ok(1), "{q} before");
        assert_eq!(c.map_pos(q, After), Ok(1), "{q} after");
    }
    assert_eq!(c.map_pos(0, After), Ok(0));
    assert_eq!(c.map_pos(4, Before), Ok(2));
}

#[test]
fn map_replace_edges() {
    let c = ChangeSet::from_edits(4, [(1..3, "XYZ")]).unwrap(); // abcd -> aXYZd
    assert_eq!(c.map_pos(2, Before), Ok(1));
    assert_eq!(c.map_pos(2, After), Ok(4));
    assert_eq!(c.map_pos(1, Before), Ok(1));
    assert_eq!(c.map_pos(3, After), Ok(4));
    assert_eq!(c.map_pos(4, Before), Ok(5));
}

#[test]
fn map_end_of_document() {
    let c = ChangeSet::from_edits(2, [(2..2, "!")]).unwrap(); // ab -> ab!
    assert_eq!(c.map_pos(2, Before), Ok(2));
    assert_eq!(c.map_pos(2, After), Ok(3));
    let id = ChangeSet::identity(2);
    assert_eq!(id.map_pos(2, Before), Ok(2));
    assert_eq!(
        c.map_pos(3, Before),
        Err(ChangeSetError::PositionOutOfBounds {
            position: 3,
            len: 2
        })
    );
}

#[test]
fn map_on_empty_document() {
    let id = ChangeSet::identity(0);
    assert_eq!(id.map_pos(0, Before), Ok(0));
    assert_eq!(id.map_pos(0, After), Ok(0));
    let c = ChangeSet::from_edits(0, [(0..0, "hi")]).unwrap();
    assert_eq!(c.map_pos(0, Before), Ok(0));
    assert_eq!(c.map_pos(0, After), Ok(2));
    assert_eq!(
        c.map_pos(1, After),
        Err(ChangeSetError::PositionOutOfBounds {
            position: 1,
            len: 0
        })
    );
    let mut none: [usize; 0] = [];
    assert_eq!(c.map_sorted(&mut none, Before), Ok(()));
}

#[test]
fn map_sorted_maps_in_one_pass() {
    let c = ChangeSet::from_edits(6, [(1..1, "XY"), (3..5, "")]).unwrap(); // abcdef -> aXYbcf
    let mut ps = [0, 1, 1, 2, 3, 4, 5, 6];
    c.map_sorted(&mut ps, After).unwrap();
    assert_eq!(ps, [0, 3, 3, 4, 5, 5, 5, 6]);
}

#[test]
fn map_sorted_errors_leave_slice_unchanged() {
    let c = ChangeSet::from_edits(4, [(1..3, "")]).unwrap();
    let mut unsorted = [3, 1];
    assert_eq!(
        c.map_sorted(&mut unsorted, Before),
        Err(ChangeSetError::UnsortedPositions { index: 1 })
    );
    assert_eq!(unsorted, [3, 1]);
    let mut out_of_bounds = [0, 99];
    assert_eq!(
        c.map_sorted(&mut out_of_bounds, After),
        Err(ChangeSetError::PositionOutOfBounds {
            position: 99,
            len: 4
        })
    );
    assert_eq!(out_of_bounds, [0, 99]);
}

/// PLAN Stage 2 "Known trap": composition preserves text, not positions.
#[test]
fn compose_loses_positions_documented() {
    let a = ChangeSet::from_edits(3, [(1..1, "Z")]).unwrap(); // abc -> aZbc
    let b = ChangeSet::from_edits(4, [(2..3, "")]).unwrap(); // aZbc -> aZc
    let ab = a.compose(&b).unwrap(); // canonical: Retain 1, Delete 1, Insert Z
    let mut t = Text::from("abc");
    ab.apply(&mut t).unwrap();
    assert_eq!(t, "aZc");
    // Position 2 (between 'b' and 'c'), Before:
    let sequential = b.map_pos(a.map_pos(2, Before).unwrap(), Before).unwrap();
    let composed = ab.map_pos(2, Before).unwrap();
    assert_eq!(sequential, 2); // after 'Z', before 'c'
    assert_eq!(composed, 1); // before 'Z': the composed replacement collapsed it
}
