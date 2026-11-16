//! History against a model that stores a full text snapshot per node.
//!
//! The model never composes or inverts anything: a merged node just takes
//! the new text. So undo, redo, and jumps reaching the model's snapshots
//! checks the composed and inverted change sets, and the model's tree
//! bookkeeping checks the head, `last_child`, grouping, and the stored
//! selections.

use proptest::prelude::*;
use proptest::sample::Index;
use slotmap::SlotMap;

use super::*;
use crate::changeset::reference::apply_ops;
use crate::selection::proptests::assert_normalised;
use crate::strategies::{floor_boundary, selection_for, text_strategy};
use crate::transaction::proptests::{build, intent};
use crate::transaction::reference::Intent;

const MS: u64 = 1_000_000;

#[derive(Debug, Clone)]
enum Step {
    /// An edit from a view, grouped or not, `dt_ms` after the previous one
    /// (negative: the clock went back).
    Edit {
        view: Index,
        intent: Intent,
        group: bool,
        dt_ms: i64,
    },
    Undo(Index),
    Redo(Index),
    Goto(Index, Index),
    Earlier(Index, usize),
    Later(Index, usize),
    Checkpoint,
    Select(Index, Index),
}

fn step() -> impl Strategy<Value = Step> {
    let dt = prop_oneof![8 => 0i64..300, 1 => 900i64..1200, 1 => -500i64..0];
    prop_oneof![
        8 => (any::<Index>(), intent(), any::<bool>(), dt).prop_map(|(view, intent, group, dt_ms)| {
            Step::Edit { view, intent, group, dt_ms }
        }),
        3 => any::<Index>().prop_map(Step::Undo),
        2 => any::<Index>().prop_map(Step::Redo),
        1 => (any::<Index>(), any::<Index>()).prop_map(|(v, n)| Step::Goto(v, n)),
        1 => (any::<Index>(), 1usize..4).prop_map(|(v, n)| Step::Earlier(v, n)),
        1 => (any::<Index>(), 1usize..4).prop_map(|(v, n)| Step::Later(v, n)),
        1 => Just(Step::Checkpoint),
        1 => (any::<Index>(), any::<Index>()).prop_map(|(v, p)| Step::Select(v, p)),
    ]
}

type Case = (String, Vec<Selection>, Vec<Step>);

fn case() -> impl Strategy<Value = Case> {
    text_strategy(6).prop_flat_map(|s| {
        (
            prop::collection::vec(selection_for(&s), 1..=3),
            prop::collection::vec(step(), 1..40),
            Just(s),
        )
            .prop_map(|(sels, steps, s)| (s, sels, steps))
    })
}

#[derive(Debug, Clone)]
struct MNode {
    text: String,
    parent: Option<usize>,
    last_child: Option<usize>,
    before: Selection,
    after: Selection,
    time: u64,
}

struct Model {
    nodes: Vec<MNode>,
    head: usize,
    open: Option<ViewId>,
    pause: u64,
}

impl Model {
    fn new(text: &str) -> Model {
        Model {
            nodes: vec![MNode {
                text: text.to_owned(),
                parent: None,
                last_child: None,
                before: Selection::point(0),
                after: Selection::point(0),
                time: 0,
            }],
            head: 0,
            open: None,
            pause: u64::try_from(History::DEFAULT_GROUP_PAUSE.as_nanos()).unwrap(),
        }
    }

    fn latest_time(&self) -> u64 {
        self.nodes.last().unwrap().time
    }

    fn record(
        &mut self,
        view: ViewId,
        text: String,
        before: &Selection,
        after: &Selection,
        time: u64,
        group: bool,
    ) {
        let t = time.max(self.latest_time());
        let newest = self.head == self.nodes.len() - 1;
        let head = &mut self.nodes[self.head];
        if group && self.open == Some(view) && newest && t - head.time <= self.pause {
            head.text = text;
            head.after = after.clone();
            head.time = t;
        } else {
            let id = self.nodes.len();
            self.nodes.push(MNode {
                text,
                parent: Some(self.head),
                last_child: None,
                before: before.clone(),
                after: after.clone(),
                time: t,
            });
            self.nodes[self.head].last_child = Some(id);
            self.head = id;
        }
        self.open = group.then_some(view);
    }

    fn ancestors(&self, mut n: usize) -> Vec<usize> {
        let mut out = vec![n];
        while let Some(p) = self.nodes[n].parent {
            out.push(p);
            n = p;
        }
        out
    }

    /// Moves to `target` the way the spec describes, returning the
    /// selection the invoking view gets. Sets `last_child` along the way
    /// as undo and redo of each step would.
    fn jump(&mut self, target: usize) -> Selection {
        let from = self.ancestors(self.head);
        let to = self.ancestors(target);
        let lca = *from.iter().find(|n| to.contains(n)).unwrap();
        let up: Vec<usize> = from.iter().copied().take_while(|&n| n != lca).collect();
        let down: Vec<usize> = to.iter().copied().take_while(|&n| n != lca).collect();
        for &u in &up {
            let p = self.nodes[u].parent.unwrap();
            self.nodes[p].last_child = Some(u);
        }
        for &d in down.iter().rev() {
            let p = self.nodes[d].parent.unwrap();
            self.nodes[p].last_child = Some(d);
        }
        self.head = target;
        self.open = None;
        match (down.first(), up.last()) {
            (Some(&d), _) => self.nodes[d].after.clone(),
            (None, Some(&u)) => self.nodes[u].before.clone(),
            (None, None) => unreachable!("target is not the head"),
        }
    }
}

fn check_tree(doc: &Document, model: &Model) -> Result<(), TestCaseError> {
    let h = doc.history();
    prop_assert_eq!(h.len(), model.nodes.len());
    prop_assert_eq!(h.head().index(), model.head);
    prop_assert_eq!(h.open_group(), model.open);
    for (id, node) in h.nodes() {
        let m = &model.nodes[id.index()];
        prop_assert_eq!(
            node.parent().map(NodeId::index),
            m.parent,
            "parent of {:?}",
            id
        );
        prop_assert_eq!(
            node.last_child().map(NodeId::index),
            m.last_child,
            "last_child of {:?}",
            id
        );
        prop_assert_eq!(node.before(), &m.before, "before of {:?}", id);
        prop_assert_eq!(node.after(), &m.after, "after of {:?}", id);
        prop_assert_eq!(node.time().as_nanos(), m.time, "time of {:?}", id);
        prop_assert_eq!(node.forward().len_after(), m.text.len());
        prop_assert_eq!(node.inverse().len_before(), m.text.len());
    }
    Ok(())
}

proptest! {
    #[test]
    fn history_matches_snapshot_model((s, sels, steps) in case()) {
        let mut doc = Document::new(Text::from(s.as_str()), None);
        let mut ids: SlotMap<ViewId, ()> = SlotMap::with_key();
        let views: Vec<ViewId> = sels.iter().map(|_| ids.insert(())).collect();
        for (&v, sel) in views.iter().zip(&sels) {
            doc.add_view(v);
            doc.set_selection(v, sel.clone()).unwrap();
        }
        let mut model = Model::new(&s);
        let mut now: u64 = 0;

        for step in &steps {
            let before: Vec<Selection> =
                views.iter().map(|&v| doc.selection(v).unwrap().clone()).collect();
            let text_before = doc.text().to_string();
            let revision = doc.revision();
            let pick = |i: &Index| i.index(views.len());
            // The change set committed, when known, and the origin view's
            // expected selection.
            let mut committed: Option<ChangeSet> = None;
            let mut origin: Option<(usize, Selection)> = None;

            match step {
                Step::Edit { view, intent, group, dt_ms } => {
                    now = now.saturating_add_signed(dt_ms * MS as i64);
                    let vi = pick(view);
                    let Ok(txn) = build(doc.text(), &before[vi], intent) else {
                        continue;
                    };
                    let want = txn.prepare(doc.text(), &before[vi]).unwrap();
                    let changes = txn.changes().clone();
                    let record = Record { time: Timestamp::from_nanos(now), group: *group };
                    doc.commit_with(views[vi], txn, record).unwrap();
                    if changes.is_identity() {
                        if !group {
                            model.open = None;
                        }
                    } else {
                        let text = apply_ops(changes.ops(), &text_before);
                        model.record(views[vi], text, &before[vi], &want, now, *group);
                    }
                    committed = Some(changes);
                    origin = Some((vi, want));
                }
                Step::Undo(view) => {
                    let vi = pick(view);
                    let head = model.head;
                    match model.nodes[head].parent {
                        None => prop_assert_eq!(doc.undo(views[vi]), Ok(None)),
                        Some(parent) => {
                            let inverse = doc.history().node(doc.history().head()).unwrap().inverse().clone();
                            prop_assert!(doc.undo(views[vi]).unwrap().is_some());
                            model.nodes[parent].last_child = Some(head);
                            model.head = parent;
                            model.open = None;
                            committed = Some(inverse);
                            origin = Some((vi, model.nodes[head].before.clone()));
                        }
                    }
                }
                Step::Redo(view) => {
                    let vi = pick(view);
                    match model.nodes[model.head].last_child {
                        None => prop_assert_eq!(doc.redo(views[vi]), Ok(None)),
                        Some(child) => {
                            let forward = doc.history().node(NodeId(child)).unwrap().forward().clone();
                            prop_assert!(doc.redo(views[vi]).unwrap().is_some());
                            model.head = child;
                            model.open = None;
                            committed = Some(forward);
                            origin = Some((vi, model.nodes[child].after.clone()));
                        }
                    }
                }
                Step::Goto(..) | Step::Earlier(..) | Step::Later(..) => {
                    let (view, target) = match step {
                        Step::Goto(v, n) => (v, n.index(model.nodes.len())),
                        Step::Earlier(v, n) => {
                            let t = model.head.saturating_sub(*n);
                            prop_assert_eq!(doc.history().earlier(*n).index(), t);
                            (v, t)
                        }
                        Step::Later(v, n) => {
                            let t = (model.head + n).min(model.nodes.len() - 1);
                            prop_assert_eq!(doc.history().later(*n).index(), t);
                            (v, t)
                        }
                        _ => unreachable!(),
                    };
                    let vi = pick(view);
                    if target == model.head {
                        prop_assert_eq!(doc.goto(views[vi], NodeId(target)), Ok(None));
                    } else {
                        prop_assert!(doc.goto(views[vi], NodeId(target)).unwrap().is_some());
                        origin = Some((vi, model.jump(target)));
                    }
                }
                Step::Checkpoint => {
                    doc.checkpoint();
                    model.open = None;
                }
                Step::Select(view, pos) => {
                    let vi = pick(view);
                    let p = floor_boundary(&text_before, pos.index(text_before.len() + 1));
                    doc.set_selection(views[vi], Selection::point(p)).unwrap();
                    model.open = None;
                }
            }

            // Text, tree, and stored selections match the model.
            prop_assert_eq!(doc.text().to_string(), model.nodes[model.head].text.as_str(), "after {:?}", step);
            check_tree(&doc, &model)?;

            // Revisions advance by one exactly on non-identity commits, and
            // the newest log entry is what was committed.
            let advanced = doc.revision().get() - revision.get();
            prop_assert!(advanced <= 1);
            if doc.text().to_string() != text_before {
                prop_assert_eq!(advanced, 1);
            }
            if let Some(c) = &committed {
                prop_assert_eq!(advanced, u64::from(!c.is_identity()));
                if !c.is_identity() {
                    let (r, logged) = doc.log().entries().last().unwrap();
                    prop_assert_eq!(*r, doc.revision());
                    prop_assert_eq!(logged, c);
                }
            }

            // The invoking view gets its expected selection; every other
            // view is mapped through the committed change set.
            for (i, &v) in views.iter().enumerate() {
                let got = doc.selection(v).unwrap();
                prop_assert_eq!(got.validate(doc.text()), Ok(()));
                assert_normalised(got)?;
                match (&origin, &committed) {
                    (Some((o, want)), _) if *o == i => prop_assert_eq!(got, want),
                    (_, Some(c)) if !c.is_identity() => {
                        prop_assert_eq!(got, &before[i].map(c).unwrap());
                    }
                    (Some(_), Some(_)) => prop_assert_eq!(got, &before[i]),
                    _ => {}
                }
            }
        }

        // The log replays the whole session from the opened text.
        let mut replayed = s.clone();
        for (_, c) in doc.log().entries() {
            replayed = apply_ops(c.ops(), &replayed);
        }
        prop_assert_eq!(doc.log().len() as u64, doc.revision().get());
        prop_assert_eq!(replayed, doc.text().to_string());
    }

    /// Typing grouped into one node undoes and redoes exactly like the same
    /// edits recorded as separate nodes undone and redone one by one.
    #[test]
    fn grouped_typing_matches_ungrouped(
        (s, sel, intents) in text_strategy(6).prop_flat_map(|s| {
            (selection_for(&s), prop::collection::vec(intent(), 1..12), Just(s))
                .prop_map(|(sel, intents, s)| (s, sel, intents))
        })
    ) {
        let open = |s: &str| {
            let mut doc = Document::new(Text::from(s), None);
            let mut ids: SlotMap<ViewId, ()> = SlotMap::with_key();
            let v = ids.insert(());
            doc.add_view(v);
            doc.set_selection(v, sel.clone()).unwrap();
            (doc, v)
        };
        let (mut grouped, g) = open(&s);
        let (mut separate, u) = open(&s);
        let mut steps = 0;
        // The selection after the last edit that changed the text: what the
        // node stores (identity commits are not recorded).
        let mut end_sel = None;
        for (i, intent) in intents.iter().enumerate() {
            let time = Timestamp::from_nanos(i as u64 * 10 * MS);
            let Ok(a) = build(grouped.text(), grouped.selection(g).unwrap(), intent) else {
                continue;
            };
            let b = build(separate.text(), separate.selection(u).unwrap(), intent).unwrap();
            let effective = !b.changes().is_identity();
            steps += usize::from(effective);
            grouped.commit_with(g, a, Record::typing(time)).unwrap();
            separate.commit_with(u, b, Record::step(time)).unwrap();
            prop_assert_eq!(grouped.text(), separate.text());
            if effective {
                end_sel = Some(grouped.selection(g).unwrap().clone());
            }
        }
        let end_text = grouped.text().to_string();
        prop_assert!(grouped.history().len() <= 2);
        prop_assert_eq!(separate.history().len(), steps + 1);

        prop_assert_eq!(grouped.undo(g).unwrap().is_some(), steps > 0);
        for _ in 0..steps {
            prop_assert!(separate.undo(u).unwrap().is_some());
        }
        prop_assert_eq!(grouped.text().to_string(), s.as_str());
        prop_assert_eq!(separate.text().to_string(), s.as_str());
        prop_assert_eq!(grouped.selection(g), separate.selection(u));

        prop_assert_eq!(grouped.redo(g).unwrap().is_some(), steps > 0);
        for _ in 0..steps {
            prop_assert!(separate.redo(u).unwrap().is_some());
        }
        prop_assert_eq!(grouped.text().to_string(), end_text);
        prop_assert_eq!(separate.text().to_string(), grouped.text().to_string());
        prop_assert_eq!(grouped.selection(g), separate.selection(u));
        if steps > 0 {
            prop_assert_eq!(grouped.selection(g), end_sel.as_ref());
        }
    }
}
