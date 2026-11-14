//! Fuzz entry points, compiled for tests and with the `fuzzing` feature.
//!
//! [`changeset_ops`] is the body of the cargo-fuzz target in `fuzz/`. A
//! stable test below drives it with random bytes, so it also runs under
//! plain `cargo test`.

use arbitrary::Unstructured;

use crate::{Assoc, ChangeSet, ChangeSetBuilder, ChangeSetError, Text};

/// Largest text the fuzzer builds, in bytes.
const MAX_TEXT: usize = 4096;

/// Builds a text and change sets from `data` (often invalid on purpose) and
/// runs every change set operation on them. Returns normally for any input
/// unless an invariant is broken, in which case it panics.
pub fn changeset_ops(data: &[u8]) {
    // Running out of input just ends the case early.
    let _ = run(&mut Unstructured::new(data));
}

fn run(u: &mut Unstructured<'_>) -> arbitrary::Result<()> {
    let mut s: String = u.arbitrary()?;
    let cut = (0..=s.len().min(MAX_TEXT))
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    s.truncate(cut);
    let t = Text::from(s.as_str());

    let Ok(a) = raw_changeset(u, s.len())? else {
        return Ok(());
    };
    if let Err(e) = a.validate(&t) {
        let mut copy = t.clone();
        assert_eq!(a.apply(&mut copy), Err(e), "apply disagrees with validate");
        assert_eq!(copy, t, "failed apply changed the text");
        assert_eq!(a.invert(&t).map(|_| ()), Err(e));
        return Ok(());
    }

    let mut t2 = t.clone();
    a.apply(&mut t2).expect("validated change set applies");
    assert_eq!(t2.byte_len(), a.len_after());

    let inv = a.invert(&t).expect("validated change set inverts");
    let mut back = t2.clone();
    inv.apply(&mut back).expect("inverse applies to the result");
    assert_eq!(back, t, "inverse does not restore the text");
    assert_eq!(inv.invert(&t2).as_ref(), Ok(&a), "double inverse differs");

    let boundaries: Vec<usize> = (0..=s.len()).filter(|&q| s.is_char_boundary(q)).collect();
    for assoc in [Assoc::Before, Assoc::After] {
        let mut mapped = boundaries.clone();
        a.map_sorted(&mut mapped, assoc)
            .expect("boundaries are in range");
        assert!(
            mapped.windows(2).all(|w| w[0] <= w[1]),
            "mapping not monotonic"
        );
        assert!(
            mapped.iter().all(|&m| t2.is_char_boundary(m)),
            "mapped off a char boundary"
        );
    }

    let Ok(b) = raw_changeset(u, t2.byte_len())? else {
        return Ok(());
    };
    match (b.validate(&t2), a.compose(&b)) {
        (Ok(()), composed) => {
            let ab = composed.expect("b is valid for a's output, so a.compose(b) succeeds");
            let mut direct = t.clone();
            ab.apply(&mut direct).expect("composition applies");
            let mut sequential = t2;
            b.apply(&mut sequential)
                .expect("validated change set applies");
            assert_eq!(direct, sequential, "composition differs from sequence");
        }
        // b is not valid for a's output; compose may still succeed (its
        // check covers only a's inserted text) but must not panic.
        (Err(_), _) => {}
    }
    Ok(())
}

/// Up to 16 random builder calls for a text of `len_before` bytes. Lengths
/// are drawn either within the remaining length (usually valid) or from all
/// of `u16` (usually not), so both valid and invalid change sets occur.
fn raw_changeset(
    u: &mut Unstructured<'_>,
    len_before: usize,
) -> arbitrary::Result<Result<ChangeSet, ChangeSetError>> {
    let mut b = ChangeSetBuilder::new(len_before);
    let mut covered = 0usize;
    for _ in 0..u.int_in_range(0..=16)? {
        let remaining = len_before.saturating_sub(covered);
        let n = if u.arbitrary()? {
            u.int_in_range(0..=remaining)?
        } else {
            usize::from(u.arbitrary::<u16>()?)
        };
        match u.int_in_range(0..=3)? {
            0 => {
                b.retain(n);
                covered = covered.saturating_add(n);
            }
            1 => {
                b.delete(n);
                covered = covered.saturating_add(n);
            }
            2 => {
                b.insert(u.arbitrary::<&str>()?);
            }
            _ => {
                b.retain_rest();
                covered = covered.max(len_before);
            }
        }
    }
    if u.arbitrary()? {
        b.retain_rest();
    }
    Ok(b.finish())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]
        #[test]
        fn fuzz_entry_survives_random_bytes(data in prop::collection::vec(any::<u8>(), 0..4096)) {
            super::changeset_ops(&data);
        }
    }
}
