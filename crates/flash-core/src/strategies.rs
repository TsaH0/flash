//! Proptest strategies shared by the `text` and `changeset` property tests.

use proptest::prelude::*;

/// Pieces chosen to hit the interesting cases: multi-byte chars of every
/// width, combining marks, ZWJ sequences, regional indicators, every line
/// ending, and a long run.
pub(crate) fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => Just("a".to_owned()),
        2 => Just("xyz ".to_owned()),
        3 => Just("\n".to_owned()),
        2 => Just("\r\n".to_owned()),
        1 => Just("\r".to_owned()),
        1 => Just("\t".to_owned()),
        2 => Just("é".to_owned()),                 // 2 bytes, precomposed
        2 => Just("e\u{301}".to_owned()),          // combining acute
        2 => Just("老虎".to_owned()),               // 3 bytes each
        2 => Just("😀".to_owned()),                 // 4 bytes
        1 => Just("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".to_owned()), // ZWJ family
        1 => Just("\u{1F1FA}\u{1F1F8}".to_owned()), // flag
        1 => Just("\u{1F1EF}".to_owned()),          // lone regional indicator
        1 => Just("\u{200D}".to_owned()),           // stray ZWJ
        1 => Just("\u{301}".to_owned()),            // stray combining mark
        1 => "[a-z\u{e9}\u{8001}\n]{50,300}",        // longer mixed run
    ]
}

pub(crate) fn text_strategy(max_pieces: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(piece(), 0..max_pieces).prop_map(|v| v.concat())
}

/// The largest char boundary of `s` that is `<= offset` (clamped to `s.len()`).
pub(crate) fn floor_boundary(s: &str, offset: usize) -> usize {
    let offset = offset.min(s.len());
    (0..=offset)
        .rev()
        .find(|&o| s.is_char_boundary(o))
        .unwrap_or(0)
}

/// Sorted, non-overlapping edits on char boundaries of `s` (up to 8).
pub(crate) fn edits_for(
    s: &str,
) -> impl Strategy<Value = Vec<(std::ops::Range<usize>, String)>> + use<> {
    let s = s.to_owned();
    prop::collection::vec(
        (
            any::<prop::sample::Index>(),
            any::<prop::sample::Index>(),
            text_strategy(3),
        ),
        0..8,
    )
    .prop_map(move |raw| {
        let mut edits: Vec<_> = raw
            .into_iter()
            .map(|(a, b, text)| {
                let a = floor_boundary(&s, a.index(s.len() + 1));
                let b = floor_boundary(&s, b.index(s.len() + 1));
                (a.min(b)..a.max(b), text)
            })
            .collect();
        edits.sort_by_key(|(r, _)| r.start);
        let mut prev_end = 0;
        edits.retain(|(r, _)| {
            let keep = r.start >= prev_end;
            if keep {
                prev_end = r.end;
            }
            keep
        });
        edits
    })
}

/// A change set valid for `s` (built through `ChangeSet::from_edits`).
pub(crate) fn changeset_for(s: &str) -> impl Strategy<Value = crate::ChangeSet> + use<> {
    let len = s.len();
    edits_for(s).prop_map(move |edits| {
        crate::ChangeSet::from_edits(len, edits).expect("generated edits are valid")
    })
}

/// A raw, usually non-canonical op list covering exactly `s`, on char
/// boundaries: empty ops, same-kind runs, and `Insert` before `Delete` all
/// occur.
pub(crate) fn raw_ops_for(s: &str) -> impl Strategy<Value = Vec<crate::Op>> + use<> {
    let s = s.to_owned();
    prop::collection::vec(
        (0..3u8, any::<prop::sample::Index>(), text_strategy(2)),
        0..12,
    )
    .prop_map(move |raw| {
        let mut ops = Vec::new();
        let mut pos = 0;
        for (kind, index, text) in raw {
            let target = floor_boundary(&s, pos + index.index(s.len() - pos + 1)).max(pos);
            match kind {
                0 => ops.push(crate::Op::Retain(target - pos)),
                1 => ops.push(crate::Op::Delete(target - pos)),
                _ => {
                    ops.push(crate::Op::Insert(text.into()));
                    continue;
                }
            }
            pos = target;
        }
        ops.push(crate::Op::Retain(s.len() - pos));
        ops
    })
}

/// A selection valid for `s`: 1 to 5 ranges with char-boundary endpoints
/// and a random primary.
pub(crate) fn selection_for(s: &str) -> impl Strategy<Value = crate::Selection> + use<> {
    let s = s.to_owned();
    (
        prop::collection::vec(
            (any::<prop::sample::Index>(), any::<prop::sample::Index>()),
            1..6,
        ),
        any::<prop::sample::Index>(),
    )
        .prop_map(move |(raw, primary)| {
            let ranges: Vec<_> = raw
                .into_iter()
                .map(|(a, h)| {
                    crate::Range::new(
                        floor_boundary(&s, a.index(s.len() + 1)),
                        floor_boundary(&s, h.index(s.len() + 1)),
                    )
                })
                .collect();
            let primary = primary.index(ranges.len());
            crate::Selection::new(ranges, primary).expect("non-empty, primary in bounds")
        })
}
