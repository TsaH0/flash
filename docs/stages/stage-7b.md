# Stage 7b — Carried fixes, layout cache, soft wrap, search and replace

Status: **complete; awaiting owner approval** (no independent
fresh-context review was run; the implementer's review is section G).
Branch `stage-7`, local commits only (nothing pushed). The owner approved
Stages 6 and 7a and asked for 7b and 7c to be completed (2026-11-17).
Scope is PLAN revision 3, Stage 7b.

## A. Status

| Requirement (PLAN rev. 3, Stage 7b) | State |
|---|---|
| Carried fixes, each with a regression test | Done (`70daa8d`; DECISIONS 033–039; section C.1) |
| Display-line layout cache before soft wrap, replacing the `LineAnchor` checkpoints | Done (`c8230b7`; DECISIONS 040) |
| The 1 MB single-line fixture at or below its per-key cost | Met for moves (faster than 7a) and p99; typing +15% at p50 (section F). End to end p50 12.1 µs (7a 11.3 µs), p95/p99 as before |
| Soft wrap | Done (`cf72057`; DECISIONS 041) |
| Incremental search and replace, regex over the rope, off the main thread for large buffers | Done (`35f8a3c`; DECISIONS 042, 043) |
| Search works on `medium` without blocking input | Done: documents of 256 KiB or more search on a thread; pty `searching_a_large_file_finds_the_match` (1 MB) |
| A config file for keys and options | Done in 7a |
| Stage 7 acceptance, revision 3: 1,000,000-key run bounded per key | Done: 2.7 µs per key flat to 1,000,000 keys (DECISIONS 034) |
| Stage 7 acceptance, revision 3: a 100 MB save does not delay a key | Done in kind: keys typed after Ctrl+S on a 32 MiB file are drawn before "saved" (pty; DECISIONS 038). Not run at 100 MB in the suite |
| Byte targets for cursor moves and one-character edits | Stage 7c (the renderer still redraws everything: 0.5–2 KB per frame) |
| Input-thread decision recorded | Stage 7c |

## B. Commits

Branch `stage-7`, after `e0d5871` (the Stage 7a close):

| Commit | Content |
|---|---|
| `70daa8d` | Carried fixes: search across chunk boundaries, undo-group size bound, hook kinds, kitty keyboard flag, read-time events, save and open on an I/O thread, idle-mode benches and frame bytes; PLAN revision 3 |
| `c8230b7` | Layout cache (`flash_core::layout`) replacing `editor/column.rs`; column hints for vertical motion |
| `cf72057` | Soft wrap; `g j`/`g k` row motion |
| `35f8a3c` | Search (`/ ? n N *`, `:noh`), replace (`:s`, `:%s`), the search thread |
| (closing commit) | This report, DECISIONS 040–043, status, results |

No new dependencies.

## C. Design

### C.1 Carried fixes

As DECISIONS 033–039 record them; the measurements are in section F.
The search fix (033) matters for everything below: regex-cursor 0.1.5's
literal strategy leaves its cursor in a later chunk, so every search in
Flash now starts from a freshly placed cursor.

### C.2 The layout cache (DECISIONS 040)

**Problem.** Where an offset is on screen (its logical column, and with
soft wrap its row) is a function of everything before it on its line:
O(line) to compute. Stage 6 kept three checkpoints for the primary head
only. Soft wrap needs row starts for every visible line, and the renderer
needs where each visible line's drawn part starts.

**Algorithm.** A line of `LONG` (2 KiB) bytes or more gets a sorted list
of marks, at least `SPACING` (512 bytes) apart: without wrapping,
grapheme boundaries with their column; with wrapping, row starts with
their row index and logical column. Each records `dep`, the exclusive end
of what the layout read to place it: for a row start, the grapheme that
did not fit on the previous row (or the character at the line end for
the cursor row); for a column mark, its own offset. Queries walk from
the last mark at or before their target, appending marks as they pass
the last one. The last location found, on any line (the hot point), is
kept by the same rule and is the usual starting point: the next key is
one grapheme away, forward or (over non-tab graphemes) backward. A walk
to the left column leaves a mark where it stopped, so the next one is
short; the visible lines are checked for length by scanning for their
newline (at most 2 KiB), not by an O(log n) line lookup.

**Why it is correct.** A row start depends only on its line's text up to
`dep`, and a boundary's column only on the text before it; whether an
offset is a grapheme boundary depends on the text before it and the
character after it. `RevisionLog::map_untouched(line_start, dep)`
succeeds only if no change since the layout's revision inserted inside
`[line_start, dep)` (an insert at `dep` itself is allowed), deleted
anything overlapping it, or deleted up to the line start (which keeps
the line start a line start). So a kept mark's row and column are what
laying the line out afresh would give, at the offset shifted by the
line's shift. Only an edit at a kept mark's own offset can change the
character after it, and every edit is at or after the last kept mark's
`dep`, so only the last kept mark (and the hot point) can stop being a
grapheme boundary: both are checked on every sync. The predicate is
monotone in `dep` and `dep` grows along the marks, so the kept marks
are a prefix found by binary search.

**Wrapping rule.** Greedy per row: from the row start, add graphemes;
when one would overflow the width (and the row is not empty), the row
ends after the last space or tab seen in it, else before that grapheme.
The cursor cell at the line end counts as one cell, giving an empty last
row after a full one. Tabs expand on the logical column.

**Complexity.** A query: O(log n) to find the line, O(cached lines) to
find its marks (at most 128), O(log marks + SPACING + width) to walk.
Syncing a revision: O(cached lines · log marks · entries since). A jump
far beyond the marks: O(distance), once.

**Corner cases.** Inserting a ZWJ at a mark between two emoji makes the
mark a non-boundary; `dep` covers the character after it, so the mark is
dropped (`an_insert_joining_clusters_at_a_mark_drops_it`). A full last
row: an empty row follows. A grapheme wider than the row: alone on its
row. The last line of the text: its cursor-row mark reads one byte past
the text, so it never survives a change.

**Rejected.** Keeping the Stage 6 checkpoints and adding row starts beside
them (two invalidation schemes for one question); caching every line
(most are shorter than the walk it saves); shifting marks across an edit
(valid only over tab-free text, which needs a per-mark flag; noted in
DECISIONS 040).

**Vertical motion** keeps its definition and result; the layout only
answers two questions faster (the head's column, and a boundary at or
before the goal column on the target line). `vertical` rejects a hint
past the goal, so a wrong hint costs time, never a wrong result.

### C.3 Soft wrap (DECISIONS 041)

The view scrolls by rows: the cursor's `(line, row)` must lie in the
`height` rows from `(top, top_row)`; if not, the cursor's row becomes the
first (moving up) or last (moving down) visible row, counting rows back
across lines with `row_count`. Rows are drawn from the layout. `j`/`k`
stay logical; `g j`/`g k` move by rows.

### C.4 Search and replace (DECISIONS 042, 043)

Search runs from the selection where it started, at once below 256 KiB,
else as a request. The core never starts a thread: the frontend's
`Searcher` runs the request's job; replay runs it inline. A result is
applied only if it is for the latest search; a result for an older
revision is mapped (start `After`, end `Before`, so the match keeps its
own text) or dropped. Replace computes all edits (captures read from the
rope by group span) and commits one change set.

## D. Tests

New or changed in this stage (counts after the stage: flash-core 321
unit and property tests, 1 integration; flash-term 24; flash 12 library,
7 CLI, 10 pty plus 1 ignored measurement):

- **Layout** (`layout/tests.rs`): the wrap rule by hand; locations on
  wrapped rows; walking bounded on a long line after edits; the
  cluster-joining insert; row motion; the cache property test (8 query
  kinds plus edits and reconfiguration). Mutation checks: keeping one
  mark too many, and reading one grapheme less before a row break, both
  fail it; dropping the after-character margin fails the cluster test.
- **Editor**: `local_edits_on_a_long_line_do_not_walk_it` now bounds the
  layout's walks; the random editor test checks the cursor location
  against a fresh layout and its row's visibility, with soft wrap in half
  the runs.
- **Renderer**: frames with and without the cache are equal (long lines;
  the random test, with soft wrap in half the runs); wrapped rows and the
  cursor; scrolling inside a 50-row line; underlined matches and `:noh`.
- **Search**: whole-text matches from any offset against regex-automata
  (also with tiny leaves at 4,096 cases); wrap-around; captures; escape;
  editor flows by key; stale and superseded results; the search thread;
  1 MB in a pty.

## E. Commands run

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --offline --workspace --all-targets -- -D warnings` and
  with `--all-features`: clean.
- `cargo test --offline --workspace`: all pass.
- `cargo test --offline -p flash-core --features small-chunks`: all pass.
- `PROPTEST_CASES=4096 cargo test --offline --release -p flash-core --lib`:
  all 323 pass. **Not yet run on the final tree:** the same with
  `small-chunks` (stopped when the session ended; the layout and search
  property tests passed at 3,000–5,000 cases with tiny leaves earlier).
- `cargo bench -p flash --bench editor`, the latency bench, and three
  runs of the pty `interactive_latency` test (section F).

## F. Benchmarks

Same machine and build settings as Stage 7a. Results:
`bench/results/stage-7b-layout-editor.jsonl`,
`stage-7b-layout-interactive.jsonl`, `stage-7b-latency.jsonl` (and the
carried-fix runs `stage-7b-editor*.jsonl`, `stage-7b-interactive.jsonl`).

**Editor end to end** (`apps/flash/benches/editor.rs`, insert mode,
the Stage 6 key pattern), ns p50 / p99, 7a → 7b:

| fixture | 80×24 | 200×60 |
|---|---|---|
| small 4 KB | 25,569 / 44,628 → 28,669 / 59,484 | 51,953 / 169,008 → 55,033 / 101,061 |
| medium 1 MB | 42,006 / 104,361 → 43,282 / 131,515 | 73,375 / 133,979 → 77,048 / 162,412 |
| one-line 1 MB | 11,333 / 26.8 ms → 12,089 / 26.8 ms | 24,497 / 26.8 ms → 25,476 / 26.8 ms |
| unicode 1 MB | 40,451 / 121,398 → 42,663 / 137,495 | 72,824 / 151,049 → 77,566 / 169,309 |

p50 within about 5% (the layout's bookkeeping and the hook mask); p99
moves within the run-to-run spread seen in Stage 7a. The `one-line`
tail is End on a line that Enter just split: a jump O(distance) along a
fresh 500 KB line, the same cost as before (DECISIONS 040).

**Interactive, real pty** (500 keys 5 ms apart into 1 MB, release),
three runs, ns: total p50 / p99 145,381 / 328,956; 193,077 / 405,027;
262,674 / 488,890 (7a: 280,140 / 474,803 and two more runs up to
631,198 p99). Queue stage p50 55–78 µs. Frames are 557 bytes at p50: a
full redraw of a 24-row screen whose rows are mostly short.

**Per-operation latency** (`benches/latency.rs`, up to 100,000 samples each), ns p50 / p99, 7a → 7b (final code):

| op | small | medium | large | one-line | unicode |
|---|---|---|---|---|---|
| editor handle Right | 640 / 933 → 658 / 1,128 | 1,585 / 2,089 → 1,650 / 2,239 | 2,569 / 3,603 → 2,546 / 3,580 | 1,892 / 4,225 → 1,717 / 2,331 | 1,873 / 3,378 → 2,026 / 3,654 |
| editor normal 'l' (block) | 696 / 1,049 → 759 / 1,221 | 1,873 / 2,455 → 2,024 / 2,657 | 2,462 / 3,225 → 2,496 / 3,171 | 2,072 / 2,763 → 1,985 / 2,623 | 2,282 / 3,905 → 2,405 / 4,177 |
| editor handle key 'x' | 1,418 / 1,847 → 1,485 / 2,518 | 3,456 / 4,292 → 3,568 / 4,522 | 5,335 / 6,753 → 5,204 / 6,508 | 3,882 / 7,208 → 4,455 / 6,007 | 3,476 / 5,343 → 3,826 / 6,148 |
| editor key into open group (100) | 1,318 / 1,940 → 1,636 / 2,804 | 1,863 / 2,540 → 2,302 / 3,125 | 2,125 / 3,087 → 2,690 / 3,479 | 1,787 / 2,703 → 2,882 / 4,680 | 1,820 / 2,862 → 2,354 / 3,435 |
| undo char | 1,076 / 1,916 → 1,178 / 2,023 | 2,199 / 3,212 → 2,371 / 3,449 | 2,682 / 3,896 → 2,801 / 3,882 | 3,996 / 7,790 → 4,602 / 6,154 | 2,535 / 3,896 → 2,694 / 4,103 |

Single-key operations are within about 10% of 7a at p50 everywhere;
on the 1 MB single line, moving is now faster than 7a and typing is
+15% at p50 and lower at p99. Typing into an open undo group of 100 keys
stays 0.3–1.1 µs (25–60%) above 7a: the view, scrolled sideways by the
100 characters, checks its visible lines' lengths each key. The first
version of the cache was 10–13× slower on the single line (finding 7,
section G): this table is after the fix. `motion down` on `one-line`
(25.8 ms) is the bare motion without a layout, as in 7a.

## G. Review

Implementer's review of the stage diff (no independent fresh-context
review was run):

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | Major | The layout's first version allocated a scratch `Vec` per query on short lines, so normal-mode keys allocated again (`tests/alloc.rs` caught it). | Fixed: the scratch lives in the `Layout`. |
| 2 | Major | Hinted vertical motion scanned from the line start with the mark's column, then from the mark with a swapped tuple (found by the property test before commit). | Fixed; the property compares hinted with plain motion. |
| 3 | Minor | Random edits did not exercise the mark's after-character margin. | `an_insert_joining_clusters_at_a_mark_drops_it`. |
| 4 | Minor | A match crossing a wrapped row break is not underlined (each row searches itself only). | Accepted for 7b; Stage 7c draws matches per line. |
| 5 | Minor | `n` after a search typed in insert mode with `C-f` selects the match in insert mode; typing then inserts at its end. | Accepted (selection-first: the match is a selection). |
| 6 | Note | A started background search cannot be cancelled. | Accepted (DECISIONS 042); the slot keeps at most one waiting. |
| 7 | Major | The latency bench (section F) showed the first layout 10–13× slower per key than 7a on the 1 MB single line (about 26 µs: every key walked up to 1 KiB from a mark) and up to 2× slower on short lines (walked from their start), and typing into an open undo group slower on every fixture (the view, scrolled sideways, extended marks for every visible line through an O(log n) lookup each). The editor end-to-end bench had hidden it. | Fixed: the hot point, prefix-only dependencies with a boundary re-check (typing at the cursor no longer drops its point), stop marks, 512-byte mark spacing, and visible lines checked by scanning for their newline. Regression tests: `appending_a_combining_mark_at_the_end_drops_points_there`, the CRLF case, the property test (now with backward walks and `extend_visible`; dropping the tab guard of the backward walk fails it). |

## H. Known limitations

- A jump along a long line walks from the last mark: about 26 ms on the
  1 MB `one-line` fixture after an edit splits or shortens it.
- Backward search scans from the start of the text.
- `:%s` runs on the main thread.
- Soft wrap breaks after spaces and tabs only (no UAX #14), and has no
  indented continuation rows.
- Matches are underlined only within the drawn part of a line or row
  (4 KiB at most) until Stage 7c.

## I. Questions for the reviewer

- Should `j`/`k` move by rows when soft wrap is on? (Kept logical, as vim;
  `g j`/`g k` move by rows.)
- Is 256 KiB the right size to start searching off the main thread? Below
  it a whole-document miss costs well under a millisecond.

## J. Next stage

Stage 7c: the cell grid with views and overlays composited, renderer
alternatives judged by bytes and cells per frame, themes, slow output,
and the input-thread experiment.
