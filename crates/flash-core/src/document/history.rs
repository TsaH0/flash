//! The branching undo tree (PLAN 2.7, Stage 5).
//!
//! Every effective commit of a [`Document`](super::Document) becomes a
//! [`Node`] holding the change set from its parent's text to its own
//! (`forward`), the reverse (`inverse`), and the committing view's
//! selection before and after. Undo, redo, and jumps commit those change
//! sets through `Document::commit`'s machinery, so revisions, the revision
//! log, and other views' selections behave as for any edit; they move the
//! head instead of recording new nodes.
//!
//! Node IDs count nodes in creation order and are distinct from revisions:
//! undo advances the revision but moves the head to an older node.
//!
//! # Grouping
//!
//! A commit recorded with [`Record::group`] set merges into the head node
//! instead of creating one when the head is the open group of the same
//! view and the pause since its last edit is at most the group pause. The
//! node's change sets are then composed: `forward = compose(forward, c)`,
//! `inverse = compose(c⁻¹, inverse)`. The group closes on a commit without
//! `group`, a selection change, a checkpoint, undo, redo, or a jump, so a
//! merge only ever extends the newest node.

use std::time::Duration;

use crate::changeset::{ChangeSet, Op};
use crate::editor::ViewId;
use crate::selection::Selection;

/// A point in time, in nanoseconds since an epoch the frontend chooses
/// (the trace's session start). Time is an input so that replaying a trace
/// groups undo steps exactly as the recorded session did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const ZERO: Timestamp = Timestamp(0);

    pub fn from_nanos(nanos: u64) -> Timestamp {
        Timestamp(nanos)
    }

    pub fn as_nanos(self) -> u64 {
        self.0
    }

    /// The time `d` after this one, saturating.
    pub fn saturating_add(self, d: Duration) -> Timestamp {
        Timestamp(self.0.saturating_add(nanos(d)))
    }

    /// The time `d` before this one, saturating at zero.
    pub fn saturating_sub(self, d: Duration) -> Timestamp {
        Timestamp(self.0.saturating_sub(nanos(d)))
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// A node of a document's history, by creation order. The root is
/// [`NodeId::ROOT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) usize);

impl NodeId {
    /// The text as opened.
    pub const ROOT: NodeId = NodeId(0);

    pub fn index(self) -> usize {
        self.0
    }
}

/// How a commit is recorded in history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// When the commit happened.
    pub time: Timestamp,
    /// Whether the commit may join the open undo group (typing).
    pub group: bool,
}

impl Record {
    /// A commit that starts its own undo step.
    pub fn step(time: Timestamp) -> Record {
        Record { time, group: false }
    }

    /// A typing commit, which may join the open undo group.
    pub fn typing(time: Timestamp) -> Record {
        Record { time, group: true }
    }
}

/// One undo step: the change from its parent's text to its own.
#[derive(Debug, Clone)]
pub struct Node {
    parent: Option<NodeId>,
    last_child: Option<NodeId>,
    depth: usize,
    forward: ChangeSet,
    inverse: ChangeSet,
    before: Selection,
    after: Selection,
    time: Timestamp,
}

impl Node {
    /// The node this one was recorded on top of; `None` for the root.
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// The child redo moves to: the one last created or visited.
    pub fn last_child(&self) -> Option<NodeId> {
        self.last_child
    }

    /// Distance from the root.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Turns the parent's text into this node's (identity for the root).
    pub fn forward(&self) -> &ChangeSet {
        &self.forward
    }

    /// Turns this node's text into the parent's (identity for the root).
    pub fn inverse(&self) -> &ChangeSet {
        &self.inverse
    }

    /// The committing view's selection before the node's first edit:
    /// restored by undo.
    pub fn before(&self) -> &Selection {
        &self.before
    }

    /// The committing view's selection after the node's last edit:
    /// restored by redo.
    pub fn after(&self) -> &Selection {
        &self.after
    }

    /// When the node's last edit happened. Never decreases in creation
    /// order.
    pub fn time(&self) -> Timestamp {
        self.time
    }

    /// Approximate heap bytes held by this node.
    fn heap_bytes(&self) -> usize {
        changeset_heap_bytes(&self.forward)
            + changeset_heap_bytes(&self.inverse)
            + self.before.heap_bytes()
            + self.after.heap_bytes()
    }
}

fn changeset_heap_bytes(c: &ChangeSet) -> usize {
    let strings: usize = c
        .ops()
        .iter()
        .map(|op| match op {
            Op::Insert(s) if s.is_heap_allocated() => s.capacity(),
            _ => 0,
        })
        .sum();
    c.ops_capacity() * size_of::<Op>() + strings
}

/// A document's undo tree, its head, and the open undo group.
#[derive(Debug, Clone)]
pub struct History {
    nodes: Vec<Node>,
    head: NodeId,
    /// The view whose typing may merge into the head (always the newest
    /// node while set).
    open: Option<ViewId>,
    group_pause: Duration,
}

/// A change to the history, computed before the text changes and applied
/// after (so a failed commit changes nothing).
#[derive(Debug)]
pub(crate) enum Plan {
    /// Record nothing; close the open group unless `keep_open`.
    Nothing { keep_open: bool },
    /// Replace the head's change sets with these compositions.
    Merge {
        forward: ChangeSet,
        inverse: ChangeSet,
        after: Selection,
        time: Timestamp,
    },
    /// Add a child of the head and move there.
    Push {
        forward: ChangeSet,
        inverse: ChangeSet,
        before: Selection,
        after: Selection,
        time: Timestamp,
        view: ViewId,
        group: bool,
    },
    /// Move the head along `path` (undo, redo, jump) to its last node.
    Move(Path),
}

/// The nodes a jump passes: `up` from the head to just below the lowest
/// common ancestor, then `down` from just below it to the target.
///
/// Invariant kept by [`History::apply`]: every ancestor of the head has
/// its `last_child` on the path to the head.
#[derive(Debug, Default)]
pub(crate) struct Path {
    pub(crate) up: Vec<NodeId>,
    pub(crate) down: Vec<NodeId>,
    pub(crate) target: Option<NodeId>,
}

impl History {
    /// Default longest pause between two typing commits of one undo step.
    /// An option from Stage 7 (PLAN 2.11, "undo-group pause").
    pub const DEFAULT_GROUP_PAUSE: Duration = Duration::from_secs(1);

    /// A history whose root is a text of `len` bytes.
    pub(crate) fn new(len: usize) -> History {
        History {
            nodes: vec![Node {
                parent: None,
                last_child: None,
                depth: 0,
                forward: ChangeSet::identity(len),
                inverse: ChangeSet::identity(len),
                before: Selection::point(0),
                after: Selection::point(0),
                time: Timestamp::ZERO,
            }],
            head: NodeId::ROOT,
            open: None,
            group_pause: Self::DEFAULT_GROUP_PAUSE,
        }
    }

    /// The node whose text the document has now.
    pub fn head(&self) -> NodeId {
        self.head
    }

    /// The number of nodes, the root included. Never zero.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Always `false`: the root is always present.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0)
    }

    /// Every node, in creation order.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(i, n)| (NodeId(i), n))
    }

    /// The children of `id`, in creation order. O(nodes).
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes()
            .filter(move |(_, n)| n.parent == Some(id))
            .map(|(c, _)| c)
    }

    /// The view whose typing commits may join the head node, if any.
    pub fn open_group(&self) -> Option<ViewId> {
        self.open
    }

    pub fn group_pause(&self) -> Duration {
        self.group_pause
    }

    /// The node `steps` before the head in creation order (Vim's
    /// `:earlier`), clamped to the root.
    pub fn earlier(&self, steps: usize) -> NodeId {
        NodeId(self.head.0.saturating_sub(steps))
    }

    /// The node `steps` after the head in creation order (`:later`),
    /// clamped to the newest node.
    pub fn later(&self, steps: usize) -> NodeId {
        NodeId(self.head.0.saturating_add(steps).min(self.nodes.len() - 1))
    }

    /// The newest node whose time is at most `time`, or the root. Node
    /// times never decrease in creation order, so this is a binary search.
    pub fn at_time(&self, time: Timestamp) -> NodeId {
        let after = self.nodes.partition_point(|n| n.time <= time);
        NodeId(after.saturating_sub(1))
    }

    /// The time of the newest node: the latest time history has seen.
    pub fn latest_time(&self) -> Timestamp {
        self.nodes[self.nodes.len() - 1].time
    }

    /// Approximate heap bytes held by the history (node storage, change
    /// sets, selections). For measurement and future caps.
    pub fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<Node>()
            + self.nodes.iter().map(Node::heap_bytes).sum::<usize>()
    }

    pub(crate) fn set_group_pause(&mut self, pause: Duration) {
        self.group_pause = pause;
    }

    /// Closes the open undo group: the next commit starts a new node.
    pub(crate) fn close_group(&mut self) {
        self.open = None;
    }

    pub(crate) fn head_node(&self) -> &Node {
        &self.nodes[self.head.0]
    }

    pub(crate) fn nodes_slice(&self) -> &[Node] {
        &self.nodes
    }

    /// Whether a commit from `view` recorded as `record` merges into the
    /// head node.
    ///
    /// Invariant: while a group is open the head is the newest node (every
    /// move closes the group), so a merge never rewrites a node with
    /// children.
    pub(crate) fn merges(&self, view: ViewId, record: Record) -> bool {
        debug_assert!(self.open.is_none() || self.head.0 == self.nodes.len() - 1);
        let head = self.head_node();
        record.group
            && self.open == Some(view)
            && self.clamp(record.time).0 - head.time.0 <= nanos(self.group_pause)
    }

    /// `time`, raised to the newest node's time so times never decrease.
    pub(crate) fn clamp(&self, time: Timestamp) -> Timestamp {
        time.max(self.latest_time())
    }

    /// The path from the head to `target` (which must exist).
    pub(crate) fn path_to(&self, target: NodeId) -> Path {
        let (mut a, mut b) = (self.head, target);
        let mut up = Vec::new();
        let mut down = Vec::new();
        while self.nodes[a.0].depth > self.nodes[b.0].depth {
            up.push(a);
            a = self.parent_of(a);
        }
        while self.nodes[b.0].depth > self.nodes[a.0].depth {
            down.push(b);
            b = self.parent_of(b);
        }
        while a != b {
            up.push(a);
            down.push(b);
            a = self.parent_of(a);
            b = self.parent_of(b);
        }
        down.reverse();
        Path {
            up,
            down,
            target: Some(target),
        }
    }

    fn parent_of(&self, id: NodeId) -> NodeId {
        // Only the root has no parent, and it has depth 0, so the loops in
        // `path_to` never ask for its parent.
        self.nodes[id.0].parent.unwrap_or(NodeId::ROOT)
    }

    /// Applies a plan computed before the commit's text change.
    pub(crate) fn apply(&mut self, plan: Plan) {
        match plan {
            Plan::Nothing { keep_open } => {
                if !keep_open {
                    self.open = None;
                }
            }
            Plan::Merge {
                forward,
                inverse,
                after,
                time,
            } => {
                let head = &mut self.nodes[self.head.0];
                head.forward = forward;
                head.inverse = inverse;
                head.after = after;
                head.time = time;
            }
            Plan::Push {
                forward,
                inverse,
                before,
                after,
                time,
                view,
                group,
            } => {
                let id = NodeId(self.nodes.len());
                let parent = self.head;
                let depth = self.nodes[parent.0].depth + 1;
                self.nodes.push(Node {
                    parent: Some(parent),
                    last_child: None,
                    depth,
                    forward,
                    inverse,
                    before,
                    after,
                    time,
                });
                self.nodes[parent.0].last_child = Some(id);
                self.head = id;
                self.open = group.then_some(view);
            }
            Plan::Move(path) => {
                // Moving up needs no update: every ancestor of the head
                // already has its `last_child` on the path to the head
                // (set when the child was created or moved down into), so
                // redo after undo returns to the node just left.
                for &d in &path.down {
                    let parent = self.parent_of(d);
                    self.nodes[parent.0].last_child = Some(d);
                }
                if let Some(target) = path.target {
                    self.head = target;
                }
                self.open = None;
            }
        }
    }
}
