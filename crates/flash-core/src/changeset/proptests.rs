//! Randomised properties of change sets, checked against the `String`
//! reference model. Texts come from the same Unicode-heavy pieces as the
//! `Text` property tests, and every generated op boundary is a char boundary.

use proptest::prelude::*;

use super::reference::apply_ops;
use super::*;
use crate::strategies::{changeset_for, raw_ops_for, text_strategy};

/// Checks canonical-form rules 1–4 from the module docs.
pub(crate) fn assert_canonical(c: &ChangeSet) -> Result<(), TestCaseError> {
    let (mut before, mut after) = (0usize, 0usize);
    for op in c.ops() {
        match op {
            Op::Retain(n) => {
                prop_assert!(*n > 0, "empty retain in {:?}", c);
                before += n;
                after += n;
            }
            Op::Delete(n) => {
                prop_assert!(*n > 0, "empty delete in {:?}", c);
                before += n;
            }
            Op::Insert(s) => {
                prop_assert!(!s.is_empty(), "empty insert in {:?}", c);
                after += s.len();
            }
        }
    }
    for pair in c.ops().windows(2) {
        let same = std::mem::discriminant(&pair[0]) == std::mem::discriminant(&pair[1]);
        prop_assert!(!same, "adjacent ops of one kind in {:?}", c);
        let insert_then_delete = matches!(pair, [Op::Insert(_), Op::Delete(_)]);
        prop_assert!(!insert_then_delete, "insert before delete in {:?}", c);
    }
    prop_assert_eq!(before, c.len_before());
    prop_assert_eq!(after, c.len_after());
    Ok(())
}

fn text_and_raw_ops() -> impl Strategy<Value = (String, Vec<Op>)> {
    text_strategy(20).prop_flat_map(|s| {
        let ops = raw_ops_for(&s);
        (Just(s), ops)
    })
}

pub(crate) fn text_and_changeset() -> impl Strategy<Value = (String, ChangeSet)> {
    text_strategy(20).prop_flat_map(|s| {
        let c = changeset_for(&s);
        (Just(s), c)
    })
}

proptest! {
    #[test]
    fn builder_output_is_canonical_and_matches_raw_ops((s, raw) in text_and_raw_ops()) {
        let mut b = ChangeSetBuilder::new(s.len());
        for op in &raw {
            match op {
                Op::Retain(n) => b.retain(*n),
                Op::Delete(n) => b.delete(*n),
                Op::Insert(text) => b.insert(text),
            };
        }
        let c = b.finish().unwrap();
        assert_canonical(&c)?;
        prop_assert_eq!(apply_ops(c.ops(), &s), apply_ops(&raw, &s));
    }

    #[test]
    fn lengths_add_up((s, c) in text_and_changeset()) {
        assert_canonical(&c)?;
        prop_assert_eq!(c.len_before(), s.len());
        prop_assert_eq!(apply_ops(c.ops(), &s).len(), c.len_after());
    }
}
