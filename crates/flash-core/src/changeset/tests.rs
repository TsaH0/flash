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
    // Both strategies validate on their own as well.
    assert_eq!(c.apply_per_op(&mut t), want);
    assert_eq!(c.apply_rebuild(&mut t), want);
    assert_eq!(t, "😀");
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
fn strategies_agree_on_multi_leaf_text() {
    let s = "line\n".repeat(5_000);
    let edits: Vec<_> = (0..50)
        .map(|i| {
            let at = i * 499;
            (at..at + 3, if i % 2 == 0 { "é😀" } else { "" })
        })
        .collect();
    let c = ChangeSet::from_edits(s.len(), edits).unwrap();
    let mut a = Text::from(s.as_str());
    let mut b = a.clone();
    assert!(a.chunks().count() > 1, "fixture should span several chunks");
    c.apply_per_op(&mut a).unwrap();
    c.apply_rebuild(&mut b).unwrap();
    assert_eq!(a, b);
    assert_eq!(a, super::reference::apply_ops(c.ops(), &s).as_str());
}
