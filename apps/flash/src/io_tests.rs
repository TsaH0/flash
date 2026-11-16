use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use flash_core::{Event, KeyCode, KeyEvent, Modifiers};

use super::*;

const WAIT: Duration = Duration::from_secs(10);

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), Modifiers::NONE))
}

/// A worker whose results arrive on a channel the test drains itself, so a
/// job stays "in flight" until the test completes it.
fn worker() -> (Io, mpsc::Receiver<Done>) {
    let (tx, rx) = mpsc::channel();
    let io = Io::spawn(move |d| tx.send(d).is_ok()).unwrap();
    (io, rx)
}

fn editor_on(path: &std::path::Path) -> (Editor, DocumentId) {
    let mut ed = Editor::new(40, 10);
    ed.opened(path.to_path_buf(), file::load(path));
    let doc = ed.view(ed.focus().unwrap()).unwrap().doc();
    ed.handle(&key('i'));
    (ed, doc)
}

fn save(ed: &mut Editor, io: &mut Io, doc: DocumentId, path: &std::path::Path) {
    io.submit(
        ed,
        Request::Save {
            doc,
            path: path.to_path_buf(),
            force: false,
        },
    )
    .unwrap();
}

#[test]
fn a_save_runs_on_the_worker_and_reports_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "abc").unwrap();
    let (mut ed, doc) = editor_on(&path);
    let (mut io, rx) = worker();
    ed.handle(&key('x'));
    save(&mut ed, &mut io, doc, &path);
    assert!(io.busy());
    assert!(ed.message().starts_with("saving"), "{}", ed.message());
    // Typed while the save runs: not part of it.
    ed.handle(&key('y'));
    let done = rx.recv_timeout(WAIT).unwrap();
    io.complete(&mut ed, done);
    assert!(!io.busy());
    assert_eq!(fs::read_to_string(&path).unwrap(), "xabc");
    assert!(ed.message().starts_with("saved"), "{}", ed.message());
    assert!(
        ed.document(doc).unwrap().is_modified(),
        "the y is not saved"
    );
    assert!(io.finish(WAIT));
}

#[test]
fn a_second_save_waits_for_the_first_without_a_false_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "abc").unwrap();
    let (mut ed, doc) = editor_on(&path);
    let (mut io, rx) = worker();
    ed.handle(&key('1'));
    save(&mut ed, &mut io, doc, &path);
    ed.handle(&key('2'));
    save(&mut ed, &mut io, doc, &path);
    // The second waits: one result only, until the first completes.
    let first = rx.recv_timeout(WAIT).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    io.complete(&mut ed, first);
    assert!(io.busy(), "the second save started");
    let second = rx.recv_timeout(WAIT).unwrap();
    io.complete(&mut ed, second);
    assert!(
        ed.prompt().is_none(),
        "no conflict question: {}",
        ed.message()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "12abc");
    assert!(!ed.document(doc).unwrap().is_modified());
    assert!(!io.busy());
}

#[test]
fn an_open_runs_on_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "").unwrap();
    fs::write(&b, "bee").unwrap();
    let (mut ed, _) = editor_on(&a);
    let (mut io, rx) = worker();
    io.submit(&mut ed, Request::Open { path: b.clone() })
        .unwrap();
    let done = rx.recv_timeout(WAIT).unwrap();
    io.complete(&mut ed, done);
    let doc = ed.view(ed.focus().unwrap()).unwrap().doc();
    assert_eq!(ed.document(doc).unwrap().text().to_string(), "bee");
    // Other requests are given back.
    assert_eq!(
        io.submit(&mut ed, Request::Suspend).unwrap_err(),
        Request::Suspend
    );
}

#[test]
fn finish_waits_for_a_save_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let (mut ed, doc) = editor_on(&path);
    // A few megabytes, so the write is still running when finish starts.
    let big = "0123456789abcdef\n".repeat(1 << 18);
    ed.handle(&Event::Paste { text: big.clone() });
    let (mut io, _rx) = worker();
    save(&mut ed, &mut io, doc, &path);
    assert!(io.finish(WAIT));
    assert_eq!(fs::read(&path).unwrap().len(), big.len());
    // A worker whose receiver is gone still finishes.
    let (io, rx) = worker();
    drop(rx);
    assert!(io.finish(WAIT));
}
