# Stage 3 — Selections, transactions, multi-cursor, walking skeleton

Status: **complete, awaiting owner approval.** Branch `stage-3`, local commits
only (nothing pushed). Plan:
[2026-11-14-stage-3-selections-transactions.md](../superpowers/plans/2026-11-14-stage-3-selections-transactions.md).

## A. Status

| Requirement (PLAN Stage 3) | State |
|---|---|
| `Range`, `Selection` (sorted, non-overlapping, primary, `SmallVec` inline 1) | Done (`selection/`) |
| Normalisation by the 2.5 merge rule; primary survives | Done; direction rule in DECISIONS 011 |
| Motions in *move* and *extend* forms | Done (`Motion::apply`, `Movement`) |
| Selection mapping per endpoint (cursor `After`; range does not absorb edge inserts) | Done (`Selection::map`, new `ChangeSet::map_sorted_with`) |
| Grapheme-aware motions: left/right, word, line start/end, up/down with goal column, document start/end | Done (`motion/`, `width.rs`) |
| Motions O(distance moved), never O(document) | Done, except up/down: O(columns on the two lines) — see H |
| `Transaction { changes, selection }` and insert / delete / replace / backspace constructors (+ delete forward) | Done (`transaction/`) |
| Deterministic collision rules | Done (DECISIONS 013) |
| Walking skeleton `flash <file>` (raw mode, full redraw, type/delete/move/quit, no save) | Done (`apps/flash`); end-to-end number in F |
| Tests: normalisation, Unicode motion, randomised multi-selection edits vs a naive back-to-front model, validity after every transaction, failed transaction changes nothing | Done (section D) |
| Benchmarks: 1 / 10 / 100 / 1k / 10k cursors, clustered and spread, full transaction | Done (section F) |

Deviations from the plan (each ledgered as a ruling; listed in G):

- crop's `RopeSlice::graphemes` turned out to be buggy, so the plan's
  `TextSlice::graphemes` wrapper was replaced by `Text::graphemes`, built on
  the stepping core of `move_graphemes` (DECISIONS 012).
- `Selection::map` reorders the endpoints of a range strictly inside a
  replaced span (it would otherwise come out inverted).
- Small API additions for the skeleton: `Skeleton::{message, set_selection}`
  and `flash_app::latency`.

## B. Commits

Branch `stage-3`, from `4e7b1d5` (Stage 2 close):

| Commit | Content |
|---|---|
| `7e51a07` | Stage 3 implementation plan |
| `1852b81` | `Range`, `Selection`, normalisation, `Selection::map`, `ChangeSet::map_sorted_with`; `smallvec`; DECISIONS 011 |
| `073bb33` | `width.rs`, `motion/`, `Text::graphemes` (replacing crop's iterator), `move_graphemes` on the shared core; `unicode-width`; DECISIONS 012 |
| `b8c40fe` | `Transaction`, constructors, validate-first `apply` with the post-change selection check; DECISIONS 013 |
| `60da591` | `benches/transaction.rs`, latency rows, cursor layouts |
| `a29da88` | Walking skeleton (`flash_app::skeleton`, `main.rs`, skeleton bench); `crossterm` |
| (closing commit) | Review attack tests, benchmark results, this report, status |

New dependencies: `smallvec 1.16` (selection storage; one range needs no
heap allocation), `unicode-width 0.2` (display columns for vertical motion
and rendering), `crossterm 0.29` (skeleton only; fixed by PLAN 2.9). All
three resolve and build offline from the local cache.

## C. Design and algorithms

### Normalisation

**Problem.** Keep ranges sorted and non-overlapping under PLAN 2.5, and know
which range is primary afterwards.

**How.** If the ranges are already sorted by `(start, end)` (the usual case
after a motion or edit), skip sorting; otherwise sort and find the primary
again by value (an equal range has the same span, so it lands in the same
group). Then sweep with a current group `[gs, ge)`: the next range `r` joins
iff `r.start < ge`, or `r.start == ge` and `r` is empty or the group so far
is empty. A multi-range group becomes its union, pointing the way the *kept*
range points (the primary if the group has it, else the group's first), with
the kept range's goal column.

**Why correct.** Sorted by start, a range can only overlap or touch the group
being built, so one sweep finds every merge; the join condition is the 2.5
rule written for a sorted sweep (strict overlap; touching only with an empty
range). `normalise_invariants` checks the output is sorted, no neighbours
would merge, every input range is covered, the primary output covers the
primary input, endpoints are not invented, and normalising is idempotent.

**Cost.** O(k) sorted, O(k log k) otherwise. 10,000 identical cursors become
one (`ten_thousand_cursors_merge`).

**Rejected.** Always-forward merged ranges (a backward primary flips);
Helix's "direction of the last range".

### Selection mapping

Endpoints are collected as `[start0, end0, start1, …]`, which is sorted
because the selection is normalised, and mapped in one pass with
`ChangeSet::map_sorted_with`, a generalisation of `map_sorted` taking an
`Assoc` per position. Starts map `After`; ends map `After` for an empty range
and `Before` otherwise. So typing at a cursor advances it, and a selection
does not swallow text typed at its edges. Under DECISIONS 009 a range
strictly inside a replaced span would map its start to the replacement's
right edge and its end to the left edge; the endpoints are reordered, so it
covers the replacement (a pure deletion collapses it to a point). Goal
columns are cleared, and the result is normalised, since mapping can make
ranges touch. Cost O(ops + k); no heap allocation for one range.
`map_matches_naive` compares against per-endpoint `map_pos_naive`.

### Grapheme iteration and motions

**Finding.** The motion property tests found two bugs in crop 0.4.3's
`RopeSlice::graphemes` when a text spans two chunks (common even for short
texts, because a leaf's gap splits it):

1. backward, after asking for the previous chunk it computes that chunk's
   start from the cursor position, which has not moved: offsets drift until
   `attempt to subtract with overflow`;
2. forward, it carries one `GraphemeCursor` across chunks and splits a
   regional-indicator pair that straddles the boundary, the failure class
   DECISIONS 008 already recorded.

`Text::graphemes(offset, direction)` replaces it. It is the stepping loop of
`move_graphemes` (reuse the cursor while steps stay in one chunk; redo a step
that needs another chunk with a fresh, context-fetching cursor) turned into
an iterator, and `move_graphemes` now runs on it. Items are borrowed unless
a cluster spans chunks. Latency of `move_graphemes` is within noise of
Stage 2 (e.g. 1 MB, 16 steps forward p50 380 → 414 ns, back 257 → 290 ns).

**Word motion.** A grapheme's class is that of its first char: whitespace,
word (alphanumeric or `_`), or punctuation. Forward: skip the run of the
head's class unless it is whitespace, then skip whitespace. Backward: skip
whitespace, then the run before it. One lazy iterator per motion, so the cost
is O(log n + distance).

**Vertical motion.** The goal is the remembered goal column or the head's
display column. The target line is clamped to the text. The result is the
start of the first non-zero-width grapheme whose cells cover the goal, else
the line's content end (never between `\r` and `\n`). Tabs advance to tab
stops; other widths come from unicode-width 0.2. Cost: O(columns walked on
both lines).

All motions validate the selection first, never panic, return char
boundaries, and return grapheme boundaries when the head was one
(`results_are_boundaries`).

### Transactions

**Constructors** validate the selection, then walk the ranges once, emitting
each range's edit through one `ChangeSetBuilder` while tracking the new
offset of the old position reached. Each result range is computed directly
from its edit's new start `n` (for example, insert: cursor → `n + |s|`;
forward range keeps its anchor and grows; backward range keeps its text, with
the insertion before it). Cost: O(k + inserted bytes), plus O(log n) per
grapheme step.

**Collision rules** (DECISIONS 013). Two touching ranges can share a head:
each inserts its own copy, in selection order, and each head lands after its
own copy. Backspace and delete-forward deletions can overlap (a cursor
inside a cluster, or a cursor whose grapheme reaches into a neighbouring
range). They merge on a stack: a new deletion pops and absorbs every group it
overlaps, which handles a backward reach over several earlier ranges.
Members of a group are contiguous in selection order and all collapse to the
group's start. Touching deletions stay separate.

**Atomic apply.** Validate first, then mutate, as in DECISIONS 010 (no
clone-and-swap). The change set is checked against the text. Then either the
transaction's selection is checked against the post-change text *without
building it*, or the current selection is validated and mapped. The check
walks ops and sorted endpoints together: a position in a retained run is a
boundary iff its old offset is, a position in an inserted string iff it is
one of the string's; anything past `len_after` is out of bounds. Cost
O(ops + k log n). Only then does the text change.

**Why correct.** `transaction_matches_naive_model` compares every intent
against an independent model, which merges deletions pairwise to a fixpoint
and applies edits one at a time from last to first, shifting results already
computed (PLAN's "naive model"). Mutating the backward-anchor formula or the
grapheme span each makes two properties fail.

### Walking skeleton

`flash_app::skeleton::Skeleton` holds text, selection, scroll, and size;
`handle(&Event)` maps hard-coded keys to motions and transactions (Stage 7
deletes them); `render(&mut impl Write)` queues a full frame. Rows are drawn
grapheme by grapheme up to the width (a wide char that would cross the edge
ends the row). Cells inside a non-empty range and secondary cursors are drawn
reversed, found with `partition_point` (O(log k + visible cells)). The status
line shows file, cursor count, line:column, and the last frame's latency.
Every control character in text or file name other than tab is drawn as
U+FFFD, so a file cannot send escape sequences to the terminal. `main.rs`
owns the terminal through a guard (raw mode, alternate screen, bracketed
paste), restored on drop and in a panic hook. It times each event from
`read` returning to `flush` returning, and appends a summary line to
`$FLASH_LATENCY_LOG` on exit.

## D. Invariants and how they are tested

| Invariant | Tests |
|---|---|
| Selection is sorted, normalised, primary in bounds, after every operation | `normalise_invariants`, `selection_valid_after_every_step` (random motions and edits), unit cases in `selection/tests.rs` |
| Merge rule cases and direction rule | `overlapping_ranges_merge`, `touching_non_empty_ranges_stay_separate`, `empty_touching_or_inside_merges`, `merged_direction_*` |
| Mapping per endpoint; matches the naive per-position model | `map_*` unit cases, `map_matches_naive`, `map_sorted_with_matches_map_pos` |
| Motions match a whole-string model; results are (grapheme) boundaries; extend keeps the anchor | `word_and_vertical_match_reference`, `results_are_boundaries`, `grapheme_round_trip`, Unicode unit cases (combining, ZWJ, CRLF, CJK, tabs) |
| `Text::graphemes` equals whole-text segmentation from any char boundary, both directions | `graphemes_match_reference` (property), `graphemes_match_reference_boundaries` (texts that broke crop) |
| Transactions match the back-to-front model, including collisions | `transaction_matches_naive_model`, `same_position_inserts`, `backspace_overlapping_deletes_merge`, `delete_forward_overlapping_deletes_merge` |
| A failed transaction changes nothing | `failed_apply_leaves_state_unchanged`, `apply_rejects_*` |
| Skeleton never panics; selection stays valid; no raw control bytes reach the terminal | `random_events_never_panic` (20,000 events), `render_*` (control characters, tiny terminals, wide chars, scrolling) |

Every property test runs with default and tiny (`small-chunks`) leaves.
Totals: `flash-core` 136 tests, `flash` 15 tests.

Review Focus (from the plan) and where each is pinned: escape injection
(`render_replaces_control_characters`, `…_in_name`, `random_events_never_panic`);
degenerate sizes (`render_survives_tiny_terminals`,
`render_never_splits_wide_char`); empty document (`motions_on_empty_text`,
`transactions_on_empty_text`); non-grapheme-boundary positions
(`motions_from_inside_cluster`, `backspace_overlapping_deletes_merge`); many
cursors collapsing (`ten_thousand_cursors_merge`, overlap merges).

## E. Commands and results

All with `--offline`, on `a29da88` plus the closing commit's tests:

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings` (plain and
  `--all-features`): clean.
- `cargo test --workspace`: flash-core 136 passed, flash 15 passed.
- `cargo test -p flash-core --features small-chunks`: 136 passed.
- `PROPTEST_CASES=4096 cargo test -p flash-core --release` (plain and
  `--features small-chunks`): 135 passed in each (58 s and 214 s). The
  review's added property `graphemes_match_reference` was then run at 4,096
  cases on both leaf sizes: passed.
- `cargo bench -p flash-core --bench transaction -- --save-baseline stage-3`
  → `bench/results/stage-3-criterion.json` (60 benchmarks, via
  `bench/collect.py`).
- `FLASH_BENCH_JSON=… cargo bench -p flash-core --features internals --bench latency`
  → `bench/results/stage-3-latency.jsonl`.
- `FLASH_BENCH_JSON=… cargo bench -p flash --bench skeleton`
  → `bench/results/stage-3-skeleton.jsonl`.
- CLI: no arguments → usage, exit 2; `--help` → exit 0; missing or
  non-UTF-8 file → `flash: <path>: <error>`, exit 1; no TTY → `flash:
  terminal setup failed: …`, exit 1.

## F. Benchmark results

Machine: Intel Core i7-1355U (12 threads), Linux 6.18.63, rustc 1.99.1,
bench profile (release + debug symbols), laptop on default power settings,
not isolated: treat differences under ~10% as noise.

**Transactions** (Criterion means; build + validate + apply + new selection
on a snapshot-shared tree; cursors at line starts, `clustered` = consecutive
lines from the middle, `spread` = one per band of lines; `map` = a one-edit
change set with no selection, i.e. mapping `n` ranges):

| op | 1 | 10 | 100 | 1,000 | 10,000 cursors |
|---|---|---|---|---|---|
| insert, 1 MB clustered | 942 ns | 2.2 µs | 13.9 µs | 169 µs | 2.04 ms |
| insert, 1 MB spread | 1.0 µs | 9.0 µs | 86.9 µs | 710 µs | 2.71 ms |
| backspace, 1 MB clustered | 671 ns | 2.3 µs | 16.9 µs | 221 µs | 2.73 ms |
| backspace, 1 MB spread | 676 ns | 6.0 µs | 68.6 µs | 806 µs | 3.49 ms |
| map, 1 MB spread | 973 ns | 1.5 µs | 5.5 µs | 81 µs | 506 µs |
| insert, 100 MB clustered | 1.7 µs | 3.5 µs | 21.3 µs | 231 µs | 2.67 ms |
| insert, 100 MB spread | 1.7 µs | 15.6 µs | 146 µs | 2.04 ms | 17.9 ms |
| backspace, 100 MB spread | 1.0 µs | 9.1 µs | 119 µs | 1.92 ms | 17.4 ms |
| map, 100 MB spread | 1.6 µs | 2.3 µs | 8.0 µs | 170 µs | 1.92 ms |

Against PLAN section 4: **1,000 cursors, one edit: at most 2.04 ms** (target
under 16 ms, met). 10,000 cursors: 2.0–3.5 ms on 1 MB, 17.4–17.9 ms on
100 MB spread (reported; the spread case on 100 MB is just over one frame).
Spread costs more than clustered because each edit copies its own tree path.

**Per-operation latency**, 100k samples, ns (p50 / p99):

| op | small | medium 1 MB | large 100 MB | one-line 1 MB | unicode 1 MB |
|---|---|---|---|---|---|
| txn insert char (1 cursor) | 399 / 611 | 1,381 / 1,669 | 2,424 / 2,897 | 1,398 / 1,669 | 1,004 / 1,682 |
| txn backspace (1 cursor) | 365 / 499 | 1,147 / 1,424 | 2,026 / 2,550 | 1,146 / 1,405 | 1,213 / 1,495 |
| motion right | 87 / 107 | 233 / 348 | 540 / 855 | 232 / 360 | 269 / 420 |
| motion down | 1,083 / 2,527 | 1,884 / 3,292 | 2,700 / 4,204 | 26.4 ms / 52.2 ms | 2,038 / 3,692 |
| motion word forward | 229 / 567 | 652 / 1,371 | 797 / 1,277 | 420 / 790 | 607 / 1,478 |

PLAN target, 1 MB single-character edit, core transaction commit well under
100 µs p99: **1.7 µs p99.** Motions do not grow with document size beyond
the O(log n) descent: 100 MB / 1 MB is 2.3x for `right`, 1.4x for `down`,
1.2x for `word`, against 3.5x for a single `is_grapheme_boundary` lookup
(tree depth and cache misses). The plan's "within 2x" check was too strict
for that reason (ruling in G). `motion down` on the 1 MB single-line
fixture walks the line for the column: slow but bounded (H).

**Walking skeleton, end to end in process** (event handled + full frame
rendered into a sink; terminal write excluded), 2,000 events of a fixed
20-event trace (type, delete, move, newline, add cursor), ns:

| fixture | 80×24 p50 / p99 | 200×60 p50 / p99 |
|---|---|---|
| small 4 KB | 23,102 / 41,756 | 50,115 / 77,393 |
| medium 1 MB | 36,365 / 96,594 | 68,177 / 126,609 |
| unicode 1 MB | 35,544 / 101,168 | 67,436 / 215,417 |
| one-line 1 MB | 10,711 / 26.3 ms | 24,341 / 26.3 ms |

On ordinary text one event costs at most about 0.2 ms p99, almost all of it
the full redraw (it doubles from 80×24 to 200×60). The one-line tail is the
`End` key (5% of the trace): the status line computes the cursor's column by
walking the 1 MB line. The interactive number (real terminal, `read` →
`flush`) needs a TTY and is pending the owner's smoke test (`FLASH_LATENCY_LOG`, see
main.rs); it will be added here when run.

## G. Review

**Independent review: not done by a fresh reviewer.** Dispatching a
fresh-context reviewer was refused by the session's permission policy (the
owner's rule: no delegation without an explicit request). The implementer
did the adversarial pass instead. This is weaker than PLAN section 3
requires, because the author has the same blind spots. **The owner should
decide whether to run an independent review before approving.**

Implementer adversarial pass: two attack tests were added and kept,
`graphemes_match_reference` (property, both leaf sizes) and
`random_events_never_panic` (20,000 random keys, pastes, and resizes, down
to 0×0, over Unicode texts with BEL and CSI bytes). Both passed on their
first run. A read of the full diff checked: the stack merge of overlapping
deletes, including a deletion reaching back over several groups;
`check_after` at end of text; primary tracking with duplicate ranges;
termination for `usize::MAX` counts; C1 controls and DEL; cursor placement
at height 1; terminal restore on every error path. No Critical or Important
finding.

Bugs found during the stage by the planned tests, all fixed with tests:

| # | Finding | Resolution |
|---|---|---|
| 1 | crop 0.4.3 `RopeSlice::graphemes`: backward offset drift (overflow panic), forward regional-indicator split across chunks | Replaced by `Text::graphemes` (DECISIONS 012); `graphemes_match_reference_boundaries` |
| 2 | `Selection::map` inverted a range strictly inside a replacement | Endpoints reordered; `map_range_inside_replacement_covers_it` |
| 3 | First draft of `insert`: forward-range anchor shifted by the inserted text; a cursor came out as a range | Fixed before commit; `insert_extends_forward_range`, `insert_at_cursors` |

Rulings made during execution (details and cost if wrong in the plan
ledger):

- `Text::graphemes` instead of a crop slice wrapper.
- Map reorders the endpoints of a range inside a replaced span.
- `display_column` does not count a cluster that contains the offset.
- Stack-based delete merging (the plan's "previous edit only" rule missed a
  backward reach over several ranges).
- Stale `dead_code` allows on `Text` mutators removed.
- Spread benchmark cursors centred in their bands (`n = 1` was at offset 0,
  where backspace is a no-op).
- Benchmarks run once, quietly, after the skeleton.
- The "within 2x" motion check replaced by "no worse than one O(log n)
  lookup".
- Small skeleton API additions.
- File-name sanitising.
- The TTY smoke test left to the owner.

Deferred minors:

- `failed_apply_leaves_state_unchanged` rarely reaches its success branch.
- The skeleton ignores Ctrl+Alt+char (AltGr layouts).
- Zero-width graphemes are written raw.
- Vertical motion and the status column are O(line length) on huge
  single-line files.

## H. Known limitations

- Up/down motion is O(columns on the source and target lines), not
  O(distance): on a 1 MB line without newlines it takes about 26 ms. PLAN
  section 4 accepts slow-but-bounded; the Stage 6 renderer should cache
  display columns per line.
- `Transaction::apply` is `pub` so the skeleton can drive it. It becomes
  `pub(crate)` when `Document::commit` lands in Stage 4.
- The skeleton has no horizontal scrolling (the cursor column is clamped to
  the last cell), no save, and hard-coded keys. It is throwaway by design.
- Word classes and tab width are constants; Stage 7 makes them options
  (PLAN 2.11).

## I. Questions for the reviewer

1. Can any public `flash-core` method, or `Skeleton::handle`/`render`, panic
   for any input?
2. Is there a selection and intent where the transaction result differs
   from applying each range's edit one at a time, back to front, under the
   documented collision rules?
3. Can a failed `Transaction::apply` change the text or the selection?
4. Can any byte of a file's content other than printable text and tab reach
   the terminal?

## J. Next stage

Stage 4: documents, views, revisions, and the headless editor. Not started;
waiting for approval.
