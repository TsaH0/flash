# Flash decision log

Only real decisions: something chosen over a credible alternative. Owner-level
product decisions (D1–D4) are recorded in [PLAN.md section 8](PLAN.md#8-decisions-needed-from-the-owner).

---

## 001 — Text lives in `flash-core`, mutators are `pub(crate)`

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** `Text` is a module of `flash-core`, not a separate `flash-text`
  crate. `insert`/`delete`/`replace` are `pub(crate)`; benchmarks reach them
  through the `internals` feature.
- **Alternatives:** a separate `flash-text` crate with public mutators.
- **Why:** "only transactions mutate text" is then enforced by the compiler,
  not by convention.
- **Trade-off:** benchmarks need a feature flag; editor code must never enable it.
- **Reopen if:** another crate genuinely needs raw text storage without the
  rest of the core.

## 002 — crop pinned at `=0.4.3` with `graphemes`

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** exact version pin.
- **Why:** `Text`'s line model and validation depend on documented details of
  this version (panic conditions, `line_len` and raw line metric semantics).
  An upgrade must re-run the reference-model property tests deliberately.
- **Reopen if:** a crop release fixes something we need (e.g. an O(log n)
  grapheme boundary or a chunk-at-offset accessor).
- **Amended by 008:** the pinned 0.4.3 is now a vendored copy with one added
  method; the version and every other line of source are unchanged.

## 003 — Line model: cursor rows

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** `line_count = '\n' count + 1`; `"\n"` and `"\r\n"` are
  terminators; a lone `'\r'` is content; the last line never has a terminator;
  text is stored byte-for-byte, never normalised.
- **Alternatives:** crop's `line_len()` (`"a\n"` has 1 line), which has no row
  for a cursor after a final newline; treating lone CR as a terminator
  (classic Mac), which almost no current tooling does and which crop's line
  metric does not count.
- **Evidence:** unit tests of every case in `text/tests.rs`; reference-model
  property tests.
- **Reopen if:** LSP or Tree-sitter integration needs a different row
  definition (both use `\n`-based rows, so this is not expected).

## 004 — Own grapheme-boundary check instead of crop's

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** `Text::is_grapheme_boundary` drives
  `unicode_segmentation::GraphemeCursor` over rope slices.
- **Evidence:** crop 0.4.3's `Rope::is_grapheme_boundary` iterates chunks
  backwards from the end of the rope to the offset (source:
  `crop-0.4.3/src/rope/utils.rs`, with a TODO acknowledging it), so its cost is
  proportional to the distance from the offset to the end of the document.
  See `grapheme_boundary_at_start` in the Stage 1 benchmarks.
- **Trade-off:** a direct dependency on `unicode-segmentation` (already in the
  tree through crop, same version).
- **Superseded in part by 008:** the cursor is now fed chunks from
  `Rope::chunk_at_byte` instead of slice iterators, which removed the ~4
  allocations per call the first version made.

## 005 — Every offset argument must be a char boundary

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** all `Text` methods taking an offset, including pure queries
  such as `line_of_offset`, reject offsets that are out of bounds or inside a
  character, returning `TextError`. Boolean predicates (`is_char_boundary`,
  `is_grapheme_boundary`) return `false` instead.
- **Alternatives:** accept any in-bounds byte for queries.
- **Why:** one uniform rule catches offset bugs at the first call that sees
  them. The extra check is O(log n).

## 006 — Benchmark tooling

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** Criterion for means with confidence intervals; a custom
  harness (`benches/latency.rs`) that times each operation individually for
  p50/p95/p99/max and counts allocations with a counting global allocator.
  Fixtures are generated in memory from fixed seeds.
- **Alternatives:** Divan (has allocation counting, but no per-sample
  percentiles); checked-in fixture files (100 MB in git).
- **Reopen if:** Stage 4's trace-replay harness subsumes the latency harness.

## 007 — crop's `utf16-metric` stays off

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** do not enable the feature now. Stage 10 (LSP) decides between
  enabling it and converting positions per line on demand.
- **Evidence:** same machine, Criterion, compared against the saved `plain`
  baseline. Run-to-run noise on the plain build was within ±5% on edits and
  about ±10% on builds.

  | benchmark | change with `utf16-metric` |
  |---|---|
  | 1 MB edits (owned insert+delete, insert/delete/replace after snapshot) | +15% to +37% |
  | 100 MB edits | +9% to +22% |
  | build from `&str`, 4 KB / 1 MB / 1 MB one line | +55% / +44% / +42% |
  | build from `&str`, 100 MB | +11% |

- **Trade-off:** LSP servers that only speak UTF-16 will need a per-line
  conversion (cost proportional to the line length, not the document).
- **Reopen if:** most target language servers lack UTF-8 position encoding
  and per-line conversion shows up in Stage 10 latency measurements.

## 008 — Vendored crop with `Rope::chunk_at_byte`; counted grapheme motion

- **Date:** 2026-11-12 (Stage 1)
- **Decision:** `vendor/crop` is crop 0.4.3 from crates.io plus one public
  method, `Rope::chunk_at_byte(byte) -> Option<(&str, usize)>`, wired in with
  `[patch.crates-io]`. The complete diff is `vendor/crop-chunk-at-byte.patch`
  (22 added lines in `src/rope/rope.rs`; verified against the registry copy of
  0.4.3). `Text::is_grapheme_boundary` and the new `Text::move_graphemes`
  drive `GraphemeCursor` with chunks from it.
- **Why:** crop 0.4.3 has no way to borrow the chunk at an offset except
  through slice iterators, which allocate their traversal stack. The method
  is a single descent of the tree (`leaf_at_measure`), O(log n), no allocation.
- **Counted motion:** `move_graphemes(offset, count, direction)` reuses one
  cursor and chunk while the steps stay inside the chunk. When a step needs
  another chunk or earlier context, that step is redone from the last
  boundary with a fresh cursor, and the next step starts fresh. Carrying a
  cursor across chunks/context gives wrong offsets on regional-indicator
  runs: injecting that variant fails four tests, shrinking to `"a🇺🇸"`.
- **Evidence:** latency harness p50, same machine, before → after:

  | op | 4 KB | 1 MB | 100 MB | unicode 1 MB |
  |---|---|---|---|---|
  | `is_grapheme_boundary` | 246 → 42 ns | 643 → 101 ns | 1283 → 378 ns | 686 → 135 ns |
  | allocations per call | 0 → 0 | 3.9 → 0 | 4.0 → 0 | 3.9 → 0 |
  | `move_graphemes` ×16 forward / backward | 319 / 196 ns | 378 / 255 ns | 684 / 608 ns | 675 / 545 ns |

  One 16-step motion on 1 MB costs about three single-step calls (126 ns).
- **Alternatives:** keep crop's iterators (allocating); fork crop as a git
  dependency (network needed to build); bounded windows via `byte_slice`
  (still allocates).
- **Trade-off:** upgrading crop means re-applying the patch (or dropping it
  if upstream adds an equivalent) and re-running the reference-model tests.
- **Reopen if:** crop publishes a chunk-at-offset accessor.

## 009 — ChangeSet canonical form and position-mapping policy

- **Date:** 2026-11-14 (Stage 2; mapping rule revised after the adversarial
  review the same day)
- **Decision:** a `ChangeSet` is `Vec<Op>` (`Retain(usize)`, `Delete(usize)`,
  `Insert(CompactString)`) built only through `ChangeSetBuilder`, which keeps
  canonical form: no empty ops, no adjacent ops of one kind, and `Delete`
  before `Insert` within an edit group. Positions map by the policy in the
  `changeset` module docs: inside a `Retain` they shift. In the closed range
  `[o, o + d]` of an edit group, if `d > 0` the old edges map to the
  replacement's edges (`o` to its left edge, `o + d` to its right edge), and
  every other position (a pure insertion point, or strictly inside the
  deleted range) goes to the left edge with `Assoc::Before` and the right
  edge with `Assoc::After`.
- **Why:** canonical form makes structural equality meaningful
  (`invert(invert(c)) == c` is checked structurally) and gives apply and
  mapping one shape to handle. The edge rule keeps ranges whole: mapping a
  range with start `After` and end `Before` (PLAN Stage 3: a range does not
  absorb inserts at its edges) keeps a replacement that the range touches
  or covers, and never inverts the range.
- **Rejected:** the first version collapsed the whole closed range by
  `Assoc` alone. The adversarial review showed it drops a replacement from a
  range ending at it (`"abc"` over `abcd → aQd` became `"a"`) and inverts a
  range equal to the replaced range (`1..3` became `3..1`). Its stated reason
  for rejecting half-open groups (that a pure deletion would treat its two
  edges differently) was wrong: both edges map to the deletion point either
  way. Tests: `range_ending_at_replacement_end_keeps_the_replacement`,
  `range_equal_to_replaced_range_covers_the_replacement`.
- **Consequence (PLAN Stage 2 "Known trap"):** composition does not preserve
  positions. Example: on `"abc"`, `a` inserts `"Z"` at 1 and `b` deletes the
  old `"b"`; mapping 1 with `After` through `a` then `b` gives 2 (after `Z`),
  through `compose(a, b)` gives 1 (`compose_loses_positions_documented`).
  Position tracking maps through each change set in order.
- **Alternatives:** `Insert` before `Delete` in canonical form (equivalent,
  arbitrary).
- **Trade-off:** a cursor exactly at the start or end of a replaced range
  ignores its `Assoc`; Stage 3 cursors that should jump past a replacement
  must do so explicitly.
- **Reopen if:** Stage 3 selection mapping or Stage 8 markers need a
  different rule at replacement edges.

## 010 — ChangeSet application: per-op edits only

- **Date:** 2026-11-14 (Stage 2)
- **Decision:** `ChangeSet::apply` validates, then makes one rope edit per
  `(Delete, Insert)` group, back to front. The `RopeBuilder` rebuild strategy
  was implemented, measured, and removed.
- **Evidence:** Criterion `--quick`, spread edits replacing ≤16 bytes with
  `"x"`, applied to a snapshot-shared tree, same machine as Stage 1:

  | edits | 1 MB per-op | 1 MB rebuild | 100 MB per-op | 100 MB rebuild |
  |---|---|---|---|---|
  | 1 | 1.0 µs | 323 µs | 1.7 µs | 36 ms |
  | 100 | 77 µs | 201 µs | 80 µs | 22 ms |
  | 1,000 | 526 µs | 734 µs | 1.04 ms | 23 ms |
  | 3,000 | 845 µs | 1.14 ms | 3.8 ms | 23 ms |
  | 10,000 | 1.95 ms | 2.73 ms | 11.5 ms | 27 ms |
  | 30,000 | 3.9 ms | 5.6 ms | — | — |

  Per-op is faster at every count measured. Rebuilding costs a full
  traversal and re-chunking of the document (≥ 200 µs per MB), while
  per-op cost grows sub-linearly with edit count because nearby edits share
  copied tree paths.
- **Alternatives:** a size- and count-based switch to rebuild (PLAN Stage 2);
  apply to a clone and swap for atomicity (rejected: validation first gives
  atomicity without forcing path copies).
- **Trade-off:** none measured; one strategy less to test.
- **Reopen if:** a workload applies change sets with more than ~1 edit per
  30 bytes of document (e.g. whole-file reformatting) and per-op shows up in
  latency measurements; re-measure rebuild then.

## 011 — Selection normalisation and mapping

- **Date:** 2026-11-14 (Stage 3)
- **Decision:** `Selection` keeps its ranges sorted by `(start, end)` and
  merges by PLAN 2.5: overlapping ranges merge, touching non-empty ranges
  stay separate, an empty range touching or inside another merges into it.
  A merged range is the union of its group and takes its direction and goal
  column from the *kept* range: the primary if the group contains it,
  otherwise the group's first range in sorted order. The primary survives
  merging. `Selection::map` maps each range's start with `Assoc::After` and
  its end with `After` if the range is empty, `Before` otherwise, through
  the new `ChangeSet::map_sorted_with` (per-position `Assoc`, one pass). A
  range strictly inside a replaced span would come out inverted, so its
  endpoints are reordered and it covers the replacement. Mapping clears goal
  columns: after an edit, vertical motion starts from the current column.
- **Evidence:** `selection/tests.rs` (merge, direction, mapping cases),
  `selection/proptests.rs` (`normalise_invariants`, `map_matches_naive`
  against per-position `map_pos_naive`); mutating the endpoint `Assoc` rule
  makes three tests fail.
- **Alternatives:** the merged range always forward (loses a backward
  primary's orientation); direction from the last range in the group
  (Helix); mapping both endpoints with one `Assoc` (a cursor would either
  not advance when typing, or a selection would swallow text typed at its
  edge).
- **Trade-off:** none measured; the sort is skipped for already-sorted
  input, so normalising after a motion or edit is O(k).
- **Reopen if:** Stage 7 modes need a different merge direction, or markers
  (Stage 8) need different edge behaviour (they get their own `Assoc`).

## 012 — Display width, word classes, and grapheme iteration

- **Date:** 2026-11-14 (Stage 3)
- **Decision:**
  - *Width:* a grapheme takes `UnicodeWidthStr::width` cells (unicode-width
    0.2: CJK and ZWJ emoji 2, combining sequences 1, ZWSP 0, controls 1); a
    tab advances to the next multiple of the tab width. Renderers draw a
    control character as U+FFFD in its one cell, never raw.
  - *Word classes* by a grapheme's first char: whitespace (incl. `\n`,
    `\r\n`, `\t`), word (alphanumeric or `_`), punctuation. Forward word
    motion skips the head's run (unless whitespace) then whitespace;
    backward skips whitespace then the run before it.
  - *Vertical motion* keeps a goal display column on the range; on the
    target line it lands on the start of the first non-zero-width grapheme
    covering the goal, else the line's content end. Line index clamps.
  - *Grapheme iteration:* `Text::graphemes(offset, direction)` steps with
    the same cursor-reuse rule as `move_graphemes` (DECISIONS 008), which
    now uses it. crop's `RopeSlice::graphemes` is not used.
- **Evidence:** the motion property tests against a whole-string model
  found two bugs in crop 0.4.3's `Graphemes` iterator on texts whose leaf
  gap splits them into two chunks (default leaf size):
  1. Backward, after a `PrevChunk` request, it computes the new chunk's
     start as `cursor.cur_cursor() - chunk.len()`, but the cursor has not
     moved, so offsets drift: "attempt to subtract with overflow" in
     `iterators.rs`, e.g. iterating `..33` of
     `"aa老虎老虎老虎é😀\u{200d}👨\u{200d}👩\u{200d}👧a"` backward.
  2. Forward, it carries one `GraphemeCursor` across chunks for the whole
     walk, and splits a regional-indicator pair that straddles the chunk
     boundary (`0..27` of `"🇯\u{200d}é🇯\r🇯\r🇺🇸\né🇺🇸老虎\r\n"` yields
     `"🇺", "🇸"`): the failure class already recorded in 008.
  `text/tests.rs::graphemes_match_reference_boundaries` pins both texts.
- **Alternatives:** patch crop's iterator (bug 1 is one line; bug 2 is
  the cursor-reuse design, so a second fork of crop's logic); iterate via
  repeated `move_graphemes(_, 1, _)` (an O(log n) descent per grapheme);
  char-based word classes (split combining marks from their base);
  `unicode-segmentation` word bounds (no word/punctuation distinction).
- **Trade-off:** one more iterator to maintain in `Text`, sharing its core
  with `move_graphemes`.
- **Reopen if:** crop fixes `Graphemes` upstream, or Stage 7 makes the word
  definition an option (PLAN 2.11).

## 013 — Transaction collision rules and atomic apply

- **Date:** 2026-11-14 (Stage 3)
- **Decision:**
  - Constructors (`insert`, `delete`, `replace`, `backspace`,
    `delete_forward`) walk the ranges once, emitting edits through one
    `ChangeSetBuilder` and computing each result range directly.
  - *Same-position inserts* (two touching ranges sharing a head): each range
    inserts its own copy, in selection order, and each head lands right after
    its own copy.
  - *Overlapping deletes* (backspace / delete forward, when a cursor sits
    inside a cluster or a cursor's grapheme reaches into a neighbouring
    range): overlapping deletions merge into their union, and their ranges
    collapse to its start. Touching deletions stay separate edits.
  - `Transaction::apply` validates first and then mutates, like
    `ChangeSet::apply` (010): the change set against the text, then either
    the transaction's selection against the *post-change* text, checked in
    one pass over the ops without building that text, or the current
    selection against the text before mapping it. Nothing is cloned.
  - `Transaction::apply` is `pub` until `Document::commit` lands (Stage 4),
    so the walking skeleton can drive it; then it becomes `pub(crate)`.
- **Evidence:** `transaction/tests.rs` (collision, Unicode, error, and
  atomicity cases); `transaction/proptests.rs` checks every intent against a
  naive model that merges deletions pairwise and applies edits one at a time
  back to front (PLAN Stage 3), selection validity after random
  motion/edit sequences, and that a failed apply changes nothing. Mutating
  the backward-anchor formula or the grapheme span each fail two properties.
- **Alternatives:** one shared copy for same-position inserts (two cursors
  typing at one spot would produce one character); rejecting overlapping
  deletes (backspace would fail for a cursor inside a cluster); apply to a
  clone and swap (rejected in 010: forces path copies).
- **Trade-off:** the post-change selection check is a second small walk over
  the ops; constructor-made selections are valid by construction, so it
  only guards hand-built transactions.
- **Reopen when:** Stage 5 history needs grouped or merged transactions.

## 014 — Documents own per-view selections; commit is the only text mutator

- **Date:** 2026-11-15 (Stage 4)
- **Decision:** `Document` holds its text, revision, revision log, path,
  line ending, and a `SecondaryMap<ViewId, Selection>` with the selection
  of every view showing it. `View` holds the document ID, scroll, and size.
  `Document::commit(view, Transaction)` is the only way text changes: it
  validates the change set and the origin view's new selection
  (`Transaction::prepare`), maps every other view's selection, and only
  then applies the text, stores the selections, increments the revision
  (unless the change set is the identity), and moves the change set into
  the log. `Transaction::apply` becomes `pub(crate)` (benchmarks reach it
  through `internals`).
- **Why:** the document can keep "every view's selection is valid for my
  text" as a private invariant, and commit needs no access to the editor's
  view table, so the all-or-nothing rule (DECISIONS 010, 013) covers every
  view in one function. A selection-only (identity) commit does not map
  other views, so their goal columns survive.
- **Alternatives:** selections on `View` as PLAN Stage 4 sketched, with
  commit on `Editor` (atomicity then spans two tables and every caller of
  commit needs the editor); `Document::commit` taking `&mut` views (every
  caller must split borrows; the document cannot own the invariant).
- **Evidence:** `document/tests.rs` (`commit_maps_other_views`,
  `failed_commit_changes_nothing`, `identity_commit_keeps_revision`);
  `views_stay_valid_under_commits` (random commits from 1–3 views against a
  String model and per-view `Selection::map`). Making commit skip mapping
  other views fails the property.
- **Trade-off:** a view's selection is looked up through its document
  (`editor.selection(view)` does both lookups).
- **Reopen if:** Stage 7 per-view state or Stage 8 markers need selections
  outside the document.

## 015 — Revision log bounds and stale-position mapping

- **Date:** 2026-11-15 (Stage 4)
- **Decision:** the log keeps `(revision, change set)` for consecutive
  revisions, bounded by 1,024 entries and 2^20 ops in total, evicting the
  oldest first and always keeping the newest. `map_from(from, positions,
  assoc)` maps sorted positions through each entry after `from` in order
  (never a composed change set, DECISIONS 009); `from` older than the log
  is `Evicted`, newer than the document is `Future`.
  `Document::map_from` also checks every result against the current text,
  since a position that was not a char boundary at `from` cannot be
  detected from the change sets alone; on any error the input is
  unchanged.
- **Why:** an entry bound alone is not a memory bound: one 10,000-cursor
  edit holds about 20,000 ops, so 1,024 of them could keep hundreds of
  megabytes. 2^20 ops is tens of megabytes at most.
- **Alternatives:** compose old entries into one (loses positions,
  DECISIONS 009); keep text snapshots per revision and diff (O(n));
  unbounded log (memory grows with session length).
- **Evidence:** `log_bounds_and_eviction`, `map_from_matches_sequential_map_pos`,
  `map_from_validates_against_text`, and the end of
  `views_stay_valid_under_commits` (log mapping equals chained `map_pos`).
- **Trade-off:** mapping cost grows with the number of revisions behind;
  a worker more than 1,024 edits behind must recompute.
- **Reopen if:** Stage 9/10 measurements show background results falling
  out of the log, or the bounds become options (PLAN 2.11).

## 016 — Traces: raw input events as JSON Lines

- **Date:** 2026-11-15 (Stage 4; event level chosen by the owner)
- **Decision:** a trace records *raw input* (key with modifiers, bracketed
  paste, resize) in core-defined, terminal-independent types
  (`flash_core::Event`); the temporary key map lives in the core editor
  (`editor/keys.rs`, deleted in Stage 7). The file is JSON Lines via
  serde_json: a header (`flash_trace` version, document source as a path or
  fixture name, byte length, FNV-1a 64 content hash, screen size, starting
  cursor), then one `{"t_ns", "event"}` per line. Replaying the events into
  an editor started from the header (`trace::start`, which checks length,
  hash, and cursor) reproduces the session exactly. Reading bounds every
  line (default 16 MiB) before allocating past it, returns errors with
  1-based line numbers, and stops at the first error. `flash` records with
  `FLASH_TRACE=<file>` and replays with `flash --replay <trace>`.
- **Why raw input (owner, option A):** traces then measure the whole input
  path, including key resolution once Stage 7 adds keymaps, so the
  standing latency benchmark does not under-measure. The cost is that a
  trace means something different after keys are rebound.
- **Why JSON Lines:** human-readable and diffable, appendable while
  recording, and serde_json was already in the dependency tree (through
  criterion). The header references the document instead of embedding it,
  so a trace of a large or private file stays small and holds only what
  was typed.
- **Alternatives:** command-level events (stable across rebinding, but skip
  key resolution); a hand-written line format (more parser code to fuzz);
  bincode (not human-readable, a new dependency).
- **Evidence:** `trace_tests.rs` (`round_trip` and
  `replay_reproduces_session` properties, `read_rejects_malformed`,
  `read_bounds_line_length`, `replay_rejects_bad_header`, `event_json_shape`);
  `apps/flash/tests/cli.rs`. Removing the line bound or the hash check
  each fails a test.
- **Trade-off:** `t_ns` is recorded but not used by replay until Stage 6
  replays coalescing; a recorded trace includes everything typed or
  pasted, so it is as sensitive as the session.
- **Reopen if:** Stage 7 needs traces that survive rebinding (add a
  command-level event kind alongside), or trace parsing shows up in the
  latency budget.

## 017 — History: a tree of change sets inside the document, moved through commit

- **Date:** 2026-11-16 (Stage 5)
- **Decision:** each `Document` owns a `History`: nodes in creation order
  (`NodeId`, root = the text as opened), each with forward and inverse
  change sets, parent, `last_child`, depth, the committing view's
  selection before and after, and a time. `Document::commit_with` records
  every non-identity commit (the inverse is computed against the old text
  before anything changes). `undo`, `redo`, and `goto` build a transaction
  from the stored change set and selection and commit it through the same
  path as an edit, then move the head instead of recording a node; so they
  bump the revision, enter the revision log, and map every other view (the
  invoking view gets the stored selection). `goto` composes the path (up
  to the common ancestor, then down) into one change set and commits it
  once: a jump is atomic and one revision. Redo follows `last_child`, the
  branch last created or moved down into (Vim's rule); every ancestor of
  the head points toward it, so moving up needs no bookkeeping. Nodes are
  stored in 256-node chunks.
- **Why:** PLAN 2.7 and Stage 5 require forward and inverse change sets and
  undo through commit. Keeping history in the document keeps atomicity in
  one function (DECISIONS 010, 014). Composing a jump makes it all-or-
  nothing; mapping bystanders through a composed change set differs from
  mapping through each step (DECISIONS 009) but both are valid, and a jump
  is one edit. Chunked storage: a `Vec<Node>` doubling copied the whole
  history on one keystroke (measured max 2.17 ms at 65,536 nodes, linear
  in nodes); with chunks the worst event over 100,000 steps is 114 µs.
- **Alternatives:** text snapshots per node (O(1) undo with shared ropes,
  but no change set to log or map views through); linear undo/redo stacks
  (lose branches); a jump as one commit per step (not atomic, many
  revisions); storing children lists (nothing needs them yet; `children`
  scans).
- **Evidence:** `history_tests.rs` (17 cases plus chunk boundaries);
  `history_matches_snapshot_model` (random walks against full text
  snapshots per node; checks text, tree, stored selections, revisions, the
  log, and every view's selection after every step);
  `grouped_typing_matches_ungrouped`. Mutation checks in the Stage 5
  report.
- **Trade-off:** history memory grows with the session (DECISIONS 019).
- **Reopen if:** persistent undo, history pruning, or a history UI needs
  children or stable IDs across pruning.

## 018 — Undo grouping: time as input, typing merges by composition

- **Date:** 2026-11-16 (Stage 5)
- **Decision:** time is an input, `Timestamp` nanoseconds; the editor never
  reads a clock. `Editor::handle_at(event, now)` (and `handle`, which keeps
  the last time); the binary passes its session clock, which it also
  records as the trace's `t_ns`, and `trace::replay` passes each `t_ns`.
  A commit recorded with `Record { group: true }` merges into the head
  node when the open group is the same view's and the pause since the
  node's last edit is at most the group pause (default 1 s): `forward =
  compose(forward, c)`, `inverse = compose(c⁻¹, inverse)`, `after` and
  `time` replaced. The group closes on a commit without `group`, any
  selection change, `checkpoint`, undo, redo, a jump, or closing the
  view. Typing keys (characters, Enter, Tab, Backspace, Delete) group;
  paste and `Editor::commit` do not; every motion and Esc close the group
  even when nothing moves. Node times are clamped so they never decrease
  (`t_ns` is not monotonic), which makes `at_time` a binary search.
- **Why:** deterministic replay is what makes a trace a test (DECISIONS
  016); a wall clock inside the core would make grouping differ on replay.
  Composition keeps a group's change sets the size of its distinct edit
  positions, not its keystrokes.
- **Alternatives:** wall clock in `Document` (non-deterministic replay);
  grouping by keystroke count or word boundaries (not what PLAN names);
  putting time on `Event` (changes the trace event format; `t_ns` already
  carries it).
- **Evidence:** `typing_groups_into_one_node`, `each_boundary_ends_a_group`,
  `pause_exactly_at_the_limit_still_groups`, the editor key tests,
  `replay_reproduces_session` (now with session-like times; comparing
  history length and head). Making replay ignore `t_ns` fails it.
- **Trade-off:** a frontend that calls `handle` without ever calling
  `handle_at` never ends a group by pause. The group pause is not in the
  trace header, so replay assumes the default until Stage 7 options.
- **Reopen if:** Stage 7 options make the pause configurable (record it in
  the trace), or users want word-level undo steps.

## 019 — No history cap yet

- **Date:** 2026-11-16 (Stage 5)
- **Decision:** history is unbounded. PLAN Stage 5 asks for a cap with
  oldest-leaf pruning only if the measurement asks for one; for ordinary
  editing it does not.
- **Evidence** (`benches/history.rs`, medium fixture, chunked storage,
  `History::heap_bytes`): one cursor, typing grouped by sentence pauses:
  about 520–530 bytes per node, 31.7 MB after 1,000,000 keystrokes
  (≈ 32 bytes per keystroke). One cursor, every key its own step (keys
  2 s apart): 384 bytes per node, 36.8 MB per 100,000 keystrokes. 100
  cursors, every key a step: 18 KB per node. 10,000 cursors, every key a
  step: 2.0 MB per node (two change sets of about 20,000 ops and two
  10,000-range selections), so a few hundred separate 10,000-cursor edits
  reach hundreds of megabytes.
- **Why not now:** the single-cursor numbers are far inside the memory
  budget (PLAN 0); only many ungrouped huge multi-cursor edits are large,
  and grouped typing with any number of cursors stays one node per group
  (composition keeps one op pair per cursor). Oldest-leaf pruning would
  not help a linear history anyway (its only leaf is the head): a useful
  cap must trim from the root, which needs stable IDs across trimming.
- **Alternatives:** a node-count cap like Vim's `undolevels` (bounds
  count, not bytes: one 10k-cursor node is 4,000 single-cursor nodes); a
  byte budget with root trimming (the likely design when needed).
- **Trade-off:** a long session of large multi-cursor edits holds its
  whole history in memory.
- **Reopen if:** Stage 7's option registry adds an undo budget, a real
  session's history exceeds about 100 MB, or Stage 8 markers make nodes
  larger.

## 020 — The core performs no I/O: commands queue requests

- **Date:** 2026-11-16 (Stage 6)
- **Decision:** `Editor::handle` never touches the disk, the clipboard, or
  the terminal. Save, open, the system clipboard (OSC 52), and suspend
  queue a `Request`; `Effects::requests` flags them; the frontend runs
  them (`apps/flash/src/requests.rs`) and reports back with
  `Editor::saved` / `Editor::opened`. The load and save algorithms are in
  `flash_core::file` (std only, no terminal), called by the frontend. The
  prompt that asks for paths and confirmations is core state.
- **Why:** replay must be deterministic and harmless: a replayed trace
  drops requests, so it can never write a file; the core stays testable
  without a disk; the prompt is in traces like any other input.
- **Alternatives:** a storage trait with real and in-memory backends
  (replay would need a fake disk to be harmless, and `handle` would
  block on I/O); I/O directly in `handle` (replay could overwrite files).
- **Evidence:** `editor/commands_tests.rs`, `apps/flash/src/requests_tests.rs`,
  `replay_prints_latency_summary` (the document is unchanged after a
  replay), `a_recorded_session_replays` (pty).
- **Trade-off:** a save's outcome arrives as a separate call, so the
  editor holds no "saving" state; replays do not reproduce save outcomes.
- **Reopen if:** saving or loading moves off the main thread (Stage 9
  workers), which fits this shape.

## 021 — File policy: strict loading, checked atomic saving

- **Date:** 2026-11-16 (Stage 6)
- **Decision:** loading streams 64 KiB reads into `TextBuilder`,
  validating UTF-8 across read boundaries; invalid UTF-8 and non-regular
  files are refused with a message (no lossy conversion); a BOM is
  stripped and written back; line endings are counted while streaming; a
  missing file opens empty. Saving resolves symlinks, refuses on a
  conflict (size or mtime differ, else the content hash differs; a file
  that appeared or vanished also counts) unless forced, then writes a
  temp file in the target's directory, fsyncs it, copies permissions and
  ownership, renames it over the target, and fsyncs the directory (best
  effort). With more than one hard link, or ownership that cannot be
  kept, it writes in place behind a fsynced backup copy and says so. A
  read-only directory is an error, not a fallback. Every failure before
  the rename removes the temp file and leaves the original intact.
- **Why:** PLAN Stage 6; no silent data loss (PLAN 6).
- **Alternatives:** lossy loading (silent corruption on save); always in
  place (a crash mid-write loses the file); always atomic (splits hard
  links, changes ownership); falling back to in place for read-only
  directories (needs the backup in that directory too).
- **Evidence:** `file/tests.rs` (19 cases, including an injected failure
  at every phase); mutation checks in the Stage 6 report. Measured: load
  ≈ 450 MB/s; an atomic, checked save of 1 MB on ext4 ≈ 4 ms.
- **Trade-off:** the conflict check reads the old file when size and
  mtime match (O(n)); FNV-1a is a change detector, not a cryptographic
  hash. The ownership path itself (`fchown`) is not exercised by tests
  (needs root or a second group).
- **Reopen if:** large-file mode, file watching, or crash recovery
  (journal) arrive.

## 022 — Ctrl+Z suspends; undo moves to Alt+Z

- **Date:** 2026-11-16 (Stage 6; owner decision)
- **Decision:** Ctrl+Z suspends (and an external SIGTSTP does the same):
  restore the terminal, stop, re-enter and redraw on continue. Undo is
  Alt+Z, redo Alt+Y and Alt+Shift+Z. Supersedes the Stage 5 temporary
  Ctrl+Z / Ctrl+Y.
- **Why:** PLAN Stage 6 and terminal convention (vim, less, helix); the
  modal keymap of Stage 7 will put undo on its own keys anyway.
- **Reopen if:** Stage 7's default keymap changes either (both are
  rebindable then).

## 023 — Renderer: full redraw, synchronized, with re-sync and a line anchor

- **Date:** 2026-11-16 (Stage 6)
- **Decision:** each frame is a full redraw of the focused view into a
  reused buffer between synchronized-update escapes, written once.
  Graphemes are measured with `unicode-width` per cluster; after a
  cluster whose per-character width differs (ZWJ emoji), the cursor is
  re-positioned so later columns are right on terminals that do not join
  clusters. When a view scrolls, the editor records a `LineAnchor` (the
  head's column, and a grapheme boundary at or before the left column,
  found by walking back at most one view width) that the renderer reuses
  while revision, head, and tab width match, instead of walking the line
  again.
- **Why:** the full redraw is the simple baseline Stage 7 benchmarks
  against; the anchor took the 1 MB single-line fixture from 78 ms back
  to 26 ms per event (one walk instead of three).
- **Evidence:** `render/tests.rs` (a small terminal emulator in both
  cluster models), `random_events_render_cleanly` (300,000 events in a
  one-off run; frames with and without anchors identical; found two
  anchor bugs with zero-width graphemes, fixed),
  `anchors_match_walking_from_the_line_start`.
- **Trade-off:** a tab between the head and the left column disables
  the left-edge part of the anchor (the renderer then walks that line).
  The head's column no longer walks the line: see 025.
- **Reopen if:** Stage 7's renderer work (dirty lines, per-line layout
  cache) replaces it.
- **Superseded in part by 040:** the line anchor is gone; the layout
  cache's marks give where a line's visible part starts.

## 024 — Main loop: threads, one channel, 8 ms drain, signals

- **Date:** 2026-11-16 (Stage 6)
- **Decision:** an input thread (`crossterm::event::read`) and a signal
  thread (`signal-hook`: SIGTSTP, SIGCONT, SIGTERM, SIGHUP, SIGINT) send
  to one `mpsc` channel. The loop blocks for one message, drains queued
  messages for up to 8 ms, runs requests after each, and renders once.
  SIGTERM/SIGHUP/SIGINT restore the terminal and exit with 128 + signal
  (no save). The editor refuses to start unless stdin and stdout are
  terminals.
- **Why:** PLAN 2.8; the drain coalesces bursts (pastes, held keys)
  into one frame; refusing without a terminal keeps crossterm from
  grabbing `/dev/tty`.
- **Evidence:** `apps/flash/tests/pty.rs`; interactive latency through a
  pty (Stage 6 report F).
- **Trade-off:** a terminating signal loses unsaved changes (this is not
  crash recovery).
- **Reopen if:** worker results (Stage 9) need priority over input.

## 025 — The head's column from checkpoints, not from the line start

- **Date:** 2026-11-16 (Stage 6 review)
- **Decision:** the editor keeps the primary head's display column
  incrementally (`editor/column.rs`). Each view's `LineAnchor` holds up
  to three checkpoints (a grapheme boundary and its column): the head,
  the left edge of the visible part, and a *rear* checkpoint 1–2 KB
  behind the head on long lines. After any change, a checkpoint is
  reused only if it was a grapheme boundary when recorded and still is,
  it is on the head's line, and the revision log shows that no change
  since touched its line's prefix (`RevisionLog::map_untouched`: no
  insert at or inside it, no delete overlapping it or ending at its
  start). The column is a walk from the nearest valid checkpoint:
  forward, or backward over non-tab graphemes only. The left-edge
  checkpoint is the line start when the view is not scrolled sideways,
  else a forward walk from the previous one when the view scrolled by
  less than its width, else a walk back from the head.
- **Why:** the review found that Stage 6 walked the head's line from its
  start on every event: on the 1 MB `one-line` fixture a key took
  milliseconds in the core (Stage 5: about 2 µs), and the core latency
  bench (100,000 samples per operation) could not finish. PLAN 4 asks
  for `one-line` to be slow but bounded, not slow on every key.
- **Evidence:** `random_events_never_panic` now checks every current
  anchor against a walk from the line start, with `REAR` = 16 bytes in
  tests so short lines use the rear checkpoint; it found two bugs before
  this was committed: a head inside a cluster used as a checkpoint after
  an edit made it a boundary, and lines dense with tabs falling back to
  the line start on every Backspace. A scaled one-off run (300,000
  events) passed. `local_edits_on_a_long_line_do_not_walk_it` bounds the
  graphemes walked per key; `checkpoints_on_another_line_are_ignored`;
  `map_untouched` unit and property tests. Mutation checks: removing the
  boundary, tab, or same-line guards, or loosening either touch rule,
  fails a test.
- **Trade-off:** a jump along a long line (Home/End, a click, a search
  result, or the benchmark's setup) still walks from the nearest
  checkpoint or the line start: O(distance). An edit at least 1 KB
  behind the head, or one that touches every checkpoint (undoing a large
  group), walks from the line start once. The bookkeeping (line start,
  boundary checks, the log check) costs about 0.2–1 µs per key over
  Stage 5 on `small` and `medium` (stage report F).
- **Reopen if:** a per-line layout cache (Stage 7 or soft wrap) can
  answer columns directly.
- **Superseded by 040** (Stage 7b): `editor/column.rs` is deleted; the
  layout cache's marks answer the same questions for every line, with
  the same revision-log validity rule.


## 026 — Stage 7 is gated three times

- **Date:** 2026-11-17 (owner decision)
- **Decision:** Stage 7 is delivered as 7a (commands, events, keymaps,
  options and config, modes, multi-selection commands), 7b (search and
  replace, soft wrap), and 7c (renderer benchmarks, themes, slow
  output), each with its own report and approval. PLAN's Stage 7
  acceptance list is split the same way.
- **Why:** Stage 7 is about three normal stages of work; separate gates
  give review points hours apart instead of one at the end.
- **Reopen if:** the owner prefers to merge the remaining gates.

## 027 — Commands: named, owned, run without keys

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** every action is a command in a registry on the editor:
  `CommandId(u32)` never reused (a removed command leaves a tombstone),
  a unique whitespace-free name, an `Owner`, a doc line, and an
  `Arc<dyn Fn(&mut Editor, &Context) -> Effects>` body. `Context` is
  `{ view, count, arg }`; commands never see keys. `run`/`run_named`
  are the only ways in (keys, the `:` line, plugins). `remove_owner`
  removes an owner's modes, key layers, commands, options, and hooks;
  the built-in owner cannot be removed.
- **Why:** PLAN 2.10/2.11: no privileged built-ins; Luau (Stage 11) only
  adapts this API. Running a command clones an `Arc` (no allocation).
- **Evidence:** `editor/command/tests.rs` (every built-in runs by name on
  empty and non-empty documents, with and without a view).
- **Reopen if:** commands need typed arguments beyond one string.

## 028 — Keymaps: layers, a trie per mode, input-time sequences

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** Helix-style notation (`C-s`, `A-z`, `S-left`, `g g`);
  a character key never carries Shift (`z` with Shift is `Z`). Each mode
  has layers ranked user config > plugins (later owners higher) >
  default config; `false` unbinds a lower layer's binding. Any change
  recompiles that mode's trie (sorted children, binary search). The
  pending sequence lives in the keymap; a key that does not continue it
  starts over from the root; an ambiguous prefix's own action runs when
  the next key does not continue it or arrives after
  `sequence-timeout-ms` of input time (no timer events, so replay is
  exact). Unbound printable keys are typed in modes with
  `insert_unbound`; elsewhere an unbound key is reported on the status
  line. `Editor::handle` handles no key itself.
- **Why:** PLAN 2.11 (everything rebindable, no hard-coded keys) and the
  allocation-free resolution PLAN Stage 7 asks for.
- **Evidence:** `keymap/tests.rs` (a property test against a naive
  buffer resolver), `tests/alloc.rs` (zero allocations resolving keys
  and handling normal-mode motions), `mode/tests.rs`
  (`without_the_default_keymap_no_key_does_anything`).
- **Trade-off:** a lone ambiguous prefix waits for the next key (the
  default keymap has none).

## 029 — Modes and the block cursor

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** modes are data (`insert_unbound`, `min_grapheme`,
  `extend`, a `cursor-shape-<mode>` option) held per view; built-ins
  normal, insert, extend, prompt, and question. In a `min_grapheme` mode
  the cursor is a grapheme: the last of a forward range, the first of a
  backward one. Motions run from it: a move makes the range that
  grapheme, an extend keeps the anchor grapheme. After every command
  (and on entering such a mode, and when a view opens) every empty range
  is widened to the grapheme after it, except at the end of the text
  (PLAN 2.5). `w`/`b` select words with Kakoune's rule for progress.
  `Editor::cursor_position` (the block's grapheme) drives scrolling, the
  line anchor, the status column, and the terminal cursor. Insert mode
  keeps every Stage 6 key, so the Stage 3–6 suites run in insert mode.
  `O` is one edit (the new line and the cursor before it), so it undoes
  with what is typed after it.
- **Why:** PLAN 2.5 (D1, selection-first modal editing).
- **Evidence:** `mode/tests.rs`; `random_events_never_panic` checks the
  minimum after every event; the renderer's random test compares frames
  with and without the anchor.
- **Trade-off:** typing in normal mode via `insert-text` or `a`/`i`
  follows the head; a paste in normal mode inserts at the head (after
  the block).

## 030 — Options and config: scopes, layers, field reads

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** typed, owned options resolved view → document → global
  → default (PLAN's language scope arrives with languages in Stage 9).
  Each view keeps a resolved `ViewOptions` (tab width, sequence timeout,
  normal-mode minimum), recomputed whenever any value changes; the
  undo-group pause is pushed into each document's history. Config is
  TOML, walked by hand so every entry is checked alone and a bad one is
  reported and skipped. The default keymap is `default-config.toml`,
  embedded and applied through the same function into the default layer
  (its `[options]` set defaults); the user's file is the user layer (its
  `[options]` set globals) and a reload replaces that layer and the
  global values as a whole. The binary reads the file (DECISIONS 020)
  and re-applies `--tab-width` after each load. Traces record the
  config's hash; `--replay` warns on a mismatch.
- **Why:** PLAN 2.11.
- **Evidence:** `config/tests.rs` (the PLAN acceptance config: rebind and
  unbind in every mode, options at runtime; bad entries; reload
  layering), `pty.rs` `a_config_file_binds_keys_and_reloads_at_runtime`.
- **Trade-off:** a reload also clears global values set with `:set`.

## 031 — Hooks: batches at the outermost call, bounded rounds

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** document-changed, selection-changed, mode-changed,
  view-opened/closed events queue while the editor works and are
  delivered when the outermost `handle` or `run` returns (or on
  `flush_events`): each subscriber gets the whole batch once if it holds
  a kind it wants. While subscribers run, nested calls queue rather than
  deliver; events they raise go to at most 8 further rounds, then are
  dropped with a message. Nothing is queued when there are no
  subscribers. The built-in native consumer cancels a pending key
  sequence and count when a view closes.
- **Why:** PLAN Stage 7 (batch payloads, a native consumer so Stage 11
  only adds a Luau adapter).
- **Evidence:** `hooks/tests.rs`; the bound was missing in the first
  version (a subscriber that ran a command delivered recursively) and
  the round-limit test caught it.
- **Trade-off:** ending the undo group on a mode change is done directly
  in `set_mode`, not by a hook: a hook runs after the command, too late
  for `o` (mode change and edit in one command). A deferred hook that
  changed selections also ended other views' undo groups, so the
  one-grapheme minimum for new views is applied synchronously too.

## 032 — Regex over the rope for selection commands

- **Date:** 2026-11-17 (Stage 7a)
- **Decision:** `regex-cursor` 0.1.5 runs `regex-automata`'s meta engine
  over the rope's chunks (`crate::search`), only inside each selection.
  Select, split, keep, remove, split-lines, and rotate-primary; a
  command that would leave no selection changes nothing and says so.
- **Why:** PLAN Stage 7 names `regex-cursor`; matching stays linear in
  the bytes searched whatever the pattern, and chunks never split a code
  point, so Unicode classes and word boundaries work.
- **Evidence:** `search.rs` (agrees with the `regex` crate on random
  Unicode text), `select/tests.rs`.

## 033 — Regex search: one cursor per search, not `find_iter`

- **Date:** 2026-11-17 (Stage 7b carried fixes; found by the 4,096-case
  property run with small rope leaves)
- **Decision:** `Matcher::find_in` runs each search from a fresh cursor
  placed on the chunk holding the search start (binary search over chunk
  offsets), instead of regex-cursor's `find_iter`. After a non-empty
  match the next search starts at its end; after an empty one, at the
  next char, as `find_iter` does.
- **Why:** regex-cursor 0.1.5's literal strategy (a pattern that is an
  alternation of literals, e.g. `é|e\u{301}`) reads into the next chunk
  to confirm a match and never moves the cursor back (`Pre::search` and
  `literal::find` do not call `Input::move_to`, unlike the PikeVM and DFA
  paths; `set_span` only moves forward). `find_iter`'s next search then
  starts in the later chunk and misses a match that begins before it:
  `"ée\u{301}"` split as `["ée", "\u{301}"]` found `[0..2]`, not
  `[0..2, 2..5]`. Any file where such a match crosses a rope leaf
  boundary was affected (select, split, keep, remove).
- **Evidence:** `search.rs` `matches_across_every_chunk_split` (every
  split of six strings; fails on `find_iter`), the property
  `matches_regex_over_random_chunks` (fails on `find_iter` at 4,096
  cases), and the committed regression seeds in
  `proptest-regressions/search.txt`.
- **Trade-off:** O(log chunks) per match to place the cursor; one more
  `Vec` per call (chunk offsets).
- **Reopen if:** regex-cursor repositions the cursor at the start of every
  search (then `find_iter` is fine again; the tests above decide).

## 034 — Undo groups are bounded by size

- **Date:** 2026-11-17 (Stage 7b carried fixes; PLAN rev. 3 Appendix B 1)
- **Decision:** a typing commit merges into the open group only while
  the bytes the group's commits inserted and deleted stay within
  `undo-group-max-bytes` (an option, default 64 KiB; 0 turns grouping
  off). Past it, the commit starts a new node, which opens a new group.
- **Why:** merging composes the whole group's change sets on every key, so
  one unbroken run cost O(run) per key. Measured before, one run of keys
  1 ms apart: 1.7 µs per key at 10,000 keys, 41 µs at 640,000 with a
  28.7 ms worst key, 26 s in total. After: 1.7 / 2.4 / 2.7 / 2.7 µs per
  key at 10k / 160k / 640k / 1M keys, worst 59 / 100 / 147 / 256 µs,
  1.7 s for 640,000 keys.
- **Evidence:** `a_long_typing_run_splits_at_the_group_size_bound`
  (fails without the bound), `the_default_group_bound_keeps_ordinary_typing_in_one_node`,
  the options test for the new option.
- **Alternatives:** append to the head node's insert in place (no
  re-composition at all, but only for the single-cursor, same-position
  case, and a second code path for history); a key-count bound (does
  not bound bytes).
- **Trade-off:** typing more than 64 KiB without a pause undoes in
  several steps.
- **Reopen if:** an in-place merge for the common case shows a
  measurable gain.

## 035 — Hooks queue only kinds someone wants

- **Date:** 2026-11-17 (Stage 7b carried fixes; amends 031)
- **Decision:** the hook registry keeps the union of the live
  subscribers' kinds; `emit` queues an event only if its kind is in it
  (O(1); it used to scan the subscribers). A batch therefore holds only
  kinds some subscriber wants, not every event of the call.
- **Why:** the built-in view-closed consumer is always subscribed, so
  "nothing is queued without subscribers" never held: every edit queued
  and delivered a document and a selection event nobody wanted.
- **Evidence:** `only_wanted_kinds_are_queued` (fails on the old `emit`);
  `subscribers_get_only_batches_of_their_kinds` updated for the batch
  contents.

## 036 — Kitty keyboard flag, pushed without a query

- **Date:** 2026-11-17 (Stage 7b carried fixes; PLAN rev. 3 2.9)
- **Decision:** entering the terminal pushes the kitty protocol's
  disambiguate-escape-codes flag; every restore path (quit, error,
  panic, signals, suspend) pops it first, while still on the alternate
  screen. The `kitty-keyboard` option (default on) turns it off; a config
  reload pushes or pops at once.
- **Why:** without it an escape sequence split across reads arrives as
  Esc and literal keys, which in a modal editor leaves insert mode and
  runs commands. No support query: crossterm's blocks for up to 2 s on a
  terminal that does not answer.
- **Evidence:** `flash-term` `terminal` tests (sequence order); pty
  `the_keyboard_flag_is_pushed_and_a_config_turns_it_off`, and every pty
  test now checks the flag is popped after the last push.
- **Trade-off:** a terminal that misreads the private `CSI > 1 u` would
  show it; not observed, and not tested on real terminals beyond the pty.
- **Reopen if:** a terminal in use mishandles it (then query
  asynchronously instead).

## 037 — An event's time is when it was read

- **Date:** 2026-11-17 (Stage 7b carried fixes; amends 018)
- **Decision:** the frontend passes `handle_at` (and records as `t_ns`)
  the time the input thread read the event, measured from the session
  start, not the time the main loop dequeued it.
- **Why:** after a stall on the main thread, dequeue times bunch up, so
  undo grouping and key-sequence timeouts followed the stall, not the
  typing.
- **Evidence:** `latency::event_time_is_read_time_not_now`.

## 038 — Saves and opens run on an I/O thread

- **Date:** 2026-11-17 (Stage 7b carried fixes; amends 020)
- **Decision:** the binary runs `Request::Save` and `Request::Open` on one
  I/O worker thread (`apps/flash/src/io.rs`), in order; results return
  through the main channel and are applied on the main thread. A save
  writes an `Editor::save_snapshot` (O(1) text clone, BOM, expected disk
  state, and the history node), which also closes the open undo group;
  `Editor::saved_node` marks that node saved, so typing during the save
  stays modified. A second save of a document still being saved waits
  for the first. Quit treats a document whose head a running save is
  writing as about to be saved: it waits (as `:wq` does) instead of
  asking. On exit the loop waits up to 30 s for running jobs. If the
  worker is gone, a job runs inline.
- **Why:** 282 ms to save and 220 ms to load 100 MB were spent inside the
  event loop.
- **Evidence:** `io_tests.rs` (off-thread save, deferred second save
  without a false conflict, open, exit waits for a running save; making
  the second save start at once fails), `a_save_marks_its_snapshot_not_later_typing`
  (fails without the checkpoint), `quit_waits_for_a_save_of_the_current_text`,
  pty `a_large_save_does_not_block_typing` (32 MiB; keys typed after
  Ctrl+S are drawn before "saved"; fails with the inline path).
- **Trade-off:** a save's outcome arrives later; the status line says
  "saving …" meanwhile. Replay still drops requests.
- **Reopen if:** watching files or crash recovery need a different I/O
  owner.

## 039 — Measuring after idle, and bytes per frame

- **Date:** 2026-11-17 (Stage 7b carried fixes; PLAN rev. 3 section 4)
- **Decision:** `FLASH_BENCH_IDLE_US` makes the latency and editor
  benches pause before each timed sample (`benches/common::idle`), and
  their JSON says so (`idle_us`). The editor bench, the interactive log,
  and replay report bytes written per frame (`frame_bytes`). Cells
  changed per frame needs the cell grid of Stage 7c and is not measured
  yet.
- **Why:** the 7a pty run cost about ten times the hot loop for the same
  work, and timing to `flush` cannot see what a frame costs the terminal.
- **Evidence** (`apps/flash/benches/editor.rs`, release, same machine;
  `bench/results/stage-7b-editor{,-idle}.jsonl`), ns p50 / p99, and frame
  bytes p50:

  | fixture, viewport | hot | idle 5 ms | frame bytes |
  |---|---|---|---|
  | small 80×24 | 25,212 / 45,391 | 178,117 / 420,105 | 666 |
  | medium 80×24 | 41,692 / 103,668 | 256,663 / 665,793 | 823 |
  | medium 200×60 | 73,792 / 176,399 | 372,600 / 811,076 | 1,746 |
  | unicode 200×60 | 73,208 / 156,540 | 419,527 / 962,455 | 2,107 |

  The same event costs 5–7× more after a 5 ms pause, close to the 7a
  pty run's 280 µs p50: most of the gap between in-process benches and
  the interactive path is the pause between keys (caches, CPU idle
  states), not the input thread. The hot numbers match Stage 7a's.
  Every frame is a full redraw, so a one-character edit writes about a
  screen's worth of escapes and text; Stage 7c's renderer is judged
  against these sizes.

## 040 — A display-line layout cache replaces the line anchor

- **Date:** 2026-11-17 (Stage 7b; PLAN rev. 3 Stage 7b item 2; supersedes
  023's anchor and 025)
- **Decision:** `flash_core::layout::Layout`, one per view, answers where
  an offset is (logical column, wrapped row, row start), how many rows a
  line has, and where to start drawing a line or row. Lines of 2 KiB or
  more keep *marks* at least 512 bytes apart (row starts when wrapping,
  grapheme boundaries otherwise): offset, logical column, row, and `dep`,
  how far the layout read to place the mark (a row start depends on the
  grapheme that did not fit; a column only on the text before it). On a
  new revision a mark is kept only if `RevisionLog::map_untouched` shows
  its line untouched from the line start to `dep`, and is shifted with
  the line; the kept marks are a prefix (the property is monotone in
  `dep`, so a binary search finds it), and the last kept one is dropped
  if an edit right at it made it a non-boundary. Marks are extended
  lazily, as far as a query needs; a walk to the left column leaves a
  mark where it stopped (at least 16 bytes on). The last location found
  (the *hot point*, on any line) is kept by the same rule and is where
  the next query walks from, forward or backward over non-tab graphemes.
  At most 128 long lines are cached (LRU). Vertical motion takes
  optional column hints (`motion::Columns`) the layout provides.
- **Why:** soft wrap needs row starts for every visible line, and the
  Stage 6 checkpoints answered one question (the primary head's column)
  for one line. Marks generalise them: one mechanism for columns, rows,
  and the left edge, kept valid by the same revision-log rule.
- **Evidence:** `layout/tests.rs`: a property test against laying each
  line out from its start under random edits, tab widths, and wrap widths
  (locate, row counts, rows, the left-edge start, column extension,
  hinted against plain vertical motion, row motion cached against fresh);
  mutations that keep one extra mark or read one grapheme less are
  caught; `an_insert_joining_clusters_at_a_mark_drops_it` and
  `appending_a_combining_mark_at_the_end_drops_points_there` pin the
  boundary re-check (random edits did not find it in 2,000 cases);
  `queries_on_a_long_line_walk_near_the_offset`;
  `local_edits_on_a_long_line_do_not_walk_it`; the random editor test
  checks the cursor's location and visibility (soft wrap in half the
  runs); the random render test compares frames with and without the
  cache. The hinted-motion property found two bugs in the wiring before
  commit (the scan restarted at the line start; a swapped tuple). The
  latency bench found the first version 10–13× slower per key on the
  1 MB single line (walks of up to 1 KiB from a mark on every key) and
  slower on short lines (walked from their start): the hot point,
  prefix-only dependencies, stop marks, and skipping short visible lines
  by scanning for their newline brought it back (stage-7b.md F, G).
- **Trade-off:** a jump along a long line (End on a line just split, a
  search result) walks from the last mark: O(distance), about 26 ms on
  the 1 MB `one-line` fixture, as before. Marks after an edit are
  dropped, not shifted (shifting is valid only across tab-free text,
  which would need a per-mark flag).
- **Reopen if:** jumps along long lines matter (shift marks across
  tab-free text), or Stage 9 highlighting wants per-line layout caching.

## 041 — Soft wrap: greedy rows, logical vertical motion, g j / g k

- **Date:** 2026-11-17 (Stage 7b)
- **Decision:** the `soft-wrap` option (default off). A row holds at most
  the view's width; it ends after its last space or tab when the next
  grapheme does not fit, else before that grapheme; a grapheme wider
  than the row gets one of its own; the cursor cell after a line's last
  grapheme counts, so a full last row is followed by an empty row. Tabs
  expand on the logical column. The view scrolls by rows (`top_row`);
  there is no horizontal scroll. `j`/`k` and the arrows move by logical
  lines (as vim); `row-up`/`row-down` (`g k`/`g j`) move by rows with a
  goal column within the row, and are exactly up/down without wrapping.
- **Alternatives:** Unicode line breaking (UAX #14; a new dependency and
  lookahead past one row), wrap at graphemes only (breaks words), moving
  by rows on `j`/`k` (surprising with counts and in macros).
- **Evidence:** `rows_break_after_spaces_else_at_graphemes`,
  `row_motion_moves_by_wrapped_rows`, the emulator tests
  `soft_wrap_draws_rows_and_the_cursor_on_its_row` and
  `soft_wrap_scrolls_by_rows_inside_a_long_line`, the random tests.
- **Reopen if:** users want UAX #14 breaking or indented continuation
  rows.

## 042 — Search: incremental, off the main thread for large documents

- **Date:** 2026-11-17 (Stage 7b)
- **Decision:** `/` and `?` search as the pattern is typed, from where the
  search started, wrapping around; Enter keeps the match and pattern,
  Esc restores both. `n`/`N` repeat (in extend mode adding the match as a
  new selection), `*` searches for the selected text (escaped), `:noh`
  stops the underlining. Forward searches start after the primary
  selection (a match at the cursor is skipped, as in vim), backward ones
  before its start. The regex sees the whole text from any start (a
  cursor over rope chunks via `chunk_at_byte`). Documents of 256 KiB or
  more are searched by `Request::Search` (a snapshot, the revision, the
  pattern) on a search thread with a one-job latest-wins slot; only the
  latest search's result is applied, mapped forward through the revision
  log if the text changed, else dropped with a message. A replay runs
  searches inline.
- **Why:** PLAN Stage 7 (search off the main thread for large buffers,
  without blocking input); requests keep the core free of threads and
  replays exact (DECISIONS 020).
- **Evidence:** `search.rs` (against regex-automata on the whole string,
  also with tiny leaves), `editor/find/tests.rs` (including stale and
  superseded results; removing the id check or the mapping fails it),
  `searcher.rs`, pty `searching_a_large_file_finds_the_match` (1 MB).
- **Trade-off:** a search that has started cannot be cancelled
  (regex-cursor has none); its result is ignored if superseded. A
  backward search scans from the start of the text. Matches are
  underlined only within what each drawn line or row shows (at most
  4 KiB from where drawing starts): a match crossing a wrapped row break
  is not underlined (Stage 7c draws per line).

## 043 — Replace is `:s` and `:%s` over selections or the text

- **Date:** 2026-11-17 (Stage 7b)
- **Decision:** `:s/pattern/replacement/` replaces every match inside the
  selections, `:%s/…/…/` inside the whole text; the first character is
  the delimiter (a backslash escapes it), flag `i` ignores case (`g` is
  accepted and implied), `$1`/`${name}`/`$$` expand from captures read
  from the rope. One transaction, one undo step. Selection-first editing
  still has its own way: select matches (`s`), then change (`c`).
- **Why:** PLAN Stage 7 asks for replace; `:s` is what users of modal
  editors type, and doing it as one change set keeps undo whole.
- **Evidence:** `replace_in_selections_and_in_the_whole_text`,
  `substitution_specs_parse`, `replacements_expand_captures`.
- **Trade-off:** replacing runs on the main thread (a one-off command;
  `:%s` over 100 MB blocks while it runs).
