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
