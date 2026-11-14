//! Hand-written cases for the documented behaviour of [`ChangeSet`].

#![allow(
    clippy::reversed_empty_ranges,
    reason = "inverted ranges are deliberate invalid inputs"
)]

use super::*;

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
