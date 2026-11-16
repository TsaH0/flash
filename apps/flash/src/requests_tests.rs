use std::fs;
use std::path::PathBuf;

use flash_core::{Event, KeyCode, KeyEvent, Modifiers, Text};

use super::*;

fn key(c: char, modifiers: Modifiers) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), modifiers))
}

/// Runs every pending request; returns what was written to the terminal.
fn run_requests(ed: &mut Editor) -> Vec<u8> {
    let mut out = Vec::new();
    for r in ed.take_requests() {
        execute(ed, r, &mut out, &Settings::default()).unwrap();
    }
    out
}

#[test]
fn save_open_and_conflict_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "\u{feff}one\r\n").unwrap();
    let mut ed = Editor::new(40, 10);
    ed.opened(path.clone(), file::load(&path));
    let view = ed.focus().unwrap();
    let doc = ed.view(view).unwrap().doc();
    assert!(ed.document(doc).unwrap().bom());
    // Views start in normal mode: `i` to insert.
    ed.handle(&key('i', Modifiers::NONE));
    ed.handle(&key('x', Modifiers::NONE));
    ed.handle(&key('s', Modifiers::CTRL));
    run_requests(&mut ed);
    assert_eq!(fs::read(&path).unwrap(), "\u{feff}xone\r\n".as_bytes());
    assert!(!ed.document(doc).unwrap().is_modified());
    // Someone else edits; our save asks; 'y' overwrites.
    fs::write(&path, "theirs").unwrap();
    ed.handle(&key('y', Modifiers::NONE));
    ed.handle(&key('s', Modifiers::CTRL));
    run_requests(&mut ed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
    assert!(ed.prompt().is_some(), "{}", ed.message());
    ed.handle(&key('y', Modifiers::NONE));
    run_requests(&mut ed);
    assert_eq!(fs::read(&path).unwrap(), "\u{feff}xyone\r\n".as_bytes());
    assert!(ed.message().starts_with("saved"), "{}", ed.message());
}

#[test]
fn save_as_to_an_existing_file_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let taken = dir.path().join("taken.txt");
    fs::write(&taken, "keep").unwrap();
    let mut ed = Editor::new(40, 10);
    let doc = ed.open(Text::from("new"), None);
    ed.open_view(doc).unwrap();
    ed.handle(&key('s', Modifiers::CTRL));
    ed.handle(&Event::Paste {
        text: taken.display().to_string(),
    });
    ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Enter)));
    run_requests(&mut ed);
    assert_eq!(fs::read_to_string(&taken).unwrap(), "keep");
    assert!(ed.prompt().unwrap().label().contains("changed on disk"));
}

#[test]
fn open_errors_are_messages() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::write(&bin, b"\xff").unwrap();
    let mut ed = Editor::new(40, 10);
    let fx = execute(
        &mut ed,
        Request::Open { path: bin },
        &mut Vec::new(),
        &Settings::default(),
    )
    .unwrap();
    assert!(matches!(fx, Outcome::Done(e) if e.message));
    assert!(ed.message().contains("UTF-8"));
    assert_eq!(ed.documents().count(), 0);
    execute(
        &mut ed,
        Request::Open {
            path: PathBuf::from(dir.path()),
        },
        &mut Vec::new(),
        &Settings::default(),
    )
    .unwrap();
    assert!(ed.message().contains("not a regular file"));
}

#[test]
fn clipboard_writes_osc52_or_reports_size() {
    let mut ed = Editor::new(40, 10);
    let mut out = Vec::new();
    execute(
        &mut ed,
        Request::Clipboard { text: "hi".into() },
        &mut out,
        &Settings::default(),
    )
    .unwrap();
    assert_eq!(out, b"\x1b]52;c;aGk=\x07");
    let big = "x".repeat(osc52::MAX_TEXT + 1);
    let mut out = Vec::new();
    execute(
        &mut ed,
        Request::Clipboard { text: big },
        &mut out,
        &Settings::default(),
    )
    .unwrap();
    assert!(out.is_empty());
    assert!(ed.message().contains("too large"));
    assert_eq!(
        execute(&mut ed, Request::Suspend, &mut out, &Settings::default()).unwrap(),
        Outcome::Suspend
    );
}
