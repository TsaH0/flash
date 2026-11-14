//! Randomised equivalence between [`Text`] and the reference model.
//!
//! Each case builds a text from Unicode-heavy pieces, then applies a random
//! sequence of insert/delete/replace operations to both implementations.
//! Offsets are drawn from `0..=len + 2`, so many operations are deliberately
//! invalid (out of bounds, inside a multi-byte char, inverted ranges): both
//! implementations must return the same `Result` and stay identical.
//!
//! After every operation every query is compared. For texts up to
//! `FULL_CHECK_LEN` bytes every offset is checked; longer texts are checked on
//! a stride plus the neighbourhood of each chunk boundary.
//!
//! Run with `--features small-chunks` as well: crop then uses tiny leaves, so
//! even short texts exercise multi-leaf and chunk-boundary code paths.

#![allow(
    clippy::reversed_empty_ranges,
    reason = "inverted ranges are deliberate invalid inputs"
)]

use proptest::prelude::*;
use proptest::sample::Index;

use super::reference::{RefText, move_over_boundaries};
use super::*;
use crate::strategies::text_strategy;

const FULL_CHECK_LEN: usize = 600;

#[derive(Debug, Clone)]
enum Op {
    Insert(Index, String),
    Delete(Index, Index),
    Replace(Index, Index, String),
    /// A range whose start is after its end.
    DeleteInverted(Index, Index),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (any::<Index>(), text_strategy(4)).prop_map(|(a, s)| Op::Insert(a, s)),
        3 => (any::<Index>(), any::<Index>()).prop_map(|(a, b)| Op::Delete(a, b)),
        3 => (any::<Index>(), any::<Index>(), text_strategy(4))
            .prop_map(|(a, b, s)| Op::Replace(a, b, s)),
        1 => (any::<Index>(), any::<Index>()).prop_map(|(a, b)| Op::DeleteInverted(a, b)),
    ]
}

/// Maps an index to an offset in `0..=len + 2`, i.e. including invalid ones.
fn offset(i: &Index, len: usize) -> usize {
    i.index(len + 3)
}

fn ordered(a: usize, b: usize) -> Range<usize> {
    a.min(b)..a.max(b)
}

fn apply(op: &Op, t: &mut Text, r: &mut RefText) -> Result<(), TestCaseError> {
    let len = r.len();
    let (got, want) = match op {
        Op::Insert(a, s) => {
            let at = offset(a, len);
            (t.insert(at, s), r.insert(at, s))
        }
        Op::Delete(a, b) => {
            let range = ordered(offset(a, len), offset(b, len));
            (t.delete(range.clone()), r.delete(range))
        }
        Op::Replace(a, b, s) => {
            let range = ordered(offset(a, len), offset(b, len));
            (t.replace(range.clone(), s), r.replace(range, s))
        }
        Op::DeleteInverted(a, b) => {
            let range = ordered(offset(a, len), offset(b, len));
            // Swap so start > end whenever the two offsets differ.
            let inverted = range.end..range.start;
            (t.delete(inverted.clone()), r.delete(inverted))
        }
    };
    prop_assert_eq!(got, want, "result of {:?}", op);
    Ok(())
}

/// Offsets to compare for a text of `len` bytes.
fn offsets_to_check(t: &Text, len: usize) -> Vec<usize> {
    if len <= FULL_CHECK_LEN {
        return (0..=len + 2).collect();
    }
    let mut offsets: Vec<usize> = (0..=len + 2).step_by(13).collect();
    let mut chunk_start: usize = 0;
    for chunk in t.chunks() {
        for d in 0..4 {
            offsets.push(chunk_start.saturating_sub(d));
            offsets.push(chunk_start + d);
        }
        chunk_start += chunk.len();
    }
    offsets.extend(len.saturating_sub(4)..=len + 2);
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Compares every query between the two implementations.
fn assert_equivalent(t: &Text, r: &RefText) -> Result<(), TestCaseError> {
    prop_assert_eq!(t.to_string(), r.s.clone());
    prop_assert_eq!(t.byte_len(), r.len());
    prop_assert_eq!(t.is_empty(), r.s.is_empty());

    let chunks: Vec<&str> = t.chunks().collect();
    prop_assert!(chunks.iter().all(|c| !c.is_empty()));
    prop_assert_eq!(chunks.concat(), r.s.clone());

    let line_count = r.line_count();
    prop_assert_eq!(t.line_count(), line_count);
    for line in 0..=line_count {
        prop_assert_eq!(
            t.line_start(line),
            r.line_start(line),
            "line_start({})",
            line
        );
        prop_assert_eq!(
            t.line_range(line),
            r.line_range(line),
            "line_range({})",
            line
        );
        prop_assert_eq!(
            t.line_range_with_terminator(line),
            r.line_range_with_terminator(line),
            "line_range_with_terminator({})",
            line
        );
        if let Ok(range) = r.line_range(line) {
            prop_assert_eq!(t.line(line).unwrap().to_string(), &r.s[range]);
        }
    }

    // Grapheme motion: whole-text walks, then counted moves from every
    // checked offset (expected values from the precomputed boundary list).
    let bounds = r.grapheme_boundaries();
    prop_assert_eq!(
        t.move_graphemes(0, usize::MAX, Direction::Forward),
        Ok(r.len())
    );
    prop_assert_eq!(
        t.move_graphemes(r.len(), usize::MAX, Direction::Backward),
        Ok(0)
    );
    let mut p = 0;
    let mut walked = vec![p];
    while p < r.len() {
        p = t.move_graphemes(p, 1, Direction::Forward).unwrap();
        walked.push(p);
    }
    prop_assert_eq!(&walked, &bounds);

    for offset in offsets_to_check(t, r.len()) {
        for direction in [Direction::Forward, Direction::Backward] {
            for count in [0, 1, 2, 5] {
                let want = r
                    .check_offset(offset)
                    .map(|()| move_over_boundaries(&bounds, offset, count, direction));
                prop_assert_eq!(
                    t.move_graphemes(offset, count, direction),
                    want,
                    "move_graphemes({}, {}, {:?})",
                    offset,
                    count,
                    direction
                );
            }
        }
        prop_assert_eq!(
            t.check_offset(offset),
            r.check_offset(offset),
            "check_offset({})",
            offset
        );
        prop_assert_eq!(t.is_char_boundary(offset), r.is_char_boundary(offset));
        prop_assert_eq!(
            t.is_grapheme_boundary(offset),
            r.is_grapheme_boundary(offset),
            "is_grapheme_boundary({})",
            offset
        );
        prop_assert_eq!(
            t.line_of_offset(offset),
            r.line_of_offset(offset),
            "line_of_offset({})",
            offset
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Short texts: every offset is checked after every operation.
    #[test]
    fn edits_match_reference(
        initial in text_strategy(12),
        ops in prop::collection::vec(op(), 0..24),
    ) {
        let mut t = Text::from(initial.as_str());
        let mut r = RefText::new(&initial);
        assert_equivalent(&t, &r)?;
        let first_snapshot = (t.snapshot(), r.clone());
        for op in &ops {
            apply(op, &mut t, &mut r)?;
            assert_equivalent(&t, &r)?;
        }
        // Snapshots are unaffected by every later edit.
        assert_equivalent(&first_snapshot.0, &first_snapshot.1)?;
    }

    /// Long texts spanning several default-size rope leaves.
    #[test]
    fn edits_match_reference_long(
        initial in text_strategy(160),
        ops in prop::collection::vec(op(), 0..12),
    ) {
        let mut t = Text::from(initial.as_str());
        let mut r = RefText::new(&initial);
        assert_equivalent(&t, &r)?;
        for op in &ops {
            apply(op, &mut t, &mut r)?;
            assert_equivalent(&t, &r)?;
        }
    }

    /// Building in pieces produces the same text as building at once.
    #[test]
    fn builder_matches_reference(pieces in prop::collection::vec(text_strategy(6), 0..40)) {
        let mut b = TextBuilder::new();
        for p in &pieces {
            b.append(p);
        }
        let t = b.build();
        assert_equivalent(&t, &RefText::new(&pieces.concat()))?;
    }
}
