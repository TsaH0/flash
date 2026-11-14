//! Randomised properties of change sets, checked against the `String`
//! reference model. Texts come from the same Unicode-heavy pieces as the
//! `Text` property tests, and every generated op boundary is a char boundary.

use proptest::prelude::*;

use super::reference::apply_ops;
use super::*;
use crate::Text;
use crate::strategies::{changeset_for, floor_boundary, raw_ops_for, text_strategy};

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

type ApplyFn = fn(&ChangeSet, &mut Text) -> Result<(), ChangeSetError>;

const STRATEGIES: [(&str, ApplyFn); 3] = [
    ("apply", ChangeSet::apply),
    ("apply_per_op", ChangeSet::apply_per_op),
    ("apply_rebuild", ChangeSet::apply_rebuild),
];

/// `u`, or (if `same_len`) `u` cut or padded with 'a' to exactly `len`
/// bytes, so its char boundaries differ from the text a change set was made
/// for while its length matches.
fn other_text(u: String, len: usize, same_len: bool) -> String {
    if !same_len {
        return u;
    }
    let mut u = u;
    u.truncate(floor_boundary(&u, len));
    while u.len() < len {
        u.push('a');
    }
    u
}

proptest! {
    #[test]
    fn apply_matches_reference((s, c) in text_and_changeset()) {
        let want = apply_ops(c.ops(), &s);
        for (name, strategy) in STRATEGIES {
            let mut t = Text::from(s.as_str());
            prop_assert_eq!(strategy(&c, &mut t), Ok(()), "{}", name);
            prop_assert_eq!(t.to_string(), want.as_str(), "{}", name);
        }
    }

    #[test]
    fn invert_round_trips((s, c) in text_and_changeset()) {
        let t = Text::from(s.as_str());
        let mut t2 = t.clone();
        c.apply(&mut t2).unwrap();
        let inv = c.invert(&t).unwrap();
        assert_canonical(&inv)?;
        prop_assert_eq!(inv.len_before(), c.len_after());
        prop_assert_eq!(inv.len_after(), c.len_before());
        let mut back = t2.clone();
        inv.apply(&mut back).unwrap();
        prop_assert_eq!(&back, &t);
        prop_assert_eq!(inv.invert(&t2), Ok(c.clone()));
    }

    #[test]
    fn invalid_changeset_leaves_text_unchanged(
        (s, c) in text_and_changeset(),
        u in text_strategy(20),
        same_len in any::<bool>(),
    ) {
        let u = other_text(u, s.len(), same_len);
        let original = Text::from(u.as_str());
        let valid = c.validate(&original);
        for (name, strategy) in STRATEGIES {
            let mut t = original.clone();
            let result = strategy(&c, &mut t);
            prop_assert_eq!(result, valid, "{}", name);
            if valid.is_ok() {
                prop_assert_eq!(t.to_string(), apply_ops(c.ops(), &u), "{}", name);
            } else {
                prop_assert_eq!(&t, &original, "{} changed the text", name);
            }
        }
        prop_assert_eq!(c.invert(&original).map(|_| ()), valid);
    }
}
