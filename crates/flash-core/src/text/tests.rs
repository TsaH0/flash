//! Hand-written cases for the documented behaviour of [`Text`]. Each line-model
//! and boundary case is also run against the reference model, so a wrong
//! expectation here cannot silently agree with a wrong implementation.

#![allow(
    clippy::reversed_empty_ranges,
    reason = "inverted ranges are deliberate invalid inputs"
)]

use super::reference::RefText;
use super::*;

/// (line content without terminator, line range with terminator) for every line.
fn lines_of(t: &Text) -> Vec<(String, Range<usize>)> {
    (0..t.line_count())
        .map(|i| {
            (
                t.line(i).unwrap().to_string(),
                t.line_range_with_terminator(i).unwrap(),
            )
        })
        .collect()
}

fn ref_lines_of(r: &RefText) -> Vec<(String, Range<usize>)> {
    (0..r.line_count())
        .map(|i| {
            (
                r.s[r.line_range(i).unwrap()].to_owned(),
                r.line_range_with_terminator(i).unwrap(),
            )
        })
        .collect()
}

fn assert_lines(s: &str, expected: &[(&str, Range<usize>)]) {
    let expected: Vec<(String, Range<usize>)> = expected
        .iter()
        .map(|(c, r)| ((*c).to_owned(), r.clone()))
        .collect();
    assert_eq!(lines_of(&Text::from(s)), expected, "Text lines of {s:?}");
    assert_eq!(
        ref_lines_of(&RefText::new(s)),
        expected,
        "RefText lines of {s:?}"
    );
}

#[test]
fn line_model_basic_cases() {
    assert_lines("", &[("", 0..0)]);
    assert_lines("a", &[("a", 0..1)]);
    assert_lines("a\n", &[("a", 0..2), ("", 2..2)]);
    assert_lines("\n", &[("", 0..1), ("", 1..1)]);
    assert_lines("\n\n", &[("", 0..1), ("", 1..2), ("", 2..2)]);
    assert_lines("ab\ncd", &[("ab", 0..3), ("cd", 3..5)]);
}

#[test]
fn line_model_crlf_and_lone_cr() {
    assert_lines("a\r\nb", &[("a", 0..3), ("b", 3..4)]);
    assert_lines("a\r\n", &[("a", 0..3), ("", 3..3)]);
    assert_lines("\r\n", &[("", 0..2), ("", 2..2)]);
    // A lone CR is content, not a terminator.
    assert_lines("a\rb", &[("a\rb", 0..3)]);
    assert_lines("a\r", &[("a\r", 0..2)]);
    // CR CR LF: only the final CR LF is the terminator.
    assert_lines("a\r\r\nb", &[("a\r", 0..4), ("b", 4..5)]);
    // Mixed endings are preserved as stored.
    assert_lines("a\nb\r\nc\rd", &[("a", 0..2), ("b", 2..5), ("c\rd", 5..8)]);
}

#[test]
fn line_count_is_never_zero() {
    assert_eq!(Text::new().line_count(), 1);
    assert_eq!(Text::from("x\ny\n").line_count(), 3);
}

#[test]
fn line_of_offset_cases() {
    let t = Text::from("ab\r\ncd\n");
    let r = RefText::new("ab\r\ncd\n");
    let expected = [0, 0, 0, 0, 1, 1, 1, 2];
    for (offset, &line) in expected.iter().enumerate() {
        assert_eq!(t.line_of_offset(offset), Ok(line), "offset {offset}");
        assert_eq!(r.line_of_offset(offset), Ok(line), "ref offset {offset}");
    }
}

#[test]
fn line_errors() {
    let t = Text::from("a\nb");
    assert_eq!(
        t.line_start(2),
        Err(TextError::LineOutOfBounds {
            line: 2,
            line_count: 2
        })
    );
    assert_eq!(
        t.line_range(usize::MAX).unwrap_err(),
        TextError::LineOutOfBounds {
            line: usize::MAX,
            line_count: 2
        }
    );
    assert!(t.line(5).is_err());
    assert_eq!(
        t.line_of_offset(4),
        Err(TextError::OffsetOutOfBounds { offset: 4, len: 3 })
    );
}

#[test]
fn offset_validation() {
    let t = Text::from("aé老😀"); // 1 + 2 + 3 + 4 bytes
    let valid = [0, 1, 3, 6, 10];
    for offset in 0..=12 {
        let expected = if offset > 10 {
            Err(TextError::OffsetOutOfBounds { offset, len: 10 })
        } else if valid.contains(&offset) {
            Ok(())
        } else {
            Err(TextError::NotCharBoundary { offset })
        };
        assert_eq!(t.check_offset(offset), expected, "offset {offset}");
        assert_eq!(t.is_char_boundary(offset), expected.is_ok());
    }
    assert!(t.check_offset(usize::MAX).is_err());
    assert!(!t.is_char_boundary(usize::MAX));
}

#[test]
fn range_validation() {
    let t = Text::from("aé");
    assert_eq!(t.check_range(&(0..3)), Ok(()));
    assert_eq!(
        t.check_range(&(2..1)),
        Err(TextError::InvertedRange { start: 2, end: 1 })
    );
    assert_eq!(
        t.check_range(&(0..2)),
        Err(TextError::NotCharBoundary { offset: 2 })
    );
    assert_eq!(
        t.check_range(&(2..3)),
        Err(TextError::NotCharBoundary { offset: 2 })
    );
    assert_eq!(
        t.check_range(&(0..9)),
        Err(TextError::OffsetOutOfBounds { offset: 9, len: 3 })
    );
    assert!(t.slice(0..2).is_err());
    assert_eq!(t.slice(1..3).unwrap(), "é");
}

#[test]
fn grapheme_boundaries() {
    let cases: &[(&str, &[usize])] = &[
        ("", &[0]),
        ("ab", &[0, 1, 2]),
        // CR LF is one grapheme.
        ("a\r\nb", &[0, 1, 3, 4]),
        // e + combining acute accent.
        ("e\u{301}x", &[0, 3, 4]),
        // Family emoji joined by ZWJ (man, ZWJ, woman, ZWJ, girl).
        ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}!", &[0, 18, 19]),
        // Two flags: regional indicators pair up.
        ("\u{1F1FA}\u{1F1F8}\u{1F1EF}\u{1F1F5}", &[0, 8, 16]),
        // Hangul syllable as conjoining jamo.
        ("\u{1100}\u{1161}\u{11A8}", &[0, 9]),
    ];
    for &(s, boundaries) in cases {
        let t = Text::from(s);
        let r = RefText::new(s);
        for offset in 0..=s.len() + 1 {
            let expected = boundaries.contains(&offset);
            assert_eq!(t.is_grapheme_boundary(offset), expected, "{s:?} @ {offset}");
            assert_eq!(
                r.is_grapheme_boundary(offset),
                expected,
                "ref {s:?} @ {offset}"
            );
        }
    }
}

#[test]
fn grapheme_boundary_needs_context_across_chunks() {
    // Long runs of regional indicators make the cursor request context far
    // behind the offset, across many rope chunks.
    let flag = "\u{1F1FA}\u{1F1F8}"; // 8 bytes, one grapheme
    let s = format!("x{}", flag.repeat(2_000)); // 16 KB, several leaves
    let t = Text::from(s.as_str());
    let r = RefText::new(&s);
    for offset in (0..=s.len()).step_by(4) {
        assert_eq!(
            t.is_grapheme_boundary(offset),
            r.is_grapheme_boundary(offset),
            "@ {offset}"
        );
    }
}

#[test]
fn chunks_cover_text_and_are_never_empty() {
    let s = "line one\n".repeat(2_000) + "é老😀";
    let t = Text::from(s.as_str());
    let forward: Vec<&str> = t.chunks().collect();
    assert!(forward.len() > 1, "fixture should span several chunks");
    assert!(forward.iter().all(|c| !c.is_empty()));
    assert_eq!(forward.concat(), s);
    let mut backward: Vec<&str> = t.chunks().rev().collect();
    backward.reverse();
    assert_eq!(backward, forward);
    assert_eq!(Text::new().chunks().count(), 0);

    let sub = t.slice(5..s.len() - 4).unwrap();
    assert_eq!(sub.chunks().collect::<String>(), &s[5..s.len() - 4]);
}

#[test]
fn snapshot_is_independent_of_later_edits() {
    let mut t = Text::from("hello world");
    let snap = t.snapshot();
    t.replace(0..5, "goodbye").unwrap();
    t.insert(t.len(), "!").unwrap();
    assert_eq!(snap, "hello world");
    assert_eq!(t, "goodbye world!");
}

#[test]
fn failed_edits_leave_text_unchanged() {
    let mut t = Text::from("aé");
    let before = t.snapshot();
    assert_eq!(
        t.insert(2, "x"),
        Err(TextError::NotCharBoundary { offset: 2 })
    );
    assert_eq!(
        t.insert(4, "x"),
        Err(TextError::OffsetOutOfBounds { offset: 4, len: 3 })
    );
    assert_eq!(
        t.delete(1..2),
        Err(TextError::NotCharBoundary { offset: 2 })
    );
    assert_eq!(
        t.delete(3..1),
        Err(TextError::InvertedRange { start: 3, end: 1 })
    );
    assert_eq!(
        t.replace(0..usize::MAX, ""),
        Err(TextError::OffsetOutOfBounds {
            offset: usize::MAX,
            len: 3
        })
    );
    assert_eq!(t, before);
}

#[test]
fn edits_split_and_join_crlf() {
    let mut t = Text::from("a\r\nb");
    // Inserting between CR and LF turns one terminator into a lone CR + LF.
    t.insert(2, "x").unwrap();
    assert_eq!(t, "a\rx\nb");
    assert_eq!(t.line(0).unwrap(), "a\rx");
    // Deleting the inserted byte rejoins them.
    t.delete(2..3).unwrap();
    assert_eq!(t.line(0).unwrap(), "a");
    assert_eq!(t.line_range(0).unwrap(), 0..1);
    // Deleting the LF leaves a lone CR as content.
    t.delete(2..3).unwrap();
    assert_eq!(t.line_count(), 1);
    assert_eq!(t.line(0).unwrap(), "a\rb");
}

#[test]
fn empty_edits_are_no_ops() {
    let mut t = Text::from("abc");
    t.insert(1, "").unwrap();
    t.delete(2..2).unwrap();
    t.replace(3..3, "").unwrap();
    assert_eq!(t, "abc");
    let mut e = Text::new();
    e.insert(0, "").unwrap();
    e.delete(0..0).unwrap();
    assert!(e.is_empty());
}

#[test]
fn builder_matches_from() {
    let mut b = TextBuilder::new();
    for _ in 0..1_000 {
        b.append("abc\n").append("é");
    }
    let built = b.build();
    assert_eq!(built, Text::from("abc\né".repeat(1_000)));
    assert_eq!(built.line_count(), 1_001);
}

#[test]
fn error_messages() {
    assert_eq!(
        TextError::NotCharBoundary { offset: 3 }.to_string(),
        "byte offset 3 is not on a UTF-8 character boundary"
    );
}
