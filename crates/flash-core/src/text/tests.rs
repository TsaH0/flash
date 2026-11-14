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

/// Walks the whole text one grapheme at a time in `direction`.
fn walk(t: &Text, direction: Direction) -> Vec<usize> {
    let mut p = match direction {
        Direction::Forward => 0,
        Direction::Backward => t.byte_len(),
    };
    let mut seen = vec![p];
    loop {
        let next = t.move_graphemes(p, 1, direction).unwrap();
        if next == p {
            return seen;
        }
        seen.push(next);
        p = next;
    }
}

#[test]
fn move_graphemes_visits_every_boundary() {
    let cases = [
        "",
        "ab",
        "a\r\nb",
        "e\u{301}x",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}!",
        "\u{1F1FA}\u{1F1F8}\u{1F1EF}\u{1F1F5}",
        "\u{1100}\u{1161}\u{11A8}",
    ];
    for s in cases {
        let t = Text::from(s);
        let bounds = RefText::new(s).grapheme_boundaries();
        assert_eq!(walk(&t, Direction::Forward), bounds, "{s:?} forward");
        let mut backward = walk(&t, Direction::Backward);
        backward.reverse();
        assert_eq!(backward, bounds, "{s:?} backward");
    }
}

#[test]
fn move_graphemes_counts_clamps_and_validates() {
    let s = "ae\u{301}\u{1F1FA}\u{1F1F8}b"; // a | é | flag | b
    let t = Text::from(s);
    let fwd = Direction::Forward;
    let back = Direction::Backward;
    assert_eq!(t.move_graphemes(0, 0, fwd), Ok(0));
    assert_eq!(t.move_graphemes(0, 2, fwd), Ok(4));
    assert_eq!(t.move_graphemes(0, 3, fwd), Ok(12));
    assert_eq!(t.move_graphemes(0, 99, fwd), Ok(s.len()));
    assert_eq!(t.move_graphemes(s.len(), 2, back), Ok(4));
    assert_eq!(t.move_graphemes(s.len(), usize::MAX, back), Ok(0));
    assert_eq!(t.move_graphemes(0, 1, back), Ok(0));
    assert_eq!(t.move_graphemes(s.len(), 1, fwd), Ok(s.len()));
    // From inside a cluster (between 'e' and the combining mark), the first
    // step goes to the nearest boundary in that direction.
    assert_eq!(t.move_graphemes(2, 1, fwd), Ok(4));
    assert_eq!(t.move_graphemes(2, 1, back), Ok(1));
    // Invalid offsets are rejected like everywhere else.
    assert_eq!(
        t.move_graphemes(3, 1, fwd),
        Err(TextError::NotCharBoundary { offset: 3 })
    );
    assert_eq!(
        t.move_graphemes(99, 0, back),
        Err(TextError::OffsetOutOfBounds {
            offset: 99,
            len: s.len()
        })
    );
}

#[test]
fn move_graphemes_across_chunks_with_long_context() {
    // Regional-indicator pairing depends on every indicator back to the
    // start of the run, so steps near chunk edges need context from many
    // chunks behind. Both parities of the run's start are covered.
    let flag = "\u{1F1FA}\u{1F1F8}";
    for prefix in ["x", "\u{1F1EF}"] {
        let s = format!("{prefix}{}y\n", flag.repeat(2_000));
        let t = Text::from(s.as_str());
        assert!(t.chunks().count() > 1, "fixture should span several chunks");
        let r = RefText::new(&s);
        let bounds = r.grapheme_boundaries();
        assert_eq!(walk(&t, Direction::Forward), bounds);
        for count in [1, 2, 3, 7, 64, 1_000] {
            for start in (0..=s.len()).step_by(4 * 97) {
                for direction in [Direction::Forward, Direction::Backward] {
                    assert_eq!(
                        t.move_graphemes(start, count, direction),
                        r.move_graphemes(start, count, direction),
                        "{prefix:?} start {start} count {count} {direction:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn move_graphemes_after_edits() {
    // Edits split leaves into gap buffers with two chunks each; motion must
    // see both sides.
    let mut s = "fn main() { println!(\"héllo 😀\"); }\n".repeat(400);
    let mut t = Text::from(s.as_str());
    for i in 0..50 {
        let at = (i * 7_919) % s.len();
        let at = (0..=at).rev().find(|&o| s.is_char_boundary(o)).unwrap();
        let piece = ["e\u{301}", "\u{1F1FA}\u{1F1F8}", "\r\n", "\u{200D}"][i % 4];
        t.insert(at, piece).unwrap();
        s.insert_str(at, piece);
    }
    let r = RefText::new(&s);
    assert_eq!(walk(&t, Direction::Forward), r.grapheme_boundaries());
    let mut backward = walk(&t, Direction::Backward);
    backward.reverse();
    assert_eq!(backward, r.grapheme_boundaries());
    for start in (0..s.len()).filter(|&o| s.is_char_boundary(o)).step_by(31) {
        for direction in [Direction::Forward, Direction::Backward] {
            assert_eq!(
                t.move_graphemes(start, 16, direction),
                r.move_graphemes(start, 16, direction),
                "start {start} {direction:?}"
            );
        }
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
fn chunk_at_returns_the_chunk_containing_each_byte() {
    // Pins the vendored `Rope::chunk_at_byte` (DECISIONS 008), especially at
    // leaf and gap edges, so a crop upgrade that changes its lookup fails
    // here rather than only through grapheme results. Edits split leaves
    // into gap buffers, whose two sides are separate chunks.
    let mut s = "line one\né老😀\n".repeat(1_500);
    let mut t = Text::from(s.as_str());
    for i in 0..40 {
        let at = (i * 4_099) % s.len();
        let at = (0..=at).rev().find(|&o| s.is_char_boundary(o)).unwrap();
        t.insert(at, "ab😀").unwrap();
        s.insert_str(at, "ab😀");
    }
    assert_eq!(t, s.as_str());

    // Expected tiling, from the public chunk iterator.
    let mut expected = Vec::new();
    let mut start = 0;
    for c in t.chunks() {
        expected.push((start, c));
        start += c.len();
    }
    assert!(expected.len() > 1, "fixture should span several chunks");

    let mut i = 0;
    for b in 0..t.byte_len() {
        if b == expected[i].0 + expected[i].1.len() {
            i += 1;
        }
        let chunk = t.chunk_at(b).unwrap_or_else(|| panic!("no chunk at {b}"));
        assert!(!chunk.text.is_empty(), "empty chunk at {b}");
        assert!(chunk.start <= b && b < chunk.end(), "{b} not in chunk");
        assert_eq!(chunk.text, &s[chunk.start..chunk.end()], "content at {b}");
        assert_eq!(
            (chunk.start, chunk.text),
            expected[i],
            "chunk at {b} differs from chunks()"
        );
    }
    assert!(t.chunk_at(t.byte_len()).is_none());
    assert!(t.chunk_at(usize::MAX).is_none());
    assert!(Text::new().chunk_at(0).is_none());
}

#[test]
fn snapshot_is_independent_of_later_edits() {
    let mut t = Text::from("hello world");
    let snap = t.snapshot();
    t.replace(0..5, "goodbye").unwrap();
    t.insert(t.byte_len(), "!").unwrap();
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
fn text_can_be_sent_to_worker_threads() {
    // PLAN 2.8: background workers receive snapshots. Compile-time check.
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Text>();
    assert_send_sync::<TextError>();

    let mut t = Text::from("shared");
    let snap = t.snapshot();
    let worker = std::thread::spawn(move || snap.to_string());
    t.insert(0, "un").unwrap();
    assert_eq!(worker.join().unwrap(), "shared");
    assert_eq!(t, "unshared");
}

#[test]
fn line_and_slice_errors_match_reference_for_every_line() {
    let s = "a\r\nb\n\nc";
    let (t, r) = (Text::from(s), RefText::new(s));
    for line in 0..=r.line_count() + 1 {
        assert_eq!(
            t.line(line).is_ok(),
            r.line_range(line).is_ok(),
            "line {line}"
        );
        assert_eq!(
            t.line_with_terminator(line).is_ok(),
            r.line_range(line).is_ok()
        );
    }
}

#[test]
fn error_messages() {
    assert_eq!(
        TextError::NotCharBoundary { offset: 3 }.to_string(),
        "byte offset 3 is not on a UTF-8 character boundary"
    );
}
