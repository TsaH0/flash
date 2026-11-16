//! Hand-written cases for the headless editor: keys, lifecycle, scrolling,
//! and effects.

use super::*;
use crate::{
    ChangeSet, CommitError, NodeId, Range, Revision, Selection, Text, Timestamp, Transaction,
};

fn key(code: KeyCode, modifiers: Modifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn plain(code: KeyCode) -> Event {
    Event::Key(KeyEvent::plain(code))
}

fn ctrl(code: KeyCode) -> Event {
    key(code, Modifiers::CTRL)
}

/// An 80×24 editor with one document showing `s` in one focused view.
fn editor(s: &str) -> (Editor, DocumentId, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    (ed, doc, view)
}

fn text(ed: &Editor, doc: DocumentId) -> String {
    ed.document(doc).unwrap().text().to_string()
}

fn heads(ed: &Editor, view: ViewId) -> Vec<usize> {
    ed.selection(view)
        .unwrap()
        .ranges()
        .iter()
        .map(Range::head)
        .collect()
}

#[test]
fn typing_moving_and_deleting() {
    let (mut ed, doc, view) = editor("ab\ncd");
    for e in [
        plain(KeyCode::Char('x')),
        plain(KeyCode::Right),
        plain(KeyCode::Down),
        plain(KeyCode::Backspace),
    ] {
        ed.handle(&e);
    }
    assert_eq!(text(&ed, doc), "xab\nc");
    assert_eq!(heads(&ed, view), [5]);
}

#[test]
fn shift_extends() {
    let (mut ed, doc, view) = editor("abc");
    ed.handle(&key(KeyCode::Right, Modifiers::SHIFT));
    ed.handle(&key(KeyCode::Right, Modifiers::SHIFT));
    assert_eq!(
        ed.selection(view),
        Some(&Selection::single(Range::new(0, 2)))
    );
    ed.handle(&plain(KeyCode::Backspace));
    assert_eq!(text(&ed, doc), "c");
}

#[test]
fn alt_down_adds_cursor_and_typing_hits_both() {
    let (mut ed, doc, view) = editor("ab\ncd");
    ed.handle(&key(KeyCode::Down, Modifiers::ALT));
    assert_eq!(heads(&ed, view), [0, 3]);
    assert_eq!(ed.selection(view).unwrap().primary_index(), 1);
    ed.handle(&plain(KeyCode::Char('y')));
    assert_eq!(text(&ed, doc), "yab\nycd");
    ed.handle(&plain(KeyCode::Esc));
    assert_eq!(ed.selection(view), Some(&Selection::point(5)));
}

#[test]
fn paste_is_one_insert() {
    let (mut ed, doc, _) = editor("ab\ncd");
    ed.handle(&key(KeyCode::Down, Modifiers::ALT));
    ed.handle(&Event::Paste {
        text: "p\nq".to_owned(),
    });
    assert_eq!(text(&ed, doc), "p\nqab\np\nqcd");
    assert_eq!(ed.document(doc).unwrap().revision().get(), 1);
}

#[test]
fn ctrl_q_quits() {
    let (mut ed, doc, _) = editor("");
    assert!(!ed.should_quit());
    let fx = ed.handle(&ctrl(KeyCode::Char('q')));
    assert!(fx.quit);
    assert!(ed.should_quit());
    assert_eq!(text(&ed, doc), "");
}

#[test]
fn backspace_at_start_is_harmless() {
    let (mut ed, doc, _) = editor("ab");
    ed.handle(&plain(KeyCode::Backspace));
    assert_eq!(text(&ed, doc), "ab");
    assert_eq!(ed.message(), "");
}

#[test]
fn word_and_document_keys() {
    let (mut ed, _, view) = editor("foo bar\nbaz");
    ed.handle(&ctrl(KeyCode::Right));
    assert_eq!(heads(&ed, view), [4]);
    ed.handle(&ctrl(KeyCode::End));
    assert_eq!(heads(&ed, view), [11]);
    ed.handle(&plain(KeyCode::Home));
    assert_eq!(heads(&ed, view), [8]);
    ed.handle(&ctrl(KeyCode::Home));
    assert_eq!(heads(&ed, view), [0]);
}

#[test]
fn enter_uses_document_line_ending() {
    let (mut ed, doc, view) = editor("a\r\nb");
    ed.handle(&plain(KeyCode::Enter));
    assert_eq!(text(&ed, doc), "\r\na\r\nb");
    assert_eq!(heads(&ed, view), [2]);
    let (mut ed, doc, _) = editor("a\nb");
    ed.handle(&plain(KeyCode::Enter));
    assert_eq!(text(&ed, doc), "\na\nb");
}

#[test]
fn set_selection_validates() {
    let (mut ed, _, view) = editor("é");
    assert!(ed.set_selection(view, Selection::point(1)).is_err());
    assert!(ed.set_selection(view, Selection::point(2)).is_ok());
    assert_eq!(heads(&ed, view), [2]);
}

#[test]
fn stale_ids_rejected_after_reuse() {
    let (mut ed, doc, view) = editor("abc");
    ed.close_view(view).unwrap();
    let reused = ed.open_view(doc).unwrap();
    assert_ne!(view, reused);
    assert!(ed.view(view).is_none());
    assert!(ed.selection(view).is_none());
    assert_eq!(ed.close_view(view), Err(EditorError::NoSuchView(view)));
    assert_eq!(ed.set_focus(view), Err(EditorError::NoSuchView(view)));
    assert_eq!(
        ed.set_selection(view, Selection::point(0)),
        Err(EditorError::NoSuchView(view))
    );
    let txn = Transaction::new(ChangeSet::identity(3), None);
    assert_eq!(ed.commit(view, txn), Err(EditorError::NoSuchView(view)));
    assert_eq!(ed.document(doc).unwrap().views().count(), 1);

    ed.close_document(doc).unwrap();
    let doc2 = ed.open(Text::from("x"), None);
    assert_ne!(doc, doc2);
    assert!(ed.document(doc).is_none());
    assert_eq!(ed.open_view(doc), Err(EditorError::NoSuchDocument(doc)));
    assert_eq!(
        ed.close_document(doc),
        Err(EditorError::NoSuchDocument(doc))
    );
}

#[test]
fn close_focused_view_moves_focus() {
    let (mut ed, doc, first) = editor("abc");
    let second = ed.open_view(doc).unwrap();
    assert_eq!(ed.focus(), Some(second));
    ed.close_view(second).unwrap();
    assert_eq!(ed.focus(), Some(first));
    ed.close_view(first).unwrap();
    assert_eq!(ed.focus(), None);
    // The document stays open with no views.
    assert_eq!(ed.document(doc).unwrap().views().count(), 0);
}

#[test]
fn close_document_closes_its_views() {
    let (mut ed, doc, v1) = editor("abc");
    let v2 = ed.open_view(doc).unwrap();
    let other = ed.open(Text::from("xyz"), None);
    let v3 = ed.open_view(other).unwrap();
    ed.set_focus(v1).unwrap();
    ed.close_document(doc).unwrap();
    assert!(ed.view(v1).is_none() && ed.view(v2).is_none());
    assert_eq!(ed.focus(), Some(v3));
    assert_eq!(text(&ed, other), "xyz");
    assert_eq!(
        ed.document(other).unwrap().views().collect::<Vec<_>>(),
        [v3]
    );
}

#[test]
fn handle_without_focus_is_noop() {
    let (mut ed, doc, view) = editor("abc");
    ed.close_view(view).unwrap();
    assert_eq!(ed.handle(&plain(KeyCode::Char('x'))), Effects::default());
    assert_eq!(text(&ed, doc), "abc");
    // Quit and resize still work without a view.
    assert!(ed.handle(&ctrl(KeyCode::Char('q'))).quit);
    assert!(
        ed.handle(&Event::Resize {
            width: 10,
            height: 5
        })
        .resize
    );
    assert_eq!(ed.screen(), (10, 5));
}

#[test]
fn commit_from_background_view_maps_focused() {
    let (mut ed, doc, background) = editor("abc");
    let focused = ed.open_view(doc).unwrap();
    ed.set_selection(focused, Selection::point(2)).unwrap();
    let txn = Transaction::insert(
        ed.document(doc).unwrap().text(),
        ed.selection(background).unwrap(),
        "xy",
    )
    .unwrap();
    assert_eq!(ed.commit(background, txn).unwrap().get(), 1);
    assert_eq!(ed.focus(), Some(focused));
    assert_eq!(heads(&ed, focused), [4]);
    ed.handle(&plain(KeyCode::Char('z')));
    assert_eq!(text(&ed, doc), "xyabzc");
    assert_eq!(heads(&ed, background), [2]);
}

#[test]
fn scroll_follows_primary_head() {
    let s: String = (0..40).map(|i| format!("line{i}\n")).collect();
    let mut ed = Editor::new(80, 11);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    assert_eq!(ed.view(view).unwrap().size(), (80, 10));
    for _ in 0..15 {
        ed.handle(&plain(KeyCode::Down));
    }
    assert_eq!(ed.view(view).unwrap().top(), 6);
    let fx = ed.handle(&ctrl(KeyCode::Home));
    assert!(fx.scroll);
    assert_eq!(ed.view(view).unwrap().top(), 0);
    ed.handle(&ctrl(KeyCode::End));
    assert_eq!(ed.view(view).unwrap().top(), 31);
}

#[test]
fn resize_sets_view_size() {
    let (mut ed, doc, view) = editor("abc");
    let other = ed.open_view(doc).unwrap();
    let fx = ed.handle(&Event::Resize {
        width: 100,
        height: 30,
    });
    assert!(fx.resize && fx.needs_redraw());
    assert_eq!(ed.screen(), (100, 30));
    assert_eq!(ed.view(other).unwrap().size(), (100, 29));
    assert_eq!(ed.view(view).unwrap().size(), (100, 29));
    ed.handle(&Event::Resize {
        width: 0,
        height: 0,
    });
    assert_eq!(ed.view(other).unwrap().size(), (0, 0));
    ed.handle(&plain(KeyCode::Down));
}

#[test]
fn effects_report_changes() {
    let (mut ed, _, _) = editor("ab");
    let typed = ed.handle(&plain(KeyCode::Char('x')));
    assert!(typed.text && typed.selection && !typed.scroll && !typed.quit);
    let moved = ed.handle(&plain(KeyCode::Right));
    assert!(!moved.text && moved.selection);
    let nothing = ed.handle(&plain(KeyCode::F(5)));
    assert_eq!(nothing, Effects::default());
    assert!(!nothing.needs_redraw());
    // Right at the end of the text: nothing moves.
    ed.handle(&ctrl(KeyCode::End));
    assert_eq!(ed.handle(&plain(KeyCode::Right)), Effects::default());
}

#[test]
fn failed_command_sets_message() {
    let (mut ed, doc, view) = editor("ab");
    // A hand-made transaction from outside the key map that does not fit.
    let txn = Transaction::new(ChangeSet::identity(9), None);
    assert!(matches!(ed.commit(view, txn), Err(EditorError::Commit(_))));
    assert_eq!(text(&ed, doc), "ab");
    // Commits through the API report errors as values, not messages.
    assert_eq!(ed.message(), "");
}

#[test]
fn modifiers_and_key_constructors() {
    assert_eq!(Modifiers::default(), Modifiers::NONE);
    assert_eq!(
        Modifiers::SHIFT,
        Modifiers {
            shift: true,
            ctrl: false,
            alt: false
        }
    );
    assert_eq!(
        KeyEvent::plain(KeyCode::Esc),
        KeyEvent::new(KeyCode::Esc, Modifiers::NONE)
    );
}

// ---- history (Stage 5) ----

fn at(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// Types `s` into the focused view, one key every 10 ms from `start_ms`.
fn type_at(ed: &mut Editor, s: &str, start_ms: u64) {
    for (i, c) in s.chars().enumerate() {
        ed.handle_at(&plain(KeyCode::Char(c)), at(start_ms + 10 * i as u64));
    }
}

#[test]
fn undo_and_redo_keys() {
    let (mut ed, doc, view) = editor("");
    type_at(&mut ed, "ab", 0);
    let fx = ed.handle(&ctrl(KeyCode::Char('z')));
    assert!(fx.text && fx.selection, "{fx:?}");
    assert_eq!(text(&ed, doc), "");
    assert_eq!(heads(&ed, view), [0]);
    let fx = ed.handle(&ctrl(KeyCode::Char('y')));
    assert!(fx.text && fx.selection, "{fx:?}");
    assert_eq!(text(&ed, doc), "ab");
    assert_eq!(heads(&ed, view), [2]);
    ed.handle(&ctrl(KeyCode::Char('z')));
    let ctrl_shift = Modifiers {
        ctrl: true,
        shift: true,
        alt: false,
    };
    for code in [KeyCode::Char('z'), KeyCode::Char('Z')] {
        ed.handle(&key(code, ctrl_shift));
        assert_eq!(text(&ed, doc), "ab", "{code:?}");
        ed.handle(&ctrl(KeyCode::Char('z')));
    }
    assert_eq!(text(&ed, doc), "");
}

#[test]
fn undo_at_the_ends_reports() {
    let (mut ed, doc, _) = editor("abc");
    let fx = ed.handle(&ctrl(KeyCode::Char('z')));
    assert!(!fx.text && !fx.selection && fx.message, "{fx:?}");
    assert_eq!(ed.message(), "already at the oldest change");
    let fx = ed.handle(&ctrl(KeyCode::Char('y')));
    assert!(!fx.text && fx.message, "{fx:?}");
    assert_eq!(ed.message(), "already at the newest change");
    ed.handle(&plain(KeyCode::Char('x')));
    assert_eq!(ed.message(), "");
    assert_eq!(text(&ed, doc), "xabc");
}

#[test]
fn a_typed_word_is_one_undo_step() {
    let (mut ed, doc, _) = editor("");
    type_at(&mut ed, "héllo wörld", 0);
    ed.handle(&ctrl(KeyCode::Char('z')));
    assert_eq!(text(&ed, doc), "");
}

#[test]
fn every_motion_key_ends_the_group_even_without_moving() {
    for boundary in [
        plain(KeyCode::Right), // at the end: does not move
        plain(KeyCode::End),
        plain(KeyCode::Esc),
        ctrl(KeyCode::End),
        key(KeyCode::Down, Modifiers::ALT), // one line: no cursor added
    ] {
        let (mut ed, doc, _) = editor("");
        type_at(&mut ed, "ab", 0);
        ed.handle_at(&boundary, at(30));
        type_at(&mut ed, "cd", 40);
        ed.handle(&ctrl(KeyCode::Char('z')));
        assert_eq!(text(&ed, doc), "ab", "{boundary:?}");
    }
}

#[test]
fn a_pause_ends_the_group() {
    let (mut ed, doc, _) = editor("");
    type_at(&mut ed, "ab", 0);
    type_at(&mut ed, "cd", 5_000);
    ed.handle(&ctrl(KeyCode::Char('z')));
    assert_eq!(text(&ed, doc), "ab");
}

#[test]
fn handle_without_a_time_keeps_the_last_one() {
    let (mut ed, doc, _) = editor("");
    ed.handle_at(&plain(KeyCode::Char('a')), at(5_000));
    assert_eq!(ed.now(), at(5_000));
    ed.handle(&plain(KeyCode::Char('b')));
    assert_eq!(ed.now(), at(5_000));
    ed.handle(&ctrl(KeyCode::Char('z')));
    assert_eq!(text(&ed, doc), "");
}

#[test]
fn a_paste_is_its_own_undo_step() {
    let (mut ed, doc, _) = editor("");
    type_at(&mut ed, "a", 0);
    ed.handle_at(
        &Event::Paste {
            text: "XY".to_owned(),
        },
        at(10),
    );
    type_at(&mut ed, "b", 20);
    for expected in ["aXY", "a", ""] {
        ed.handle(&ctrl(KeyCode::Char('z')));
        assert_eq!(text(&ed, doc), expected);
    }
}

#[test]
fn editing_keys_group_together() {
    let (mut ed, doc, _) = editor("");
    type_at(&mut ed, "ab", 0);
    for (i, code) in [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Backspace,
        KeyCode::Delete,
    ]
    .into_iter()
    .enumerate()
    {
        ed.handle_at(&plain(code), at(30 + i as u64));
    }
    assert_eq!(text(&ed, doc), "ab");
    let node = ed.document(doc).unwrap().history().len();
    assert_eq!(node, 2, "one node");
}

#[test]
fn history_api_on_the_editor() {
    let (mut ed, doc, view) = editor("");
    let r1 = ed
        .commit(
            view,
            Transaction::new(ChangeSet::from_edits(0, [(0..0, "a")]).unwrap(), None),
        )
        .unwrap();
    ed.checkpoint(view).unwrap();
    let txn = Transaction::insert(
        ed.document(doc).unwrap().text(),
        ed.selection(view).unwrap(),
        "b",
    )
    .unwrap();
    ed.commit(view, txn).unwrap();
    assert_eq!(
        ed.document(doc).unwrap().history().len(),
        3,
        "commits are separate steps"
    );
    assert_eq!(ed.undo(view), Ok(Some(Revision(3))));
    assert_eq!(text(&ed, doc), "a");
    assert_eq!(ed.redo(view), Ok(Some(Revision(4))));
    assert_eq!(ed.goto(view, NodeId::ROOT), Ok(Some(Revision(5))));
    assert_eq!(text(&ed, doc), "");
    assert_eq!(ed.goto(view, NodeId::ROOT), Ok(None));
    assert!(r1 < Revision(5));
    assert_eq!(
        ed.goto(view, NodeId(9)),
        Err(EditorError::Commit(CommitError::NoSuchNode(NodeId(9))))
    );
    ed.close_view(view).unwrap();
    for result in [ed.undo(view), ed.redo(view), ed.goto(view, NodeId::ROOT)] {
        assert_eq!(result, Err(EditorError::NoSuchView(view)));
    }
    assert_eq!(ed.checkpoint(view), Err(EditorError::NoSuchView(view)));
}

#[test]
fn undo_scrolls_the_focused_view_to_the_restored_cursor() {
    let (mut ed, doc, view) = editor(&"line\n".repeat(100));
    ed.handle(&Event::Resize {
        width: 20,
        height: 6,
    });
    ed.handle(&plain(KeyCode::Char('x')));
    ed.handle(&ctrl(KeyCode::End));
    assert!(ed.view(view).unwrap().top() > 0);
    ed.handle(&ctrl(KeyCode::Char('z')));
    assert_eq!(heads(&ed, view), [0]);
    assert_eq!(ed.view(view).unwrap().top(), 0);
    assert_eq!(text(&ed, doc), "line\n".repeat(100));
}

#[test]
fn undo_from_another_view_maps_the_typing_view() {
    let (mut ed, doc, a) = editor("hello");
    let b = ed.open_view(doc).unwrap();
    ed.set_focus(a).unwrap();
    ed.set_selection(a, Selection::point(5)).unwrap();
    type_at(&mut ed, " world", 0);
    ed.set_focus(b).unwrap();
    ed.handle(&ctrl(KeyCode::Char('z')));
    assert_eq!(text(&ed, doc), "hello");
    // b gets the stored selection; a is mapped back to the end.
    assert_eq!(heads(&ed, b), [5]);
    assert_eq!(heads(&ed, a), [5]);
}
