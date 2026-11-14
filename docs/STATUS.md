# Flash status

Updated: 2026-11-14, end of Stage 2 (awaiting owner approval).

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
- **Benchmarks** (`benches/text.rs`, `benches/latency.rs`): Criterion suite
  plus per-operation p50/p95/p99/max and allocation counts on deterministic
  fixtures. Results in `bench/results/`.

## Implemented but not fully verified

- Nothing.

## In progress

- Nothing. Stage 2 is waiting for review and approval
  ([stage-2.md](stages/stage-2.md)).

## Planned

- Stage 3: selections, transactions, multi-cursor, walking skeleton (next,
  after approval).
- Stages 4–11 as in [PLAN.md](PLAN.md).

## Deferred

- UTF-16 metric: overhead measured in Stage 1; enabling it is decided in
  Stage 10 (LSP).
