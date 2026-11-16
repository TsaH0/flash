//! Stage 6 editor behaviour: requests, prompt, files, register, views.

use std::path::PathBuf;

use super::*;
use crate::file::{DiskState, LoadError, Loaded, SaveError, SaveMethod, Saved};
use crate::{Range, Selection, Text};

fn key(code: KeyCode, modifiers: Modifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn plain(code: KeyCode) -> Event {
    key(code, Modifiers::NONE)
}

fn ctrl(c: char) -> Event {
    key(KeyCode::Char(c), Modifiers::CTRL)
}

fn alt(c: char) -> Event {
    key(KeyCode::Char(c), Modifiers::ALT)
}

fn typed(ed: &mut Editor, s: &str) {
    for c in s.chars() {
        ed.handle(&plain(KeyCode::Char(c)));
    }
}

fn editor_with(s: &str, path: Option<&str>) -> (Editor, DocumentId, ViewId) {
    let mut ed = Editor::new(40, 10);
    let doc = ed.open(Text::from(s), path.map(PathBuf::from));
    let view = ed.open_view(doc).unwrap();
    super::tests::insert_mode(&mut ed, view);
    (ed, doc, view)
}

fn text(ed: &Editor, doc: DocumentId) -> String {
    ed.document(doc).unwrap().text().to_string()
}

fn ok_saved(size: u64) -> Result<Saved, SaveError> {
    Ok(Saved {
        target: PathBuf::from("/abs/a.txt"),
        method: SaveMethod::Atomic,
        disk: DiskState {
            size,
            modified: None,
            hash: 7,
        },
        dir_synced: true,
    })
}

fn loaded(s: &str) -> Loaded {
    Loaded {
        text: Text::from(s),
        bom: false,
        line_ending: LineEnding::Lf,
        disk: DiskState {
            size: s.len() as u64,
            modified: None,
            hash: 1,
        },
    }
}

#[test]
fn save_requests_and_records_the_result() {
    let (mut ed, doc, _) = editor_with("abc", Some("a.txt"));
    typed(&mut ed, "x");
    assert!(ed.document(doc).unwrap().is_modified());
    let fx = ed.handle(&ctrl('s'));
    assert!(fx.requests);
    let requests = ed.take_requests();
    assert_eq!(
        requests,
        [Request::Save {
            doc,
            path: PathBuf::from("a.txt"),
            force: false
        }]
    );
    assert!(ed.take_requests().is_empty());
    let fx = ed.saved(doc, PathBuf::from("a.txt"), ok_saved(4));
    assert!(fx.needs_redraw());
    let d = ed.document(doc).unwrap();
    assert!(!d.is_modified());
    assert_eq!(d.disk().unwrap().size, 4);
    assert!(ed.message().starts_with("saved a.txt"), "{}", ed.message());
    // Typing right after the save (inside the group pause) is a change.
    typed(&mut ed, "y");
    assert!(ed.document(doc).unwrap().is_modified());
    // Undo back to the saved text: unmodified again.
    ed.handle(&alt('z'));
    assert!(!ed.document(doc).unwrap().is_modified());
    assert_eq!(text(&ed, doc), "xabc");
}

#[test]
fn in_place_saves_and_errors_are_reported() {
    let (mut ed, doc, _) = editor_with("", Some("a.txt"));
    let mut saved = ok_saved(0).unwrap();
    saved.method = SaveMethod::InPlaceHardLinks(3);
    saved.dir_synced = false;
    ed.saved(doc, PathBuf::from("a.txt"), Ok(saved));
    assert!(ed.message().contains("in place"), "{}", ed.message());
    assert!(ed.message().contains("3 hard links"));
    assert!(ed.message().contains("directory"));
    typed(&mut ed, "x");
    let err = SaveError::Io {
        phase: crate::file::Phase::CreateTemp,
        kind: std::io::ErrorKind::PermissionDenied,
        message: "denied".into(),
    };
    ed.saved(doc, PathBuf::from("a.txt"), Err(err));
    assert!(ed.message().contains("denied"));
    assert!(
        ed.document(doc).unwrap().is_modified(),
        "a failed save keeps it modified"
    );
    // A result for a closed document is ignored.
    let other = ed.open(Text::from(""), None);
    ed.close_document(other).unwrap();
    assert_eq!(
        ed.saved(other, PathBuf::from("b"), ok_saved(0)),
        Effects::default()
    );
}

#[test]
fn save_without_a_path_asks_for_one() {
    let (mut ed, doc, view) = editor_with("body", None);
    let fx = ed.handle(&ctrl('s'));
    assert!(fx.prompt && !fx.requests);
    let prompt = ed.prompt().unwrap();
    assert_eq!(prompt.kind(), PromptKind::SaveAs(doc));
    assert_eq!(prompt.label(), "save as: ");
    typed(&mut ed, "newx");
    ed.handle(&plain(KeyCode::Backspace));
    typed(&mut ed, ".txt");
    assert_eq!(ed.prompt().unwrap().input(), "new.txt");
    assert_eq!(
        text(&ed, doc),
        "body",
        "prompt keys never reach the document"
    );
    assert_eq!(ed.selection(view), Some(&Selection::point(0)));
    let fx = ed.handle(&plain(KeyCode::Enter));
    assert!(fx.prompt && fx.requests);
    assert!(ed.prompt().is_none());
    assert_eq!(
        ed.take_requests(),
        [Request::Save {
            doc,
            path: PathBuf::from("new.txt"),
            force: false
        }]
    );
    ed.saved(doc, PathBuf::from("new.txt"), ok_saved(4));
    assert_eq!(
        ed.document(doc).unwrap().path(),
        Some(std::path::Path::new("new.txt"))
    );
    // An empty answer saves nothing.
    let (mut ed, _, _) = editor_with("", None);
    ed.handle(&ctrl('s'));
    ed.handle(&plain(KeyCode::Enter));
    assert!(ed.take_requests().is_empty());
    assert!(ed.prompt().is_none());
}

#[test]
fn prompt_line_editing() {
    let (mut ed, _, _) = editor_with("", None);
    ed.handle(&ctrl('o'));
    typed(&mut ed, "abé");
    ed.handle(&plain(KeyCode::Left));
    ed.handle(&plain(KeyCode::Left));
    typed(&mut ed, "X");
    assert_eq!(ed.prompt().unwrap().input(), "aXbé");
    assert_eq!(ed.prompt().unwrap().cursor(), 2);
    ed.handle(&plain(KeyCode::Delete));
    ed.handle(&plain(KeyCode::Home));
    ed.handle(&plain(KeyCode::Delete));
    ed.handle(&plain(KeyCode::End));
    ed.handle(&plain(KeyCode::Backspace));
    ed.handle(&plain(KeyCode::Right));
    assert_eq!(ed.prompt().unwrap().input(), "X");
    // A paste drops control characters (no newline in a file name).
    ed.handle(&Event::Paste {
        text: "y\nz\u{1b}".to_owned(),
    });
    assert_eq!(ed.prompt().unwrap().input(), "Xyz");
    // Unbound keys do nothing; Esc cancels.
    ed.handle(&plain(KeyCode::F(5)));
    ed.handle(&plain(KeyCode::Esc));
    assert!(ed.prompt().is_none());
    assert!(ed.take_requests().is_empty());
}

#[test]
fn a_conflict_asks_before_overwriting() {
    let (mut ed, doc, _) = editor_with("abc", Some("a.txt"));
    typed(&mut ed, "x");
    ed.handle(&ctrl('s'));
    let path = PathBuf::from("a.txt");
    ed.take_requests();
    let fx = ed.saved(doc, path.clone(), Err(SaveError::Conflict));
    assert!(fx.prompt);
    let prompt = ed.prompt().unwrap();
    assert_eq!(prompt.kind(), PromptKind::Overwrite(doc));
    assert!(prompt.label().contains("changed on disk"));
    // Other letters are ignored; 'n' declines.
    typed(&mut ed, "q");
    assert!(ed.prompt().is_some());
    typed(&mut ed, "n");
    assert!(ed.prompt().is_none());
    assert!(ed.take_requests().is_empty());
    assert!(ed.document(doc).unwrap().is_modified());
    // Again, and 'y' forces.
    ed.saved(doc, path.clone(), Err(SaveError::Conflict));
    typed(&mut ed, "y");
    assert_eq!(
        ed.take_requests(),
        [Request::Save {
            doc,
            path,
            force: true
        }]
    );
}

#[test]
fn quit_asks_when_anything_is_modified() {
    let (mut ed, _, _) = editor_with("abc", None);
    assert!(ed.handle(&ctrl('q')).quit, "nothing modified: quits");

    let (mut ed, _, _) = editor_with("abc", None);
    typed(&mut ed, "x");
    let fx = ed.handle(&ctrl('q'));
    assert!(!fx.quit && fx.prompt);
    assert_eq!(ed.prompt().unwrap().kind(), PromptKind::Quit);
    assert!(ed.prompt().unwrap().label().contains("1 document with"));
    ed.handle(&plain(KeyCode::Esc));
    assert!(!ed.should_quit());
    ed.handle(&ctrl('q'));
    assert!(ed.handle(&ctrl('q')).quit, "Ctrl+Q again confirms");
    assert!(ed.should_quit());

    let (mut ed, _, _) = editor_with("abc", None);
    typed(&mut ed, "x");
    ed.handle(&ctrl('q'));
    assert!(ed.handle(&plain(KeyCode::Char('y'))).quit);
}

#[test]
fn open_requests_and_results() {
    let (mut ed, first, _) = editor_with("one", Some("one.txt"));
    ed.handle(&ctrl('o'));
    assert_eq!(ed.prompt().unwrap().kind(), PromptKind::Open);
    typed(&mut ed, "two.txt");
    ed.handle(&plain(KeyCode::Enter));
    let path = PathBuf::from("two.txt");
    assert_eq!(ed.take_requests(), [Request::Open { path: path.clone() }]);
    let fx = ed.opened(path.clone(), Ok(loaded("two\n")));
    assert!(fx.needs_redraw());
    let focus = ed.focus().unwrap();
    let (_, doc) = ed.view_document(focus).unwrap();
    assert_eq!(doc.text().to_string(), "two\n");
    assert_eq!(doc.path(), Some(path.as_path()));
    assert!(!doc.is_modified());
    assert!(doc.disk().is_some());
    assert_eq!(ed.documents().count(), 2);
    // A missing file opens as a new empty document.
    ed.opened(PathBuf::from("new.txt"), Err(LoadError::NotFound));
    assert!(ed.message().contains("new file"));
    let (_, doc) = ed.view_document(ed.focus().unwrap()).unwrap();
    assert!(doc.disk().is_none());
    assert_eq!(doc.text().byte_len(), 0);
    // Errors open nothing.
    ed.opened(
        PathBuf::from("bin"),
        Err(LoadError::InvalidUtf8 { offset: 3 }),
    );
    assert!(ed.message().contains("UTF-8"));
    assert_eq!(ed.documents().count(), 3);
    assert_eq!(text(&ed, first), "one");
}

#[test]
fn closing_views_and_the_last_view() {
    let (mut ed, doc, a) = editor_with("abc", None);
    let b = ed.open_view(doc).unwrap();
    super::tests::insert_mode(&mut ed, b);
    typed(&mut ed, "x");
    // Another view remains: no question.
    ed.handle(&ctrl('w'));
    assert!(ed.view(b).is_none());
    assert_eq!(ed.focus(), Some(a));
    // The last view of a modified document asks.
    ed.handle(&ctrl('w'));
    assert_eq!(ed.prompt().unwrap().kind(), PromptKind::CloseView(a));
    ed.handle(&plain(KeyCode::Char('n')));
    assert!(ed.view(a).is_some());
    ed.handle(&ctrl('w'));
    let fx = ed.handle(&plain(KeyCode::Char('y')));
    assert!(fx.quit, "closing the last view quits");
    assert!(ed.document(doc).is_none(), "its document is closed");

    // An unmodified last view of one document closes without asking, and
    // focus moves to the other document.
    let (mut ed, one, _) = editor_with("one", None);
    ed.opened(PathBuf::from("two"), Ok(loaded("two")));
    ed.handle(&ctrl('w'));
    assert!(ed.prompt().is_none());
    assert_eq!(ed.documents().count(), 1);
    assert_eq!(ed.view_document(ed.focus().unwrap()).unwrap().0.doc(), one);
}

#[test]
fn copy_cut_and_paste_with_several_cursors() {
    let (mut ed, doc, view) = editor_with("ab cd ef", None);
    let sel = Selection::new([Range::new(0, 2), Range::new(3, 5)], 0).unwrap();
    ed.set_selection(view, sel).unwrap();
    let fx = ed.handle(&ctrl('c'));
    assert!(fx.requests && fx.message);
    assert_eq!(ed.register(), ["ab", "cd"]);
    assert_eq!(
        ed.take_requests(),
        [Request::Clipboard {
            text: "ab\ncd".to_owned()
        }]
    );
    assert_eq!(ed.message(), "copied 2 selections");
    // Same number of ranges: piece i goes to range i.
    let sel = Selection::new([Range::point(8), Range::point(0)], 0).unwrap();
    ed.set_selection(view, sel).unwrap();
    ed.handle(&ctrl('v'));
    assert_eq!(text(&ed, doc), "abab cd efcd");
    // Different number: everything at each range.
    ed.set_selection(view, Selection::point(0)).unwrap();
    ed.handle(&ctrl('v'));
    assert_eq!(text(&ed, doc), "ab\ncdabab cd efcd");
    // Cut deletes and fills the register; each paste is one undo step.
    ed.set_selection(view, Selection::single(Range::new(0, 2)))
        .unwrap();
    ed.handle(&ctrl('x'));
    assert_eq!(ed.register(), ["ab"]);
    assert_eq!(text(&ed, doc), "\ncdabab cd efcd");
    ed.handle(&alt('z'));
    assert_eq!(text(&ed, doc), "ab\ncdabab cd efcd");
    ed.handle(&alt('z'));
    assert_eq!(text(&ed, doc), "abab cd efcd");
}

#[test]
fn copy_with_nothing_selected_and_paste_with_nothing_copied() {
    let (mut ed, doc, _) = editor_with("abc", None);
    ed.handle(&ctrl('v'));
    assert_eq!(ed.message(), "nothing to paste");
    ed.handle(&ctrl('c'));
    assert_eq!(ed.message(), "nothing selected");
    assert!(ed.register().is_empty());
    assert!(ed.take_requests().is_empty());
    assert_eq!(text(&ed, doc), "abc");
}

#[test]
fn ctrl_z_suspends_and_alt_keys_undo() {
    let (mut ed, doc, _) = editor_with("", None);
    typed(&mut ed, "ab");
    let fx = ed.handle(&ctrl('z'));
    assert!(fx.requests && !fx.text);
    assert_eq!(ed.take_requests(), [Request::Suspend]);
    assert_eq!(text(&ed, doc), "ab");
    ed.handle(&alt('z'));
    assert_eq!(text(&ed, doc), "");
    ed.handle(&alt('y'));
    assert_eq!(text(&ed, doc), "ab");
}

#[test]
fn page_keys_and_view_cycling() {
    let s: String = (0..100).map(|i| format!("{i}\n")).collect();
    let (mut ed, doc, a) = editor_with(&s, None);
    // 40×10 screen: 9 text rows.
    ed.handle(&plain(KeyCode::PageDown));
    let text_now = ed.document(doc).unwrap().text().clone();
    let line = |ed: &Editor, v| {
        text_now
            .line_of_offset(ed.selection(v).unwrap().primary().head())
            .unwrap()
    };
    assert_eq!(line(&ed, a), 9);
    ed.handle(&plain(KeyCode::PageUp));
    assert_eq!(line(&ed, a), 0);
    let b = ed.open_view(doc).unwrap();
    assert_eq!(ed.focus(), Some(b));
    ed.handle(&key(KeyCode::PageDown, Modifiers::CTRL));
    assert_eq!(ed.focus(), Some(a));
    ed.handle(&key(KeyCode::PageUp, Modifiers::CTRL));
    assert_eq!(ed.focus(), Some(b));
    ed.close_view(b).unwrap();
    assert_eq!(
        ed.handle(&key(KeyCode::PageDown, Modifiers::CTRL)),
        Effects::default(),
        "one view: nothing to cycle"
    );
}

#[test]
fn horizontal_scroll_follows_the_cursor() {
    let long = format!("{}\nshort", "x".repeat(100));
    let (mut ed, _, view) = editor_with(&long, None);
    // 40 columns of text.
    let fx = ed.handle(&plain(KeyCode::End));
    assert!(fx.scroll);
    assert_eq!(ed.view(view).unwrap().left(), 100 + 1 - 40);
    ed.handle(&plain(KeyCode::Home));
    assert_eq!(ed.view(view).unwrap().left(), 0);
    // Tabs count by the tab width.
    let (mut ed, _, view) = editor_with("\t\t\t\t\t\t\tx", None);
    ed.handle(&plain(KeyCode::End));
    assert_eq!(ed.view(view).unwrap().left(), 0, "29 columns fit");
    ed.set_tab_width(std::num::NonZeroU8::new(8).unwrap());
    assert_eq!(ed.tab_width().get(), 8);
    assert_eq!(ed.view(view).unwrap().left(), 57 + 1 - 40);
}

#[test]
fn paste_needs_one_piece_per_range() {
    let text = Text::from("ab");
    let sel = Selection::new([Range::point(0), Range::point(1)], 0).unwrap();
    assert_eq!(
        Transaction::paste(&text, &sel, &["x"]),
        Err(TransactionError::PieceCount {
            pieces: 1,
            ranges: 2
        })
    );
    let txn = Transaction::paste(&text, &sel, &["x", "yz"]).unwrap();
    assert_eq!(
        txn.selection(),
        Some(&Selection::new([Range::point(1), Range::point(4)], 0).unwrap())
    );
}

#[test]
fn commit_time_is_measured() {
    let (mut ed, _, _) = editor_with("", None);
    assert_eq!(ed.last_commit_ns(), 0);
    typed(&mut ed, "x");
    assert!(ed.last_commit_ns() > 0);
}
