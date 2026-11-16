# Stage 5 — Undo, redo, branching history

Status: **complete; approved by the owner 2026-11-16** (no independent
review was run). Branch `stage-5` (on top of `stage-4`), local commits
only (nothing pushed). Design:
[2026-11-16-stage-5-design.md](../superpowers/specs/2026-11-16-stage-5-design.md).
Plan: [2026-11-16-stage-5-history.md](../superpowers/plans/2026-11-16-stage-5-history.md).

The owner asked for this stage with "complete the next stage as well"; no
design conversation was held. The design's choices marked **(call)** are
the implementer's and are listed in section G for review.

## A. Status

| Requirement (PLAN Stage 5) | State |
|---|---|
| History tree: forward and inverse change sets, parent, children, selection before and after, timestamp | Done (`document/history.rs`); children found by scan, `last_child` stored (DECISIONS 017) |
| Undo, redo, branch on edit-after-undo, move to an arbitrary node (earlier/later by time) | Done: `undo`, `redo`, `goto(node)`; `History::earlier`/`later` (creation order) and `at_time` |
| Grouping by composition until motion, mode change, pause, or checkpoint | Done (DECISIONS 018); modes arrive in Stage 7, whose mode change will call `checkpoint` |
| Invoking view gets the stored selection; other views mapped through the inverse | Done (through `commit`) |
| Undo and redo go through `Document::commit` | Done: one shared commit path; revisions, log, and mapping as for edits |
| Bytes per node over a long typing trace; cap only if needed | Measured (section F); no cap (DECISIONS 019) |
| Tests: undo all / redo all, Unicode, multi-cursor, multiple views, branch and return, random walks against snapshots, grouped equals ungrouped | Done (section D) |

## B. Commits

Branch `stage-5`, from `61c46aa` (Stage 4 head):

| Commit | Content |
|---|---|
| `08c5cca` | Stage 5 design and plan; status |
| `1cd7bbb` | `History`, `Node`, `NodeId`, `Record`, `Timestamp`; `Document::commit_with`, `undo`, `redo`, `goto`, `checkpoint`; 18 cases |
| `24b2d36` | Snapshot-model property and grouped-vs-ungrouped property |
| `e2a3b0c` | `Editor::handle_at`, `undo`/`redo`/`goto`/`checkpoint`, keys, timed `trace::replay`, binary |
| `3792c7a` | Chunked node storage, standing benchmark undo rows, latency rows, `benches/history.rs`, results |
| (closing commit) | Review fixes and tests, this report, DECISIONS 017–019, status |

New dependencies: none.

## C. Design and algorithms

### Recording

**Problem.** Record every effective edit so it can be undone, without
weakening commit's all-or-nothing rule.

**How.** `commit_with(view, txn, record)` runs Stage 4's commit (validate
the change set and the origin selection, map every other view) and,
still before anything changes, plans the history change: the inverse is
`changes.invert(old text)`; then either a merge (below) or a new child of
the head holding `changes`, the inverse, the origin view's selection
before and after, and the clamped time. Only after the text is applied is
the plan applied. Identity change sets record nothing.

**Why correct.** `invert` and `compose` can fail only on length or
boundary mismatches, which the just-validated change set and the chain
of node lengths rule out (`debug_assert`, then an error with nothing
changed). Stored selections are valid for the texts they are restored
onto: `before` was valid for the parent's text and `after` for the
node's, and undo and redo reproduce exactly those texts. Commit still
validates them, so a bug returns an error instead of corrupting state.

**Cost.** One `invert` (O(ops · log n + deleted bytes)) and one clone of
the change set per new node; per merge, two compositions,
O(ops of the group + inserted bytes).

### Grouping

A typing commit (`Record::typing`) merges into the head when the open
group is its view's and the pause is at most the group pause:
`forward = compose(forward, c)`, `inverse = compose(c⁻¹, inverse)`. The
inverse composes in this order because `c⁻¹` runs first (it turns the new
text into the old head text) and the old inverse then turns that into
the parent's. The group is open only after a typing commit and is closed
by every other kind of commit, a selection change, a checkpoint, undo,
redo, a jump, or closing the view. Every history move closes it, so the
head is the newest node whenever a merge happens (`debug_assert`), and a
merge never rewrites a node that has children.

### Undo, redo, jumps

Undo commits `(inverse, before)` and moves the head to the parent; redo
commits `(forward, after)` of `last_child`. `goto` finds the path by
depth (climb the deeper side, then both sides until they meet), composes
inverses up and forwards down into one change set, and commits it once.
The invoking view gets `after` of the target if the path ends going
down, else `before` of the last node undone: what the same sequence of
single steps would give. `last_child` is set on every node the path
moves down into; moving up needs nothing, because every ancestor of the
head already points toward it (the property's model sets it explicitly
on every move and agrees). Cost O(path length + Σ ops along it).

### Time

`Timestamp` is an input. `Editor::handle_at(event, now)`; `handle` keeps
the previous time. The binary passes the session clock that it also
records as `t_ns`; `trace::replay` passes `t_ns`. Node times are clamped
to be non-decreasing in creation order.

### Node storage

Nodes live in 256-node chunks, so a push never moves existing nodes. The
first implementation used one `Vec<Node>`; the history bench showed
1-2 ms keystrokes exactly when the node count crossed a power of two
(8,192, 16,384, 65,536 nodes: 0.72, 0.66, 2.17 ms), growing linearly
with history. With chunks the worst event over 100,000 steps is about
0.1 ms.

## D. Invariants and how they are tested

| Invariant | Tests |
|---|---|
| Undo all / redo all reproduces every intermediate text exactly (Unicode replacements, multi-cursor, CRLF) | `undo_all_then_redo_all_is_exact`, `undo_restores_multi_cursor_selection`, the snapshot property |
| Composed (grouped) change sets undo and redo exactly | `history_matches_snapshot_model` (the model stores texts, never composes), `grouped_typing_matches_ungrouped`, `grouping_includes_deletions`, `a_group_that_cancels_out_still_undoes` |
| Tree shape: parent, `last_child`, head, branch on edit-after-undo, redo follows the branch last visited | `edit_after_undo_branches_and_redo_follows_last_visited`, the property's `check_tree` after every step |
| Invoking view gets the stored selection, others are mapped through the committed change set | `undo_maps_other_views_through_the_inverse`, `goto_an_ancestor_restores_the_selection_undo_would`, the property (every view after every step) |
| Undo, redo, and jumps bump the revision exactly on effective changes and enter the log | `undo_and_redo_one_edit`, the property (revision delta, newest log entry equals the committed change set, the log replays the session) |
| A failed commit, an unknown node, or an unknown view changes nothing | `goto_rejects_unknown_nodes_and_views`, `failed_commit_leaves_history_alone` |
| Grouping and its boundaries; times never decrease | `typing_groups_into_one_node`, `each_boundary_ends_a_group`, `pause_exactly_at_the_limit_still_groups`, `identity_commits_record_nothing`, `removing_the_typing_view_ends_its_group`, `times_never_decrease_and_lookups_use_them` |
| Editor keys and policy | `undo_and_redo_keys`, `undo_at_the_ends_reports`, `a_typed_word_is_one_undo_step`, `every_motion_key_ends_the_group_even_without_moving`, `a_pause_ends_the_group`, `a_paste_is_its_own_undo_step`, `editing_keys_group_together`, `handle_without_a_time_keeps_the_last_one`, `history_api_on_the_editor`, `undo_scrolls_the_focused_view_to_the_restored_cursor`, `undo_from_another_view_maps_the_typing_view` |
| `handle` never panics, now with undo keys and timed events | `random_events_never_panic` |
| Replay reproduces the session's grouping | `replay_reproduces_session` (session-like and arbitrary times; compares history length and head) |
| Chunked storage | `histories_longer_than_one_storage_chunk` |

Every property runs with default and tiny (`small-chunks`) leaves.

Totals: `flash-core` 213 (180 in Stage 4) tests, `flash` 10 library + 4 CLI tests.

## E. Commands and results

All with `--offline`, at the closing commit:

Gate run (2026-11-16, after the review fixes; every step passed):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings` (plain and
  `--all-features`): clean.
- `cargo test --workspace`: flash-core 213 passed; flash 10 + 4 passed.
- `cargo test -p flash-core --features small-chunks`: 213 passed (164 s).
- `PROPTEST_CASES=4096 cargo test -p flash-core --release`: 213 passed
  (65 s); with `--features small-chunks`: 213 passed (224 s).
- `cargo check` in `crates/flash-core/fuzz`: builds (re-checked after
  touching a core file, so not a cached result).

Benchmarks (results in `bench/results/stage-5-*`; the JSON's `commit`
field names the commit the run started from, with the chunked storage of
`3792c7a` already in the tree):

- `cargo bench -p flash-core --bench document -- --save-baseline stage-5`
  → `stage-5-criterion.json` via `bench/collect.py stage-5`. The 100 MB
  rows were noisy at the end of the full run (an 8-view row faster than the
  1-view one, a 38–56 ms interval); those rows were re-run in isolation
  with `--save-baseline stage-5` and the isolated numbers are the ones
  stored and reported.
- `FLASH_BENCH_JSON=$PWD/bench/results/stage-5-latency.jsonl cargo bench -p
  flash-core --features internals --bench latency`
- `FLASH_BENCH_JSON=$PWD/bench/results/stage-5-skeleton.jsonl cargo bench -p
  flash --bench skeleton`
- `FLASH_BENCH_JSON=$PWD/bench/results/stage-5-history.jsonl cargo bench -p
  flash-core --bench history`

## F. Benchmark results

Machine: Intel Core i7-1355U (12 threads), Linux 6.18.63, rustc 1.99.1,
bench profile (release + debug symbols), laptop on default power settings,
not isolated: treat differences under ~10% as noise.

**Standing benchmark** (Criterion means; commit now records history):

| fixture | origin | views | commit, Stage 4 | commit, Stage 5 | undo, Stage 5 |
|---|---|---|---|---|---|
| medium 1 MB | 1 cursor | 1 | 0.98 µs | 1.16 µs | 0.80 µs |
| medium 1 MB | 1 cursor | 2 | 110 µs | 104 µs | 101 µs |
| medium 1 MB | 1 cursor | 8 | 1.21 ms | 1.27 ms | 0.71 ms |
| medium 1 MB | 10k cursors | 1 | 2.70 ms | 3.41 ms | 2.94 ms |
| medium 1 MB | 10k cursors | 2 | 4.86 ms | 3.62 ms | 3.10 ms |
| medium 1 MB | 10k cursors | 8 | 6.54 ms | 5.25 ms | 4.91 ms |
| large 100 MB | 1 cursor | 1 | 1.67 µs | 1.70 µs | 1.32 µs |
| large 100 MB | 1 cursor | 2 | 145 µs | 181 µs | 107 µs |
| large 100 MB | 1 cursor | 8 | 1.17 ms | 1.19 ms | 0.75 ms |
| large 100 MB | 10k cursors | 1 | 19.5 ms | 22.1 ms | 20.4 ms |
| large 100 MB | 10k cursors | 2 | 19.7 ms | 22.3 ms | 20.5 ms |
| large 100 MB | 10k cursors | 8 | 21.3 ms | 24.3 ms | 23.0 ms |

History costs about 0.2 µs on a one-cursor commit and 13–26% on a
10,000-cursor commit (inverting and cloning about 20,000 ops). The Stage 4
medium 10k-cursor 2- and 8-view numbers were higher than this run's;
nothing in Stage 5 makes bystander mapping cheaper, so read that as
run-to-run spread. Undo costs about the same as the edit it undoes (one
commit of the stored change set). On 100 MB a 10k-cursor edit or undo is
more than one 16 ms frame, as in Stages 3 and 4.

**Per-operation latency**, 100k samples, ns (p50 / p99; allocations per op):

| op | small | medium 1 MB | large 100 MB | one-line 1 MB | unicode 1 MB |
|---|---|---|---|---|---|
| doc commit insert char (1 view), Stage 4 | 555 / 755 | 1,551 / 2,207 | 2,402 / 3,707 | 1,637 / 1,957 | 1,175 / 1,941 |
| doc commit insert char (1 view), Stage 5 | 700 / 882 | 1,774 / 2,092 | 2,549 / 2,930 | 1,780 / 2,130 | 1,798 / 2,931 |
| editor handle key 'x' | 710 / 902 | 1,788 / 2,130 | 2,537 / 2,890 | 1,796 / 2,148 | 1,772 / 3,062 |
| editor key into open group (100) | 681 / 907 | 1,027 / 1,216 | 1,241 / 1,360 | 984 / 1,132 | 1,129 / 1,741 |
| undo char | 251 / 426 | 887 / 1,200 | 1,183 / 1,656 | 893 / 1,221 | 819 / 1,305 |
| redo char | 216 / 531 | 599 / 1,357 | 754 / 1,742 | 595 / 1,340 | 609 / 1,543 |
| editor handle Right | 193 / 248 | 461 / 647 | 603 / 774 | 453 / 611 | 705 / 1,199 |

Recording adds about 150–250 ns at p50 and 3–4 allocations per commit
(the inverse, the change-set clone for the node, the node's chunk
amortised); one-cursor commit on 1 MB stays at **2.1 µs p99** against the
PLAN's "well under 100 µs". Typing into an open group is cheaper than a
new node (no clone; two small compositions). `handle Right` still
allocates nothing. The "into open group" row reports about 12.7 KB allocated
per op against 6.6 KB for a new node. Each sample starts from a fresh
clone of the editor, so copy-on-write of shared storage is included; the
difference was not broken down further (unverified).

**Skeleton end to end** (Stage 4 trace pattern, now with 100 ms between
events so edits group), ns:

| fixture | 80×24 p50 / p99 | 200×60 p50 / p99 |
|---|---|---|
| small 4 KB | 24,278 / 48,186 | 50,361 / 77,981 |
| medium 1 MB | 40,549 / 102,587 | 72,368 / 134,001 |
| unicode 1 MB | 38,901 / 118,053 | 70,745 / 149,444 |
| one-line 1 MB | 11,284 / 26.0 ms | 24,962 / 26.1 ms |

Within noise of Stage 4 (medium 80×24 p99 99.1 → 102.6 µs). The one-line
tail is still the status column (Stage 3 H).

**History memory** (`benches/history.rs`, medium fixture; synthetic
sessions: words at 80 ms per key, Enter every 12 words, 1 key in 20 a
Backspace and 1 in 40 an arrow; "grouped" pauses 1.5 s every 8 words;
"every key a step" spaces keys 2 s apart):

| scenario | nodes | history bytes | bytes / node | live heap added | p50 / p99 / max per event |
|---|---|---|---|---|---|
| 1 cursor, grouped, 100k keys | 5,880 | 3.1 MB | 521 | 4.2 MB | 557 ns / 1.4 µs / 49 µs |
| 1 cursor, grouped, 1M keys | 59,867 | 31.7 MB | 530 | 41.2 MB | 604 ns / 1.3 µs / 327 µs |
| 1 cursor, every key a step, 100k keys | 95,950 | 36.8 MB | 384 | 38.0 MB | 476 ns / 1.6 µs / 13 µs |
| 100 cursors, every key a step, 10k keys | 9,532 | 172 MB | 18,079 | 179 MB | 41 µs / 125 µs / 317 µs |
| 10,000 cursors, every key a step, 200 keys | 185 | 372 MB | 2.0 MB | 401 MB | 4.5 ms / 8.3 ms / 8.4 ms |

"Live heap added" also counts the revision log (bounded at 1,024
entries). Per-event cost does not grow with history. Before chunked
storage, the same runs showed maxima of 1.9 ms (100k grouped) and 2.2 ms
(100k steps), at power-of-two node counts. Cap decision: none
(DECISIONS 019).

## G. Review

**Independent review: not done by a fresh reviewer** (owner's standing
rule: no delegation without an explicit request). The implementer
reviewed the whole branch diff as a separate pass. This is weaker than
PLAN section 3 requires; **the owner should decide whether to run an
independent review before approving.**

Findings:

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | Important | Node storage in one `Vec` copied the whole history when it doubled: a keystroke cost O(history) (2.17 ms at 65,536 nodes; about one frame near 500,000). Found by the history bench. | Fixed: 256-node chunks (`Nodes`); worst event 0.1 ms over 100,000 steps; `histories_longer_than_one_storage_chunk`. |
| 2 | Minor (accepted) | A far-future time (e.g. `t_ns` near `u64::MAX` in a hostile trace) clamps every later time to it, so later typing never ends a group by pause. Nothing breaks; groups only get longer, and motions still end them. | Pinned by `extreme_ids_steps_and_times_do_not_panic`. |
| 3 | Minor (deferred) | The group pause is not in the trace header, so a replay with a different pause would group differently. | Today the pause is a constant; record it in the header when Stage 7 makes it an option (DECISIONS 018). |
| 4 | Minor (deferred) | Ctrl+Z is undo here and suspend in PLAN Stage 6. | Stage 6 must move one of them; Stage 7 makes both rebindable (`editor/keys.rs` docs). |
| 5 | Minor (accepted) | An identity typing commit that moves the cursor (none of today's keys do) is not reflected in the group's `after` selection, so redo restores the selection after the last effective edit. | Accepted; matches the ungrouped behaviour, and `grouped_typing_matches_ungrouped` checks against it. |

Attack tests added and kept, which found nothing beyond 2:
`extreme_ids_steps_and_times_do_not_panic` (node `usize::MAX`, step counts
`usize::MAX`, `Duration::MAX`, a zero pause, `u64::MAX` times). Also
checked by reading: no underflow in the pause check (the head's time is at
most the latest, which is at most the clamped time), every
`unreachable_algebra` path is preceded by validation, a closed view's
group cannot be resumed by a reused slot (`ViewId` is generational and
`remove_view` closes its group).

Mutation checks (each made the code wrong, then was reverted):

| Mutation | Result |
|---|---|
| Merge ignores the view / ignores the pause / no time clamp / `set_selection` keeps the group open | caught by `history_tests` |
| Merge keeps the old inverse / keeps the old `after` | caught by both properties |
| Jump does not set `last_child` going down / walks the down path in reverse | caught by the snapshot property |
| Undo restores `after` instead of `before` | caught by both properties |
| Motions do not checkpoint / paste groups / replay ignores `t_ns` | caught by editor and trace tests |
| Jump to an ancestor restores the target's `after` | **survived**; added `goto_an_ancestor_restores_the_selection_undo_would`, now caught |
| Undo does not set `last_child` going up | **survived**: redundant by the invariant that ancestors of the head point toward it; the code was removed and the invariant documented (the model sets it explicitly and agrees) |
| Merge into a head that is not the newest node | **survived**: unreachable because every move closes the group; now a `debug_assert` |

The mutation runs left proptest regression seeds
(`proptest-regressions/document/history_proptests.txt`,
`proptest-regressions/trace_tests.txt`), committed as the project does;
they pass on the real code.

Implementer calls made without the owner (cost if wrong in brackets):

- Time is an input (`handle_at`), not a clock in core [an API change in
  `Editor`; cheap to revisit before Stage 6].
- Default group pause 1 s [a constant; Stage 7 option].
- Paste is its own undo step; `Editor::commit` too [one flag per call
  site].
- Every motion key and Esc end a group even when nothing moves [one line
  in `reselect`].
- Ctrl+Z undo, Ctrl+Y and Ctrl+Shift+Z redo, clashing with the Stage 6
  suspend key [temporary keys].
- A jump is one composed commit (one revision, atomic), not one commit per
  step [bystanders map through the composition].
- `earlier`/`later` step through creation order (Vim's `:earlier N`);
  `at_time` finds a node by time [lookup helpers only].
- No children list per node (`children` scans) [add a field if a history
  UI needs it].
- No history cap (DECISIONS 019).
- Stage 4's approval status is left for the owner to record; this branch
  sits on top of `stage-4`.

## H. Known limitations

- History is unbounded: large ungrouped multi-cursor edits cost about 2 MB
  per step at 10,000 cursors (DECISIONS 019).
- Mode changes are not boundaries yet because there are no modes; Stage 7's
  mode layer must call `checkpoint` on a mode change (PLAN Stage 5).
- A frontend that calls only `handle` never ends groups by pause.
- The group pause is not recorded in traces (G3); hostile far-future times
  lengthen groups (G2).
- Mapping bystanders through a jump uses the composed change set, which can
  place a bystander cursor differently from undoing step by step
  (DECISIONS 009); the result is always valid.
- History node IDs are indices; pruning (if ever added) needs stable IDs.
- Stage 4 limitations stand (traces store the path as given; one-line
  status column; `LineEnding::detect` at open).

## I. Questions for the reviewer

1. Is there a sequence of edits, undos, redos, and jumps after which the
   text differs from the snapshot of the head node, or some view's
   selection is invalid?
2. Can a merge, undo, redo, or jump fail halfway and leave text, history,
   revision, log, and selections inconsistent?
3. Can redo, after any sequence, go somewhere other than the branch last
   visited?
4. Can replaying a trace group undo steps differently from the recorded
   session?
5. Can any public method or `Editor::handle_at` panic for any input?

## J. Next stage

Stage 6: minimal usable terminal editor (main loop, terminal lifecycle,
rendering, loading and saving). It must settle the Ctrl+Z conflict (G4)
and keep passing the session clock to `handle_at`. Not started; waiting
for approval of Stages 4 and 5.
