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
