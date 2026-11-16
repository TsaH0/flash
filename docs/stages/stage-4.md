# Stage 4 — Documents, views, revisions, headless editor, trace replay

Status: **complete; approved by the owner 2026-11-16** (no independent
review was run). Branch `stage-4`, local
commits only (nothing pushed). Design:
[2026-11-15-stage-4-design.md](../superpowers/specs/2026-11-15-stage-4-design.md)
(approved 2026-11-15). Plan:
[2026-11-15-stage-4-documents-editor.md](../superpowers/plans/2026-11-15-stage-4-documents-editor.md).

## A. Status

| Requirement (PLAN Stage 4) | State |
|---|---|
| `Document { text, revision, revision_log, path, line_ending, … }` | Done (`document/`); also holds every view's selection (DECISIONS 014) |
| `View { doc, selection, scroll, mode state }` | Done as `View { doc, top, size }`; selection lives in the document (014); mode state deferred to Stage 7, which defines modes |
| `Editor { documents, views }` with generational IDs (`slotmap`) | Done (`editor/`) |
| `Document::commit(txn, origin_view)` the only way to change text: validate, apply, bump revision, log, map other views | Done; `Transaction::apply` is now crate-private (test/`internals` only) |
| Revision log with `map_from(revision, positions)` | Done; bounded by entries and ops (DECISIONS 015) |
| Headless API `Editor::handle(Event) -> Effects`; skeleton rewired onto it | Done; keys moved to `editor/keys.rs` (temporary, PLAN 2.11) |
| Trace record/replay driving the headless editor | Done (`trace.rs`, DECISIONS 016); `FLASH_TRACE` records, `flash --replay` replays |
| Tests: two views valid under random edits; close view/document cleans up; stale IDs rejected; revisions monotonic; stale mapping through the log matches direct mapping | Done (section D) |
| Standing benchmark: N views, 10k selections, one edit, complete commit | Done (`benches/document.rs`, section F) |

Owner decision during design: trace events are **raw input** (option A),
not commands.

## B. Commits

Branch `stage-4`, from `9135723` (Stage 3 close):

| Commit | Content |
|---|---|
| `9e19308` | Stage 3 interactive latency recorded; Stage 3 marked approved; Stage 4 design and plan |
| `8a8fa04` | `Revision`, `RevisionLog`, `MapError` |
| `a09027f` | `Document`, `commit`, per-view selections, `LineEnding`, `Transaction::prepare`; `slotmap`; DECISIONS 014, 015 |
| `92ed029` | `Editor`, `View`, `Event`/`KeyEvent`/`Modifiers`, key map, `Effects`, scrolling; `serde` |
| `f0cbea6` | `trace.rs`; skeleton rewired onto `Editor`; `--replay`, `FLASH_TRACE`; `Transaction::apply` crate-private; `serde_json`; DECISIONS 016 |
| `3620950` | Standing benchmark, latency rows, results |
| (closing commit) | Output-file clobber fix, review tests, this report, status |

New dependencies: `slotmap 1.1` (generational document and view IDs;
named by PLAN), `serde 1.0` with `derive` and `serde_json 1.0` (trace
format; both were already in the lock file through criterion). All resolve
and build offline.

## C. Design and algorithms

### Commit

**Problem.** Change a document's text so that every view's selection stays
valid, and so that a failure changes nothing.

**How.** `Document::commit(view, txn)`:

1. `txn.prepare(text, current)`: validate the change set against the text,
   and either check the transaction's selection against the post-change
   text without building it, or validate and map the current selection
   (the Stage 3 logic, split out of `Transaction::apply`).
2. Unless the change set is the identity, map every other view's selection
   through it into a `SmallVec` (no heap allocation for up to four views).
3. Only now apply the text, store all selections, increment the revision
   (non-identity only), and move the change set into the log.

**Why correct.** Steps 1–2 are pure, so any error leaves everything as it
was. Mapping in step 2 cannot fail: other selections are valid for the
text by the document's invariant, and the change set was just validated
against it (a `debug_assert` guards the unreachable branch). An identity
commit skips mapping, because mapping clears goal columns and nothing
moved. `views_stay_valid_under_commits` checks the result against a String
model and per-view `Selection::map`, for random commits from 1–3 views;
making commit skip the mapping fails it.

**Cost.** O(ops · log n + Σ k over views). Mapping 10,000 ranges is about
100 µs per bystander view (section F).

### Revision log and stale positions

Entry `(r, c)` turns revision `r − 1` into `r`. `map_from(from, positions,
assoc)` checks `from` against `[oldest, current]`, then maps through the
entries after `from`, in order. Only the first change set needs to check
the positions: entries are consecutive, so each `len_before` is the
previous `len_after`, and mapping keeps positions sorted and in bounds.
`Document::map_from` maps a copy, checks every result is a char boundary
of the current text (an input that was not a boundary at `from` cannot be
detected any other way), and writes back only on success. Bounds: 1,024
entries and 2^20 ops, oldest evicted first, newest always kept (one
10,000-cursor edit is about 20,000 ops). Cost O(Σ ops + k · entries).

### Editor

`Editor` owns `SlotMap<DocumentId, Document>` and `SlotMap<ViewId, View>`.
A closed ID never matches a later one, even in a reused slot. `handle`:
resize sets the screen and every view's text area (the screen minus the
status row); Ctrl+Q quits (with or without a view); keys and pastes go to
the focused view through `keys::handle_key`; afterwards the focused view
scrolls by the least amount that shows its primary head's line (O(log n)).
Commands report `Effects` without cloning the selection: edits report a
change when the revision moved, and motions compare the new selection
with the old one before storing it.

### Traces

JSON Lines (DECISIONS 016). Reading uses `read_until` through
`Read::take(max_line + 1)`, so a line longer than the limit is rejected
before the buffer grows past it. Blank lines are skipped, errors carry
1-based line numbers, and the iterator stops after the first error.
`trace::start` checks the document's length, FNV-1a hash, and the cursor
before building the editor. Replay is deterministic:
`replay_reproduces_session` records 200 random events while applying them,
replays the recording into a fresh editor, and compares text, selection,
revision, scroll, size, message, and quit.

## D. Invariants and how they are tested

| Invariant | Tests |
|---|---|
| Every view's selection is valid and normalised after every commit; other views map, origin gets the transaction's result | `views_stay_valid_under_commits` (property), `commit_maps_other_views`, `commit_from_background_view_maps_focused` |
| A failed commit changes nothing (text, revision, log, every selection) | `failed_commit_changes_nothing`, `set_selection_validates` |
| Revisions strictly increase on effective changes only | `commit_bumps_revision_and_logs`, `identity_commit_keeps_revision`, the property's revision check |
| Stale mapping through the log equals sequential `map_pos`; bounds and errors | `map_from_matches_sequential_map_pos`, `log_bounds_and_eviction`, `map_from_future_rejected`, `map_from_error_leaves_positions`, `map_from_validates_against_text`, end of the property |
| Closing cleans up; stale IDs rejected even after slot reuse | `stale_ids_rejected_after_reuse`, `close_focused_view_moves_focus`, `close_document_closes_its_views`, `add_and_remove_views` |
| `handle` never panics; focused head stays visible | `random_events_never_panic` (core, 20,000 events, two views, focus switches; removing the scroll call fails it), `handle_without_focus_is_noop`, `scroll_follows_primary_head`, `resize_sets_view_size` |
| Key behaviour moved intact from the skeleton | the ported key tests in `editor/tests.rs`, `enter_uses_document_line_ending` |
| Traces round-trip, replay exactly, and reject bad input | `round_trip`, `replay_reproduces_session` (properties), `read_rejects_malformed`, `read_bounds_line_length`, `replay_rejects_bad_header`, `event_json_shape`, `header_line_shape`, `content_hash_is_fnv1a_over_chunks`; CLI `replay_*` |
| No raw control byte reaches the terminal; rendering bounded | `render_*` and `random_events_never_panic` (app), `render_survives_huge_screen` |
| Output files never clobber the document | `output_files_never_clobber_the_document` |

Every property test runs with default and tiny (`small-chunks`) leaves.
Totals: `flash-core` 180 tests, `flash` 10 library + 4 CLI tests.

## E. Commands and results

All with `--offline`, at the closing commit (outputs in the closing
session):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings` (plain and
  `--all-features`): clean.
- `cargo test --workspace`: flash-core 180 passed; flash 10 + 4 passed.
- `cargo test -p flash-core --features small-chunks`: 180 passed.
- `PROPTEST_CASES=4096 cargo test -p flash-core --release` (plain and
  `--features small-chunks`): 180 passed in each (66 s and 210 s).
- `cargo check` in `crates/flash-core/fuzz`: builds.
- `cargo bench -p flash-core --bench document -- --save-baseline stage-4`
  and `cargo bench -p flash-core --features internals --bench transaction
  -- --save-baseline stage-4` → `bench/results/stage-4-criterion.json` (74
  benchmarks, via `bench/collect.py`).
- `FLASH_BENCH_JSON=$PWD/bench/results/stage-4-latency.jsonl cargo bench -p
  flash-core --features internals --bench latency`;
  `FLASH_BENCH_JSON=$PWD/bench/results/stage-4-skeleton.jsonl cargo bench -p
  flash --bench skeleton`. (The path must be absolute: benches run in the
  package directory.)
- CLI: no arguments → usage, exit 2; `--help` → exit 0 and mentions
  `--replay`; `--replay` on a good trace prints one JSON summary line; a
  mismatched, fixture, malformed, or missing trace → `flash: …`, exit 1;
  `FLASH_TRACE` or `FLASH_LATENCY_LOG` naming the document (or a symlink to
  it) → refused, exit 1, document unchanged.

Gate run (2026-11-16, after the review fix): every command above passed;
`small-chunks` debug run 149 s.

## F. Benchmark results

Machine: Intel Core i7-1355U (12 threads), Linux 6.18.63, rustc 1.99.1,
bench profile (release + debug symbols), laptop on default power settings,
not isolated: treat differences under ~10% as noise.

**Standing benchmark** (Criterion means; `Transaction::insert(.., "x")` from
the origin view plus `Editor::commit`; bystander views hold 10,000 spread
cursors each):

| fixture | origin | 1 view | 2 views | 8 views |
|---|---|---|---|---|
| medium 1 MB | 1 cursor | 1.01 µs | 105 µs | 1.21 ms |
| medium 1 MB | 10k cursors | 2.70 ms | 4.86 ms | 6.54 ms |
| large 100 MB | 1 cursor | 1.66 µs | 181 µs | 1.17 ms |
| large 100 MB | 10k cursors | 19.4 ms | 19.7 ms | 21.2 ms |

Each 10,000-range bystander costs about 100–170 µs to map. The
10k-cursor, one-view numbers match Stage 3's bare transaction (`insert,
1 MB spread, 10,000`: 2.71 ms; 100 MB: 17.9 ms), so the document layer
adds nothing measurable. On 100 MB, a 10k-cursor edit is just over one
frame (16 ms), as in Stage 3. `LineEnding::detect` (at open): 0.87 ms per
MB (1 MB: 870 µs, 100 MB: 84.8 ms).

**Regression check.** The Stage 3 `transaction` suite re-run under the
`stage-4` baseline: across 60 benchmarks the median stage-4/stage-3 ratio
is 0.985 (range 0.71–1.43, the extremes on 100 MB spread fixtures, where
run-to-run spread is largest). The `prepare` split costs nothing.

**Per-operation latency**, 100k samples, ns (p50 / p99; allocations per op):

| op | small | medium 1 MB | large 100 MB | one-line 1 MB | unicode 1 MB |
|---|---|---|---|---|---|
| txn insert char (Stage 3 path) | 446 / 635 | 1,412 / 1,773 | 2,485 / 3,082 | 1,481 / 1,898 | 1,038 / 1,755 |
| doc commit insert char (1 view) | 555 / 755 | 1,551 / 2,207 | 2,402 / 3,707 | 1,637 / 1,957 | 1,175 / 1,941 |
| doc commit insert char (2 views) | 616 / 807 | 1,674 / 2,599 | 2,497 / 3,088 | 1,759 / 2,186 | 1,302 / 2,145 |
| editor handle key 'x' | 535 / 723 | 1,543 / 1,881 | 2,356 / 2,785 | 1,641 / 1,957 | 1,187 / 1,982 |
| editor handle Right | 193 / 242 | 482 / 675 | 662 / 885 | 471 / 650 | 536 / 768 |

The commit layer costs about 100–150 ns at p50 and exactly one more
allocation than the bare transaction (the revision-log entry; 8.0 vs 7.0
on `small`). `handle Right` allocates nothing. PLAN target, 1 MB
single-character edit, core commit well under 100 µs p99: **2.2 µs p99**
(through `Document::commit`), 1.9 µs through `Editor::handle`.

**Skeleton, end to end in process** (trace written and read back, then
`Editor::handle` + full frame into a sink), 2,000 events, ns:

| fixture | 80×24 p50 / p99 | 200×60 p50 / p99 |
|---|---|---|
| small 4 KB | 23,104 / 41,953 | 49,536 / 76,269 |
| medium 1 MB | 37,067 / 99,121 | 67,889 / 129,444 |
| unicode 1 MB | 35,860 / 116,660 | 67,252 / 147,557 |
| one-line 1 MB | 10,648 / 26.4 ms | 23,554 / 26.4 ms |

Within noise of Stage 3 (e.g. medium 80×24 p99 96.6 → 99.1 µs). The
one-line tail is the status line's column on a 1 MB line, as before (H).

## G. Review

**Independent review: not done by a fresh reviewer.** The owner's standing
rule is no delegation without an explicit request, so the implementer did
the whole-branch review as a separate pass. This is weaker than PLAN
section 3 requires; **the owner should decide whether to run an
independent review before approving.**

Findings:

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | Important | `FLASH_TRACE=notes.txt flash notes.txt` truncated the document to write the trace (the skeleton cannot save, so the file was lost; reproduced: 9 bytes became a 108-byte trace header). `FLASH_LATENCY_LOG` naming the document appended JSON to it. | Both refused before any file is opened for writing when they name the document (canonical path, or same device and inode on Unix). `output_files_never_clobber_the_document` (RED, then GREEN). |
| 2 | Minor (deferred) | With CRLF line endings, a trace line of exactly `max_line` content bytes is rejected (the limit counts the `\r`). | Deferred: 16 MiB default; a 1-byte margin. |
| 3 | Minor (deferred) | `trace::start` reports its two unreachable editor errors as `TraceError::Io`. | Deferred: unreachable. |

Attack tests added and kept, which found nothing: `render_survives_huge_screen`
(65535×65535 resize from a hostile trace). Also checked by reading: commit
indexing after validation (no panic reachable), `oldest()` underflow
(impossible: entries start at revision 1 or later), `map_from` skip
arithmetic, the reader limit at `max_line` 0 and `usize::MAX`, focus and
scroll after background commits and closes, `same_file` for a trace path
that does not exist yet.

Mutation checks (each made a test fail, then reverted): commit not mapping
other views; `handle` not scrolling; reader without the line bound; `start`
without the hash check.

Rulings made during execution (with cost if wrong, in the plan ledger):

- `RevisionLog::map_from` checks positions against the first entry and
  maps in place (no scratch copy); `Document::map_from` copies.
- `Transaction::into_changes` instead of `into_parts`.
- Identity commits do not map other views (their goal columns survive).
- Temporary `expect(dead_code)` markers between commits, removed by the
  next commit, so every commit is clippy-clean.
- `EditorError` has no `Selection` variant; selection errors come through
  `Commit`.
- Ctrl+Q and resize work without a focused view; resize sizes every view.
- `Editor::commit`, `set_selection`, and `set_focus` also scroll the
  focused view.
- The core random-event test is a deterministic xorshift loop, not
  proptest.
- Edit `Effects` report a selection change only with a text change (no
  O(k) clone per key).
- Added `TraceError::MissingHeader` and `trace::replay`.
- `Transaction::apply` compiled only for tests and `internals`.
- Skeleton bench ported in Task 4 (with the trace round trip planned for
  Task 5).
- `apps/flash` does not depend on `serde_json`.
- The binary renders only when `Effects::needs_redraw()`, records trace
  entries after the flush, and stops recording (reported at exit) on a
  write error.
- Rendering with no focused view clears the screen.
- The standing benchmark goes through `Editor::commit` on cloned editors
  (`Document::add_view` is crate-private) with `BatchSize::LargeInput`.

Process notes: the Task 1 commit was amended once (local, unpushed,
minutes old) to add the dead-code markers, after a piped clippy failure
went unnoticed; later steps ran with `pipefail`. Trace tests and
`trace.rs` landed in one module, so there was no separate RED run; the
mutation checks above stand in for it.

## H. Known limitations

- Interactive latency samples now include events that need no redraw
  (e.g. a motion that does not move), which are cheap; Stage 3's
  interactive number rendered every event. Compare percentiles with that
  in mind.
- The `FLASH_TRACE` recording path is not covered by an automated test (it
  needs a TTY); `--replay` and the trace format are.
- A recorded trace stores the document path as given on the command line,
  so `--replay` must run from the same directory, and a non-UTF-8 path is
  recorded lossily. A trace contains everything typed or pasted.
- Each bystander view with 10,000 selections adds ~100–170 µs per commit
  (mapping is O(k) per view). Fine at the PLAN's scale; revisit if Stage 8
  markers make it a hot path.
- Up/down motion and the status column remain O(line length) on huge
  single-line files (Stage 3 H).
- `t_ns` is recorded but replay does not use it yet (Stage 6 replays
  coalescing).
- `LineEnding::detect` is O(n) at open (85 ms on 100 MB); Stage 6 loading
  can fold it into the streaming read.

## I. Questions for the reviewer

1. Can any public `flash-core` method, `Editor::handle`, or the trace reader
   panic for any input?
2. Is there a commit sequence after which some view's selection is invalid
   for its document's text, or a failed commit that changes anything?
3. Can `map_from` return a position that differs from mapping through each
   recorded change set in order, or an invalid offset?
4. Can a trace replay diverge from the session that recorded it?
5. Can any file the binary writes destroy user data?

## J. Next stage

Stage 5: undo, redo, branching history (high risk). History nodes go
through `Document::commit`, so revisions, the log, and cross-view mapping
apply to undo and redo unchanged. Not started; waiting for approval.
