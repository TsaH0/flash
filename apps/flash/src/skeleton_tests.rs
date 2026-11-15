//! Tests for the skeleton frontend: event conversion and rendering of the
//! headless editor. Key behaviour is tested in `flash_core::editor`.

use crossterm::event::{
    self as ct, KeyCode as CtKey, KeyEvent as CtKeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
};
use flash_core::{Editor, Event, KeyCode, KeyEvent, Modifiers, Text};

use super::{Skeleton, convert};

fn plain(code: KeyCode) -> Event {
    Event::Key(KeyEvent::plain(code))
}

fn editor_with(s: &str, width: u16, height: u16) -> Editor {
    let mut ed = Editor::new(width, height);
    let doc = ed.open(Text::from(s), None);
    ed.open_view(doc).unwrap();
    ed
}

fn rendered(ed: &Editor, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    Skeleton::new(name.to_owned()).render(ed, &mut out).unwrap();
    out
}

fn ct_key(code: CtKey, modifiers: KeyModifiers) -> ct::Event {
    ct::Event::Key(CtKeyEvent::new(code, modifiers))
}

#[test]
fn convert_maps_keys() {
    assert_eq!(
        convert(&ct_key(CtKey::Char('x'), KeyModifiers::NONE)),
        Some(plain(KeyCode::Char('x')))
    );
    assert_eq!(
        convert(&ct_key(
            CtKey::Right,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT
        )),
        Some(Event::Key(KeyEvent::new(
            KeyCode::Right,
            Modifiers {
                shift: true,
                ctrl: true,
                alt: true
            }
        )))
    );
    assert_eq!(
        convert(&ct_key(CtKey::BackTab, KeyModifiers::SHIFT)),
        Some(Event::Key(KeyEvent::new(KeyCode::Tab, Modifiers::SHIFT)))
    );
    assert_eq!(
        convert(&ct_key(CtKey::F(7), KeyModifiers::NONE)),
        Some(plain(KeyCode::F(7)))
    );
    let release = ct::Event::Key(CtKeyEvent {
        code: CtKey::Char('x'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    });
    assert_eq!(convert(&release), None);
    assert_eq!(convert(&ct_key(CtKey::CapsLock, KeyModifiers::NONE)), None);
    assert_eq!(convert(&ct::Event::FocusGained), None);
    assert_eq!(
        convert(&ct::Event::Paste("p\nq".to_owned())),
        Some(Event::Paste {
            text: "p\nq".to_owned()
        })
    );
    assert_eq!(
        convert(&ct::Event::Resize(3, 4)),
        Some(Event::Resize {
            width: 3,
            height: 4
        })
    );
}

#[test]
fn render_replaces_control_characters() {
    let ed = editor_with("a\u{1b}[2Jb\u{7}", 80, 24);
    let out = rendered(&ed, "test");
    assert!(!out.windows(4).any(|w| w == b"\x1b[2J"));
    assert!(!out.contains(&0x07));
    let s = String::from_utf8_lossy(&out);
    assert_eq!(s.matches('\u{fffd}').count(), 2);
}

#[test]
fn render_replaces_control_characters_in_name() {
    let ed = editor_with("x", 80, 24);
    let out = rendered(&ed, "evil\u{1b}[2J");
    assert!(!out.windows(4).any(|w| w == b"\x1b[2J"));
}

#[test]
fn render_survives_tiny_terminals() {
    for s in ["", "abc", "漢字\tx"] {
        for (w, h) in [(0, 0), (1, 1), (2, 1), (1, 5), (0, 5), (5, 0)] {
            let mut ed = editor_with(s, 80, 24);
            ed.handle(&Event::Resize {
                width: w,
                height: h,
            });
            rendered(&ed, "t");
            ed.handle(&plain(KeyCode::Char('z')));
            rendered(&ed, "t");
        }
    }
}

#[test]
fn render_never_splits_wide_char() {
    let ed = editor_with("a漢", 2, 3);
    let s = String::from_utf8_lossy(&rendered(&ed, "t")).into_owned();
    assert!(s.contains('a'));
    assert!(!s.contains('漢'));
}

#[test]
fn render_scrolls_to_primary() {
    let text: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let mut ed = editor_with(&text, 40, 10);
    ed.handle(&Event::Key(KeyEvent::new(KeyCode::End, Modifiers::CTRL)));
    let s = String::from_utf8_lossy(&rendered(&ed, "t")).into_owned();
    assert!(s.contains("line99"));
    assert!(!s.contains("line0\u{1b}"));
}

#[test]
fn render_without_focus_clears() {
    let mut ed = editor_with("hello", 20, 5);
    let view = ed.focus().unwrap();
    ed.close_view(view).unwrap();
    let s = String::from_utf8_lossy(&rendered(&ed, "t")).into_owned();
    assert!(!s.contains("hello"));
}

/// Random keys, pastes, and resizes over Unicode-heavy texts, rendered
/// after every event: never panics, and no raw control byte from the text
/// reaches the output (BEL never appears; ESC only as crossterm's own CSI).
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
        Modifiers::NONE,
        Modifiers::SHIFT,
        Modifiers::CTRL,
        Modifiers::ALT,
        Modifiers {
            shift: true,
            ctrl: true,
            alt: false,
        },
    ];
    for _ in 0..200 {
        let text: String = (0..next(40))
            .map(|_| PIECES[next(PIECES.len() as u64)])
            .collect();
        let mut ed = editor_with(&text, 1 + next(30) as u16, 1 + next(8) as u16);
        for _ in 0..100 {
            let event = match next(20) {
                0 => Event::Paste {
                    text: PIECES[next(PIECES.len() as u64)].to_owned(),
                },
                1 => Event::Resize {
                    width: next(6) as u16,
                    height: next(4) as u16,
                },
                _ => Event::Key(KeyEvent::new(
                    codes[next(codes.len() as u64)],
                    mods[next(mods.len() as u64)],
                )),
            };
            ed.handle(&event);
            let out = rendered(&ed, "t");
            assert!(!out.contains(&0x07), "raw BEL after {event:?}");
            assert!(
                !out.windows(5).any(|w| w == b"\x1b[31m"),
                "raw SGR after {event:?}"
            );
        }
    }
}
