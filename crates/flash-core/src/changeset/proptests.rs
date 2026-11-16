//! Randomised properties of change sets, checked against the `String`
//! reference model. Texts come from the same Unicode-heavy pieces as the
//! `Text` property tests, and every generated op boundary is a char boundary.

use proptest::prelude::*;

use super::reference::{apply_ops, map_pos_naive};
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
        let mut t = Text::from(s.as_str());
        prop_assert_eq!(c.apply(&mut t), Ok(()));
        prop_assert_eq!(t.to_string(), apply_ops(c.ops(), &s));
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
        let mut t = original.clone();
        prop_assert_eq!(c.apply(&mut t), valid);
        if valid.is_ok() {
            prop_assert_eq!(t.to_string(), apply_ops(c.ops(), &u));
        } else {
            prop_assert_eq!(&t, &original, "failed apply changed the text");
        }
        prop_assert_eq!(c.invert(&original).map(|_| ()), valid);
    }
}

/// `(s, [a, b, c])` where `a` is valid for `s`, `b` for `a`'s output, and
/// `c` for `b`'s output.
pub(crate) fn chain() -> impl Strategy<Value = (String, Vec<ChangeSet>)> {
    text_and_changeset()
        .prop_flat_map(|(s, a)| {
            let mid = apply_ops(a.ops(), &s);
            (Just(s), Just(a), changeset_for(&mid))
        })
        .prop_flat_map(|(s, a, b)| {
            let mid = apply_ops(b.ops(), &apply_ops(a.ops(), &s));
            (Just(s), Just(a), Just(b), changeset_for(&mid))
        })
        .prop_map(|(s, a, b, c)| (s, vec![a, b, c]))
}

fn apply_str(c: &ChangeSet, s: &str) -> String {
    apply_ops(c.ops(), s)
}

proptest! {
    #[test]
    fn compose_matches_sequential_apply((s, cs) in chain()) {
        let (a, b) = (&cs[0], &cs[1]);
        let ab = a.compose(b).unwrap();
        assert_canonical(&ab)?;
        prop_assert_eq!(ab.len_before(), a.len_before());
        prop_assert_eq!(ab.len_after(), b.len_after());
        prop_assert_eq!(apply_str(&ab, &s), apply_str(b, &apply_str(a, &s)));
        let mut t = Text::from(s.as_str());
        ab.apply(&mut t).unwrap();
        prop_assert_eq!(t.to_string(), apply_str(b, &apply_str(a, &s)));
    }

    #[test]
    fn compose_is_associative_on_text((s, cs) in chain()) {
        let (a, b, c) = (&cs[0], &cs[1], &cs[2]);
        let left = a.compose(b).unwrap().compose(c).unwrap();
        let right = a.compose(&b.compose(c).unwrap()).unwrap();
        prop_assert_eq!(apply_str(&left, &s), apply_str(&right, &s));
    }

    #[test]
    fn compose_with_identity((_s, a) in text_and_changeset()) {
        let id_before = ChangeSet::identity(a.len_before());
        let id_after = ChangeSet::identity(a.len_after());
        prop_assert_eq!(id_before.compose(&a), Ok(a.clone()));
        prop_assert_eq!(a.compose(&id_after), Ok(a.clone()));
    }

    #[test]
    fn compose_with_inverse_is_identity_on_text((s, a) in text_and_changeset()) {
        let t = Text::from(s.as_str());
        let undo = a.compose(&a.invert(&t).unwrap()).unwrap();
        prop_assert_eq!(apply_str(&undo, &s), s.as_str());
    }
}

const ASSOCS: [Assoc; 2] = [Assoc::Before, Assoc::After];

proptest! {
    #[test]
    fn map_pos_matches_naive((s, c) in text_and_changeset()) {
        for assoc in ASSOCS {
            for q in 0..=s.len() + 2 {
                let want = if q <= c.len_before() {
                    Ok(map_pos_naive(c.ops(), q, assoc))
                } else {
                    Err(ChangeSetError::PositionOutOfBounds { position: q, len: c.len_before() })
                };
                prop_assert_eq!(c.map_pos(q, assoc), want, "{} {:?}", q, assoc);
            }
        }
    }

    #[test]
    fn map_sorted_matches_map_pos(
        (s, c) in text_and_changeset(),
        picks in prop::collection::vec(any::<prop::sample::Index>(), 0..40),
    ) {
        let mut positions: Vec<usize> = picks.iter().map(|i| i.index(s.len() + 1)).collect();
        positions.sort_unstable();
        for assoc in ASSOCS {
            let mut mapped = positions.clone();
            c.map_sorted(&mut mapped, assoc).unwrap();
            let one_by_one: Vec<usize> =
                positions.iter().map(|&q| c.map_pos(q, assoc).unwrap()).collect();
            prop_assert_eq!(mapped, one_by_one, "{:?}", assoc);
        }
    }

    #[test]
    fn mapped_positions_are_char_boundaries_and_monotonic((s, c) in text_and_changeset()) {
        let after = apply_str(&c, &s);
        let boundaries: Vec<usize> = (0..=s.len()).filter(|&q| s.is_char_boundary(q)).collect();
        for assoc in ASSOCS {
            let mut mapped = boundaries.clone();
            c.map_sorted(&mut mapped, assoc).unwrap();
            for (&q, &m) in boundaries.iter().zip(&mapped) {
                prop_assert!(after.is_char_boundary(m), "{} -> {} {:?} in {:?}", q, m, assoc, after);
            }
            prop_assert!(mapped.windows(2).all(|w| w[0] <= w[1]), "not monotonic: {:?}", mapped);
            // The end maps to the new end, except that `Before` stays in
            // front of text inserted at the end.
            let last = *mapped.last().unwrap();
            match assoc {
                Assoc::After => prop_assert_eq!(last, c.len_after()),
                Assoc::Before => prop_assert!(last <= c.len_after()),
            }
        }
    }

    #[test]
    fn map_through_chain_is_sequential((s, cs) in chain()) {
        let (a, b) = (&cs[0], &cs[1]);
        let end = apply_str(b, &apply_str(a, &s));
        for assoc in ASSOCS {
            for q in (0..=s.len()).filter(|&q| s.is_char_boundary(q)) {
                let m = b.map_pos(a.map_pos(q, assoc).unwrap(), assoc).unwrap();
                prop_assert!(end.is_char_boundary(m), "{} -> {} {:?}", q, m, assoc);
            }
        }
    }
}

proptest! {
    #[test]
    fn map_sorted_with_matches_map_pos(
        (c, mut positions, assocs) in text_and_changeset().prop_flat_map(|(s, c)| {
            let len = c.len_before();
            let n = s.len().min(16) + 1;
            (
                Just(c),
                prop::collection::vec(0..=len, 0..n),
                prop::collection::vec(prop_oneof![Just(Assoc::Before), Just(Assoc::After)], n),
            )
        })
    ) {
        positions.sort_unstable();
        let expected: Vec<usize> = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| c.map_pos(p, assocs[i]).unwrap())
            .collect();
        c.map_sorted_with(&mut positions, |i| assocs[i]).unwrap();
        prop_assert_eq!(positions, expected);
    }
}

proptest! {
    /// What `map_untouched` promises: the range keeps its text, and the
    /// character before it (a line terminator, for a line start) is the
    /// same one, so a line start stays a line start.
    #[test]
    fn map_untouched_keeps_the_range_and_what_precedes_it(
        (s, c, a, b) in text_and_changeset().prop_flat_map(|(s, c)| {
            let n = s.len();
            (Just(s), Just(c), 0..=n, 0..=n)
        })
    ) {
        let (a, b) = (floor_boundary(&s, a.min(b)), floor_boundary(&s, a.max(b)));
        let after = apply_str(&c, &s);
        if let Some((a2, b2)) = c.map_untouched(a, b) {
            prop_assert_eq!(&after[a2..b2], &s[a..b]);
            prop_assert_eq!(a2 == 0, a == 0);
            if a > 0 {
                prop_assert_eq!(after[..a2].chars().next_back(), s[..a].chars().next_back());
            }
            // And the mapped ends are where `map_pos` puts them.
            prop_assert_eq!(c.map_pos(a, Assoc::Before).unwrap(), a2);
            prop_assert_eq!(c.map_pos(b, Assoc::Before).unwrap(), b2);
        }
    }
}
