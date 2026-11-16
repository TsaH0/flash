//! Random event streams through the headless editor.

use super::*;
use crate::selection::proptests::assert_normalised;
use crate::width::display_column;

pub(crate) const PIECES: &[&str] = &[
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

/// A small deterministic xorshift generator: `next(n)` is in `0..n`.
pub(crate) struct Xorshift(pub(crate) u64);

impl Xorshift {
    pub(crate) fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A random event: mostly keys from the full `KeyCode` set with random
/// modifiers, some pastes (including BEL and CSI bytes), some resizes up to
/// 300×100 (including 0).
pub(crate) fn random_event(rng: &mut Xorshift) -> Event {
    let codes = [
        KeyCode::Char('x'),
        KeyCode::Char('é'),
        KeyCode::Char('\u{301}'),
        KeyCode::Char('\u{1F1EF}'),
        KeyCode::Char('\u{200D}'),
        KeyCode::Char('\u{1F600}'),
        KeyCode::Char('q'),
        KeyCode::Char('z'),
        KeyCode::Char('Z'),
        KeyCode::Char('y'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Esc,
        KeyCode::F(1),
    ];
    match rng.next(20) {
        0 => Event::Paste {
            text: PIECES[rng.next(PIECES.len())].to_owned(),
        },
        1 => Event::Resize {
            width: rng.next(301) as u16,
            height: rng.next(101) as u16,
        },
        _ => {
            let bits = rng.next(8);
            let modifiers = Modifiers {
                shift: bits & 1 != 0,
                ctrl: bits & 2 != 0,
                alt: bits & 4 != 0,
            };
            Event::Key(KeyEvent::new(codes[rng.next(codes.len())], modifiers))
        }
    }
}

/// Every view's selection is valid and normalised; the focused view shows
/// its primary cursor's line and has a current line anchor; and every
/// current anchor agrees with walking the head's line from its start.
fn check(ed: &Editor, after: &Event) {
    let tab = ed.tab_width();
    for (id, view) in ed.views() {
        let doc = ed.document(view.doc()).unwrap();
        let sel = doc.selection(id).unwrap();
        assert_eq!(sel.validate(doc.text()), Ok(()), "after {after:?}");
        assert_normalised(sel).unwrap();
        let head = sel.primary().head();
        let current = view
            .anchor()
            .filter(|a| a.revision == doc.revision() && a.head == head && a.tab_width == tab);
        if ed.focus() == Some(id) {
            assert!(
                current.is_some(),
                "focused anchor {:?} stale after {after:?}",
                view.anchor()
            );
        }
        if let Some(a) = current {
            check_anchor(doc.text(), a, view.left(), after);
        }
    }
    if let Some(focus) = ed.focus() {
        let (view, doc) = ed.view_document(focus).unwrap();
        let line = doc
            .text()
            .line_of_offset(doc.selection(focus).unwrap().primary().head())
            .unwrap();
        let height = usize::from(view.size().1);
        if height > 0 {
            assert!(
                (view.top()..view.top() + height).contains(&line),
                "line {line} not visible from top {} height {height} after {after:?}",
                view.top()
            );
        }
    }
}

fn check_anchor(text: &Text, a: &LineAnchor, left: usize, after: &Event) {
    let line = text.line_of_offset(a.head).unwrap();
    assert_eq!(a.line, line, "anchor line after {after:?}");
    assert_eq!(
        a.line_start,
        text.line_start(line).unwrap(),
        "line start after {after:?}"
    );
    let column = display_column(text, a.head, a.tab_width).unwrap();
    assert_eq!(
        a.head_column, column,
        "head column of {a:?} after {after:?}"
    );
    assert_eq!(a.head_on_boundary, text.is_grapheme_boundary(a.head));
    if let Some((offset, at)) = a.start {
        assert!(
            text.is_grapheme_boundary(offset),
            "start {a:?} after {after:?}"
        );
        assert!(
            at <= left && offset <= a.head,
            "start {a:?} left {left} after {after:?}"
        );
        assert_eq!(
            text.line_of_offset(offset).unwrap(),
            line,
            "{a:?} after {after:?}"
        );
        assert_eq!(
            display_column(text, offset, a.tab_width).unwrap(),
            at,
            "{a:?} after {after:?}"
        );
    }
    // Wherever it came from, the start is the point walking back from
    // the head finds (when that walk succeeds).
    if let Some(expected) = super::column::line_start_at(text, a.head, column, left) {
        assert_eq!(
            a.start,
            Some(expected),
            "start of {a:?} left {left} after {after:?}"
        );
    }
    if let Some((offset, at)) = a.rear {
        assert!(
            text.is_grapheme_boundary(offset),
            "rear {a:?} after {after:?}"
        );
        assert!(
            offset >= a.line_start && offset <= a.head,
            "rear {a:?} after {after:?}"
        );
        assert_eq!(
            display_column(text, offset, a.tab_width).unwrap(),
            at,
            "{a:?} after {after:?}"
        );
    }
}

#[test]
fn random_events_never_panic() {
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200 {
        let text: String = (0..rng.next(40))
            .map(|_| PIECES[rng.next(PIECES.len())])
            .collect();
        let mut ed = Editor::new(1 + rng.next(30) as u16, 1 + rng.next(8) as u16);
        let doc = ed.open(Text::from(text.as_str()), None);
        let views = [ed.open_view(doc).unwrap(), ed.open_view(doc).unwrap()];
        let mut now: u64 = 0;
        for i in 0..100 {
            if i % 50 == 0 {
                ed.set_focus(views[rng.next(2)]).unwrap();
            }
            let event = random_event(&mut rng);
            // Mostly short steps, so typing groups; sometimes a long pause.
            now += if rng.next(10) == 0 {
                2_000_000_000
            } else {
                50_000_000
            };
            ed.handle_at(&event, Timestamp::from_nanos(now));
            check(&ed, &event);
        }
    }
}
