//! Randomised properties of normalisation and mapping.

use proptest::prelude::*;

use super::*;
use crate::changeset::reference::{apply_ops, map_pos_naive};
use crate::changeset::{Assoc, ChangeSet};
use crate::strategies::{changeset_for, selection_for, text_strategy};

/// Whether `r` would join a group spanning `[start, end)` (module docs).
fn joins(start: usize, end: usize, r: &Range) -> bool {
    r.start() < end || (r.start() == end && (r.is_empty() || start == end))
}

/// Checks that `sel` is sorted, that no two neighbours would merge, and that
/// the primary index is in bounds.
pub(crate) fn assert_normalised(sel: &Selection) -> Result<(), TestCaseError> {
    let ranges = sel.ranges();
    prop_assert!(!ranges.is_empty());
    prop_assert!(sel.primary_index() < ranges.len());
    for w in ranges.windows(2) {
        prop_assert!(
            (w[0].start(), w[0].end()) <= (w[1].start(), w[1].end()),
            "unsorted {:?}",
            sel
        );
        prop_assert!(
            !joins(w[0].start(), w[0].end(), &w[1]),
            "mergeable neighbours in {:?}",
            sel
        );
    }
    Ok(())
}

fn raw_ranges() -> impl Strategy<Value = (Vec<Range>, usize)> {
    prop::collection::vec((0..=50usize, 0..=50usize), 1..12).prop_flat_map(|v| {
        let n = v.len();
        let ranges: Vec<Range> = v.into_iter().map(|(a, h)| Range::new(a, h)).collect();
        (Just(ranges), 0..n)
    })
}

fn contains(outer: &Range, inner: &Range) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

proptest! {
    #[test]
    fn normalise_invariants((ranges, primary) in raw_ranges()) {
        let sel = Selection::new(ranges.iter().copied(), primary).unwrap();
        assert_normalised(&sel)?;
        for r in &ranges {
            prop_assert!(sel.ranges().iter().any(|o| contains(o, r)), "{:?} lost in {:?}", r, sel);
        }
        prop_assert!(contains(sel.primary(), &ranges[primary]));
        let endpoints: Vec<usize> = ranges.iter().flat_map(|r| [r.anchor(), r.head()]).collect();
        for o in sel.ranges() {
            prop_assert!(endpoints.contains(&o.anchor()) && endpoints.contains(&o.head()));
        }
        let again = Selection::new(sel.ranges().iter().copied(), sel.primary_index()).unwrap();
        prop_assert_eq!(again, sel);
    }

    #[test]
    fn map_matches_naive(
        (s, c, sel) in text_strategy(20).prop_flat_map(|s| {
            let c = changeset_for(&s);
            let sel = selection_for(&s);
            (Just(s), c, sel)
        })
    ) {
        let mapped = sel.map(&c).unwrap();
        let naive: Vec<Range> = sel
            .ranges()
            .iter()
            .map(|r| {
                let end_assoc = if r.is_empty() { Assoc::After } else { Assoc::Before };
                let a = map_pos_naive(c.ops(), r.start(), Assoc::After);
                let b = map_pos_naive(c.ops(), r.end(), end_assoc);
                Range::spanning(a.min(b), a.max(b), r.direction())
            })
            .collect();
        let expected = Selection::new(naive, sel.primary_index()).unwrap();
        prop_assert_eq!(&mapped, &expected);
        assert_normalised(&mapped)?;
        let t2 = crate::Text::from(apply_ops(c.ops(), &s));
        prop_assert_eq!(mapped.validate(&t2), Ok(()));
    }

    #[test]
    fn identity_map_is_noop_except_goals((s, sel) in text_strategy(10).prop_flat_map(|s| {
        let sel = selection_for(&s);
        (Just(s), sel)
    })) {
        prop_assert_eq!(sel.map(&ChangeSet::identity(s.len())).unwrap(), sel);
    }
}
