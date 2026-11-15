//! Hand-written cases for revisions, the revision log, and documents.

use std::num::NonZeroUsize;

use super::*;
use crate::{Assoc, ChangeSet, ChangeSetError};

fn rev(n: u64) -> Revision {
    Revision(n)
}

/// `n` single-character inserts at 0 on a text of length `len`, each as one
/// change set: lengths `len`, `len + 1`, ...
fn inserts(len: usize, n: usize) -> Vec<ChangeSet> {
    (0..n)
        .map(|i| ChangeSet::from_edits(len + i, [(0..0, "x")]).unwrap())
        .collect()
}

fn log_of(changes: &[ChangeSet], log: RevisionLog) -> RevisionLog {
    let mut log = log;
    for (i, c) in changes.iter().enumerate() {
        log.push(rev(i as u64 + 1), c.clone());
    }
    log
}

#[test]
fn map_from_current_is_identity() {
    let log = log_of(&inserts(5, 2), RevisionLog::new());
    let mut p = [0, 3, 7];
    log.map_from(rev(2), rev(2), &mut p, Assoc::After).unwrap();
    assert_eq!(p, [0, 3, 7]);
    let empty = RevisionLog::new();
    let mut q = [usize::MAX];
    empty
        .map_from(rev(0), rev(0), &mut q, Assoc::After)
        .unwrap();
    assert_eq!(q, [usize::MAX]);
}

#[test]
fn map_from_matches_sequential_map_pos() {
    // "hello world" → "hello, world" → "hey, world" → "hey, world!"
    let a = ChangeSet::from_edits(11, [(5..5, ",")]).unwrap();
    let b = ChangeSet::from_edits(12, [(2..5, "y")]).unwrap();
    let c = ChangeSet::from_edits(10, [(10..10, "!")]).unwrap();
    let log = log_of(&[a.clone(), b.clone(), c.clone()], RevisionLog::new());
    for assoc in [Assoc::Before, Assoc::After] {
        for from in 0..=3u64 {
            let chain = &[&a, &b, &c][from as usize..];
            let start = [0, 3, 5, 11];
            let mut p = start.map(|x| {
                // Positions are expressed in revision `from`'s text.
                [&a, &b, &c][..from as usize]
                    .iter()
                    .fold(x, |x, cs| cs.map_pos(x, assoc).unwrap())
            });
            let expected = p.map(|x| chain.iter().fold(x, |x, cs| cs.map_pos(x, assoc).unwrap()));
            log.map_from(rev(from), rev(3), &mut p, assoc).unwrap();
            assert_eq!(p, expected, "from {from}, {assoc:?}");
        }
    }
}

#[test]
fn map_from_future_rejected() {
    let log = log_of(&inserts(0, 1), RevisionLog::new());
    let mut p = [0];
    assert_eq!(
        log.map_from(rev(2), rev(1), &mut p, Assoc::After),
        Err(MapError::Future {
            from: rev(2),
            current: rev(1)
        })
    );
}

#[test]
fn log_bounds_and_eviction() {
    let three = NonZeroUsize::new(3).unwrap();
    let log = log_of(&inserts(0, 5), RevisionLog::with_limits(three, usize::MAX));
    assert_eq!(log.len(), 3);
    assert_eq!(log.oldest(rev(5)), rev(2));
    let mut p = [0, 2];
    log.map_from(rev(2), rev(5), &mut p, Assoc::After).unwrap();
    assert_eq!(p, [3, 5]);
    let mut q = [0];
    assert_eq!(
        log.map_from(rev(1), rev(5), &mut q, Assoc::After),
        Err(MapError::Evicted {
            from: rev(1),
            oldest: rev(2)
        })
    );
    assert_eq!(q, [0]);

    // Op bound: a 3-op change set, then a 7-op one that alone exceeds it.
    let small = ChangeSet::from_edits(4, [(1..1, "x")]).unwrap();
    let big =
        ChangeSet::from_edits(5, [(0..0, "a"), (2..2, "b"), (4..4, "c"), (5..5, "d")]).unwrap();
    assert_eq!((small.ops().len(), big.ops().len()), (3, 7));
    let hundred = NonZeroUsize::new(100).unwrap();
    let log = log_of(&[small, big], RevisionLog::with_limits(hundred, 4));
    assert_eq!(log.len(), 1);
    assert_eq!(log.oldest(rev(2)), rev(1));
    assert!(RevisionLog::new().is_empty());
    assert_eq!(RevisionLog::new().oldest(rev(7)), rev(7));
}

#[test]
fn map_from_error_leaves_positions() {
    let log = log_of(&inserts(2, 2), RevisionLog::new());
    let mut p = [1, 3];
    assert_eq!(
        log.map_from(rev(0), rev(2), &mut p, Assoc::After),
        Err(MapError::ChangeSet(ChangeSetError::PositionOutOfBounds {
            position: 3,
            len: 2
        }))
    );
    assert_eq!(p, [1, 3]);
    let mut unsorted = [2, 1];
    assert!(matches!(
        log.map_from(rev(0), rev(2), &mut unsorted, Assoc::After),
        Err(MapError::ChangeSet(ChangeSetError::UnsortedPositions {
            index: 1
        }))
    ));
    assert_eq!(unsorted, [2, 1]);
}

#[test]
fn revision_accessors() {
    assert_eq!(Revision::ZERO.get(), 0);
    assert_eq!(Revision::ZERO.next(), rev(1));
    assert!(rev(1) < rev(2));
}

// ---- Document ----

use slotmap::SlotMap;

use crate::editor::ViewId;
use crate::{Range, Selection, Text, TextError, Transaction, TransactionError};

/// `n` fresh view IDs.
fn view_ids(n: usize) -> Vec<ViewId> {
    let mut ids: SlotMap<ViewId, ()> = SlotMap::with_key();
    (0..n).map(|_| ids.insert(())).collect()
}

fn doc_with_views(s: &str, n: usize) -> (Document, Vec<ViewId>) {
    let mut doc = Document::new(Text::from(s), None);
    let views = view_ids(n);
    for &v in &views {
        doc.add_view(v);
    }
    (doc, views)
}

#[test]
fn commit_bumps_revision_and_logs() {
    let (mut doc, v) = doc_with_views("abc", 1);
    assert_eq!(doc.revision(), Revision::ZERO);
    let txn = Transaction::insert(doc.text(), doc.selection(v[0]).unwrap(), "x").unwrap();
    assert_eq!(doc.commit(v[0], txn), Ok(rev(1)));
    assert_eq!(doc.text().to_string(), "xabc");
    assert_eq!(doc.selection(v[0]), Some(&Selection::point(1)));
    assert_eq!(doc.revision(), rev(1));
    assert_eq!(doc.log().len(), 1);
    let txn = Transaction::backspace(doc.text(), doc.selection(v[0]).unwrap()).unwrap();
    assert_eq!(doc.commit(v[0], txn), Ok(rev(2)));
    assert_eq!(doc.text().to_string(), "abc");
    assert_eq!(doc.log().len(), 2);
}

#[test]
fn identity_commit_keeps_revision() {
    let (mut doc, v) = doc_with_views("abc", 2);
    let goal = Selection::single(Range::point(1).with_goal_column(Some(5)));
    doc.set_selection(v[1], goal.clone()).unwrap();
    let txn = Transaction::new(ChangeSet::identity(3), Some(Selection::point(2)));
    assert_eq!(doc.commit(v[0], txn), Ok(Revision::ZERO));
    assert_eq!(doc.selection(v[0]), Some(&Selection::point(2)));
    // Other views are untouched, goal column included.
    assert_eq!(doc.selection(v[1]), Some(&goal));
    assert!(doc.log().is_empty());
}

#[test]
fn commit_maps_other_views() {
    let (mut doc, v) = doc_with_views("abc", 3);
    doc.set_selection(v[1], Selection::point(3)).unwrap();
    doc.set_selection(v[2], Selection::single(Range::new(0, 2)))
        .unwrap();
    let txn = Transaction::insert(doc.text(), doc.selection(v[0]).unwrap(), "xy").unwrap();
    doc.commit(v[0], txn).unwrap();
    assert_eq!(doc.text().to_string(), "xyabc");
    assert_eq!(doc.selection(v[0]), Some(&Selection::point(2)));
    assert_eq!(doc.selection(v[1]), Some(&Selection::point(5)));
    // A range does not absorb text inserted at its start.
    assert_eq!(
        doc.selection(v[2]),
        Some(&Selection::single(Range::new(2, 4)))
    );
}

#[test]
fn failed_commit_changes_nothing() {
    let (mut doc, v) = doc_with_views("aé", 2);
    doc.set_selection(v[1], Selection::point(1)).unwrap();
    let before = doc.clone();
    let check = |doc: &Document| {
        assert_eq!(doc.text(), before.text());
        assert_eq!(doc.revision(), before.revision());
        assert_eq!(doc.log().len(), before.log().len());
        for &id in &v {
            assert_eq!(doc.selection(id), before.selection(id));
        }
    };
    // Change set of the wrong length.
    let wrong_len = Transaction::new(ChangeSet::identity(5), None);
    assert!(matches!(
        doc.commit(v[0], wrong_len),
        Err(CommitError::Transaction(TransactionError::ChangeSet(_)))
    ));
    check(&doc);
    // Change set splitting "é".
    let split = Transaction::new(ChangeSet::from_edits(3, [(2..2, "x")]).unwrap(), None);
    assert!(doc.commit(v[0], split).is_err());
    check(&doc);
    // A valid change set with a selection that is out of bounds afterwards.
    let bad_sel = Transaction::new(
        ChangeSet::from_edits(3, [(0..0, "x")]).unwrap(),
        Some(Selection::point(10)),
    );
    assert!(matches!(
        doc.commit(v[0], bad_sel),
        Err(CommitError::Transaction(TransactionError::Selection(_)))
    ));
    check(&doc);
}

#[test]
fn commit_unknown_view_rejected() {
    let (mut doc, _) = doc_with_views("abc", 1);
    let stranger = view_ids(2)[1];
    let txn = Transaction::new(ChangeSet::identity(3), None);
    assert_eq!(
        doc.commit(stranger, txn),
        Err(CommitError::NoSuchView(stranger))
    );
    assert_eq!(
        doc.set_selection(stranger, Selection::point(0)),
        Err(CommitError::NoSuchView(stranger))
    );
    assert_eq!(doc.selection(stranger), None);
}

#[test]
fn set_selection_validates() {
    let (mut doc, v) = doc_with_views("é", 1);
    assert_eq!(
        doc.set_selection(v[0], Selection::point(1)),
        Err(CommitError::Transaction(TransactionError::Selection(
            TextError::NotCharBoundary { offset: 1 }
        )))
    );
    assert_eq!(doc.selection(v[0]), Some(&Selection::point(0)));
}

#[test]
fn add_and_remove_views() {
    let (mut doc, v) = doc_with_views("abc", 2);
    assert_eq!(doc.views().count(), 2);
    assert!(doc.remove_view(v[0]));
    assert!(!doc.remove_view(v[0]));
    assert_eq!(doc.views().collect::<Vec<_>>(), [v[1]]);
    assert_eq!(doc.selection(v[0]), None);
}

#[test]
fn map_from_validates_against_text() {
    let (mut doc, v) = doc_with_views("é", 1);
    let txn = Transaction::insert(doc.text(), doc.selection(v[0]).unwrap(), "a").unwrap();
    doc.commit(v[0], txn).unwrap();
    // 0 at revision 0 maps (After) past the insert, to 1.
    let mut ok = [0, 2];
    doc.map_from(Revision::ZERO, &mut ok, Assoc::After).unwrap();
    assert_eq!(ok, [1, 3]);
    // 1 was inside "é" at revision 0; it maps to 2, inside "é" now.
    let mut bad = [0, 1];
    assert_eq!(
        doc.map_from(Revision::ZERO, &mut bad, Assoc::After),
        Err(MapError::Text(TextError::NotCharBoundary { offset: 2 }))
    );
    assert_eq!(bad, [0, 1]);
}

#[test]
fn line_ending_detection() {
    let detect = |s: &str| LineEnding::detect(&Text::from(s));
    assert_eq!(detect(""), LineEnding::Lf);
    assert_eq!(detect("a\nb"), LineEnding::Lf);
    assert_eq!(detect("a\r\nb\r\nc\n"), LineEnding::Crlf);
    assert_eq!(detect("a\r\nb\n"), LineEnding::Lf, "ties go to LF");
    assert_eq!(detect("\r\r\n"), LineEnding::Crlf);
    // Long enough that chunk boundaries fall between '\r' and '\n'
    // (especially with `small-chunks`).
    let long = "abcdefg\r\n".repeat(500);
    assert_eq!(detect(&long), LineEnding::Crlf);
    let text = Text::from(long.as_str());
    let crlf_split = text
        .chunks()
        .scan(0, |end, c| {
            *end += c.len();
            Some(*end)
        })
        .any(|end| end < long.len() && long.as_bytes()[end - 1] == b'\r');
    if cfg!(feature = "small-chunks") {
        assert!(crlf_split, "fixture must split a CRLF across chunks");
    }
    assert_eq!(LineEnding::Crlf.as_str(), "\r\n");
    assert_eq!(LineEnding::Lf.as_str(), "\n");
    assert_eq!(
        Document::new(Text::from(long), None).line_ending(),
        LineEnding::Crlf
    );
}
