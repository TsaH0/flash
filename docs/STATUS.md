# Flash status

Updated: 2026-11-12, end of Stage 1 (awaiting owner approval).

## Completed and tested

- **Workspace** (`Cargo.toml`, `crates/flash-core`, `apps/flash`): builds
  offline; fmt and clippy clean with `-D warnings`.
- **`Text`** (`crates/flash-core/src/text/mod.rs`): validated byte-offset API
  over crop 0.4.3; cursor-row line model; O(log n) grapheme-boundary check;
  O(1) snapshots; `pub(crate)` mutators.
- **Reference model and property tests** (`text/reference.rs`,
  `text/proptests.rs`): random Unicode edit sequences including invalid
  offsets, run with default and tiny (`small-chunks`) rope leaves.
- **Benchmarks** (`benches/text.rs`, `benches/latency.rs`): Criterion suite
  plus per-operation p50/p95/p99/max and allocation counts on deterministic
  fixtures. Results in `bench/results/`.

## Implemented but not fully verified

- Nothing.

## In progress

- Nothing. Stage 1 is waiting for review and approval.

## Planned

- Stage 2: ChangeSet algebra (next, after approval).
- Stages 3–11 as in [PLAN.md](PLAN.md).

## Deferred

- UTF-16 metric: overhead measured in Stage 1; enabling it is decided in
  Stage 10 (LSP).
- Allocation-free grapheme boundary checks: revisit with Stage 3 cursor
  motion benchmarks (DECISIONS 004).
