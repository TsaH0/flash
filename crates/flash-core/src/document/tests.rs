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
