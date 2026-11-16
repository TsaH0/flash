//! Trace format, reading limits, and deterministic replay.

use std::io::Cursor;

use proptest::prelude::*;

use super::*;
use crate::editor::proptests::{Xorshift, random_event};
use crate::strategies::text_strategy;
use crate::{KeyCode, KeyEvent, Modifiers, Selection};

fn header_for(text: &Text) -> TraceHeader {
    TraceHeader {
        flash_trace: TRACE_VERSION,
        document: TraceSource::Fixture("test".to_owned()),
        len: text.byte_len() as u64,
        hash: content_hash(text),
        size: (40, 12),
        cursor: 0,
        config: None,
    }
}

fn write_trace(header: &TraceHeader, entries: &[TraceEntry]) -> Vec<u8> {
    let mut w = TraceWriter::new(Vec::new(), header).unwrap();
    for e in entries {
        w.write(e).unwrap();
    }
    w.into_inner().unwrap()
}

fn read_all(bytes: &[u8], max_line: usize) -> Result<(TraceHeader, Vec<TraceEntry>), TraceError> {
    let reader = TraceReader::new(Cursor::new(bytes), max_line)?;
    let header = reader.header().clone();
    let entries = reader.collect::<Result<Vec<_>, _>>()?;
    Ok((header, entries))
}

#[test]
fn event_json_shape() {
    let key = Event::Key(KeyEvent::new(KeyCode::Char('x'), Modifiers::CTRL));
    let json =
        r#"{"kind":"key","code":{"char":"x"},"modifiers":{"shift":false,"ctrl":true,"alt":false}}"#;
    assert_eq!(serde_json::to_string(&key).unwrap(), json);
    assert_eq!(serde_json::from_str::<Event>(json).unwrap(), key);
    let bare: Event = serde_json::from_str(r#"{"kind":"key","code":"enter"}"#).unwrap();
    assert_eq!(bare, Event::Key(KeyEvent::plain(KeyCode::Enter)));
    let paste = Event::Paste {
        text: "a\u{1b}b".to_owned(),
    };
    let s = serde_json::to_string(&paste).unwrap();
    assert_eq!(s, r#"{"kind":"paste","text":"a\u001bb"}"#);
    assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), paste);
    let resize: Event = serde_json::from_str(r#"{"kind":"resize","width":3,"height":4}"#).unwrap();
    assert_eq!(
        resize,
        Event::Resize {
            width: 3,
            height: 4
        }
    );
}

#[test]
fn header_line_shape() {
    let text = Text::from("ab");
    let bytes = write_trace(&header_for(&text), &[]);
    let line = String::from_utf8(bytes).unwrap();
    assert_eq!(
        line,
        format!(
            "{{\"flash_trace\":1,\"document\":{{\"fixture\":\"test\"}},\"len\":2,\"hash\":{},\"size\":[40,12],\"cursor\":0}}\n",
            content_hash(&text)
        )
    );
}

#[test]
fn content_hash_is_fnv1a_over_chunks() {
    // FNV-1a 64 test vectors.
    assert_eq!(content_hash(&Text::from("")), 0xcbf2_9ce4_8422_2325);
    assert_eq!(content_hash(&Text::from("a")), 0xaf63_dc4c_8601_ec8c);
    let long = "abcdefghij".repeat(1000);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in long.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    assert_eq!(content_hash(&Text::from(long)), h);
}

#[test]
fn read_rejects_malformed() {
    let text = Text::from("ab");
    let good = write_trace(&header_for(&text), &[]);
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (Vec::new(), "missing header"),
        (b"\n\n".to_vec(), "blank only"),
        (b"{\"flash_trace\":1,".to_vec(), "truncated header"),
        (
            String::from_utf8(good.clone())
                .unwrap()
                .replace("\"flash_trace\":1", "\"flash_trace\":2")
                .into_bytes(),
            "version 2",
        ),
        (
            [
                &good[..],
                b"{\"t_ns\":1,\"event\":{\"kind\":\"teleport\"}}\n",
            ]
            .concat(),
            "unknown kind",
        ),
        (
            [
                &good[..],
                b"{\"t_ns\":1,\"event\":{\"kind\":\"key\",\"code\":",
            ]
            .concat(),
            "truncated entry",
        ),
        (
            [
                &good[..],
                b"{\"t_ns\":1,\"event\":{\"kind\":\"resize\",\"width\":70000,\"height\":1}}\n",
            ]
            .concat(),
            "resize out of range",
        ),
        ([&good[..], b"\xff\xfe\n"].concat(), "invalid UTF-8"),
    ];
    for (bytes, what) in cases {
        assert!(read_all(&bytes, DEFAULT_MAX_LINE).is_err(), "{what}");
    }
    assert!(matches!(
        read_all(
            &String::from_utf8(good.clone())
                .unwrap()
                .replace(":1,", ":2,")
                .into_bytes(),
            1024
        ),
        Err(TraceError::Version(2))
    ));
    // Line numbers are 1-based and count blank lines.
    let bad = [&good[..], b"\n{oops}\n"].concat();
    assert!(matches!(
        read_all(&bad, 1024),
        Err(TraceError::Json { line: 3, .. })
    ));
}

#[test]
fn read_bounds_line_length() {
    let text = Text::from("ab");
    let header = header_for(&text);
    let paste = |n: usize| TraceEntry {
        t_ns: 0,
        event: Event::Paste {
            text: "x".repeat(n),
        },
    };
    let bytes = write_trace(&header, &[paste(10), paste(500)]);
    let header_len = bytes.iter().position(|&b| b == b'\n').unwrap();
    // Long enough for the header and the short entry, not the long one.
    let max = header_len.max(60);
    let mut reader = TraceReader::new(Cursor::new(&bytes[..]), max).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), paste(10));
    assert!(matches!(
        reader.next(),
        Some(Err(TraceError::LineTooLong { line: 3, max: m })) if m == max
    ));
    assert!(reader.next().is_none(), "reader stops after an error");
    // A header longer than the limit is rejected too.
    assert!(matches!(
        TraceReader::new(Cursor::new(&bytes[..]), 8),
        Err(TraceError::LineTooLong { line: 1, max: 8 })
    ));
    // A last line without a newline is fine.
    let no_newline = &bytes[..bytes.len() - 1];
    assert_eq!(read_all(no_newline, 1024).unwrap().1.len(), 2);
}

#[test]
fn replay_rejects_bad_header() {
    let text = Text::from("aé");
    let good = header_for(&text);
    assert!(start(&good, text.snapshot()).is_ok());
    let wrong_len = TraceHeader {
        len: 9,
        ..good.clone()
    };
    assert!(matches!(
        start(&wrong_len, text.snapshot()),
        Err(TraceError::Mismatch { what: "len" })
    ));
    let wrong_hash = TraceHeader {
        hash: good.hash ^ 1,
        ..good.clone()
    };
    assert!(matches!(
        start(&wrong_hash, text.snapshot()),
        Err(TraceError::Mismatch { what: "hash" })
    ));
    for cursor in [2, 4, u64::MAX] {
        let bad = TraceHeader {
            cursor,
            ..good.clone()
        };
        assert!(
            matches!(start(&bad, text.snapshot()), Err(TraceError::Cursor(_))),
            "cursor {cursor}"
        );
    }
    let (ed, view) = start(&TraceHeader { cursor: 3, ..good }, text).unwrap();
    assert_eq!(ed.selection(view), Some(&Selection::point(3)));
    assert_eq!(ed.screen(), (40, 12));
}

/// `n` random entries from `seed`, with arbitrary (not monotonic) times.
fn random_entries(seed: u64, n: usize) -> Vec<TraceEntry> {
    let mut rng = Xorshift(seed | 1);
    (0..n)
        .map(|_| TraceEntry {
            t_ns: rng.next(usize::MAX) as u64,
            event: random_event(&mut rng),
        })
        .collect()
}

/// `n` random entries from `seed` with session-like times: mostly short
/// steps (typing groups), some pauses past the group limit, and sometimes
/// a step back.
fn timed_entries(seed: u64, n: usize) -> Vec<TraceEntry> {
    let mut rng = Xorshift(seed | 1);
    let mut t: u64 = 0;
    (0..n)
        .map(|_| {
            t = match rng.next(10) {
                0 => t.saturating_sub(rng.next(500_000_000) as u64),
                1 => t + 1_000_000_000 + rng.next(1_000_000_000) as u64,
                _ => t + rng.next(200_000_000) as u64,
            };
            TraceEntry {
                t_ns: t,
                event: random_event(&mut rng),
            }
        })
        .collect()
}

proptest! {
    #[test]
    fn round_trip(seed in any::<u64>(), n in 0usize..50, s in text_strategy(4)) {
        let text = Text::from(s.as_str());
        let header = header_for(&text);
        let entries = random_entries(seed, n);
        let (h, e) = read_all(&write_trace(&header, &entries), DEFAULT_MAX_LINE).unwrap();
        prop_assert_eq!(h, header);
        prop_assert_eq!(e, entries);
    }

    #[test]
    fn replay_reproduces_session(seed in any::<u64>(), s in text_strategy(8)) {
        let text = Text::from(s.as_str());
        let header = header_for(&text);
        // Session-like times (real grouping) or arbitrary ones.
        let entries = if seed % 2 == 0 {
            timed_entries(seed, 200)
        } else {
            random_entries(seed, 200)
        };

        // Live session, recorded as it runs.
        let (mut live, view) = start(&header, text.snapshot()).unwrap();
        let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
        for entry in &entries {
            live.handle_at(&entry.event, Timestamp::from_nanos(entry.t_ns));
            writer.write(entry).unwrap();
        }
        let bytes = writer.into_inner().unwrap();

        // Replayed from the recording.
        let mut reader = TraceReader::new(Cursor::new(&bytes[..]), DEFAULT_MAX_LINE).unwrap();
        let (mut replayed, view2) = start(reader.header(), text).unwrap();
        prop_assert_eq!(replay(&mut reader, &mut replayed).unwrap(), 200);

        // Both sessions closed the view (a random Ctrl+W), or neither.
        match (live.view_document(view), replayed.view_document(view2)) {
            (Some((lv, ld)), Some((rv, rd))) => {
                prop_assert_eq!(ld.text(), rd.text());
                prop_assert_eq!(ld.selection(view), rd.selection(view2));
                prop_assert_eq!(ld.revision(), rd.revision());
                prop_assert_eq!(ld.history().len(), rd.history().len());
                prop_assert_eq!(ld.history().head(), rd.history().head());
                prop_assert_eq!(lv.top(), rv.top());
                prop_assert_eq!(lv.size(), rv.size());
                prop_assert_eq!(lv.mode(), rv.mode());
            }
            (None, None) => {}
            (l, r) => prop_assert!(false, "live view {:?}, replayed {:?}", l.is_some(), r.is_some()),
        }
        prop_assert_eq!(live.now(), replayed.now());
        prop_assert_eq!(live.message(), replayed.message());
        prop_assert_eq!(live.keymap().pending(), replayed.keymap().pending());
        prop_assert_eq!(live.count(), replayed.count());
        prop_assert_eq!(live.should_quit(), replayed.should_quit());
    }
}
