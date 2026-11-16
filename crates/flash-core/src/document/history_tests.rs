//! Hand-written cases for history: undo, redo, branches, jumps, grouping.

use std::time::Duration;

use slotmap::SlotMap;

use super::*;
use crate::changeset::Op;
use crate::{Range, Text};

fn view_ids(n: usize) -> Vec<ViewId> {
    let mut ids: SlotMap<ViewId, ()> = SlotMap::with_key();
    (0..n).map(|_| ids.insert(())).collect()
}

fn doc_with_views(s: &str, n: usize) -> (Document, Vec<ViewId>) {
    let mut doc = Document::new(Text::from(s), None);
    let views = view_ids(n);
    for &v in &views {
        doc.add_view(v);
    }
    (doc, views)
}

fn ms(n: u64) -> Timestamp {
    Timestamp::from_nanos(n * 1_000_000)
}

fn text(doc: &Document) -> String {
    doc.text().to_string()
}

fn sel(doc: &Document, v: ViewId) -> Selection {
    doc.selection(v).unwrap().clone()
}

/// Inserts `s` at `v`'s selection, recorded as `record`.
fn insert(doc: &mut Document, v: ViewId, s: &str, record: Record) {
    let txn = Transaction::insert(doc.text(), doc.selection(v).unwrap(), s).unwrap();
    doc.commit_with(v, txn, record).unwrap();
}

fn type_str(doc: &mut Document, v: ViewId, s: &str, start: Timestamp) {
    for (i, c) in s.chars().enumerate() {
        let mut buf = [0; 4];
        let time = start.saturating_add(Duration::from_millis(10 * i as u64));
        insert(doc, v, c.encode_utf8(&mut buf), Record::typing(time));
    }
}

/// Replaces `v`'s selection with `s`, as its own step.
fn replace(doc: &mut Document, v: ViewId, range: Range, s: &str) {
    doc.set_selection(v, Selection::single(range)).unwrap();
    let txn = Transaction::replace(doc.text(), doc.selection(v).unwrap(), s).unwrap();
    doc.commit(v, txn).unwrap();
}

#[test]
fn fresh_history_is_a_root() {
    let (doc, v) = doc_with_views("abc", 1);
    let h = doc.history();
    assert_eq!(h.head(), NodeId::ROOT);
    assert_eq!(h.len(), 1);
    assert!(!h.is_empty());
    let root = h.node(NodeId::ROOT).unwrap();
    assert_eq!(root.parent(), None);
    assert_eq!(root.depth(), 0);
    assert!(root.forward().is_identity());
    assert_eq!(root.forward().len_before(), 3);
    assert_eq!(h.node(NodeId(1)).map(|_| ()), None);
    let mut doc = doc;
    assert_eq!(doc.undo(v[0]), Ok(None));
    assert_eq!(doc.redo(v[0]), Ok(None));
    assert_eq!(doc.revision(), Revision::ZERO);
}

#[test]
fn undo_and_redo_one_edit() {
    let (mut doc, v) = doc_with_views("abc", 1);
    doc.set_selection(v[0], Selection::point(1)).unwrap();
    insert(&mut doc, v[0], "XY", Record::step(ms(5)));
    assert_eq!(text(&doc), "aXYbc");
    assert_eq!(doc.history().head(), NodeId(1));
    let node = doc.history().node(NodeId(1)).unwrap();
    assert_eq!(node.parent(), Some(NodeId::ROOT));
    assert_eq!(node.before(), &Selection::point(1));
    assert_eq!(node.after(), &Selection::point(3));
    assert_eq!(node.time(), ms(5));

    // Move away, then undo: the stored selection comes back.
    doc.set_selection(v[0], Selection::point(5)).unwrap();
    assert_eq!(doc.undo(v[0]), Ok(Some(Revision(2))));
    assert_eq!(text(&doc), "abc");
    assert_eq!(sel(&doc, v[0]), Selection::point(1));
    assert_eq!(doc.history().head(), NodeId::ROOT);
    assert_eq!(doc.log().len(), 2, "undo enters the revision log");
    assert_eq!(doc.undo(v[0]), Ok(None));

    assert_eq!(doc.redo(v[0]), Ok(Some(Revision(3))));
    assert_eq!(text(&doc), "aXYbc");
    assert_eq!(sel(&doc, v[0]), Selection::point(3));
    assert_eq!(doc.redo(v[0]), Ok(None));
    assert_eq!(doc.history().len(), 2, "undo and redo record no nodes");
    // The log maps a revision-0 position through edit, undo, and redo.
    let mut p = [2];
    doc.map_from(Revision::ZERO, &mut p, Assoc::After).unwrap();
    assert_eq!(p, [4]);
}

#[test]
fn undo_all_then_redo_all_is_exact() {
    let (mut doc, v) = doc_with_views("héllo wörld\r\n", 1);
    let mut snapshots = vec![text(&doc)];
    // Unicode replacements, deletions, multi-cursor inserts.
    replace(
        &mut doc,
        v[0],
        Range::new(1, 3),
        "\u{1F468}\u{200D}\u{1F469}",
    );
    snapshots.push(text(&doc));
    replace(&mut doc, v[0], Range::new(0, 1), "");
    snapshots.push(text(&doc));
    // The family emoji is bytes 0..11 (boundaries 0, 4, 7, 11).
    let multi = Selection::new([Range::point(0), Range::point(7), Range::point(12)], 1).unwrap();
    doc.set_selection(v[0], multi).unwrap();
    insert(&mut doc, v[0], "e\u{301}", Record::step(ms(1)));
    snapshots.push(text(&doc));
    let len = doc.text().byte_len();
    replace(&mut doc, v[0], Range::new(len, 0), "老虎");
    snapshots.push(text(&doc));
    assert_eq!(doc.history().len(), snapshots.len());

    for i in (0..snapshots.len() - 1).rev() {
        doc.undo(v[0]).unwrap().unwrap();
        assert_eq!(text(&doc), snapshots[i]);
        assert_eq!(sel(&doc, v[0]).validate(doc.text()), Ok(()));
    }
    assert_eq!(doc.undo(v[0]), Ok(None));
    for snapshot in &snapshots[1..] {
        doc.redo(v[0]).unwrap().unwrap();
        assert_eq!(&text(&doc), snapshot);
    }
    assert_eq!(doc.redo(v[0]), Ok(None));
    assert_eq!(doc.revision().get(), 4 + 4 + 4);
}

#[test]
fn undo_restores_multi_cursor_selection() {
    let (mut doc, v) = doc_with_views("one two three", 1);
    let cursors = Selection::new([Range::new(0, 3), Range::new(4, 7)], 1).unwrap();
    doc.set_selection(v[0], cursors.clone()).unwrap();
    let txn = Transaction::replace(doc.text(), &cursors, "X").unwrap();
    doc.commit(v[0], txn).unwrap();
    assert_eq!(text(&doc), "X X three");
    let after = sel(&doc, v[0]);
    doc.undo(v[0]).unwrap();
    assert_eq!(text(&doc), "one two three");
    assert_eq!(sel(&doc, v[0]), cursors);
    doc.redo(v[0]).unwrap();
    assert_eq!(sel(&doc, v[0]), after);
}

#[test]
fn undo_maps_other_views_through_the_inverse() {
    let (mut doc, v) = doc_with_views("abcdef", 2);
    doc.set_selection(v[0], Selection::point(2)).unwrap();
    insert(&mut doc, v[0], "XYZ", Record::step(ms(0)));
    // v1 sits after the insertion and around part of it.
    let watcher = Selection::new([Range::new(3, 4), Range::point(8)], 0).unwrap();
    doc.set_selection(v[1], watcher.clone()).unwrap();
    let inverse = doc.history().node(NodeId(1)).unwrap().inverse().clone();
    // Undo invoked from v1: v1 gets the stored selection, v0 is mapped.
    let v0_before = sel(&doc, v[0]);
    doc.undo(v[1]).unwrap();
    assert_eq!(text(&doc), "abcdef");
    assert_eq!(sel(&doc, v[1]), Selection::point(2));
    assert_eq!(sel(&doc, v[0]), v0_before.map(&inverse).unwrap());
    // Redo from v0: v0 gets the stored after-selection, v1 is mapped.
    let v1_before = sel(&doc, v[1]);
    let forward = doc.history().node(NodeId(1)).unwrap().forward().clone();
    doc.redo(v[0]).unwrap();
    assert_eq!(sel(&doc, v[0]), Selection::point(5));
    assert_eq!(sel(&doc, v[1]), v1_before.map(&forward).unwrap());
}

#[test]
fn edit_after_undo_branches_and_redo_follows_last_visited() {
    let (mut doc, v) = doc_with_views("", 1);
    insert(&mut doc, v[0], "a", Record::step(ms(1))); // node 1: "a"
    insert(&mut doc, v[0], "b", Record::step(ms(2))); // node 2: "ab"
    doc.undo(v[0]).unwrap(); // at 1
    insert(&mut doc, v[0], "c", Record::step(ms(3))); // node 3: "ac", child of 1
    assert_eq!(text(&doc), "ac");
    let h = doc.history();
    assert_eq!(h.len(), 4);
    assert_eq!(h.node(NodeId(3)).unwrap().parent(), Some(NodeId(1)));
    assert_eq!(
        h.children(NodeId(1)).collect::<Vec<_>>(),
        [NodeId(2), NodeId(3)]
    );
    assert_eq!(h.node(NodeId(1)).unwrap().last_child(), Some(NodeId(3)));
    assert_eq!(h.node(NodeId(3)).unwrap().depth(), 2);

    // Redo after undo goes back down the newest branch.
    doc.undo(v[0]).unwrap();
    doc.redo(v[0]).unwrap();
    assert_eq!(text(&doc), "ac");

    // Jump to the old branch; then undo + redo returns to it.
    let r = doc.revision();
    assert_eq!(
        doc.goto(v[0], NodeId(2)),
        Ok(Some(r.next())),
        "one revision"
    );
    assert_eq!(text(&doc), "ab");
    assert_eq!(sel(&doc, v[0]), Selection::point(2));
    assert_eq!(doc.history().head(), NodeId(2));
    assert_eq!(
        doc.history().node(NodeId(1)).unwrap().last_child(),
        Some(NodeId(2))
    );
    doc.undo(v[0]).unwrap();
    doc.redo(v[0]).unwrap();
    assert_eq!(text(&doc), "ab");

    // Jump to an ancestor: the selection is the one undo would give.
    doc.goto(v[0], NodeId::ROOT).unwrap();
    assert_eq!(text(&doc), "");
    assert_eq!(sel(&doc, v[0]), Selection::point(0));
    // Redo from the root follows the branch just left.
    doc.redo(v[0]).unwrap();
    doc.redo(v[0]).unwrap();
    assert_eq!(text(&doc), "ab");
    assert_eq!(doc.goto(v[0], NodeId(2)), Ok(None), "already there");
}

#[test]
fn goto_an_ancestor_restores_the_selection_undo_would() {
    let (mut doc, v) = doc_with_views("", 1);
    insert(&mut doc, v[0], "a", Record::step(ms(1))); // node 1, after = 1
    doc.set_selection(v[0], Selection::point(0)).unwrap();
    insert(&mut doc, v[0], "b", Record::step(ms(2))); // node 2, before = 0
    insert(&mut doc, v[0], "c", Record::step(ms(3))); // node 3
    doc.goto(v[0], NodeId(1)).unwrap();
    assert_eq!(text(&doc), "a");
    assert_eq!(sel(&doc, v[0]), Selection::point(0), "node 2's before");
    // Down again: the target's after.
    doc.goto(v[0], NodeId(3)).unwrap();
    assert_eq!(text(&doc), "bca");
    assert_eq!(sel(&doc, v[0]), Selection::point(2));
}

#[test]
fn goto_rejects_unknown_nodes_and_views() {
    let (mut doc, v) = doc_with_views("abc", 1);
    insert(&mut doc, v[0], "x", Record::step(ms(0)));
    let before = doc.clone();
    assert_eq!(
        doc.goto(v[0], NodeId(7)),
        Err(CommitError::NoSuchNode(NodeId(7)))
    );
    let stranger = view_ids(2)[1];
    assert_eq!(
        doc.goto(stranger, NodeId::ROOT),
        Err(CommitError::NoSuchView(stranger))
    );
    assert_eq!(doc.undo(stranger), Err(CommitError::NoSuchView(stranger)));
    assert_eq!(doc.redo(stranger), Err(CommitError::NoSuchView(stranger)));
    assert_eq!(doc.text(), before.text());
    assert_eq!(doc.revision(), before.revision());
    assert_eq!(doc.history().head(), before.history().head());
}

#[test]
fn typing_groups_into_one_node() {
    let (mut doc, v) = doc_with_views("", 1);
    type_str(&mut doc, v[0], "héllo", ms(0));
    assert_eq!(text(&doc), "héllo");
    assert_eq!(doc.history().len(), 2, "one node for the whole word");
    assert_eq!(doc.revision().get(), 5, "each keystroke is a revision");
    assert_eq!(doc.history().open_group(), Some(v[0]));
    let node = doc.history().node(NodeId(1)).unwrap();
    assert_eq!(node.time(), ms(40));
    assert_eq!(node.before(), &Selection::point(0));
    assert_eq!(node.after(), &Selection::point(6));
    doc.undo(v[0]).unwrap();
    assert_eq!(text(&doc), "");
    assert_eq!(doc.history().open_group(), None);
    doc.redo(v[0]).unwrap();
    assert_eq!(text(&doc), "héllo");
    assert_eq!(sel(&doc, v[0]), Selection::point(6));
}

/// Bytes a node's change sets carry: inserted plus deleted text.
fn node_bytes(node: &Node) -> usize {
    node.forward()
        .ops()
        .iter()
        .chain(node.inverse().ops())
        .map(|op| match op {
            Op::Insert(s) => s.len(),
            _ => 0,
        })
        .sum()
}

#[test]
fn a_long_typing_run_splits_at_the_group_size_bound() {
    // Merging composes the whole group per key, so an unbounded group makes
    // each key cost O(group size) (PLAN revision 3, Appendix B item 1).
    let (mut doc, v) = doc_with_views("", 1);
    doc.set_group_max_bytes(100);
    // One unbroken run, 1 ms apart: the pause never ends the group.
    for i in 0..1_000u64 {
        insert(&mut doc, v[0], "x", Record::typing(ms(i)));
    }
    assert_eq!(text(&doc).len(), 1_000);
    let nodes: Vec<&Node> = doc.history().nodes().skip(1).map(|(_, n)| n).collect();
    assert_eq!(nodes.len(), 10, "1,000 bytes in groups of at most 100");
    for n in &nodes {
        assert!(
            node_bytes(n) <= 2 * 100,
            "node holds {} bytes",
            node_bytes(n)
        );
    }
    // Still one undo step per group, and the whole run undoes.
    for _ in 0..10 {
        doc.undo(v[0]).unwrap();
    }
    assert_eq!(text(&doc), "");
    // A bound of zero turns grouping off.
    let (mut doc, v) = doc_with_views("", 1);
    doc.set_group_max_bytes(0);
    type_str(&mut doc, v[0], "abc", ms(0));
    assert_eq!(doc.history().len(), 4);
}

#[test]
fn the_default_group_bound_keeps_ordinary_typing_in_one_node() {
    let (mut doc, v) = doc_with_views("", 1);
    let word = "y".repeat(10_000);
    type_str(&mut doc, v[0], &word, ms(0));
    assert_eq!(doc.history().len(), 2);
    assert_eq!(History::DEFAULT_GROUP_MAX_BYTES, 64 * 1024);
}

#[test]
fn grouping_includes_deletions() {
    let (mut doc, v) = doc_with_views("ab", 1);
    doc.set_selection(v[0], Selection::point(2)).unwrap();
    type_str(&mut doc, v[0], "cd", ms(0));
    for i in 0..3 {
        let txn = Transaction::backspace(doc.text(), doc.selection(v[0]).unwrap()).unwrap();
        doc.commit_with(v[0], txn, Record::typing(ms(50 + i)))
            .unwrap();
    }
    assert_eq!(text(&doc), "a");
    assert_eq!(doc.history().len(), 2);
    doc.undo(v[0]).unwrap();
    assert_eq!(text(&doc), "ab");
    assert_eq!(sel(&doc, v[0]), Selection::point(2));
}

#[test]
fn each_boundary_ends_a_group() {
    type Boundary = fn(&mut Document, &[ViewId]);
    let boundaries: [(&str, Boundary); 6] = [
        ("pause", |doc, v| {
            insert(doc, v[0], "z", Record::typing(ms(10_000)));
        }),
        ("other view", |doc, v| {
            insert(doc, v[1], "z", Record::typing(ms(100)));
        }),
        ("ungrouped commit", |doc, v| {
            insert(doc, v[0], "z", Record::step(ms(100)));
        }),
        ("selection change", |doc, v| {
            let s = sel(doc, v[0]);
            doc.set_selection(v[0], s).unwrap();
            insert(doc, v[0], "z", Record::typing(ms(100)));
        }),
        ("checkpoint", |doc, v| {
            doc.checkpoint();
            insert(doc, v[0], "z", Record::typing(ms(100)));
        }),
        ("undo and redo", |doc, v| {
            doc.undo(v[0]).unwrap();
            doc.redo(v[0]).unwrap();
            insert(doc, v[0], "z", Record::typing(ms(100)));
        }),
    ];
    for (name, boundary) in boundaries {
        let (mut doc, v) = doc_with_views("", 2);
        type_str(&mut doc, v[0], "ab", ms(0));
        boundary(&mut doc, &v);
        assert_eq!(doc.history().len(), 3, "{name}: a second node");
        doc.undo(v[0]).unwrap();
        assert_eq!(
            doc.text().to_string().replace('z', ""),
            "ab",
            "{name}: undo removes only the part after the boundary"
        );
    }
}

#[test]
fn pause_exactly_at_the_limit_still_groups() {
    let (mut doc, v) = doc_with_views("", 1);
    doc.set_group_pause(Duration::from_millis(100));
    assert_eq!(doc.history().group_pause(), Duration::from_millis(100));
    insert(&mut doc, v[0], "a", Record::typing(ms(0)));
    insert(&mut doc, v[0], "b", Record::typing(ms(100)));
    assert_eq!(doc.history().len(), 2);
    insert(&mut doc, v[0], "c", Record::typing(ms(201)));
    assert_eq!(doc.history().len(), 3);
}

#[test]
fn identity_commits_record_nothing() {
    let (mut doc, v) = doc_with_views("abc", 1);
    type_str(&mut doc, v[0], "x", ms(0));
    let selection_only = |s| Transaction::new(ChangeSet::identity(4), Some(Selection::point(s)));
    // A typing identity commit (e.g. backspace at the start) keeps the group.
    doc.commit_with(v[0], selection_only(1), Record::typing(ms(1)))
        .unwrap();
    assert_eq!(doc.history().open_group(), Some(v[0]));
    // Any other identity commit ends it.
    doc.commit_with(v[0], selection_only(1), Record::step(ms(2)))
        .unwrap();
    assert_eq!(doc.history().open_group(), None);
    assert_eq!(doc.history().len(), 2);
    assert_eq!(doc.revision().get(), 1);
}

#[test]
fn a_group_that_cancels_out_still_undoes() {
    let (mut doc, v) = doc_with_views("abc", 2);
    doc.set_selection(v[1], Selection::point(3)).unwrap();
    type_str(&mut doc, v[0], "x", ms(0));
    let txn = Transaction::backspace(doc.text(), doc.selection(v[0]).unwrap()).unwrap();
    doc.commit_with(v[0], txn, Record::typing(ms(1))).unwrap();
    assert!(
        doc.history()
            .node(NodeId(1))
            .unwrap()
            .forward()
            .is_identity()
    );
    let r = doc.revision();
    doc.set_selection(v[0], Selection::point(2)).unwrap();
    // Undoing it changes no text and no revision, but restores the cursor.
    assert_eq!(doc.undo(v[0]), Ok(Some(r)));
    assert_eq!(text(&doc), "abc");
    assert_eq!(sel(&doc, v[0]), Selection::point(0));
    assert_eq!(sel(&doc, v[1]), Selection::point(3));
    assert_eq!(doc.history().head(), NodeId::ROOT);
}

#[test]
fn removing_the_typing_view_ends_its_group() {
    let (mut doc, v) = doc_with_views("", 2);
    type_str(&mut doc, v[0], "a", ms(0));
    assert!(doc.remove_view(v[0]));
    assert_eq!(doc.history().open_group(), None);
}

#[test]
fn times_never_decrease_and_lookups_use_them() {
    let (mut doc, v) = doc_with_views("", 1);
    insert(&mut doc, v[0], "a", Record::step(ms(100)));
    insert(&mut doc, v[0], "b", Record::step(ms(50))); // clock went back
    insert(&mut doc, v[0], "c", Record::step(ms(300)));
    let times: Vec<_> = doc.history().nodes().map(|(_, n)| n.time()).collect();
    assert_eq!(times, [ms(0), ms(100), ms(100), ms(300)]);
    assert_eq!(doc.history().latest_time(), ms(300));
    let h = doc.history();
    assert_eq!(h.at_time(ms(99)), NodeId::ROOT);
    assert_eq!(h.at_time(ms(100)), NodeId(2));
    assert_eq!(h.at_time(ms(299)), NodeId(2));
    assert_eq!(h.at_time(Timestamp::from_nanos(u64::MAX)), NodeId(3));
    assert_eq!(h.earlier(1), NodeId(2));
    assert_eq!(h.earlier(100), NodeId::ROOT);
    assert_eq!(h.later(1), NodeId(3));
    // `commit` without a time uses the latest one.
    insert(&mut doc, v[0], "d", Record::step(Timestamp::ZERO));
    let t = doc.history().latest_time();
    let txn = Transaction::insert(doc.text(), doc.selection(v[0]).unwrap(), "e").unwrap();
    doc.commit(v[0], txn).unwrap();
    assert_eq!(doc.history().latest_time(), t);
    // earlier/later by creation order cross branches.
    doc.goto(v[0], NodeId(1)).unwrap();
    insert(&mut doc, v[0], "z", Record::step(ms(400))); // node 6, child of 1
    assert_eq!(text(&doc), "az");
    let target = doc.history().earlier(1);
    assert_eq!(target, NodeId(5));
    doc.goto(v[0], target).unwrap();
    assert_eq!(text(&doc), "abcde");
    assert_eq!(doc.history().later(5), NodeId(6));
}

#[test]
fn heap_bytes_grows_with_history() {
    let (mut doc, v) = doc_with_views("", 1);
    let empty = doc.history().heap_bytes();
    insert(&mut doc, v[0], &"x".repeat(100), Record::step(ms(0)));
    assert!(doc.history().heap_bytes() >= empty + 100);
}

#[test]
fn failed_commit_leaves_history_alone() {
    let (mut doc, v) = doc_with_views("aé", 1);
    type_str(&mut doc, v[0], "x", ms(0));
    let len = doc.history().len();
    let split = Transaction::new(ChangeSet::from_edits(4, [(3..3, "y")]).unwrap(), None);
    assert!(doc.commit_with(v[0], split, Record::typing(ms(1))).is_err());
    assert_eq!(doc.history().len(), len);
    assert_eq!(doc.history().open_group(), Some(v[0]));
    let node = doc.history().node(NodeId(1)).unwrap();
    assert_eq!(node.forward().len_after(), 4);
}

#[test]
fn histories_longer_than_one_storage_chunk() {
    // Node storage is chunked (256 per chunk); cross several boundaries.
    let (mut doc, v) = doc_with_views("", 1);
    let n = 700;
    for i in 0..n {
        insert(&mut doc, v[0], "x", Record::step(ms(i as u64 + 1)));
    }
    let h = doc.history();
    assert_eq!(h.len(), n + 1);
    assert_eq!(h.nodes().count(), n + 1);
    for i in [255, 256, 257, 511, 512, n] {
        let node = h.node(NodeId(i)).unwrap();
        assert_eq!(node.depth(), i);
        assert_eq!(node.time(), ms(i as u64));
        assert_eq!(h.at_time(ms(i as u64)), NodeId(i));
    }
    assert!(h.node(NodeId(n + 1)).is_none());
    doc.goto(v[0], NodeId(256)).unwrap();
    assert_eq!(doc.text().byte_len(), 256);
    doc.goto(v[0], NodeId(n)).unwrap();
    assert_eq!(doc.text().byte_len(), n);
    for _ in 0..n {
        doc.undo(v[0]).unwrap().unwrap();
    }
    assert_eq!(text(&doc), "");
    assert_eq!(doc.undo(v[0]), Ok(None));
}

#[test]
fn extreme_ids_steps_and_times_do_not_panic() {
    let (mut doc, v) = doc_with_views("abc", 1);
    let max = Timestamp::from_nanos(u64::MAX);
    insert(&mut doc, v[0], "x", Record::typing(max));
    // Time cannot go past the maximum; later typing is clamped to it and
    // keeps merging (a hostile clock can only make groups longer).
    insert(&mut doc, v[0], "y", Record::typing(ms(1)));
    assert_eq!(doc.history().len(), 2);
    assert_eq!(doc.history().latest_time(), max);
    assert_eq!(max.saturating_add(Duration::MAX), max);
    assert_eq!(ms(1).saturating_sub(Duration::MAX), Timestamp::ZERO);
    let h = doc.history();
    assert_eq!(h.earlier(usize::MAX), NodeId::ROOT);
    assert_eq!(h.later(usize::MAX), NodeId(1));
    assert_eq!(h.at_time(max), NodeId(1));
    assert_eq!(
        doc.goto(v[0], NodeId(usize::MAX)),
        Err(CommitError::NoSuchNode(NodeId(usize::MAX)))
    );
    doc.set_group_pause(Duration::MAX);
    doc.set_group_pause(Duration::ZERO);
    doc.checkpoint();
    insert(&mut doc, v[0], "z", Record::typing(max));
    insert(&mut doc, v[0], "w", Record::typing(max));
    assert_eq!(
        doc.history().len(),
        3,
        "zero pause still merges at equal times"
    );
    assert_eq!(text(&doc), "xyzwabc");
}
