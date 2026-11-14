# Stage 2 — ChangeSet algebra

Status: **complete, awaiting owner approval.** Branch `stage-2`, local commits
only (nothing pushed). Plan: [2026-11-14-stage-2-changeset.md](../superpowers/plans/2026-11-14-stage-2-changeset.md).

## A. Status

| Requirement (PLAN Stage 2) | State |
|---|---|
| `Vec<Op>` with small-string inserts | Done: `Op::Insert(CompactString)` (inline up to 24 bytes) |
| Builder: merges, drops empty ops, canonical Delete/Insert order, rejects length mismatches | Done (`ChangeSetBuilder`; overflow is an error too) |
| `validate(&Text)`: lengths and char boundaries | Done |
| `apply`: per-op baseline vs `RopeBuilder` rebuild, picked by measurement | Done; see section F and DECISIONS 010 |
| Atomic application | Done: full validation before any edit |
| `invert(&original)`, `compose(a, b)` | Done |
| `map_pos(pos, Assoc)`, `map_sorted(&mut [pos])` in one pass | Done |
| Mapping policy defined explicitly | Done (`changeset` module docs, DECISIONS 009) |
| Known trap (compose vs. positions) documented and pinned | Done (`compose_loses_positions_documented`) |
| Property tests for every listed property | Done (section D) |
| cargo-fuzz target | Done (`crates/flash-core/fuzz`, section E) |
| Benchmarks: build/apply/invert/compose/map at 1/100/10k ops on medium and large | Done (section F) |

Deviations from the plan: none in scope. `ChangeSet::apply` is `pub(crate)`
(only transactions may change text, PLAN 2.2); benchmarks reach it through
the `internals` feature.

## B. Commits

Branch `stage-2`, from `139851f` (Stage 1 close):

| Commit | Content |
|---|---|
| `38845e2` | `ChangeSet`, `Op`, `Assoc`, `ChangeSetError`, `ChangeSetBuilder`, `from_edits`; String reference model; shared proptest strategies (`src/strategies.rs`); plan document; `compact_str` dependency |
| `9ad708a` | `validate`, `apply`, `invert` |
| `1102211` | `compose` |
| `258b56b` | `map_pos`, `map_sorted`; mapping-policy docs; DECISIONS 009 |
| `fc0573f` | Fuzz entry point and cargo-fuzz crate; `arbitrary` dependency |
| `da71f53` | Benchmarks, latency row, apply-strategy decision (DECISIONS 010), results |
| (closing commit) | Review fixes (mapping edge rule, capacity cap), this report, status |

## C. Design and algorithms

### Representation and canonical form

`ChangeSet { ops: Vec<Op>, len_before, len_after }` with
`Op::{Retain(usize), Delete(usize), Insert(CompactString)}`. The fields are
private; `ChangeSetBuilder` is the only constructor (also used by
`from_edits`, `invert`, and `compose`), so every change set is canonical:

1. no empty op; 2. no two adjacent ops of one kind; 3. `Delete` never
directly after `Insert`; 4. `len_before = Σ Retain + Σ Delete`,
`len_after = Σ Retain + Σ |Insert|`.

- **How.** `retain`/`insert` merge into a same-kind last op. `delete` after
  an `Insert` merges into the `Delete` just before it, or is placed in front
  of the `Insert` (the two orders have the same effect). Running sums use
  `checked_add` with a sticky overflow flag; `finish` reports `Overflow` or
  `LengthMismatch`. The builder never panics.
- **Why.** Equal effects from equal edit sequences give structurally equal
  values, which the invert and compose laws are checked against. Apply and
  mapping only ever see `Retain` runs separated by `(Delete?, Insert?)`
  groups.
- **Complexity.** O(1) amortised per builder call; inserting a `Delete`
  before a trailing `Insert` is a one-element shift at the end of the `Vec`.

### validate and apply

- **validate.** Length must equal `len_before`; every cumulative offset at
  which a `Retain` or `Delete` ends must be a char boundary. O(ops · log n).
- **apply.** Validate, then edit. Because validation covers every offset the
  edits use, no edit can fail afterwards, so a failed `apply` has changed
  nothing. (Cloning and swapping, as PLAN suggested, would also be atomic,
  but forces every edit to copy shared tree paths; validation first costs
  O(ops · log n) and copies nothing.) A `Text` error after validation is a
  bug: `debug_assert!` plus an error return, never a panic.
- **Per-op strategy.** Walk ops in reverse from `len_before`; each
  `(Delete?, Insert?)` group becomes one `Text::replace`/`delete`/`insert`.
  Editing back to front keeps the offsets of edits still to do unchanged.
  O(groups · log n).
- **Rebuild strategy.** Walk forward into a `TextBuilder`: retained ranges
  are appended chunk by chunk, inserts appended. O(n + inserted).
- **Choice.** See section F and DECISIONS 010.

### invert

`Retain(n) → Retain(n)`, `Delete(n) → Insert(original[pos..pos+n])`,
`Insert(s) → Delete(|s|)`, through a builder of `len_after`. The builder
re-canonicalises (an inverted `Delete, Insert` group comes out as
`Delete, Insert` again). Requires the original text, validated first.
O(ops · log n + deleted bytes).

### compose

Two cursors over `a` and `b`, each holding the unconsumed rest of its op.
Rules, in priority order: `a`'s `Delete` passes through (that text never
reaches `b`); `b`'s `Insert` passes through; otherwise both heads cover
`m = min` bytes of the middle text and: `Retain/Retain → retain(m)`,
`Retain/Delete → delete(m)`, `Insert/Retain → insert(s[..m])`,
`Insert/Delete → nothing` (cancel).

- **Why correct.** Every byte of the middle text is produced by exactly one
  of `a`'s `Retain`/`Insert` and consumed by exactly one of `b`'s
  `Retain`/`Delete`; the table covers each pairing, and bytes outside the
  middle text (a's deletes, b's inserts) pass through unchanged.
- **Corner case.** If `b` splits a character that `a` inserted, `s[..m]`
  would panic; `compose` returns `NotCharBoundary` instead.
- **Complexity.** O(ops(a) + ops(b)) plus the bytes of retained inserts.
- **Known trap.** Composition preserves text, not positions (section D,
  DECISIONS 009).

### Position mapping

Policy (module docs, DECISIONS 009): a position strictly inside a `Retain`
shifts. In the closed old range `[o, o + d]` of an edit group, if `d > 0`
the old edges map to the replacement's edges (`o` to the left edge, `o + d`
to the right edge); every other position (a pure insertion point, or
strictly inside a deleted range) goes to the left edge with `Before` and the
right edge with `After`. `len_before` maps to `len_after` unless an insert
sits at the end. The edge rule was adopted after the review (section G): it
keeps a range that touches or covers a replacement whole. `map_sorted` checks the slice first (sorted, in
range; on error it is untouched), then walks ops and positions together:
O(ops + positions), no allocation. `map_pos` is `map_sorted` on one element.

## D. Invariants and how they are tested

| Invariant | Test |
|---|---|
| Builder output is canonical and has the same effect as the raw ops | `builder_output_is_canonical_and_matches_raw_ops` (raw lists with empty ops, same-kind runs, Insert-then-Delete); `builder_canonicalises` |
| `retained + deleted == len_before`, `retained + inserted == len_after` | `lengths_add_up`, `assert_canonical` on every produced change set |
| Length mismatch and overflow are errors, not panics | `builder_rejects_length_mismatch`, `builder_overflow_is_an_error`, `from_edits_cases` |
| `apply` equals the String model | `apply_matches_reference`, `apply_basic`, `apply_on_multi_leaf_text_matches_reference` |
| A change set failing validation leaves the text untouched | `invalid_changeset_leaves_text_unchanged` (random other texts, half of them the same length so char-boundary failures occur), `apply_rejects_*`; fuzz target |
| `apply(invert(c, t), apply(c, t)) == t`; `invert(invert(c, t), apply(c, t)) == c` | `invert_round_trips`, `invert_basic`; fuzz target |
| `apply(compose(a, b), t) == apply(b, apply(a, t))` | `compose_matches_sequential_apply`, `compose_basic`; fuzz target |
| Associativity at the text level | `compose_is_associative_on_text` |
| Identity and inverse laws for compose | `compose_with_identity`, `compose_with_inverse_is_identity_on_text` |
| Compose rejects splitting an inserted char | `compose_rejects_split_inside_inserted_char` |
| `map_sorted` equals per-position `map_pos`; `map_pos` equals the naive policy model | `map_sorted_matches_map_pos`, `map_pos_matches_naive` (every offset in `0..=len+2`, both `Assoc`) |
| Mapped char boundaries are char boundaries of the new text, and monotonic | `mapped_positions_are_char_boundaries_and_monotonic`, `map_through_chain_is_sequential`; fuzz target |
| Mapping policy cases (insert, delete, replace, end, empty document) | `map_insert_assoc`, `map_delete_collapses`, `map_replace_edges`, `map_end_of_document`, `map_on_empty_document`, `map_sorted_maps_in_one_pass` |
| Bad positions are errors and leave the slice unchanged | `map_sorted_errors_leave_slice_unchanged` |
| Known trap: compose does not preserve positions | `compose_loses_positions_documented` |

**Gate results** (rustc 1.99.1, `--offline`): `cargo fmt --all -- --check`
ok; `cargo clippy --workspace --all-targets -- -D warnings` ok, also with
`--all-features`; `cargo test --workspace` 66 passed;
`PROPTEST_CASES=4096 cargo test -p flash-core --release` 66 passed, also with
`--features small-chunks`.

**Test sensitivity.** Injecting `Insert, Delete` ordering into the builder
failed both builder properties with minimal inputs. The `mapped_positions`
property found a wrong assertion in the test itself (an insert at the end
with `Before`); the seed is kept in `proptest-regressions/changeset/`.

## E. Fuzzing

`flash_core::fuzzing::changeset_ops` (feature `fuzzing`; also compiled for
tests) builds a text (≤ 4 KiB) and two change sets from raw bytes, with op
lengths drawn either within the remaining length or from all of `u16`, so
many are invalid. It then checks: a change set that fails `validate` also
fails `apply` and `invert` and leaves the text unchanged; otherwise lengths,
invert round trip, double inverse, mapping of every char boundary (in order,
on boundaries), and compose against sequential application.

- Stable driver: `fuzz_entry_survives_random_bytes` (2,048 random inputs per
  `cargo test`).
- cargo-fuzz: `cargo +nightly fuzz run --fuzz-dir crates/flash-core/fuzz
  changeset -- -max_total_time=600` → **3,137,133 executions in 601 s, no
  crash or invariant failure**, coverage 2,117 edges, corpus 1,955 inputs.
  The fuzz crate is its own workspace and carries its own copy of the crop
  patch. Corpus and artifacts are git-ignored.
- Re-run after the review fixes (mapping edge rule): 180 s, **701,333
  executions, no failure**, coverage 2,119 edges.

## F. Benchmark results

Machine: Intel Core i7-1355U (12 threads), Linux 6.18.63, rustc 1.99.1,
bench profile (release + debug symbols), laptop on default power settings,
not isolated: treat differences under ~10% as noise. Full data:
`bench/results/stage-2-criterion.json` (50 benchmarks) and
`bench/results/stage-2-latency.jsonl`.

Criterion means. Edits are spread over the fixture, each replacing ≤ 16
bytes with `"x"`; `apply` runs on a snapshot-shared tree; `map_sorted` maps
1,000 sorted positions.

| op | 1 | 100 | 1,000 | 3,000 | 10,000 edits |
|---|---|---|---|---|---|
| build (`from_edits`), 1 MB | 39 ns | 2.0 µs | 19 µs | 59 µs | 168 µs |
| apply, 1 MB | 0.86 µs | 68 µs | 474 µs | 765 µs | 1.80 ms |
| apply, 100 MB | 1.44 µs | 75 µs | 1.06 ms | 3.45 ms | 9.56 ms |
| invert, 1 MB | 191 ns | 16 µs | 286 µs | 776 µs | 2.06 ms |
| invert, 100 MB | 254 ns | 23 µs | 553 µs | 1.84 ms | 5.83 ms |
| compose, 1 MB | 64 ns | 4.4 µs | 45 µs | 135 µs | 436 µs |
| map_sorted ×1,000, 1 MB | 0.98 µs | 1.3 µs | 5.3 µs | 12.5 µs | 41 µs |

Edit counts are the requested `n`; after dropping overlapping ranges the
1 MB change sets keep 2,948 (of 3,000) and 9,334 (of 10,000) edits, and the
100 MB ones are within 0.04% of `n`.

Build, compose, and map do not depend on document size (100 MB results are
within noise of 1 MB). Apply strategy: per-op beat the rebuild at every
count (DECISIONS 010; rebuild ≥ 200 µs on 1 MB and ≥ 22 ms on 100 MB even
for one edit).

Per-operation latency, one-edit change set (validate + apply) on a
snapshot-shared tree, 100k samples (ns):

| fixture | p50 | p95 | p99 | allocs/op |
|---|---|---|---|---|
| small (4 KB) | 227 | 354 | 477 | 4.3 |
| medium (1 MB) | 657 | 971 | 1,247 | 8.6 |
| large (100 MB) | 1,336 | 1,892 | 2,210 | 11.0 |
| one-line (1 MB) | 658 | 978 | 1,253 | 8.6 |
| unicode (1 MB) | 663 | 1,073 | 1,273 | 8.8 |

Against the PLAN target (1 MB single-character edit, full transaction commit
well under 100 µs p99): the change set layer costs about 1.25 µs p99.

## G. Adversarial review

A fresh-context reviewer (Claude Opus, given PLAN Stage 2, the plan, the
decisions, and the code at `da71f53`) attacked the stage with its own tests
in a separate worktree. It found no counterexample to canonical form,
apply atomicity, invert, compose (including structural associativity, which
is stronger than required), or mapping consistency. It also checked all
five Review Focus areas with extra cases, and measured that the property
generators reach the interesting cases (e.g. `b` splits `a`'s inserted text
in 897 of 4,000 chains; 17% of invalid-text cases fail on a char boundary).

| # | Finding | Severity | Resolution |
|---|---|---|---|
| 1 | Mapping policy at replacement edges: a range (start `After`, end `Before`) ending at a replacement dropped it, and a range equal to the replaced range came out inverted; DECISIONS 009's reason for rejecting the alternative was wrong | Important | **Fixed.** Edge rule adopted (section C, DECISIONS 009). Tests `range_ending_at_replacement_end_keeps_the_replacement`, `range_equal_to_replaced_range_covers_the_replacement` (failed first, now pass), plus `range_touching_an_insert_does_not_absorb_it`; the trap example was replaced, since the old one no longer differs under the new rule |
| 2 | `ChangeSetBuilder::with_capacity(0, usize::MAX)` panicked (capacity overflow) | Important | **Fixed.** Hint capped at 65,536 ops. Test `builder_capacity_hint_never_panics` (failed first, now passes) |
| 3 | `from_edits` rejects an empty edit after a non-empty one at the same start (`[(3..5, "b"), (3..3, "a")]`) though both orders are "sorted by start" | Minor | Deferred: tighten the contract in Stage 3, when transaction constructors start calling it |
| 4 | The stable fuzz driver is nearly vacuous: random bytes give mostly empty or 1-byte texts (the 600 s libFuzzer run is not affected) | Minor | Deferred: build fuzz texts from a piece table and commit a seed corpus |
| 5 | The plan's shared-insertion-buffer question was not measured: benchmark inserts are 1 byte (inline). Reviewer's measurement on 1 MB, ~10k inserts: build 141 µs (1-byte inserts) vs 270 µs (40-byte) | Minor | Recorded here; the baseline stands until a profile shows insert allocation |
| 6 | Benchmark labels overstate edit counts after overlaps are dropped (1 MB: `n = 10,000` keeps 9,334 edits; `3,000` keeps 2,948; 100 MB within 0.04%) | Minor | Recorded in section F |
| 7 | `after_validation` would leave a half-applied text in release if it were ever reached (it is unreachable) | Minor | Deferred: revisit with clone-and-swap if Stage 4 commit needs the extra insurance |
| 8 | Test gaps: `map_through_chain_is_sequential` checks only boundaries; no property generates invalid `from_edits` input; invert/compose on the empty document are covered only indirectly | Minor | Deferred |

Declined by the reviewer, with the executor's ruling: CRLF splitting and
grapheme-aware mapping (cursor-motion concerns, PLAN 2.4, Stages 3/6:
stands); structural `compose(a, invert(a)) == identity` (spec asks for text
equivalence: stands); `CompactString` in the public `Op` (already a known
limitation: stands); a frame budget for 10k-edit apply (no Stage 2 budget;
the 1,000-cursor target is measured in Stage 3: stands).

After the fixes: 66 tests pass in debug and with `PROPTEST_CASES=4096` in
release, with both leaf sizes; the fuzz target was re-run (section E).

## H. Known limitations

- `map_sorted` takes one `Assoc` for the whole slice. Stage 3 selections need
  per-endpoint `Assoc`; they can map in two passes, or the API grows then.
- `Op` exposes `CompactString` in the public API; changing the insert
  representation later (e.g. a shared buffer) is a breaking change inside the
  workspace.

## I. Questions for the reviewer

1. Can any public `ChangeSet`/`ChangeSetBuilder` method panic for any input?
2. Is there a valid `(text, a, b)` where `apply(compose(a, b))` differs from
   applying `a` then `b`?
3. Does any mapped char boundary land off a char boundary, or out of order?
4. Can a failed `apply` change the text?

## J. Next stage

Stage 3: selections, transactions, multi-cursor, and the walking skeleton.
Not started; waiting for approval.
