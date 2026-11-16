# Stage 5 design — undo, redo, branching history

Date: 2026-11-16. Spec: `docs/PLAN.md` Stage 5, sections 2.6, 2.7, 2.11, 3,
4, 6. Binding: DECISIONS 009 (never map positions through a composed change
set), 010/013 (validate, then mutate), 014 (documents own selections;
`Document::commit` is the only text mutator), 015 (revision log).

Owner instruction (2026-11-16): "complete the next stage as well". No
design conversation was held and this design was not separately
approved; every choice below marked **(call)** is the implementer's,
listed again in the stage report for review. Stage 5 builds on the
`stage-4` branch, whose approval is still the owner's to record.

## Intent

Every document gets a branching undo tree. Undo, redo, and jumps to any
node go through `Document::commit`, so revisions, the revision log, and
mapping of every other view behave exactly as for ordinary edits. Typing
groups into one undo step by composing change sets.

**Goals:** the PLAN Stage 5 build list and tests; history added to the
standing benchmark; bytes per node measured over a long typing trace.
**Non-goals:** persistent undo files, a history UI, undo bound to keymaps
beyond the temporary key map, a history cap (unless the measurement asks
for one).
**Anti-goals:** a single-cursor edit slower than Stage 4 by more than a
few hundred nanoseconds at p99; replay that depends on wall-clock time;
any path where undo leaves a view's selection invalid or applies part of
a jump.

## Approaches

1. **Tree of nodes holding forward and inverse change sets (chosen;
   named by PLAN).** Undo applies the inverse, redo the forward; a jump
   composes the path. O(ops of the step) per undo; memory O(change size)
   per node.
2. Snapshots of the text per node. O(1) undo with crop's shared trees, but
   no change set to map other views or log through (the revision log and
   bystander mapping would need diffs). Rejected.
3. Linear undo/redo stacks. Loses branches on edit-after-undo, which PLAN
   requires keeping. Rejected.

## Design

### Time (call)

Grouping by pause needs a clock, and replay must be deterministic. Time is
an input: `Timestamp(u64)` nanoseconds since an arbitrary epoch the
frontend chooses. `Editor::handle_at(&Event, Timestamp)` stores it and
handles the event; `Editor::handle(&Event)` keeps the previous timestamp
(so pauses never end a group without a clock). The binary passes the
session clock it already records as the trace's `t_ns`, and
`trace::replay` passes each entry's `t_ns`, so a replay groups exactly as
the session did. The trace format is unchanged (version 1).

### History (`document/history.rs`)

```text
History { nodes: Vec<Node>, head: NodeId, open: Option<ViewId>, group_pause: u64 }
Node    { parent, last_child: Option<NodeId>, depth, forward, inverse,
          before: Selection, after: Selection, time: Timestamp }
```

- `NodeId(usize)`, an index in creation order; node 0 is the root (the
  text as opened; identity change sets, empty-document selections). Node
  IDs and revisions are distinct (PLAN 2.7).
- `forward` turns the parent's text into the node's; `inverse` the
  reverse. `before` is the committing view's selection before the first
  edit of the node; `after` its selection after the last.
- `time` is the time of the node's last edit, clamped so times never
  decrease in creation order (traces do not promise monotonic `t_ns`),
  which keeps time lookups a binary search.
- `last_child` is the redo target: set when a child is created and when
  undo or a jump leaves a node upward or passes through it downward, so
  redo returns along the branch last visited (Vim's rule). Children are
  not stored (call): nothing needs them yet; `children(node)` scans.
- Read API: `head()`, `len()`, `node(id)` (parent, time, selections,
  change sets), `children(id)`, `earlier(steps)`, `later(steps)` (node IDs
  by creation order, clamped), `at_time(t)` (newest node with
  `time <= t`, or the root).

### Recording and grouping

`Document::commit_with(view, txn, Record { time, group })`;
`Document::commit(view, txn)` is `Record { time: latest node time,
group: false }`.

A non-identity commit computes the inverse against the pre-change text
(before anything changes), then either:

- **merges** into the head node, when `group` is true, the open group
  belongs to `view` (`open == Some(view)`), and `time - head.time <=
  group_pause`: `forward = compose(head.forward, c)`, `inverse =
  compose(c⁻¹, head.inverse)`, `after` and `time` replaced; or
- **creates** a child of the head with `before` = the view's old selection
  and `after` = its new one, and moves the head to it.

`open` becomes `Some(view)` after a commit with `group`, `None` after one
without. It is also cleared by: `set_selection` from any view (a motion),
`checkpoint()`, undo, redo, and jumps. An identity commit records nothing
and clears `open` unless `group` is set. So a merge only ever extends the
newest node, which is the head.

All composition happens before the text changes; a composition error is
unreachable (lengths match by construction) and is reported as a
`CommitError` with nothing changed.

Default `group_pause`: 1 s (call). Stage 7 makes it an option (PLAN 2.11
lists "undo-group pause").

### Undo, redo, jumps

- `undo(view) -> Result<Option<Revision>, CommitError>`: `None` at the
  root. Commits `Transaction::new(head.inverse, Some(head.before))` from
  `view` (the invoking view gets the stored selection; every other view is
  mapped through the inverse by commit), then the head moves to the
  parent and the parent's `last_child` to the old head.
- `redo(view)`: `None` without `last_child`; commits `forward` with
  `after`.
- `goto(view, target)`: `None` if `target` is the head; error if unknown.
  The path is up from the head to the lowest common ancestor (inverses),
  then down to `target` (forwards), composed into **one** change set and
  committed once (call): the jump is atomic and is one revision. The
  invoking view gets `before` of the last node undone if the path ends
  going up, else `after` of `target`. `last_child` pointers along the
  path are updated as for single steps.
- These never record history nodes. Stored selections are valid for the
  text they are restored onto by construction; commit's validation still
  checks them, so a bug returns an error instead of corrupting state.

Mapping other views through a composed jump differs from mapping through
each step (DECISIONS 009); both give valid selections, and the jump is one
edit, so it is mapped as one.

### Editor and keys

- `Editor { now: Timestamp, .. }`; `handle_at` / `handle` as above.
- Typing commands (character, Enter, Tab, Backspace, Delete) commit with
  `group: true`; paste and `Editor::commit` with `group: false` (call:
  a paste is its own undo step and ends any typing group).
- Every motion and Esc closes the group even when the selection does not
  move (call: "motion" is the boundary, not "the cursor moved").
- Temporary keys (call; Stage 6 lists undo/redo, Stage 7 replaces the map):
  Ctrl+Z undo, Ctrl+Y and Ctrl+Shift+Z redo. Nothing to undo or redo sets
  the message "already at the oldest change" / "… newest change".
- `Editor::undo`, `redo`, `goto(view, node)`, `checkpoint(view)` public,
  returning `Result<Option<Revision>, EditorError>`; the focused view
  scrolls afterwards, as for `commit`.

### Memory and caps

No cap now. A history bench replays a long typing trace (single cursor
and multi-cursor) and reports live heap bytes per node from a counting
allocator; a cap with pruning is added only if those numbers ask for it
(PLAN). `History::heap_bytes()` gives an estimate for the report and later
caps.

## Tests (PLAN Stage 5)

- Undo all / redo all returns the exact text at every step; Unicode
  replacements; multi-cursor; several views (bystanders mapped through the
  inverse; the invoking view gets the stored selection).
- Branch on edit-after-undo; return to the old branch by `goto`; redo
  follows the branch last visited.
- Property: random walks of edit (grouped or not, random view, random time
  step) / undo / redo / goto / earlier / later / checkpoint / selection
  change against a model storing a full text snapshot per node; after
  every step the text equals the model's head snapshot, revisions advance
  exactly on text changes, every selection is valid and normalised, and
  the log maps positions like the sequence of committed change sets.
- Grouped typing undoes as one unit and matches the ungrouped result
  (text and selection after one grouped undo equal those after k
  ungrouped undos). Pause, motion, view change, paste, and checkpoint each
  end a group.
- Editor: Ctrl+Z / Ctrl+Y through `handle`; random events (including
  undo/redo keys) never panic; trace replay with timestamps reproduces the
  session's grouping.

## Benchmarks

- Standing benchmark: commit now records history; add undo of the same
  edit (1 / 2 / 8 views, one-cursor and 10k-cursor).
- Latency rows: commit with history (ungrouped and merged into an open
  group of 100 edits), undo and redo of one character.
- History memory: bytes per node over a long typing trace.

## New dependencies

None.

## Left to the builder

Error variants, file layout inside `document/`, the model's
representation, how the property generates walks.
