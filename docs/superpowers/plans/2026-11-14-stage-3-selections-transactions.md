# Stage 3 — Selections, Transactions, Multi-cursor, Walking Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Multi-range selections with a defined merge rule, grapheme-aware move/extend motions, transactions that turn a selection plus an intent into one ChangeSet and apply atomically, and a crude `flash <file>` terminal binary that drives them and records an end-to-end latency number.

**Architecture:** Three new `flash-core` modules beside `text/` and `changeset/`: `selection/` (`Range`, `Selection`, normalisation, mapping through a ChangeSet), `motion/` plus `width.rs` (motions over `&Text`, O(distance moved)), and `transaction/` (constructors with deterministic collision rules, validate-first atomic apply). `apps/flash` gains a `flash_app` library holding a throwaway `Skeleton` (state, key handling, full-redraw render into any `Write`), a `main.rs` that owns the terminal, and a benchmark that replays a fixed key trace into a sink.

**Tech Stack:** Rust 2024, crop 0.4.3 (vendored), `smallvec` 1 (selection storage, inline capacity 1), `unicode-width` 0.2 (display columns), `crossterm` 0.29 (skeleton only), proptest, Criterion.

**Spec:** `docs/PLAN.md` section 5, "Stage 3 — Selections, transactions, multi-cursor, and a walking skeleton", plus sections 2.4, 2.5, 2.6, 2.9, 2.11, 3, 4, and 6. DECISIONS 009 (mapping policy) and 010 (apply strategy) are binding.

## Global Constraints

- Rust edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` in `flash-core` and in the new `flash_app` library.
- Build and test with `--offline`. Verified 2026-11-14: `crossterm 0.29.0`, `smallvec 1.16.5`, `unicode-width 0.2.3` resolve and build offline from the local cache.
- New dependencies, exactly: root `[workspace.dependencies]` gets `smallvec = "1.15"`, `unicode-width = "0.2"`, `crossterm = "0.29"`. `flash-core` uses the first two and `apps/flash` uses crossterm. List all three in the stage report with reasons. Nothing else.
- Byte offsets everywhere. Every stored selection endpoint is a char boundary of its text (PLAN 2.4). Grapheme alignment is a property of motions, not an invariant.
- No panic reachable from any public method for any input. Fallible operations return `Result`. Never `unwrap` after validation. Unreachable mutator errors use `debug_assert!` and return the error, as `changeset/apply.rs` does.
- The merge rule is PLAN 2.5, verbatim: overlapping ranges merge; touching non-empty ranges stay separate; an empty range touching or inside another range merges into it. The primary survives merging.
- Atomicity follows DECISIONS 010: validate everything first, then mutate. Do not apply to a clone and swap.
- Never map positions through a composed ChangeSet (DECISIONS 009).
- Hot-path keys in the skeleton are hard-coded and temporary (PLAN 2.11). Stage 7 deletes them.
- One local commit per task on branch `stage-3`. Never push. Never add `.claude/`, `.mcp.json`, or `.gitmodules`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Gate commands (all must pass before the stage report): `cargo fmt --all -- --check`; `cargo clippy --offline --workspace --all-targets -- -D warnings` (plain and with `--all-features`); `cargo test --offline --workspace`; `PROPTEST_CASES=4096 cargo test --offline -p flash-core --release` (plain and with `--features small-chunks`).

## Review Focus

1. **Terminal escape injection.** A file containing ESC, CSI sequences, BEL, or other C0/C1 controls must never write those bytes to the terminal. Each control grapheme other than `\t` draws as `U+FFFD` in one cell. Pinned in Task 5 (`render_replaces_control_characters`).
2. **Degenerate terminal sizes.** A resize to 0×0, 1×1, or 2×1, and a wide character that straddles the last column, must not panic and must not draw past the width. Pinned in Task 5 (`render_survives_tiny_terminals`, `render_never_splits_wide_char`).
3. **Empty document.** Every motion from 0 returns 0; backspace and delete-forward are identity; insert gives `"x"` with the cursor at 1; the skeleton renders `""`. Pinned in Task 2 (`motions_on_empty_text`), Task 3 (`transactions_on_empty_text`), and Task 5 (`render_survives_tiny_terminals` includes `""`).
4. **Positions that are char boundaries but not grapheme boundaries** (between `\r` and `\n`, inside a combining sequence) arriving from outside: motions and backspace return char boundaries and never panic. Pinned in Task 2 (`motions_from_inside_cluster`) and Task 3 (`backspace_overlapping_deletes_merge`).
5. **Many cursors collapsing.** 10,000 cursors on one point normalise to one range in O(k log k), and the primary survives. Cursors whose deletions overlap merge into one. Pinned in Task 1 (`ten_thousand_cursors_merge`) and Task 3 (`backspace_overlapping_deletes_merge`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/flash-core/src/selection/mod.rs` | `Range`, `Selection`, `SelectionError`, normalisation, accessors, `validate` |
| `crates/flash-core/src/selection/map.rs` | `Selection::map` through a ChangeSet |
| `crates/flash-core/src/selection/{tests,proptests}.rs` | Hand cases; normalisation and mapping properties |
| `crates/flash-core/src/changeset/map.rs` | Add `map_sorted_with` (per-position `Assoc`); `map_sorted` delegates to it |
| `crates/flash-core/src/width.rs` | `grapheme_width`, `display_column` |
| `crates/flash-core/src/motion/mod.rs` | `Motion`, `Movement`, motion algorithms |
| `crates/flash-core/src/motion/reference.rs` | `#[cfg(test)]` naive String model for word and vertical motion |
| `crates/flash-core/src/motion/{tests,proptests}.rs` | Unicode motion cases; properties against the model |
| `crates/flash-core/src/transaction/mod.rs` | `Transaction`, `TransactionError`, `apply`, post-change selection check |
| `crates/flash-core/src/transaction/build.rs` | `insert`, `delete`, `replace`, `backspace`, `delete_forward` |
| `crates/flash-core/src/transaction/reference.rs` | `#[cfg(test)]` naive model: applies edits one at a time back to front |
| `crates/flash-core/src/transaction/{tests,proptests}.rs` | Collision cases; model and validity properties |
| `crates/flash-core/src/text/mod.rs` | Add `TextSlice::graphemes` |
| `crates/flash-core/src/strategies.rs` | Add `selection_for` |
| `crates/flash-core/benches/transaction.rs` | Criterion: 1 to 10k cursors |
| `crates/flash-core/benches/latency.rs` | Rows for 1-cursor transactions and motions |
| `apps/flash/src/lib.rs`, `apps/flash/src/skeleton.rs` | `flash_app::skeleton::Skeleton` |
| `apps/flash/src/main.rs` | CLI, terminal guard, event loop, optional latency log |
| `apps/flash/benches/skeleton.rs` | Replayed key trace into a sink: p50/p95/p99/max |

---

### Task 0: Branch

- [ ] Confirm `git status` shows only the untracked `.claude/`, `.gitmodules`, `.mcp.json`, and that `HEAD` is `stage-2` at `4e7b1d5`.
- [ ] `git switch -c stage-3`.

---

### Task 1: Range, Selection, normalisation, mapping

**Files:**
- Create: `src/selection/mod.rs`, `src/selection/map.rs`, `src/selection/tests.rs`, `src/selection/proptests.rs`
- Modify: `src/changeset/map.rs`, `src/changeset/proptests.rs`, `src/strategies.rs`, `src/lib.rs` (`pub mod selection;`, re-export `Range, Selection, SelectionError`), root `Cargo.toml`, `crates/flash-core/Cargo.toml` (`smallvec.workspace = true`), `docs/DECISIONS.md` (011)

**Interfaces (Produces):**

```rust
// changeset/map.rs
impl ChangeSet {
    /// Like `map_sorted`, but `assoc(i)` gives the Assoc for `positions[i]`. Same errors, same
    /// one pass, slice unchanged on error.
    pub fn map_sorted_with(&self, positions: &mut [usize], assoc: impl Fn(usize) -> Assoc)
        -> Result<(), ChangeSetError>;
}

// selection/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range { anchor: usize, head: usize, goal_column: Option<usize> }
impl Range {
    pub fn new(anchor: usize, head: usize) -> Range;          // goal_column None
    pub fn point(pos: usize) -> Range;
    pub fn anchor(&self) -> usize;
    pub fn head(&self) -> usize;
    pub fn start(&self) -> usize;                             // min(anchor, head)
    pub fn end(&self) -> usize;                               // max(anchor, head)
    pub fn span(&self) -> std::ops::Range<usize>;             // start..end
    pub fn is_empty(&self) -> bool;
    pub fn direction(&self) -> Direction;                     // Backward iff head < anchor
    pub fn goal_column(&self) -> Option<usize>;
    pub fn with_goal_column(self, goal: Option<usize>) -> Range;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection { ranges: SmallVec<[Range; 1]>, primary: usize }
impl Selection {
    pub fn point(pos: usize) -> Selection;
    pub fn single(range: Range) -> Selection;
    pub fn new(ranges: impl IntoIterator<Item = Range>, primary: usize) -> Result<Selection, SelectionError>;
    pub fn ranges(&self) -> &[Range];       // sorted, normalised, never empty
    pub fn primary(&self) -> &Range;
    pub fn primary_index(&self) -> usize;
    pub fn with_range(&self, range: Range) -> Selection;   // adds `range`, makes it primary, normalises
    pub fn primary_only(&self) -> Selection;
    pub fn validate(&self, text: &Text) -> Result<(), TextError>;  // check_offset on every endpoint; first error
    pub fn map(&self, changes: &ChangeSet) -> Result<Selection, ChangeSetError>;
}
#[non_exhaustive]
pub enum SelectionError { Empty, PrimaryOutOfBounds { primary: usize, len: usize } } // + Display, Error
// strategies.rs
pub(crate) fn selection_for(s: &str) -> impl Strategy<Value = Selection> + use<>;  // 1..6 ranges, char-boundary endpoints, random primary
```

Do not add a `len()` method (clippy `len_without_is_empty`). Callers use `ranges().len()`.

**Normalisation** (one private `fn normalise(ranges, primary) -> Selection`, used by every constructor and transform):
1. If the ranges are already sorted by `(start, end)`, skip sorting (O(k)). Otherwise sort by `(start, end)` and track where the primary goes (O(k log k)).
2. Sweep and keep a current group `[gs, ge)`. The next range `r` joins the group iff `r.start() < ge || (r.start() == ge && (r.is_empty() || gs == ge))`. Otherwise it starts a new group.
3. A merged group becomes the union `[gs, ge)`. Its direction and goal column come from the *kept* range: the input primary if the group contains it, otherwise the group's first range in sorted order. The union is backward iff the kept range is backward. An empty range counts as forward.
4. The output primary is the group that contains the input primary.

**Mapping** (`Selection::map`): collect the endpoints `[start0, end0, start1, end1, …]` into a `SmallVec<[usize; 2]>`. They are sorted because the selection is normalised. Call `map_sorted_with`: a range's start maps with `After`. Its end maps with `After` if the range is empty and with `Before` otherwise. So a cursor moves past text typed at it, and a non-empty range does not absorb text inserted at its edges. Rebuild each range keeping its direction, clear every goal column, and normalise (mapping can make ranges touch or coincide).

- [ ] **Step 1: Write failing unit tests** (`selection/tests.rs`). `r(a, h)` is `Range::new(a, h)`:
  - `normalise_sorts_and_tracks_primary`: `new([r(10,12), r(0,2), r(5,6)], 0)` gives ranges `[r(0,2), r(5,6), r(10,12)]` and `primary_index() == 2`.
  - `overlapping_ranges_merge`: `new([r(0,5), r(3,8)], 0)` gives `[r(0,8)]`.
  - `touching_non_empty_ranges_stay_separate`: `new([r(0,3), r(3,6)], 1)` keeps both ranges, with primary 1.
  - `empty_touching_or_inside_merges`: `[r(0,3), r(3,3)]` gives `[r(0,3)]`. `[r(3,3), r(3,6)]` gives `[r(3,6)]`. `[r(2,2), r(0,5)]` gives `[r(0,5)]`. `[r(4,4), r(4,4)]` gives `[r(4,4)]`.
  - `merged_direction_follows_primary`: `new([r(0,5), r(8,3)], 1)` gives `[r(8,0)]`. The same ranges with primary 0 give `[r(0,8)]`.
  - `merged_direction_without_primary_follows_first`: `new([r(5,0), r(3,8), r(20,21)], 2)` gives `[r(8,0), r(20,21)]` with primary 1.
  - `new_rejects_empty_and_bad_primary`: `new([], 0)` gives `Err(Empty)`; `new([r(0,0)], 1)` gives `Err(PrimaryOutOfBounds { primary: 1, len: 1 })`.
  - `validate_reports_bad_endpoints`: `point(1)` on `"é"` gives `Err(NotCharBoundary { offset: 1 })`; `point(3)` on `"ab"` gives `Err(OffsetOutOfBounds { offset: 3, len: 2 })`.
  - `with_range_and_primary_only`: `point(0).with_range(r(4,4))` has 2 ranges with primary 1; `.primary_only()` is `point(4)`.
  - `ten_thousand_cursors_merge`: `new((0..10_000).map(|_| r(0,0)), 9_999)` is `point(0)`.
  - Mapping (`ChangeSet::from_edits` on the named text):
    - `map_cursor_moves_past_insert`: `"ab"`, `point(1)`, insert `"XY"` at 1 gives `point(3)`.
    - `map_range_does_not_absorb_edge_inserts`: `"abcd"`, `r(1,3)`, edits `[(1..1,"X"), (3..3,"Y")]` give `r(2,4)`. A backward `r(3,1)` gives `r(4,2)`.
    - `map_deleted_range_collapses`: `"abcd"`, `r(1,3)`, delete `0..4` gives `point(0)`.
    - `map_touching_after_delete`: `"abc"`, `[r(0,1), r(2,3)]`, delete `1..2` gives `[r(0,1), r(1,2)]`, still two ranges. `[r(0,1), r(2,2)]` with primary 1 gives `[r(0,1)]` with primary 0.
    - `map_clears_goal_column` and `map_on_empty_document` (`""`, `point(0)`, insert `"hi"` gives `point(2)`).
- [ ] **Step 2: Run** `cargo test --offline -p flash-core selection`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement** `map_sorted_with` by generalising the existing single-pass core (`map_sorted_unchecked` takes the per-index closure; `map_sorted` passes `|_| assoc`). Then implement `selection/` as specified. Add `smallvec` to the manifests.
- [ ] **Step 4: Property tests.** In `changeset/proptests.rs`, `map_sorted_with_matches_map_pos`: random sorted positions and a random `Vec<Assoc>` give the same result as element-wise `map_pos`. In `selection/proptests.rs`, with `pub(crate) fn assert_normalised(sel: &Selection) -> Result<(), TestCaseError>`, which checks sortedness, that no adjacent pair satisfies the join condition of step 2, and that `primary < len`:
  - `normalise_invariants`: random ranges with endpoints in `0..=50` and a random primary. The output passes `assert_normalised`. Every input range lies inside some output range. The output primary contains the input primary. Every output endpoint is an input endpoint. Normalising again is a no-op.
  - `map_matches_naive`: for `s`, `c = changeset_for(s)` and `sel = selection_for(s)`, `sel.map(&c)` equals `Selection::new(...)` built from per-endpoint `reference::map_pos_naive` with the same `Assoc` rule and the same primary. The result validates against `apply_ops(c.ops(), &s)`.
- [ ] **Step 5: Run** `cargo test --offline -p flash-core`, plus `--features small-chunks`, plus clippy. Expected: pass.
- [ ] **Step 6: Write DECISIONS 011**, "Selection normalisation and mapping": the merge rule (PLAN 2.5), the kept-range direction rule, per-endpoint `Assoc`, goal columns cleared by mapping. Alternatives: the merged range always forward; direction from the last range (Helix). Reopen if Stage 7 modes need different merge directions.
- [ ] **Step 7: Commit** `Add Range and Selection with normalisation and mapping`.

---

### Task 2: Display width and motions

**Files:**
- Create: `src/width.rs`, `src/motion/mod.rs`, `src/motion/reference.rs`, `src/motion/tests.rs`, `src/motion/proptests.rs`
- Modify: `src/text/mod.rs` (`TextSlice::graphemes`), `src/lib.rs` (`pub mod motion; pub mod width;`, re-export `Motion, Movement`), manifests (`unicode-width`), `docs/DECISIONS.md` (012)

**Interfaces:**
- Consumes: Task 1 `Range`, `Selection` (normalising constructor); `Text::{check_offset, move_graphemes, line_of_offset, line_start, line_range, line, slice, line_count, byte_len}`.
- Produces:

```rust
// text/mod.rs: wraps crop's RopeSlice::graphemes (double-ended, lazy at both ends;
// allocates only for a grapheme that crosses a chunk boundary)
impl<'a> TextSlice<'a> { pub fn graphemes(&self) -> impl DoubleEndedIterator<Item = Cow<'a, str>> + 'a; }

// width.rs
/// `"\t"` advances to the next multiple of `tab_width` from `column`; anything else is
/// `UnicodeWidthStr::width(grapheme)` (controls are 1, ZWSP is 0, CJK and ZWJ emoji are 2).
pub fn grapheme_width(grapheme: &str, column: usize, tab_width: NonZeroU8) -> usize;
/// Display column of `offset` in its line: the sum of grapheme widths from the line start. O(column).
pub fn display_column(text: &Text, offset: usize, tab_width: NonZeroU8) -> Result<usize, TextError>;

// motion/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement { Move, Extend }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Grapheme { count: usize, direction: Direction },
    Word { count: usize, direction: Direction },
    LineStart,
    LineEnd,
    Vertical { count: usize, direction: Direction, tab_width: NonZeroU8 },
    DocumentStart,
    DocumentEnd,
}
impl Motion {
    /// Validates every range against `text`, moves each head, and normalises.
    pub fn apply(self, text: &Text, selection: &Selection, movement: Movement) -> Result<Selection, TextError>;
    pub fn apply_range(self, text: &Text, range: Range, movement: Movement) -> Result<Range, TextError>;
}
```

**Semantics.** A motion computes a new head `h'` from the head. `Move` gives `Range::point(h')`. `Extend` gives `Range::new(anchor, h')`. Only `Vertical` sets a goal column on the result. Every other motion clears it.
- `Grapheme`: `text.move_graphemes(head, count, direction)`.
- `LineStart` / `LineEnd`: the start and the content end (before the terminator) of `line_of_offset(head)`.
- `DocumentStart` / `DocumentEnd`: `0` / `byte_len()`.
- `Word` classes, taken from a grapheme's first char: whitespace (`char::is_whitespace`, so `\n`, `\r\n`, and `\t` count), word (`is_alphanumeric() || '_'`), punctuation (everything else). Forward, once per count: if the grapheme at the head is not whitespace, skip the run of its class; then skip whitespace. Backward: skip whitespace before the head, then skip the run of the class of the grapheme before; stop at the run's start. Both iterate `slice(head..)` / `slice(..head)` graphemes lazily, so the cost is O(distance + log n). Both stop at the text's edges.
- `Vertical`: goal `g = range.goal_column()` or `display_column(head)`. Target line `= line ± count`, clamped to `0..line_count()`. On the target line, walk graphemes with running column `c`. Return the start of the first grapheme with `width > 0` and `c <= g < c + width`. If there is none, return the line's content end. The result carries `goal_column = Some(g)`. Cost is O(columns walked on both lines).
- Inputs that are not grapheme boundaries are allowed. Results are always char boundaries, and grapheme boundaries whenever the head was one.

- [ ] **Step 1: Write failing unit tests** (`motion/tests.rs`; `T4 = NonZeroU8::new(4).unwrap()`):
  - `grapheme_motion_over_clusters`: on `"e\u{301}x"`, Right 1 from 0 gives 3; Left 1 from 4 gives 3; Right 5 from 0 gives 4. On the ZWJ family plus `"a"` (18 + 1 bytes), Right 1 from 0 gives 18. On `"a\r\nb"`, Right 1 from 1 gives 3.
  - `move_collapses_extend_keeps_anchor`: on `"abc"`, `r(0,2)` Right 1 with Move gives `point(3)`; `point(0)` Right 2 with Extend gives `r(0,2)`.
  - `line_start_and_end`: on `"ab\r\ncd"`, from head 5 LineStart gives 4 and LineEnd gives 6; from head 1 LineEnd gives 2. On `"a\n"`, LineEnd from 0 gives 1 and from 2 gives 2.
  - `word_forward`: `"foo bar"` 0 → 4; `"foo.bar"` 0 → 3, 3 → 4; `"  foo"` 0 → 2; `"a\r\nb"` 0 → 3; `"foo bar baz"` count 2 from 0 → 8; `"foo bar"` 4 → 7.
  - `word_backward`: `"foo bar"` 7 → 4, 4 → 0, 5 → 4; `"foo  "` 5 → 0.
  - `word_over_combining_marks`: on `"e\u{301}e\u{301} x"`, forward from 0 gives 7; backward from 8 gives 7, and from 7 gives 0.
  - `vertical_remembers_goal`: on `"abcdef\nab\nabcdef"`, Down from head 5 gives 9 with goal `Some(5)`, then Down gives 15. A Grapheme motion afterwards clears the goal.
  - `vertical_expands_tabs`: on `"\tx\nabcdefg"`, Down from 1 (column 4) gives 7.
  - `vertical_wide_chars`: on `"abcd\n漢字"`, Down from 3 gives 8, the start of 字.
  - `vertical_clamps_at_edges`: Up from 3 on `"abcd"` gives 3; Down 100 from 0 on `"a\nb\nc"` gives 4.
  - `motions_on_empty_text` (Review Focus 3): every `Motion` variant, both movements, from `point(0)` on `""` gives `point(0)` or `r(0,0)`.
  - `motions_from_inside_cluster` (Review Focus 4): on `"a\r\nb"` from head 2, LineEnd gives 1, Right gives 3, and Word forward gives 3. On `"e\u{301}"` from head 1, every variant returns a char boundary.
  - `motion_rejects_invalid_ranges`: `point(9)` on `"ab"` gives `Err(OffsetOutOfBounds { offset: 9, len: 2 })`; `point(1)` on `"é"` gives `Err(NotCharBoundary { offset: 1 })`.
  - `converging_cursors_merge`: `[point(2), point(5)]` with primary 1, DocumentStart, gives `point(0)` with primary 0.
  - `width_rules`: `grapheme_width("\t", 1, T4) == 3`; `"漢"` gives 2; `"e\u{301}"` gives 1; the ZWJ family gives 2; `"\u{1b}"` gives 1; `"\u{200b}"` gives 0. On `"a\tb"`, `display_column` at 2 is 4.
- [ ] **Step 2: Run** `cargo test --offline -p flash-core motion width`. Expected: FAIL.
- [ ] **Step 3: Implement** `TextSlice::graphemes`, `width.rs`, and `motion/` as specified.
- [ ] **Step 4: Write `motion/reference.rs`**: `word_naive(s: &str, head: usize, count: usize, dir: Direction) -> usize` and `vertical_naive(s: &str, head: usize, goal: Option<usize>, count: usize, dir: Direction, tab: NonZeroU8) -> (usize, usize)`. Both work over `s.grapheme_indices(true)` of the whole string and over `s.split('\n')`-style line splitting that follows the cursor-row model. Then the property tests (`s` from `text_strategy(20)`):
  - `word_and_vertical_match_reference`: for every grapheme boundary `h` of `s`, counts 1..=3, and both directions, `Motion::apply_range` matches the naive result.
  - `results_are_boundaries`: for every char boundary `h`, every variant, and both movements, the head is a char boundary in `0..=len`, and a grapheme boundary if `h` was one. With `Extend` the anchor is unchanged.
  - `grapheme_round_trip`: for a grapheme boundary `h < len`, Right 1 then Left 1 gives `h`.
- [ ] **Step 5: Run** both leaf-size configurations plus clippy. Expected: pass.
- [ ] **Step 6: Write DECISIONS 012**, "Display width and word classes": unicode-width 0.2 per grapheme, tabs to tab stops, controls 1 cell (drawn as U+FFFD by renderers), the three word classes, the vertical goal rule. Alternatives: char-based classes (they split combining marks); `unicode-segmentation` word bounds (no punctuation/word distinction for motions). Reopen if Stage 7 makes word definition an option (PLAN 2.11).
- [ ] **Step 7: Commit** `Add grapheme-aware motions and display width`.

---

### Task 3: Transactions

**Files:**
- Create: `src/transaction/mod.rs`, `src/transaction/build.rs`, `src/transaction/reference.rs`, `src/transaction/tests.rs`, `src/transaction/proptests.rs`
- Modify: `src/lib.rs` (`pub mod transaction;`, re-export `Transaction, TransactionError`), `src/changeset/apply.rs` (drop the `dead_code` allows: `Transaction::apply` is now the caller), `docs/DECISIONS.md` (013)

**Interfaces:**
- Consumes: Task 1 (`Selection`, `normalise` via `Selection::new`, `map`, `validate`); `ChangeSetBuilder`; `ChangeSet::{validate, apply}`; `Text::move_graphemes`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction { changes: ChangeSet, selection: Option<Selection> }
impl Transaction {
    pub fn new(changes: ChangeSet, selection: Option<Selection>) -> Transaction;
    pub fn changes(&self) -> &ChangeSet;
    pub fn selection(&self) -> Option<&Selection>;
    pub fn insert(text: &Text, selection: &Selection, s: &str) -> Result<Transaction, TransactionError>;
    pub fn delete(text: &Text, selection: &Selection) -> Result<Transaction, TransactionError>;
    pub fn replace(text: &Text, selection: &Selection, s: &str) -> Result<Transaction, TransactionError>;
    pub fn backspace(text: &Text, selection: &Selection) -> Result<Transaction, TransactionError>;
    pub fn delete_forward(text: &Text, selection: &Selection) -> Result<Transaction, TransactionError>;
    /// Public until `Document::commit` (Stage 4) becomes the only caller; then `pub(crate)`.
    pub fn apply(&self, text: &mut Text, selection: &mut Selection) -> Result<(), TransactionError>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionError { ChangeSet(ChangeSetError), Selection(TextError) }
// + Display, Error (with source()), From<ChangeSetError>, From<TextError>
```

**Constructors.** Each one first runs `selection.validate(text)`, which fails with `Selection(_)`. It then walks the ranges in order with a running `delta` (new offset − old offset), emits edits through one `ChangeSetBuilder::with_capacity(len, 2k + 1)`, and computes each result range directly. `n` is the edit's start in the new text (`edit.start + delta`, with `delta` taken before this edit). Every result has `goal_column` None and goes through normalisation.

| Intent | Edit for range | Result range |
|---|---|---|
| `insert(s)` | `head..head` ← `s` | empty: `point(n+|s|)`. Forward: `(anchor+delta, n+|s|)`. Backward: `(anchor+delta+|s|, n+|s|)` |
| `delete` | `start..end` ← `""` (none if empty) | `point(n)` |
| `replace(s)` | `start..end` ← `s` | covers `[n, n+|s|)`, keeping its direction (empty counts as forward) |
| `backspace` | non-empty: `start..end`. Empty: `move_graphemes(head,1,Backward)..head` | `point(n)` |
| `delete_forward` | non-empty: `start..end`. Empty: `head..move_graphemes(head,1,Forward)` | `point(n)` |

**Collision rules (deterministic; write them on the module docs and in DECISIONS 013):**
- *Same-position inserts*: two touching ranges can share a head. Each range inserts its own copy, in selection order, and each head lands right after its own copy. `ChangeSet::from_edits`-style concatenation gives that order.
- *Overlapping deletes* (only `backspace`/`delete_forward`, because a cursor can sit inside a grapheme cluster or next to a non-empty range): sort the edits by start, which is already true. Then merge each edit into the previous one while `next.start < prev.end`, giving the union. Every range whose edit merged collapses to `point(n)` of the union, and normalisation merges those points. Touching edits stay separate.

**`apply`** (validate first, then mutate; DECISIONS 010):
1. `self.changes.validate(text)?`.
2. The new selection is either `Some(s)`, checked against the post-change text *without building it*, or `None`, in which case `selection.validate(text)?` and then `selection.map(&self.changes)?`.
3. `self.changes.apply(text)` (cannot fail now: `debug_assert!` and return the error), then `*selection = new`.

Post-change check (private `fn check_after(changes: &ChangeSet, old: &Text, positions: &[usize]) -> Result<(), TextError>`, where `positions` are the endpoints in sorted order). Walk the ops tracking `(old, new)` and the sorted endpoints together. A position `p` inside a Retain run maps to old offset `old + (p − new)`, which must satisfy `old_text.is_char_boundary`. Inside an Insert `s` it needs `s.is_char_boundary(p − new)`. `p > len_after` gives `OffsetOutOfBounds { offset: p, len: len_after }`. A bad boundary gives `NotCharBoundary { offset: p }`. Errors are in new-text coordinates. Cost is O(ops + k log n).

- [ ] **Step 1: Write failing unit tests** (`transaction/tests.rs`):
  - `insert_at_cursors`: `"ab\ncd"` with `[point(1), point(4)]` and insert `"X"` gives `"aXb\ncXd"` with `[point(2), point(6)]`.
  - `insert_extends_forward_range`: `"abc"`, `r(0,2)` gives `"abXc"` with `r(0,3)`. `insert_before_backward_range`: `"abc"`, `r(2,0)` gives `"Xabc"` with `r(3,1)`.
  - `same_position_inserts`: `"abcdef"`, `[r(1,3), r(5,3)]`, `"X"` gives `"abcXXdef"` with `[r(1,4), r(7,5)]`.
  - `delete_ranges`: `"hello world"`, `[r(0,5), r(6,11)]` gives `" "` with `[point(0), point(1)]`. `delete_touching_ranges_merge_points`: `"abcd"`, `[r(0,2), r(2,4)]` gives `""` with `point(0)`.
  - `replace_ranges`: `"a b"`, `[r(0,1), r(3,2)]`, `"XY"` gives `"XY XY"` with `[r(0,2), r(5,3)]`.
  - `backspace_cursor_deletes_grapheme`: `"ae\u{301}"` from `point(4)` gives `"a"` with `point(1)`. `"a\r\n"` from `point(3)` gives `"a"` with `point(1)`. From `point(0)` the result is identity changes with `point(0)`.
  - `backspace_overlapping_deletes_merge` (Review Focus 4, 5): `"e\u{301}"`, `[point(1), point(3)]` gives `""` with `point(0)`. `"ae\u{301}"` with `[r(0,2), point(4)]`: the cursor's grapheme edit `1..4` overlaps the range's `0..2`. Their union `0..4` gives `""` with `point(0)`.
  - `delete_forward_cases`: `"ab"` from `point(0)` gives `"b"` with `point(0)`. At `point(2)` the changes are identity.
  - `transactions_on_empty_text` (Review Focus 3): insert `"x"` into `""` at `point(0)` gives `"x"` with `point(1)`. Backspace, delete-forward, and delete are identity with `point(0)`.
  - `constructors_reject_invalid_selection`: `point(5)` on `"ab"` gives `Err(Selection(OffsetOutOfBounds { offset: 5, len: 2 }))`.
  - `apply_rejects_wrong_text`: a transaction built for `"abc"` applied to `"ab"` gives `Err(ChangeSet(LengthMismatch { .. }))`. Text and selection are unchanged.
  - `apply_rejects_bad_explicit_selection`: `new(from_edits(0, [(0..0, "é")]), Some(point(1)))` on `""` gives `Err(Selection(NotCharBoundary { offset: 1 }))`. With `point(3)` it gives `OffsetOutOfBounds { offset: 3, len: 2 }`. Both leave state unchanged.
  - `apply_without_selection_maps`: `new(from_edits(2, [(0..0, "X")]), None)` on `"ab"` moves `point(1)` to `point(2)`.
- [ ] **Step 2: Run** `cargo test --offline -p flash-core transaction`. Expected: FAIL.
- [ ] **Step 3: Implement** `transaction/` as specified.
- [ ] **Step 4: Write `transaction/reference.rs`**: `pub(crate) enum Intent { Insert(String), Delete, Replace(String), Backspace, DeleteForward }` and `pub(crate) fn naive(s: &str, ranges: &[Range], primary: usize, intent: &Intent) -> (String, Selection)`. It computes each range's edit (grapheme steps via `s.grapheme_indices(true)`), coalesces overlapping deletes with its own loop, and gives each edit's ranges their results in pre-edit coordinates. It then **applies the edits one at a time from last to first** with `String::replace_range`, and after each one adds that edit's delta to every result position belonging to later edits. Then the property tests:
  - `transaction_matches_naive_model`: for `s`, `sel = selection_for(s)`, and every `Intent` (insert and replace strings from `text_strategy(2)`), building and applying gives the naive text and selection.
  - `selection_valid_after_every_step`: up to 20 random steps (any motion or intent) from `selection_for(s)`. After each step, `selection.validate(&text)` is `Ok`, `assert_normalised` holds, and `text` equals a parallel `String` advanced with `apply_ops(txn.changes().ops(), …)`.
  - `failed_apply_leaves_state_unchanged`: a transaction built for `s` and applied to an unrelated `u` either succeeds with `apply_ops` of `u` or fails with `u` and the selection unchanged.
- [ ] **Step 5: Run** both leaf-size configurations plus clippy. Expected: pass.
- [ ] **Step 6: Write DECISIONS 013**, "Transaction collision rules and atomic apply": the two rules above; validate-first with the post-change selection check instead of clone-and-swap (consistent with 010); `apply` temporarily `pub`. Alternatives: one shared copy for same-position inserts (loses per-cursor typing); rejecting overlapping deletes (a cursor inside a cluster would make backspace fail). Reopen when Stage 5 history needs grouped transactions.
- [ ] **Step 7: Commit** `Add transactions with multi-cursor collision rules`.

---

### Task 4: Benchmarks

**Files:**
- Create: `crates/flash-core/benches/transaction.rs`
- Modify: `crates/flash-core/Cargo.toml` (`[[bench]] name = "transaction"`, `harness = false`, no required features), `benches/latency.rs`, `benches/common/mod.rs` (add `pub fn cursors(text: &Text, n: usize, layout: Layout) -> Selection` and `pub enum Layout { Clustered, Spread }`)

**Fixtures.** `medium` (1 MB) and `large` (100 MB). `n ∈ {1, 10, 100, 1_000, 10_000}` cursors. `Clustered`: points at the starts of `n` consecutive lines beginning at `line_count / 2`. `Spread`: points at `line_start(i * line_count / n)`. Primary is the last range.

**Criterion** (reuse `changeset.rs`'s `config()`; group `transaction/{fixture}/{layout}`; `iter_batched` with `(text.clone(), selection.clone())` setup, `BatchSize::SmallInput`):
- `insert/{n}`: `Transaction::insert(&t, &sel, "x")` then `apply`. This is the full transaction: build, validate, apply, and the new selection.
- `backspace/{n}`: the same with `backspace`.
- `map/{n}`: `Transaction::new(from_edits(len, [(len/2..len/2, "x")]), None).apply`, which measures `Selection::map` over `n` ranges.

**Latency rows** (`latency.rs`, every fixture, setup `(base.clone(), Selection::point(offsets[i]))`): `txn insert char (1 cursor)`, `txn backspace (1 cursor)`, `motion right`, `motion down`, and `motion word forward` (motions use the base text, no clone).

- [ ] **Step 1: Write both.** Run `cargo bench --offline -p flash-core --bench transaction -- --quick`. Expected: completes.
- [ ] **Step 2: Full runs.** Run `cargo bench --offline -p flash-core --bench transaction -- --save-baseline stage-3`, then `bench/collect.py stage-3 bench/results/stage-3-criterion.json`. Run `FLASH_BENCH_JSON=bench/results/stage-3-latency.jsonl cargo bench --offline -p flash-core --features internals --bench latency`.
- [ ] **Step 3: Check against PLAN section 4 targets** and record the outcome either way in the stage report: 1-cursor insert p99 well under 100 µs on `medium`; 1,000 cursors under 16 ms; the 10,000 figure reported. Motions on `large` must stay within 2x of `medium` (O(distance), not O(document)). If they don't, stop and investigate with superpowers:systematic-debugging before continuing.
- [ ] **Step 4: Commit** `Benchmark transactions and motions`.

---

### Task 5: Walking skeleton

**Files:**
- Create: `apps/flash/src/lib.rs` (`#![forbid(unsafe_code)]`, `pub mod skeleton;`, docs: "throwaway; rewired onto the headless editor in Stage 4, replaced in Stage 6"), `apps/flash/src/skeleton.rs`, `apps/flash/benches/skeleton.rs`
- Modify: `apps/flash/Cargo.toml` (`[lib] name = "flash_app"`, `crossterm.workspace = true`, `[[bench]] name = "skeleton"`, `harness = false`), `apps/flash/src/main.rs`

**Interfaces:**
- Consumes: `Text`, `Selection`, `Range`, `Motion`, `Movement`, `Transaction`, `width::{grapheme_width, display_column}`, `Direction`.
- Produces:

```rust
pub struct Skeleton { /* text, selection, top line, width, height, name, last latency, message */ }
impl Skeleton {
    pub fn new(name: String, text: Text, width: u16, height: u16) -> Skeleton;   // selection point(0)
    pub fn handle(&mut self, event: &crossterm::event::Event);                // never panics
    pub fn render(&mut self, out: &mut impl std::io::Write) -> std::io::Result<()>; // queues a full frame, does not flush
    pub fn should_quit(&self) -> bool;
    pub fn set_last_latency(&mut self, nanos: u64);
    pub fn text(&self) -> &Text;
    pub fn selection(&self) -> &Selection;
}
```

**Keys** (handle `KeyEventKind::Press` and `Repeat` only; tab width 4):

| Key | Action |
|---|---|
| char without Ctrl/Alt | `Transaction::insert` of that char |
| Enter / Tab | insert `"\n"` / `"\t"` |
| Backspace / Delete | `backspace` / `delete_forward` |
| ←/→, ↑/↓ | `Grapheme` / `Vertical` by 1, `Move`; with Shift, `Extend` |
| Ctrl+←/→ | `Word` by 1 (Shift: extend) |
| Home/End, Ctrl+Home/End | `LineStart`/`LineEnd`, `DocumentStart`/`DocumentEnd` |
| Alt+↓ / Alt+↑ | add a cursor: `Vertical` ±1 applied to the last (↓) or first (↑) range as a point, then `with_range` (becomes primary) |
| Esc | `primary_only()` collapsed to a point at its head |
| Ctrl+Q | quit |
| `Event::Paste(s)` | one `insert` transaction |
| `Event::Resize(w, h)` | store the new size |

A failed core call puts its error in the status message and changes nothing.

**Render** (full redraw every frame): text rows `= height.saturating_sub(1)`. Before drawing, scroll so the primary head's line is inside `[top, top + rows)`. For each row, `MoveTo(0, row)`, then draw the line's graphemes left to right with `grapheme_width`. A tab is drawn as spaces. Any grapheme containing a `char::is_control` char other than `\t` is drawn as `U+FFFD` in one cell. Stop before a grapheme that would cross `width`. A cell is drawn reversed if its offset is inside a non-empty range, or is the head of a non-primary range. An empty secondary cursor at a line's end draws a reversed space. Find the first relevant range with `partition_point`, so the cost is O(visible cells + log k). Then `Clear(UntilNewLine)`. The last row is the status line: `"{name}  {k} cursors  {line+1}:{col+1}  {µs} µs  {message}"`, truncated to `width`. Finish with `MoveTo` the primary head's cell (column clamped to `width − 1`) and `Show`. If `width == 0` or `height == 0`, queue nothing.

**main.rs.** Usage: `flash <file>`; no args or `--help` print usage. Read with `fs::read_to_string`. Errors, including non-UTF-8, print `flash: {path}: {error}` and exit with code 1. Install a terminal guard (raw mode, alternate screen, bracketed paste). Its `Drop` restores the terminal, and a panic hook restores it before the panic message prints. Loop: `event::read()`, take `Instant::now()`, `handle`, `render` into a `BufWriter<Stdout>`, `flush`, then record `elapsed` with `set_last_latency` and push it to a `Vec<u64>`. Exit after `should_quit()`. If the `FLASH_LATENCY_LOG` env var names a path, append one JSON line `{"source":"interactive","events":N,"p50_ns":…,"p95_ns":…,"p99_ns":…,"max_ns":…}`. There is no save, by design.

**Skeleton bench** (`apps/flash/benches/skeleton.rs`, reusing fixtures via `#[path = "../../../crates/flash-core/benches/common/mod.rs"] mod common;`): fixtures `small`, `medium`, `one_line`, `unicode`; viewports 80×24 and 200×60. The trace is a fixed 20-event pattern repeated 100 times: `x`×8, Backspace×2, →, ←, ↓×2, ↑, Ctrl+→, End, Home, Enter, Alt+↓. It starts at the middle line. Time each `handle` + `render` into a reused `Vec<u8>` (cleared per event). Print and append to `FLASH_BENCH_JSON` lines `{"fixture","viewport","op":"skeleton event","p50_ns","p95_ns","p99_ns","max_ns"}`.

- [ ] **Step 1: Write failing tests** (`skeleton.rs`, `#[cfg(test)]`, with helper `key(code, mods) -> Event`):
  - `typing_moving_and_deleting`: on `"ab\ncd"` at `point(0)`, typing `x` then →, ↓, Backspace gives `"xab\nc"` with `point(5)`.
  - `alt_down_adds_cursor_and_typing_hits_both`: on `"ab\ncd"`, Alt+↓ gives `[point(0), point(3)]` with primary 1. Then `y` gives `"yab\nycd"`. Esc then leaves `point(5)`.
  - `paste_is_one_insert`: with two cursors, `Event::Paste("p\nq")` inserts `"p\nq"` at both.
  - `ctrl_q_quits`; `backspace_at_start_is_harmless` (no change, no message).
  - `render_replaces_control_characters` (Review Focus 1): render `"a\u{1b}[2Jb\u{7}"` at 80×24 into a `Vec<u8>`. Assert that `"\x1b[2J"` and the byte `0x07` never appear, and that `"\u{fffd}"` appears twice. Render emits only `MoveTo`, `Clear(UntilNewLine)`, attributes, and `Show`, never `Clear(All)`.
  - `render_survives_tiny_terminals` (Review Focus 2, 3): for texts `""`, `"abc"`, and `"漢字\tx"`, and sizes (0,0), (1,1), (2,1), (1,5), resize then render returns `Ok` without panicking.
  - `render_never_splits_wide_char`: on `"a漢"` at width 2, the first row shows `"a"` and not `"漢"`.
- [ ] **Step 2: Run** `cargo test --offline -p flash`. Expected: FAIL.
- [ ] **Step 3: Implement** `Skeleton`, `main.rs`, and the bench as specified.
- [ ] **Step 4: Run** `cargo test --offline -p flash`, clippy for the workspace, and `cargo bench --offline -p flash --bench skeleton` with `FLASH_BENCH_JSON=bench/results/stage-3-skeleton.jsonl`. Expected: pass, and numbers are recorded.
- [ ] **Step 5: Owner smoke test** (needs a real TTY, which the sandbox lacks): ask the owner to run `! FLASH_LATENCY_LOG=bench/results/stage-3-interactive.jsonl cargo run --offline --release -p flash -- crates/flash-core/src/text/mod.rs`, type and move for about 30 s with two cursors, quit with Ctrl+Q, and confirm the terminal is restored. Record the line in the report, or note it as not run.
- [ ] **Step 6: Commit** `Add walking-skeleton terminal binary`.

---

### Stage gate (after Task 5)

- [ ] Run every gate command in Global Constraints. Expected: all pass. Record exact output counts.
- [ ] Implementer review of `git diff stage-2..stage-3`.
- [ ] Independent adversarial review: a fresh-context reviewer given only `docs/PLAN.md` Stage 3, this plan, and the code. Its deliverable is failing tests or counterexamples. Fix or explicitly accept each finding, with severity.
- [ ] Write `docs/stages/stage-3.md` in Stage 2's format (sections A–J). Include algorithm explanations for normalisation, selection mapping, word and vertical motion, the transaction walk and collision rules, and the post-change selection check: problem, why correct, complexity, corner cases, rejected alternatives. List the three new dependencies with reasons. Give the benchmark numbers with machine and build settings, the skeleton's sink latency, and the interactive line if the owner ran it.
- [ ] Update `docs/DECISIONS.md` (011–013 written in tasks; check them against what shipped) and `docs/STATUS.md`.
- [ ] Commit `Stage 3 report, decisions, and status`. Stop for owner approval.
