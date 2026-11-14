//! Hand-written transaction cases: multi-cursor collisions, Unicode, errors.

use super::*;
use crate::{ChangeSet, ChangeSetError, Range, Selection, Text, TextError};

fn r(anchor: usize, head: usize) -> Range {
    Range::new(anchor, head)
}

fn sel(ranges: &[Range], primary: usize) -> Selection {
    Selection::new(ranges.iter().copied(), primary).unwrap()
}

/// Builds with `make`, applies, and returns the new text and selection.
fn run(
    s: &str,
    selection: Selection,
    make: impl Fn(&Text, &Selection) -> Result<Transaction, TransactionError>,
) -> (String, Selection) {
    let mut text = Text::from(s);
    let mut selection = selection;
    let txn = make(&text, &selection).unwrap();
    txn.apply(&mut text, &mut selection).unwrap();
    (text.to_string(), selection)
}

fn insert(s: &'static str) -> impl Fn(&Text, &Selection) -> Result<Transaction, TransactionError> {
    move |t, sel| Transaction::insert(t, sel, s)
}

#[test]
fn insert_at_cursors() {
    let got = run("ab\ncd", sel(&[r(1, 1), r(4, 4)], 0), insert("X"));
    assert_eq!(got, ("aXb\ncXd".to_owned(), sel(&[r(2, 2), r(6, 6)], 0)));
}

#[test]
fn insert_extends_forward_range() {
    let got = run("abc", Selection::single(r(0, 2)), insert("X"));
    assert_eq!(got, ("abXc".to_owned(), Selection::single(r(0, 3))));
}

#[test]
fn insert_before_backward_range() {
    let got = run("abc", Selection::single(r(2, 0)), insert("X"));
    assert_eq!(got, ("Xabc".to_owned(), Selection::single(r(3, 1))));
}

#[test]
fn same_position_inserts() {
    let got = run("abcdef", sel(&[r(1, 3), r(5, 3)], 1), insert("X"));
    assert_eq!(got, ("abcXXdef".to_owned(), sel(&[r(1, 4), r(7, 5)], 1)));
}

#[test]
fn delete_ranges() {
    let got = run(
        "hello world",
        sel(&[r(0, 5), r(6, 11)], 1),
        Transaction::delete,
    );
    assert_eq!(got, (" ".to_owned(), sel(&[r(0, 0), r(1, 1)], 1)));
}

#[test]
fn delete_touching_ranges_merge_points() {
    let got = run("abcd", sel(&[r(0, 2), r(2, 4)], 1), Transaction::delete);
    assert_eq!(got, (String::new(), Selection::point(0)));
}

#[test]
fn replace_ranges() {
    let got = run("a b", sel(&[r(0, 1), r(3, 2)], 0), |t, s| {
        Transaction::replace(t, s, "XY")
    });
    assert_eq!(got, ("XY XY".to_owned(), sel(&[r(0, 2), r(5, 3)], 0)));
}

#[test]
fn replace_at_cursor_selects_replacement() {
    let got = run("ab", Selection::point(1), |t, s| {
        Transaction::replace(t, s, "XY")
    });
    assert_eq!(got, ("aXYb".to_owned(), Selection::single(r(1, 3))));
}

#[test]
fn backspace_cursor_deletes_grapheme() {
    assert_eq!(
        run("ae\u{301}", Selection::point(4), Transaction::backspace),
        ("a".to_owned(), Selection::point(1))
    );
    assert_eq!(
        run("a\r\n", Selection::point(3), Transaction::backspace),
        ("a".to_owned(), Selection::point(1))
    );
    let t = Text::from("ab");
    let txn = Transaction::backspace(&t, &Selection::point(0)).unwrap();
    assert!(txn.changes().is_identity());
    assert_eq!(txn.selection(), Some(&Selection::point(0)));
}

#[test]
fn backspace_range_deletes_content() {
    assert_eq!(
        run("abcd", Selection::single(r(3, 1)), Transaction::backspace),
        ("ad".to_owned(), Selection::point(1))
    );
}

#[test]
fn backspace_overlapping_deletes_merge() {
    assert_eq!(
        run(
            "e\u{301}",
            sel(&[r(1, 1), r(3, 3)], 0),
            Transaction::backspace
        ),
        (String::new(), Selection::point(0))
    );
    assert_eq!(
        run(
            "ae\u{301}",
            sel(&[r(0, 2), r(4, 4)], 1),
            Transaction::backspace
        ),
        (String::new(), Selection::point(0))
    );
}

#[test]
fn delete_forward_cases() {
    assert_eq!(
        run("ab", Selection::point(0), Transaction::delete_forward),
        ("b".to_owned(), Selection::point(0))
    );
    assert_eq!(
        run(
            "e\u{301}x",
            Selection::point(0),
            Transaction::delete_forward
        ),
        ("x".to_owned(), Selection::point(0))
    );
    let t = Text::from("ab");
    let txn = Transaction::delete_forward(&t, &Selection::point(2)).unwrap();
    assert!(txn.changes().is_identity());
}

#[test]
fn delete_forward_overlapping_deletes_merge() {
    // Cursor 0's cluster 0..3 overlaps the range 1..3 (head inside it).
    assert_eq!(
        run(
            "e\u{301}x",
            sel(&[r(0, 0), r(1, 3)], 0),
            Transaction::delete_forward
        ),
        ("x".to_owned(), Selection::point(0))
    );
}

#[test]
fn transactions_on_empty_text() {
    assert_eq!(
        run("", Selection::point(0), insert("x")),
        ("x".to_owned(), Selection::point(1))
    );
    let t = Text::new();
    for make in [
        Transaction::backspace,
        Transaction::delete_forward,
        Transaction::delete,
    ] {
        let txn = make(&t, &Selection::point(0)).unwrap();
        assert!(txn.changes().is_identity());
        assert_eq!(txn.selection(), Some(&Selection::point(0)));
    }
}

#[test]
fn constructors_reject_invalid_selection() {
    let t = Text::from("ab");
    let err = Err(TransactionError::Selection(TextError::OffsetOutOfBounds {
        offset: 5,
        len: 2,
    }));
    assert_eq!(Transaction::insert(&t, &Selection::point(5), "x"), err);
    assert_eq!(Transaction::backspace(&t, &Selection::point(5)), err);
    assert_eq!(
        Transaction::delete(&Text::from("é"), &Selection::single(r(0, 1))),
        Err(TransactionError::Selection(TextError::NotCharBoundary {
            offset: 1
        }))
    );
}

#[test]
fn apply_rejects_wrong_text() {
    let txn = Transaction::insert(&Text::from("abc"), &Selection::point(0), "x").unwrap();
    let mut text = Text::from("ab");
    let mut selection = Selection::point(1);
    assert_eq!(
        txn.apply(&mut text, &mut selection),
        Err(TransactionError::ChangeSet(
            ChangeSetError::LengthMismatch {
                expected: 3,
                actual: 2
            }
        ))
    );
    assert_eq!(text, "ab");
    assert_eq!(selection, Selection::point(1));
}

#[test]
fn apply_rejects_bad_explicit_selection() {
    let changes = ChangeSet::from_edits(0, [(0..0, "é")]).unwrap();
    for (pos, err) in [
        (1, TextError::NotCharBoundary { offset: 1 }),
        (3, TextError::OffsetOutOfBounds { offset: 3, len: 2 }),
    ] {
        let txn = Transaction::new(changes.clone(), Some(Selection::point(pos)));
        let mut text = Text::new();
        let mut selection = Selection::point(0);
        assert_eq!(
            txn.apply(&mut text, &mut selection),
            Err(TransactionError::Selection(err))
        );
        assert_eq!(text, "");
        assert_eq!(selection, Selection::point(0));
    }
}

#[test]
fn apply_checks_selection_inside_retained_text() {
    // 'é' at 1..3 is retained; position 2 is inside it after the edit.
    let changes = ChangeSet::from_edits(3, [(0..1, "")]).unwrap();
    let txn = Transaction::new(changes, Some(Selection::point(1)));
    let mut text = Text::from("aé");
    let mut selection = Selection::point(0);
    assert_eq!(
        txn.apply(&mut text, &mut selection),
        Err(TransactionError::Selection(TextError::NotCharBoundary {
            offset: 1
        }))
    );
    assert_eq!(text, "aé");
}

#[test]
fn apply_without_selection_maps() {
    let txn = Transaction::new(ChangeSet::from_edits(2, [(0..0, "X")]).unwrap(), None);
    let mut text = Text::from("ab");
    let mut selection = Selection::point(1);
    txn.apply(&mut text, &mut selection).unwrap();
    assert_eq!(
        (text.to_string(), selection),
        ("Xab".to_owned(), Selection::point(2))
    );
}

#[test]
fn apply_without_selection_rejects_invalid_current_selection() {
    let txn = Transaction::new(ChangeSet::identity(2), None);
    let mut text = Text::from("é");
    let mut selection = Selection::point(1);
    assert_eq!(
        txn.apply(&mut text, &mut selection),
        Err(TransactionError::Selection(TextError::NotCharBoundary {
            offset: 1
        }))
    );
}

#[test]
fn error_display_and_source() {
    let e = TransactionError::Selection(TextError::NotCharBoundary { offset: 1 });
    assert!(e.to_string().contains("selection"));
    assert!(std::error::Error::source(&e).is_some());
}
