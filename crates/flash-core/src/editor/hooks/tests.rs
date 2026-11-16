//! Hook subscriptions, batching, and bounded re-entrancy.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::{Context, Event, KeyCode, KeyEvent, Selection, Text, Transaction};

type Log = Rc<RefCell<Vec<Vec<EditorEvent>>>>;

fn editor(s: &str) -> (Editor, DocumentId, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    crate::editor::tests::insert_mode(&mut ed, view);
    (ed, doc, view)
}

fn record(ed: &mut Editor, kinds: EventKinds, owner: Owner) -> (Log, HookId) {
    let log: Log = Rc::default();
    let l = Rc::clone(&log);
    let id = ed.subscribe(kinds, owner, move |_, batch| {
        l.borrow_mut().push(batch.to_vec());
        Effects::default()
    });
    (log, id)
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::plain(code))
}

#[test]
fn a_key_delivers_one_batch() {
    let (mut ed, doc, view) = editor("abc");
    let (log, _) = record(&mut ed, EventKinds::ALL, Owner(3));
    let r0 = ed.document(doc).unwrap().revision();
    ed.handle(&key(KeyCode::Char('x')));
    let r1 = ed.document(doc).unwrap().revision();
    assert_eq!(
        log.take(),
        [vec![
            EditorEvent::DocumentChanged {
                doc,
                from: r0,
                to: r1
            },
            EditorEvent::SelectionChanged { view },
        ]]
    );
    ed.handle(&key(KeyCode::Right));
    assert_eq!(log.take(), [vec![EditorEvent::SelectionChanged { view }]]);
    // A motion that moves nothing changes nothing and delivers nothing.
    ed.handle(&key(KeyCode::End));
    log.take();
    ed.handle(&key(KeyCode::End));
    assert!(log.take().is_empty());
    // Undo reports the revision move.
    ed.handle(&Event::Key(KeyEvent::new(
        KeyCode::Char('z'),
        crate::Modifiers::ALT,
    )));
    let r2 = ed.document(doc).unwrap().revision();
    assert_eq!(
        log.take(),
        [vec![
            EditorEvent::DocumentChanged {
                doc,
                from: r1,
                to: r2
            },
            EditorEvent::SelectionChanged { view },
        ]]
    );
}

#[test]
fn subscribers_get_only_batches_of_their_kinds() {
    let (mut ed, doc, _) = editor("abc");
    let (views, _) = record(&mut ed, EventKinds::VIEW, Owner(3));
    let (docs, _) = record(&mut ed, EventKinds::DOCUMENT, Owner(3));
    ed.handle(&key(KeyCode::Right));
    assert!(views.take().is_empty() && docs.take().is_empty());
    ed.handle(&key(KeyCode::Char('x')));
    assert!(views.take().is_empty());
    assert_eq!(docs.take().len(), 1, "the whole batch, once");
    let second = ed.open_view(doc).unwrap();
    ed.close_view(second).unwrap();
    assert!(views.take().is_empty(), "direct calls wait for a flush");
    ed.flush_events();
    // The whole batch, minus kinds nobody subscribes to: opening in normal
    // mode widened the new cursor, but no one wants selection events.
    assert_eq!(
        views.take(),
        [vec![
            EditorEvent::ViewOpened { view: second },
            EditorEvent::ViewClosed { view: second },
        ]]
    );
    let (_sels, _) = record(&mut ed, EventKinds::SELECTION, Owner(3));
    let third = ed.open_view(doc).unwrap();
    ed.flush_events();
    assert_eq!(
        views.take(),
        [vec![
            EditorEvent::ViewOpened { view: third },
            EditorEvent::SelectionChanged { view: third },
        ]]
    );
}

#[test]
fn nested_commands_deliver_once_at_the_outermost_call() {
    let (mut ed, _, view) = editor("abc");
    let (log, _) = record(&mut ed, EventKinds::ALL, Owner(3));
    ed.register_command("twice", Owner(3), "", |ed, cx| {
        let a = ed.run_named("move-right", cx);
        a.or(ed.run_named(
            "insert-text",
            &Context {
                arg: Some("!"),
                ..*cx
            },
        ))
    })
    .unwrap();
    ed.run_named("twice", &Context::default());
    let batches = log.take();
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].len(), 3, "{batches:?}");
    assert_eq!(batches[0][0], EditorEvent::SelectionChanged { view });
}

#[test]
fn unsubscribe_and_owner_removal() {
    let (mut ed, _, _) = editor("abc");
    let (a, id_a) = record(&mut ed, EventKinds::ALL, Owner(3));
    let (b, _) = record(&mut ed, EventKinds::ALL, Owner(4));
    assert!(ed.unsubscribe(id_a));
    assert!(!ed.unsubscribe(id_a));
    ed.handle(&key(KeyCode::Right));
    assert!(a.take().is_empty());
    assert_eq!(b.take().len(), 1);
    ed.remove_owner(Owner(4));
    ed.handle(&key(KeyCode::Left));
    assert!(b.take().is_empty());
}

#[test]
fn hooks_that_keep_causing_events_stop_after_the_round_limit() {
    let (mut ed, _, view) = editor("abcdefghijklmnopqrstuvwxyz");
    let calls = Rc::new(RefCell::new(0));
    let c = Rc::clone(&calls);
    let looping = ed.subscribe(EventKinds::SELECTION, Owner(3), move |ed, _| {
        *c.borrow_mut() += 1;
        ed.run_named("move-right", &Context::default())
    });
    let fx = ed.handle(&key(KeyCode::Right));
    assert_eq!(*calls.borrow(), MAX_ROUNDS);
    assert!(
        fx.message && ed.message().contains("dropped"),
        "{}",
        ed.message()
    );
    assert!(fx.selection, "what hooks change is reported");
    assert_eq!(ed.selection(view).unwrap().primary().head(), 1 + MAX_ROUNDS);
    // The queue is empty again: the next key delivers normally.
    *calls.borrow_mut() = 0;
    ed.unsubscribe(looping);
    ed.handle(&key(KeyCode::Right));
    assert_eq!(*calls.borrow(), 0);
}

#[test]
fn direct_api_calls_queue_until_flushed() {
    let (mut ed, doc, view) = editor("abc");
    let (log, _) = record(&mut ed, EventKinds::ALL, Owner(3));
    let txn = {
        let d = ed.document(doc).unwrap();
        Transaction::insert(d.text(), d.selection(view).unwrap(), "x").unwrap()
    };
    ed.commit(view, txn).unwrap();
    ed.set_selection(view, Selection::point(0)).unwrap();
    assert!(log.borrow().is_empty());
    ed.flush_events();
    let batches = log.take();
    assert_eq!(batches.len(), 1);
    assert!(matches!(batches[0][0], EditorEvent::DocumentChanged { .. }));
    assert_eq!(batches[0].len(), 3);
    // With no subscribers nothing is queued at all.
    let (mut quiet, _, _) = editor("abc");
    quiet.handle(&key(KeyCode::Char('x')));
    assert!(quiet.hooks.queue.is_empty());
}

/// Only kinds someone subscribes to are queued: the built-in view
/// subscriber must not make every edit queue document and selection events.
#[test]
fn only_wanted_kinds_are_queued() {
    let (mut ed, _, view) = editor("abc");
    let edit = |ed: &mut Editor| {
        let sel = ed.selection(view).unwrap().clone();
        let doc = ed.view(view).unwrap().doc();
        let txn = Transaction::insert(ed.document(doc).unwrap().text(), &sel, "x").unwrap();
        ed.commit(view, txn).unwrap();
    };
    edit(&mut ed);
    assert!(ed.hooks.queue.is_empty(), "{:?}", ed.hooks.queue);
    let (log, id) = record(&mut ed, EventKinds::SELECTION, Owner(3));
    edit(&mut ed);
    assert_eq!(ed.hooks.queue, [EditorEvent::SelectionChanged { view }]);
    ed.flush_events();
    assert_eq!(log.take().len(), 1);
    assert!(ed.unsubscribe(id));
    edit(&mut ed);
    assert!(ed.hooks.queue.is_empty());
    // Removing an owner forgets its kinds too.
    let _ = record(&mut ed, EventKinds::DOCUMENT, Owner(4));
    ed.remove_owner(Owner(4));
    edit(&mut ed);
    assert!(ed.hooks.queue.is_empty());
}
