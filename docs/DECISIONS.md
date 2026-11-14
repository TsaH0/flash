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
