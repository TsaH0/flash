# Stage 2 — ChangeSet Algebra Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A validated, canonical `ChangeSet` (Retain/Delete/Insert) in `flash-core` with apply, invert, compose, and position mapping, proven by property tests against a `String` reference model, fuzzed, and benchmarked.

**Architecture:** New module `crates/flash-core/src/changeset/`, a sibling of `text/` in the same crate, so `apply` can stay `pub(crate)` (only transactions mutate text). A `ChangeSetBuilder` is the only way to make a `ChangeSet` and always produces canonical form, so structural equality is meaningful. Every operation that touches a `Text` validates fully before mutating, so a failed operation changes nothing.

**Tech Stack:** Rust 2024, crop 0.4.3 (vendored), `compact_str` 0.9 (inline insert strings), proptest 1.12, Criterion 0.8, `arbitrary` 1 + cargo-fuzz/libfuzzer-sys (fuzzing).

**Spec:** `docs/PLAN.md` section 5, "Stage 2 — ChangeSet algebra", plus sections 2.4, 2.6, 3, and 6.

## Global Constraints

- Rust edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` stays in `flash-core`.
- Build and test with `--offline`. The sandbox blocks crates.io.
- New dependencies: `compact_str = "0.9"` (normal) and `arbitrary = "1"` (optional, behind feature `fuzzing`, plus a dev-dependency). Nothing else. List both in the stage report with reasons.
- Byte offsets everywhere. Every op boundary must be a char boundary of the text it applies to.
- No panic reachable from any public method for any input: lengths up to `usize::MAX`, any positions, any `Text`. Fallible operations return `Result<_, ChangeSetError>`.
- `ChangeSet::apply` is `pub(crate)`. Benchmarks reach it only through `flash_core::internals` (feature `internals`).
- A failed `apply` or `map_sorted` leaves its target unchanged.
- Never map positions through a composed ChangeSet (PLAN "Known trap"). Document this on `compose`.
- One local commit per task on branch `stage-2`. Never push. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Gate commands (all must pass before the stage report): `cargo fmt --all -- --check`; `cargo clippy --offline --workspace --all-targets -- -D warnings` (plain and with `--all-features`); `cargo test --offline --workspace`; `PROPTEST_CASES=4096 cargo test --offline -p flash-core --release` (plain and with `--features small-chunks`).

## Review Focus

1. **Empty document.** `len_before == 0`: the identity, an insert into empty text, `map_pos(0)` with both `Assoc` values, and invert/compose of those. Pinned in Task 1 (`empty_document_changesets`) and Task 4 (`map_on_empty_document`).
2. **Length overflow.** `retain(usize::MAX).retain(1)` and `from_edits` with `usize::MAX` ends must return `Overflow` or `InvalidEdit`, not panic. Pinned in Task 1 (`builder_overflow_is_an_error`).
3. **Right length, wrong text.** A ChangeSet whose op boundary falls inside a multi-byte char of the given text (it was valid for another text of the same length) must fail `validate`/`apply`/`invert` and leave the text unchanged. Pinned in Task 2 (`apply_rejects_boundary_inside_char`).
4. **Compose splitting a char.** `b` retains or deletes into the middle of a multi-byte char that `a` inserted. `compose` must return `NotCharBoundary`, not panic. Pinned in Task 3 (`compose_rejects_split_inside_inserted_char`).
5. **Bad positions.** `map_sorted` with an unsorted slice or a position past `len_before` must return an error and leave the slice untouched. Pinned in Task 4 (`map_sorted_errors_leave_slice_unchanged`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/flash-core/src/changeset/mod.rs` | `Op`, `Assoc`, `ChangeSet` (fields, accessors, module docs incl. mapping policy and the compose trap), `ChangeSetError`, submodule declarations |
| `crates/flash-core/src/changeset/builder.rs` | `ChangeSetBuilder` (canonicalising), `ChangeSet::identity`, `ChangeSet::from_edits` |
| `crates/flash-core/src/changeset/apply.rs` | `validate`, `apply` (per-op and rebuild strategies), `invert` |
| `crates/flash-core/src/changeset/compose.rs` | `compose` |
| `crates/flash-core/src/changeset/map.rs` | `map_pos`, `map_sorted` |
| `crates/flash-core/src/changeset/reference.rs` | `#[cfg(test)]` String model: naive apply, naive per-position map |
| `crates/flash-core/src/changeset/tests.rs` | Hand-written cases |
| `crates/flash-core/src/changeset/proptests.rs` | Algebraic properties against the reference model |
| `crates/flash-core/src/strategies.rs` | `#[cfg(test)]` shared proptest strategies (moved from `text/proptests.rs`) |
| `crates/flash-core/src/fuzzing.rs` | `#[cfg(any(test, feature = "fuzzing"))]` fuzz entry point |
| `crates/flash-core/fuzz/` | cargo-fuzz crate (own workspace, excluded from the root) |
| `crates/flash-core/benches/changeset.rs` | Criterion suite |
| `crates/flash-core/src/internals.rs` | Add `changeset_apply*` passthroughs |

---

### Task 0: Close Stage 1 and branch (owner-approved, before Task 1)

- [ ] Wait for the `PROPTEST_CASES=4096` release runs (both leaf sizes) to pass. If they fail, stop and report.
- [ ] Commit the Stage 1 closing changes on `stage-1`: `vendor/`, `Cargo.toml`, `Cargo.lock`, `.gitignore`, `crates/`, `docs/`. Do not add `.claude/`, `.mcp.json`, `.gitmodules`. Message: `Vendor crop with chunk_at_byte; add move_graphemes; close Stage 1`.
- [ ] `git switch -c stage-2`.

---

### Task 1: ChangeSet type, canonical builder, reference model

**Files:**
- Create: `src/changeset/mod.rs`, `src/changeset/builder.rs`, `src/changeset/reference.rs`, `src/changeset/tests.rs`, `src/changeset/proptests.rs`, `src/strategies.rs`
- Modify: `src/lib.rs` (add `pub mod changeset;`, `#[cfg(test)] mod strategies;`, re-export `Assoc, ChangeSet, ChangeSetBuilder, ChangeSetError, Op`), `src/text/proptests.rs` (use `crate::strategies::{piece, text_strategy}`), root `Cargo.toml` (`[workspace.dependencies] compact_str = "0.9"`), `crates/flash-core/Cargo.toml` (`compact_str.workspace = true`)

**Interfaces (Produces):**

```rust
pub enum Op { Retain(usize), Delete(usize), Insert(CompactString) }   // Debug, Clone, PartialEq, Eq
pub enum Assoc { Before, After }                                     // Debug, Clone, Copy, PartialEq, Eq, Hash
pub struct ChangeSet { ops: Vec<Op>, len_before: usize, len_after: usize } // Debug, Clone, PartialEq, Eq
impl ChangeSet {
    pub fn identity(len: usize) -> ChangeSet;
    pub fn from_edits<I, S>(len_before: usize, edits: I) -> Result<ChangeSet, ChangeSetError>
        where I: IntoIterator<Item = (Range<usize>, S)>, S: AsRef<str>;
    pub fn len_before(&self) -> usize;
    pub fn len_after(&self) -> usize;
    pub fn ops(&self) -> &[Op];
    pub fn is_identity(&self) -> bool;          // no Delete or Insert
}
pub struct ChangeSetBuilder { /* ops, len_before, consumed, inserted, overflowed */ }
impl ChangeSetBuilder {
    pub fn new(len_before: usize) -> Self;
    pub fn with_capacity(len_before: usize, ops: usize) -> Self;
    pub fn retain(&mut self, n: usize) -> &mut Self;
    pub fn delete(&mut self, n: usize) -> &mut Self;
    pub fn insert(&mut self, s: &str) -> &mut Self;
    pub fn retain_rest(&mut self) -> &mut Self;   // retains len_before - consumed (saturating)
    pub fn finish(self) -> Result<ChangeSet, ChangeSetError>;
}
#[non_exhaustive] pub enum ChangeSetError {     // Debug, Clone, Copy, PartialEq, Eq; Display; Error
    LengthMismatch { expected: usize, actual: usize },
    Overflow,
    NotCharBoundary { offset: usize },
    InvalidEdit { index: usize },
    PositionOutOfBounds { position: usize, len: usize },
    UnsortedPositions { index: usize },
}
// reference.rs (cfg(test))
pub(crate) fn apply_ops(ops: &[Op], s: &str) -> String;  // naive forward walk, no canonical assumptions
// strategies.rs (cfg(test))
pub(crate) fn piece() -> impl Strategy<Value = String>;
pub(crate) fn text_strategy(max_pieces: usize) -> impl Strategy<Value = String>;
pub(crate) fn changeset_for(s: &str) -> impl Strategy<Value = ChangeSet>; // valid for `s`
pub(crate) fn raw_ops_for(s: &str) -> impl Strategy<Value = Vec<Op>>;    // non-canonical: zero-length ops, same-kind runs, Insert-then-Delete
```

**Canonical form** (the builder's invariant; every `ChangeSet` satisfies it):
1. No op has length 0 (`Retain(0)`, `Delete(0)`, `Insert("")` are dropped).
2. No two adjacent ops of the same kind (they merge; inserts concatenate).
3. A `Delete` is never directly after an `Insert`. `delete(n)` after an `Insert` merges into a `Delete` directly before that `Insert` if there is one, otherwise it goes in front of the `Insert`. So between two `Retain`s there is at most `Delete`, then at most `Insert`.
4. `len_before == Σ Retain + Σ Delete` and `len_after == Σ Retain + Σ Insert.len()`. `finish` returns `LengthMismatch { expected: len_before, actual: consumed }` otherwise, and `Overflow` if any running sum overflowed (track a sticky flag; never panic).

`from_edits` takes edits sorted by start. Edit `i` is invalid (`InvalidEdit { index: i }`) if `start > end`, `end > len_before`, or `start < previous end`. Touching edits and several inserts at one position are allowed (inserts concatenate in input order). `changeset_for(s)` generates up to 8 edits with ends snapped down to char boundaries of `s`, sorted, overlaps dropped, and builds through `from_edits`.

- [ ] **Step 1: Move strategies.** Move `piece` and `text_strategy` from `text/proptests.rs` to `src/strategies.rs` as `pub(crate)`, then import them back. Run `cargo test --offline -p flash-core` and expect 26 passed.
- [ ] **Step 2: Write failing unit tests** in `changeset/tests.rs`:
  - `builder_canonicalises`: `retain(2).retain(0).retain(3).insert("a").insert("").delete(1).insert("b").delete(2).retain(1)` with `len_before = 9` gives ops `[Retain(5), Delete(3), Insert("ab"), Retain(1)]`, `len_after = 8`.
  - `builder_rejects_length_mismatch`: `new(5).retain(3).finish()` is `Err(LengthMismatch { expected: 5, actual: 3 })`; `new(5).retain(6).finish()` is `Err(LengthMismatch { expected: 5, actual: 6 })`.
  - `builder_overflow_is_an_error`: `new(usize::MAX).retain(usize::MAX).retain(1).finish()` (consumed overflows) and `new(usize::MAX).retain(usize::MAX).insert("a").finish()` (`len_after` overflows) are both `Err(Overflow)`; `from_edits(usize::MAX, [(usize::MAX - 1..usize::MAX, "")])` succeeds.
  - `from_edits_cases`: same-position inserts concatenate; touching ranges allowed; `[(3..5, ""), (4..6, "")]` gives `InvalidEdit { index: 1 }`; `[(5..3, "")]` gives `InvalidEdit { index: 0 }`; `[(0..11, "")]` with `len_before = 10` gives `InvalidEdit { index: 0 }`.
  - `empty_document_changesets`: `identity(0)` has no ops and `is_identity()`; `from_edits(0, [(0..0, "hi")])` has ops `[Insert("hi")]` and `len_after == 2`.
- [ ] **Step 3: Run** `cargo test --offline -p flash-core changeset`. Expected: compile failure (types missing).
- [ ] **Step 4: Implement** `mod.rs` and `builder.rs` per the interfaces and canonical form above. `ChangeSetError`'s `Display` uses lowercase messages in the style of `TextError`.
- [ ] **Step 5: Write property tests** in `changeset/proptests.rs`:
  - `builder_output_is_canonical_and_matches_raw_ops`: for `(s, raw) in text_strategy(20).prop_flat_map(|s| (Just(s.clone()), raw_ops_for(&s)))`, feed `raw` to a builder with `len_before = s.len()`. The result satisfies canonical rules 1–4 (write `fn assert_canonical(&ChangeSet)`), and `reference::apply_ops(c.ops(), &s) == reference::apply_ops(&raw, &s)`.
  - `lengths_add_up`: for `changeset_for(s)`, `retained + deleted == len_before` and `retained + inserted == len_after`.
- [ ] **Step 6: Run** `cargo test --offline -p flash-core` and `cargo test --offline -p flash-core --features small-chunks`. Expected: all pass. Then run clippy `-D warnings`.
- [ ] **Step 7: Commit** `Add ChangeSet with canonical builder and reference model`.

---

### Task 2: validate, apply, invert

**Files:**
- Create: `src/changeset/apply.rs`
- Modify: `src/changeset/tests.rs`, `src/changeset/proptests.rs`, `src/text/mod.rs` (remove the three `#[allow(dead_code)]` on the mutators once used)

**Interfaces:**
- Consumes: Task 1 types; `Text::{byte_len, is_char_boundary, slice, insert, delete, replace}`.
- Produces:

```rust
impl ChangeSet {
    pub fn validate(&self, text: &Text) -> Result<(), ChangeSetError>;
    pub(crate) fn apply(&self, text: &mut Text) -> Result<(), ChangeSetError>;
    pub(crate) fn apply_per_op(&self, text: &mut Text) -> Result<(), ChangeSetError>;  // strategy A
    pub(crate) fn apply_rebuild(&self, text: &mut Text) -> Result<(), ChangeSetError>; // strategy B
    pub fn invert(&self, original: &Text) -> Result<ChangeSet, ChangeSetError>;
}
```

- `validate`: if `text.byte_len() != len_before`, return `LengthMismatch { expected: len_before, actual: text.byte_len() }`. Otherwise every cumulative old-text offset at which a Retain or Delete ends must satisfy `text.is_char_boundary`, else `NotCharBoundary { offset }` (the first bad one).
- `apply_per_op`: walk ops **in reverse**, starting from old position `len_before`. Each (Delete?, Insert?) group becomes one `text.replace(start..start + d, s)`, applied back to front so earlier offsets stay valid. No allocation beyond crop's.
- `apply_rebuild`: forward walk into a `TextBuilder`. Retains append `text.slice(..)` chunks, inserts append the string. Then `*text = builder.build()`.
- `apply` calls `validate` and then, for now, `apply_per_op`. Task 6 sets the strategy switch from measurements. After a successful `validate`, a mutator error is unreachable: handle it with `debug_assert!` and return the error, never `unwrap`.
- `invert(original)`: validate against `original`, then `Retain(n) → retain(n)`, `Delete(n) → insert(original[pos..pos+n])`, `Insert(s) → delete(s.len())`, built through a `ChangeSetBuilder::new(len_after)`.

- [ ] **Step 1: Write failing unit tests:**
  - `apply_basic`: `from_edits(11, [(0..5, "goodbye"), (11..11, "!")])` applied to `"hello world"` gives `"goodbye world!"`.
  - `apply_rejects_wrong_length`: `identity(3)` on `"ab"` gives `Err(LengthMismatch { expected: 3, actual: 2 })`, text unchanged.
  - `apply_rejects_boundary_inside_char`: `from_edits(4, [(1..2, "")])` on `"😀"` (4 bytes) gives `Err(NotCharBoundary { offset: 1 })`; the text still equals `"😀"`. Same check for `invert`.
  - `invert_basic`: for `"hello world"` and the edits above, `invert` applied to `"goodbye world!"` gives `"hello world"`.
  - `strategies_agree_on_multi_leaf_text`: on `"line\n".repeat(5_000)` with 50 spread edits, `apply_per_op` and `apply_rebuild` give equal text.
- [ ] **Step 2: Run** `cargo test --offline -p flash-core changeset`. Expected: FAIL (methods missing).
- [ ] **Step 3: Implement** `apply.rs` as specified.
- [ ] **Step 4: Write property tests** (`(s, c)` from `text_strategy(20)` + `changeset_for`; `t = Text::from(s)`):
  - `apply_matches_reference`: `apply` gives `reference::apply_ops(c.ops(), &s)`; so do `apply_per_op` and `apply_rebuild`.
  - `invert_round_trips`: let `t2 = apply(c, t)`, `inv = c.invert(&t)`. Then `apply(inv, t2) == t`, `inv.invert(&t2) == Ok(c)` (structural), and `inv.len_before() == c.len_after()`.
  - `invalid_changeset_leaves_text_unchanged`: a ChangeSet built for `s` applied to a different random text `u` either succeeds with the reference result (if `validate` passes) or fails with `u` unchanged.
- [ ] **Step 5: Run** both leaf-size configurations plus clippy. Expected: all pass.
- [ ] **Step 6: Commit** `Add ChangeSet validate, apply, and invert`.

---

### Task 3: compose

**Files:**
- Create: `src/changeset/compose.rs`
- Modify: `src/changeset/tests.rs`, `src/changeset/proptests.rs`, `src/changeset/mod.rs` (docs on the trap)

**Interfaces:**
- Consumes: Task 1 builder; Task 2 `apply` (tests only).
- Produces: `pub fn compose(&self, next: &ChangeSet) -> Result<ChangeSet, ChangeSetError>`. It returns `LengthMismatch { expected: self.len_after, actual: next.len_before }` on mismatch. The result has `len_before == self.len_before` and `len_after == next.len_after`.

**Algorithm** (two cursors over `self` = A and `next` = B, each holding the remaining part of its current op; output goes through `ChangeSetBuilder::new(A.len_before)`, which re-canonicalises):

| A head | B head | Emit | Advance |
|---|---|---|---|
| `Delete(d)` | any | `delete(d)` | A |
| not Delete | `Insert(s)` | `insert(s)` | B |
| `Retain(i)` | `Retain(j)` | `retain(m)`, where `m = min(i, j)` | both by `m` |
| `Retain(i)` | `Delete(j)` | `delete(m)` | both by `m` |
| `Insert(s)` | `Retain(j)` | `insert(&s[..m])`, where `m = min(len s, j)` | both by `m` |
| `Insert(s)` | `Delete(j)` | nothing (cancels) | both by `m` |
| none | none | finish | — |

When `m < s.len()` and `!s.is_char_boundary(m)`, return `NotCharBoundary { offset }`, where `offset` is the position in A's output (B's input). Any other end state (one side exhausted while the other still consumes) returns `LengthMismatch` defensively; it can't happen after the length check. Rows 1 and 2 must come first, in that order.

- [ ] **Step 1: Write failing unit tests:**
  - `compose_basic`: on `"abc"`, `a = from_edits(3, [(1..1, "XY")])` and `b = from_edits(5, [(2..4, "")])`. `compose(a, b)` applied to `"abc"` gives `"aXc"`.
  - `compose_rejects_length_mismatch`.
  - `compose_rejects_split_inside_inserted_char`: `a = from_edits(0, [(0..0, "é")])` (2 bytes) and `b = from_edits(2, [(0..1, "")])`. Result is `Err(NotCharBoundary { offset: 1 })`, no panic.
  - `compose_loses_positions_documented`: `a` deletes `"b"` from `"abc"` and `b` re-inserts `"b"` at 1. The texts agree, but `compose(a, b).map_pos(2, Assoc::Before)` differs from mapping through `a` then `b`. Mark `#[ignore]` until Task 4 adds `map_pos`, and un-ignore it there.
- [ ] **Step 2: Run** and confirm the tests fail.
- [ ] **Step 3: Implement** `compose.rs`. Doc comment on `compose`: "Guarantees text equivalence only. Positions must be mapped through the original sequence, not the composition (PLAN Stage 2, Known trap)."
- [ ] **Step 4: Property tests** (chain strategy: `s`, `a = changeset_for(s)`, `b = changeset_for(apply_ops(a, s))`, `c` likewise):
  - `compose_matches_sequential_apply`: `apply(compose(a, b), s) == apply(b, apply(a, s))`, and the result is canonical (`assert_canonical`).
  - `compose_is_associative_on_text`: `apply(compose(compose(a, b), c)) == apply(compose(a, compose(b, c)))`.
  - `compose_with_identity`: `compose(identity(a.len_before), a) == a` and `compose(a, identity(a.len_after)) == a` (structural).
  - `compose_with_inverse_is_identity_on_text`: `apply(compose(a, a.invert(t)), t) == t`.
- [ ] **Step 5: Run** both configurations plus clippy. Expected: pass.
- [ ] **Step 6: Commit** `Add ChangeSet compose`.

---

### Task 4: position mapping

**Files:**
- Create: `src/changeset/map.rs`
- Modify: `src/changeset/reference.rs`, `src/changeset/tests.rs`, `src/changeset/proptests.rs`, `src/changeset/mod.rs` (mapping-policy docs), `docs/DECISIONS.md` (009 mapping policy)

**Interfaces:**
- Produces:

```rust
impl ChangeSet {
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> Result<usize, ChangeSetError>;
    pub fn map_sorted(&self, positions: &mut [usize], assoc: Assoc) -> Result<(), ChangeSetError>;
}
// reference.rs
pub(crate) fn map_pos_naive(ops: &[Op], pos: usize, assoc: Assoc) -> usize; // linear scan from the start for each call
```

**Mapping policy** (write this verbatim into the module docs and DECISIONS 009). Canonical ops alternate `Retain` runs and *edit groups* `(Delete(d)?, Insert(s)?)`. An edit group sits at old offset `o` and new offset `n`. For an old position `q`:
- `q` strictly inside a Retain run `[o, o + k)` maps to `n + (q − o)`.
- `q` in the closed range `[o, o + d]` of an edit group (for a pure insert, only `q == o`) maps to `n` with `Assoc::Before`, or `n + len(s)` with `Assoc::After`.
  - So a pure insertion: `Before` stays left of the inserted text, `After` moves right of it.
  - A pure deletion: everything inside collapses to the deletion point.
  - A replacement: the whole replaced range collapses to the replacement's left or right edge.
- `q == len_before` with no trailing group maps to `len_after`.
- `q > len_before` returns `PositionOutOfBounds { position: q, len: len_before }`.

`map_sorted` first checks the slice: an element smaller than its predecessor gives `UnsortedPositions { index }`, and an element past `len_before` gives `PositionOutOfBounds`. Then it maps all positions in one pass over the ops. Cost is O(ops + positions), no allocation. On error the slice is unchanged.

- [ ] **Step 1: Write failing unit tests:**
  - `map_insert_assoc`: on `"ab"` with an insert of `"XY"` at 1, `map_pos(1, Before) == Ok(1)` and `map_pos(1, After) == Ok(3)`; `map_pos(0, After) == Ok(0)`; `map_pos(2, Before) == Ok(4)`.
  - `map_delete_collapses`: deleting `1..3` of `"abcd"` maps 1, 2 and 3 to 1 with both `Assoc` values; 4 maps to 2.
  - `map_replace_edges`: replacing `1..3` of `"abcd"` with `"XYZ"` maps 2 to 1 (`Before`) and to 4 (`After`).
  - `map_end_of_document`: inserting at the end, `map_pos(len, Before) == Ok(len)` and `After` gives `Ok(len_after)`.
  - `map_on_empty_document`: `identity(0).map_pos(0, _) == Ok(0)`; an insert of `"hi"` into empty text maps 0 to 0 (`Before`) and 2 (`After`); `map_pos(1, _)` gives `PositionOutOfBounds { position: 1, len: 0 }`.
  - `map_sorted_errors_leave_slice_unchanged`: `[3, 1]` gives `UnsortedPositions { index: 1 }`; `[0, 99]` gives `PositionOutOfBounds`. In both cases the slice is unchanged.
  - Un-ignore `compose_loses_positions_documented`.
- [ ] **Step 2: Run** and confirm the tests fail.
- [ ] **Step 3: Implement** `map.rs`. `map_pos` delegates to the same single-pass core as `map_sorted`, with a one-element view.
- [ ] **Step 4: Property tests** (`s`, `c = changeset_for(s)`, `t2 = apply(c, s)`):
  - `map_pos_matches_naive`: for every `q` in `0..=s.len() + 2` and both `Assoc` values, `map_pos` equals `Ok(map_pos_naive(..))` for `q <= len_before`, and `Err(PositionOutOfBounds{..})` otherwise.
  - `map_sorted_matches_map_pos`: for a sorted random subset of `0..=len_before`, `map_sorted` equals element-wise `map_pos`.
  - `mapped_positions_are_char_boundaries_and_monotonic`: for every char boundary `q` of `s`, the image is a char boundary of `t2`; images of sorted inputs are non-decreasing for each `Assoc`.
  - `map_through_chain_is_sequential`: for a chain `a`, `b`, mapping through `a` then `b` gives char boundaries of the final text. Positions are mapped through the sequence, never through the composition.
- [ ] **Step 5: Run** both configurations plus clippy. Expected: pass.
- [ ] **Step 6: Commit** `Add ChangeSet position mapping`.

---

### Task 5: fuzz target

**Owner prerequisite (network; the sandbox can't do it):** `! rustup toolchain install nightly && cargo install cargo-fuzz`, then after Step 3 `! cd crates/flash-core/fuzz && cargo +nightly fetch`.

**Files:**
- Create: `src/fuzzing.rs`, `crates/flash-core/fuzz/Cargo.toml`, `crates/flash-core/fuzz/fuzz_targets/changeset.rs`, `crates/flash-core/fuzz/.gitignore` (`target/`, `corpus/`, `artifacts/`, `coverage/`)
- Modify: `crates/flash-core/Cargo.toml` (feature `fuzzing = ["dep:arbitrary"]`, optional `arbitrary`, dev-dependency `arbitrary`), root `Cargo.toml` (`[workspace.dependencies] arbitrary = "1"`; `exclude = ["vendor", "crates/flash-core/fuzz"]`), `src/lib.rs` (`#[cfg(any(test, feature = "fuzzing"))] pub mod fuzzing;`)

**Interfaces:**
- Produces: `pub fn changeset_ops(data: &[u8])`. It never returns an error. It panics **only** when an invariant is violated.

**Body:**
1. With `arbitrary::Unstructured`, draw a `String` (truncate to 4 KiB on a char boundary) and two raw op lists. Op lengths come from `u16`, so they are often invalid.
2. Build both lists through `ChangeSetBuilder`. Errors are expected; skip the rest of the input.
3. If `a.validate(&t)` passes:
   - apply a clone of `t`;
   - assert the lengths add up;
   - assert the invert round trip;
   - assert `map_sorted` over all char boundaries gives non-decreasing char boundaries.
4. If `b` is valid for the result, assert `compose` matches sequential apply.
5. If `a.validate` fails, assert that `apply` on a clone also fails and leaves the clone equal to `t`.

The fuzz crate's `Cargo.toml`:
- `[package] name = "flash-core-fuzz"`, `publish = false`, edition 2024, `[package.metadata] cargo-fuzz = true`;
- dependencies `libfuzzer-sys = "0.4"` and `flash-core = { path = "..", features = ["fuzzing"] }`;
- its own `[workspace] members = ["."]`;
- **its own** `[patch.crates-io] crop = { path = "../../../vendor/crop" }`, since the root patch doesn't apply to a separate workspace;
- `[[bin]] name = "changeset"`, path `fuzz_targets/changeset.rs`, `test = false`, `doc = false`, `bench = false`.

- [ ] **Step 1: Write the stable driver test** `fuzz_entry_survives_random_bytes` in `src/fuzzing.rs` (`#[cfg(test)]`): proptest `prop::collection::vec(any::<u8>(), 0..4096)` calls `changeset_ops`. Then run it and expect FAIL (function missing).
- [ ] **Step 2: Implement** `changeset_ops`. Run `cargo test --offline -p flash-core fuzz` and expect PASS. Run `cargo clippy --offline -p flash-core --all-targets --features fuzzing -- -D warnings` and expect it clean.
- [ ] **Step 3: Create the fuzz crate.** Ask the owner to run the prerequisite fetch.
- [ ] **Step 4: Run** `cd crates/flash-core/fuzz && cargo +nightly fuzz run changeset --offline -- -max_total_time=600`. Expected: no crash. Record executions and the run time for the report. If the sandbox blocks the build or run, say so and give the owner the exact command.
- [ ] **Step 5: Commit** `Add ChangeSet fuzz target`, including `crates/flash-core/fuzz/Cargo.lock` if one is generated.

---

### Task 6: benchmarks and apply strategy

**Files:**
- Create: `crates/flash-core/benches/changeset.rs`
- Modify: `crates/flash-core/Cargo.toml` (`[[bench]] name = "changeset"`, `required-features = ["internals"]`), `src/internals.rs` (`changeset_apply`, `changeset_apply_per_op`, `changeset_apply_rebuild`, each `(&ChangeSet, &mut Text) -> Result<(), ChangeSetError>`), `benches/latency.rs` (row: validate + apply of a 1-op ChangeSet), `src/changeset/apply.rs` (strategy switch), `docs/DECISIONS.md` (010)

**Fixtures:**
- Texts: `medium` (1 MB) and `large` (100 MB) from `benches/common`.
- `n`-edit ChangeSets for `n ∈ {1, 100, 10_000}`: take `common::random_ranges(text, n, 16, seed)`, sort them by start, drop any range overlapping the previous one, and replace each with `"x"`.
- `apply` runs on `text.clone()` via `iter_batched` (`BatchSize::SmallInput`). That measures the realistic case: the document is shared with a snapshot, so the first edit path-copies.

**Benchmarks:**
- `build/{n}` (`from_edits`);
- `apply_per_op/{n}` and `apply_rebuild/{n}`;
- `invert/{n}`;
- `compose/{n}` (two `n`-edit ChangeSets in sequence);
- `map_sorted/{n}`: 1,000 sorted positions through an `n`-edit ChangeSet.

- [ ] **Step 1: Write the benchmarks.** Run `cargo bench --offline -p flash-core --features internals --bench changeset -- --quick` and expect them to complete.
- [ ] **Step 2: Measure the crossover.** Find the edit count where `apply_rebuild` beats `apply_per_op` for each fixture; add `n ∈ {1_000, 3_000}` if the crossover falls between 100 and 10k. Rebuild cost is expected to scale with document length and per-op cost with edit count. So the switch condition is `ops.len() * K > byte_len` with `K` taken from the measurements. If no crossover exists, keep `apply_per_op` only and delete `apply_rebuild`.
- [ ] **Step 3: Implement the switch** in `apply` with the measured constant and a comment pointing to DECISIONS 010. Keep `strategies_agree_on_multi_leaf_text` and `apply_matches_reference` green. Add a unit test on each side of the threshold.
- [ ] **Step 4: Full benchmark run** with `cargo bench --offline -p flash-core --features internals --bench changeset` and the latency harness. Collect the results to `bench/results/stage-2-*.json` with `bench/collect.py`.
- [ ] **Step 5: Run the gate commands** from Global Constraints. Expected: all pass.
- [ ] **Step 6: Commit** `Benchmark ChangeSet and choose the apply strategy`.

---

### Stage gate (after Task 6)

- [ ] Implementer review of `git diff stage-1..stage-2`.
- [ ] Independent adversarial review: a fresh-context reviewer given only `docs/PLAN.md` Stage 2, this plan, and the code. Its deliverable is failing tests or counterexamples. Fix or explicitly accept each finding, with severity.
- [ ] Write `docs/stages/stage-2.md` in Stage 1's format (sections A–J). Include algorithm explanations for canonicalisation, apply, invert, compose, and mapping: problem, why correct, complexity, corner cases, rejected alternatives.
- [ ] Update `docs/DECISIONS.md` (009 mapping policy, 010 apply strategy, plus 011 representation if `compact_str` vs `String` was measured) and `docs/STATUS.md`.
- [ ] Commit `Stage 2 report, decisions, and status`. Stop for owner approval.
