# Flash status

Updated: 2026-11-15, end of Stage 3 (awaiting owner approval).

## Completed and tested

- **Workspace** (`Cargo.toml`, `crates/flash-core`, `apps/flash`): builds
  offline; fmt and clippy clean with `-D warnings`.
- **`Text`** (`crates/flash-core/src/text/mod.rs`): validated byte-offset API
  over crop 0.4.3 (vendored with `Rope::chunk_at_byte`, DECISIONS 008);
  cursor-row line model; O(log n) allocation-free grapheme-boundary check and
  counted grapheme motion (`move_graphemes`); O(1) snapshots; `pub(crate)`
  mutators.
- **Reference model and property tests** (`text/reference.rs`,
  `text/proptests.rs`): random Unicode edit sequences including invalid
  offsets, run with default and tiny (`small-chunks`) rope leaves.
- **`ChangeSet`** (`crates/flash-core/src/changeset/`): canonical builder,
  `from_edits`, validate, atomic `pub(crate)` apply, invert, compose,
  `map_pos`/`map_sorted` with a documented policy (DECISIONS 009, 010);
  property tests against a String model; fuzz target
  (`crates/flash-core/fuzz`, nightly); benchmarks (`benches/changeset.rs`).
- **Selections** (`crates/flash-core/src/selection/`): `Range`, `Selection`
  normalised by the PLAN 2.5 merge rule, primary survives, per-endpoint
  mapping through a ChangeSet (DECISIONS 011).
- **Motions** (`motion/`, `width.rs`): grapheme, word, line start/end,
  vertical with goal column, document start/end; move and extend;
  `Text::graphemes` replaces crop's buggy iterator (DECISIONS 012).
- **Transactions** (`transaction/`): insert, delete, replace, backspace,
  delete forward over every range with deterministic collision rules;
  validate-first atomic apply (DECISIONS 013); property-tested against a
  naive back-to-front model.
- **Walking skeleton** (`apps/flash`): `flash <file>`, raw-mode full-redraw
  editor over the core APIs, multi-cursor, no save; in-process end-to-end
  latency bench.
- **Benchmarks** (`benches/text.rs`, `benches/latency.rs`): Criterion suite
  plus per-operation p50/p95/p99/max and allocation counts on deterministic
  fixtures. Results in `bench/results/`.

## Implemented but not fully verified

- The skeleton in a real terminal (no TTY in the sandbox): owner smoke test
  pending; see [stage-3.md](stages/stage-3.md) section F.
- Stage 3 had no independent fresh-context review (dispatch was refused by
  the session's permission policy); the implementer's adversarial pass is in
  stage-3.md section G.

## In progress

- Nothing. Stage 3 is waiting for review and approval
  ([stage-3.md](stages/stage-3.md)).

## Planned

- Stage 4: documents, views, revisions, headless editor, trace replay
  (next, after approval). `Transaction::apply` becomes `pub(crate)` there.
- Stages 5–11 as in [PLAN.md](PLAN.md).

## Deferred

- UTF-16 metric: overhead measured in Stage 1; enabling it is decided in
  Stage 10 (LSP).
- Up/down motion on huge single-line files is O(line length); revisit with
  the Stage 6 renderer's column cache.
