//! Tests for the walking skeleton, driven through `handle` and `render`.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use flash_core::{Range, Selection, Text};

use super::Skeleton;

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn plain(code: KeyCode) -> Event {
    key(code, KeyModifiers::NONE)
}

fn skeleton(s: &str) -> Skeleton {
    Skeleton::new("test".to_owned(), Text::from(s), 80, 24)
}

fn heads(sk: &Skeleton) -> Vec<usize> {
    sk.selection().ranges().iter().map(Range::head).collect()
}

fn rendered(sk: &mut Skeleton) -> Vec<u8> {
    let mut out = Vec::new();
    sk.render(&mut out).unwrap();
    out
}

#[test]
fn typing_moving_and_deleting() {
    let mut sk = skeleton("ab\ncd");
    for e in [
        plain(KeyCode::Char('x')),
        plain(KeyCode::Right),
        plain(KeyCode::Down),
        plain(KeyCode::Backspace),
    ] {
        sk.handle(&e);
    }
    assert_eq!(sk.text().to_string(), "xab\nc");
    assert_eq!(heads(&sk), [5]);
}

#[test]
fn shift_extends() {
    let mut sk = skeleton("abc");
    sk.handle(&key(KeyCode::Right, KeyModifiers::SHIFT));
    sk.handle(&key(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(*sk.selection(), Selection::single(Range::new(0, 2)));
    sk.handle(&plain(KeyCode::Backspace));
    assert_eq!(sk.text().to_string(), "c");
}

#[test]
fn alt_down_adds_cursor_and_typing_hits_both() {
    let mut sk = skeleton("ab\ncd");
    sk.handle(&key(KeyCode::Down, KeyModifiers::ALT));
    assert_eq!(heads(&sk), [0, 3]);
    assert_eq!(sk.selection().primary_index(), 1);
    sk.handle(&plain(KeyCode::Char('y')));
    assert_eq!(sk.text().to_string(), "yab\nycd");
    sk.handle(&plain(KeyCode::Esc));
    assert_eq!(*sk.selection(), Selection::point(5));
}

#[test]
fn paste_is_one_insert() {
    let mut sk = skeleton("ab\ncd");
    sk.handle(&key(KeyCode::Down, KeyModifiers::ALT));
    sk.handle(&Event::Paste("p\nq".to_owned()));
    assert_eq!(sk.text().to_string(), "p\nqab\np\nqcd");
}

#[test]
fn ctrl_q_quits() {
    let mut sk = skeleton("");
    assert!(!sk.should_quit());
    sk.handle(&key(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(sk.should_quit());
    assert_eq!(sk.text().to_string(), "");
}

#[test]
fn backspace_at_start_is_harmless() {
    let mut sk = skeleton("ab");
    sk.handle(&plain(KeyCode::Backspace));
    assert_eq!(sk.text().to_string(), "ab");
    assert_eq!(sk.message(), "");
}

#[test]
fn word_and_document_keys() {
    let mut sk = skeleton("foo bar\nbaz");
    sk.handle(&key(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(heads(&sk), [4]);
    sk.handle(&key(KeyCode::End, KeyModifiers::CONTROL));
    assert_eq!(heads(&sk), [11]);
    sk.handle(&plain(KeyCode::Home));
    assert_eq!(heads(&sk), [8]);
    sk.handle(&key(KeyCode::Home, KeyModifiers::CONTROL));
    assert_eq!(heads(&sk), [0]);
}

#[test]
fn render_replaces_control_characters() {
    let mut sk = skeleton("a\u{1b}[2Jb\u{7}");
    let out = rendered(&mut sk);
    assert!(!out.windows(4).any(|w| w == b"\x1b[2J"));
    assert!(!out.contains(&0x07));
    let s = String::from_utf8_lossy(&out);
    assert_eq!(s.matches('\u{fffd}').count(), 2);
}

#[test]
fn render_replaces_control_characters_in_name() {
    let mut sk = Skeleton::new("evil\u{1b}[2J".to_owned(), Text::from("x"), 80, 24);
    let out = rendered(&mut sk);
    assert!(!out.windows(4).any(|w| w == b"\x1b[2J"));
}

#[test]
fn render_survives_tiny_terminals() {
    for s in ["", "abc", "漢字\tx"] {
        for (w, h) in [(0, 0), (1, 1), (2, 1), (1, 5), (0, 5), (5, 0)] {
            let mut sk = skeleton(s);
            sk.handle(&Event::Resize(w, h));
            rendered(&mut sk);
            sk.handle(&plain(KeyCode::Char('z')));
            rendered(&mut sk);
        }
    }
}

#[test]
fn render_never_splits_wide_char() {
    let mut sk = Skeleton::new("t".to_owned(), Text::from("a漢"), 2, 3);
    let s = String::from_utf8_lossy(&rendered(&mut sk)).into_owned();
    assert!(s.contains('a'));
    assert!(!s.contains('漢'));
}

#[test]
fn render_scrolls_to_primary() {
    let text: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let mut sk = Skeleton::new("t".to_owned(), Text::from(text), 40, 10);
    sk.handle(&key(KeyCode::End, KeyModifiers::CONTROL));
    let s = String::from_utf8_lossy(&rendered(&mut sk)).into_owned();
    assert!(s.contains("line99"));
    assert!(!s.contains("line0\u{1b}"));
}

#[test]
fn set_selection_validates() {
    let mut sk = skeleton("é");
    assert!(sk.set_selection(Selection::point(1)).is_err());
    assert!(sk.set_selection(Selection::point(2)).is_ok());
    assert_eq!(heads(&sk), [2]);
}

/// Random keys, pastes, and resizes over Unicode-heavy texts: never panics,
/// the selection stays valid, and no raw control byte from the text reaches
/// the output (BEL never appears; ESC only as crossterm's own CSI).
#[test]
fn random_events_never_panic() {
    const PIECES: &[&str] = &[
        "a",
        "xyz ",
        "\n",
        "\r\n",
        "\r",
        "\t",
        "é",
        "e\u{301}",
        "老虎",
        "😀",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "\u{1F1FA}\u{1F1F8}",
        "\u{7}",
        "\u{1b}[31m",
    ];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n) as usize
    };
    let codes = [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Char('x'),
        KeyCode::Char('é'),
        KeyCode::Char('\u{301}'),
    ];
    let mods = [
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ];
    for _ in 0..200 {
        let text: String = (0..next(40))
            .map(|_| PIECES[next(PIECES.len() as u64)])
            .collect();
        let mut sk = Skeleton::new(
            "t".to_owned(),
            Text::from(text.as_str()),
            1 + next(30) as u16,
            1 + next(8) as u16,
        );
        for _ in 0..100 {
            let event = match next(20) {
                0 => Event::Paste(PIECES[next(PIECES.len() as u64)].to_owned()),
                1 => Event::Resize(next(6) as u16, next(4) as u16),
                _ => key(
                    codes[next(codes.len() as u64)],
                    mods[next(mods.len() as u64)],
                ),
            };
            sk.handle(&event);
            assert_eq!(sk.selection().validate(sk.text()), Ok(()));
            let out = rendered(&mut sk);
            assert!(!out.contains(&0x07), "raw BEL after {event:?}");
            assert!(
                !out.windows(5).any(|w| w == b"\x1b[31m"),
                "raw SGR after {event:?}"
            );
        }
    }
}
