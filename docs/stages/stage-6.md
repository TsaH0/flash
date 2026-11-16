# Stage 6 — Minimal usable terminal editor

Status: **complete; awaiting owner approval** (no independent review was
run). Branch `stage-6` (on top of `stage-5`), local commits only
(nothing pushed). Design:
[2026-11-16-stage-6-design.md](../superpowers/specs/2026-11-16-stage-6-design.md).
Plan: [2026-11-16-stage-6-terminal-editor.md](../superpowers/plans/2026-11-16-stage-6-terminal-editor.md).

The owner asked for this stage with "proceed to next stage" and decided
one question (2026-11-16): **Ctrl+Z suspends**; undo moved to Alt+Z, redo
to Alt+Y and Alt+Shift+Z. The design's choices marked **(call)** are the
implementer's and are listed in section G.

## A. Status

| Requirement (PLAN Stage 6) | State |
|---|---|
| Main loop: input thread, one channel, drain then render | Done (`apps/flash/src/main.rs`; DECISIONS 024): input and signal threads, one `mpsc`, 8 ms drain, requests, one frame |
| Terminal lifecycle: raw mode, alternate screen, bracketed paste; restored on exit, on error, from a panic hook; suspend/resume | Done (`flash-term/src/terminal.rs`); pty tests for quit, panic, SIGTERM, Ctrl+Z and SIGCONT |
| Rendering: lines, selections, cursor, status line, prompt, resize, horizontal scroll, tab width, per-cluster width | Done (`flash-term/src/render.rs`; DECISIONS 023, 025); emoji width disagreement documented (H) |
| Hard-coded keys incl. register and OSC 52 clipboard | Done (`editor/keys.rs`, `flash-term/src/osc52.rs`) |
| Loading: streamed, invalid UTF-8 refused, BOM, dominant line ending, size/mtime/hash | Done (`file/mod.rs`; DECISIONS 021) |
| Saving: temp + fsync + permissions + rename over the resolved target + directory fsync; in-place behind a backup for hard links or ownership; conflict check; failure leaves the original | Done (`file/save.rs`; DECISIONS 021) |
| Instrumentation: dequeue, command, commit, layout, flush | Done (`apps/flash/src/latency.rs`); `FLASH_LATENCY_LOG` and `--replay` report per-stage percentiles |
| Tests: save paths, round trips, pty restore | Done (section D) |

## B. Commits

Branch `stage-6`, from `9ae5b68` (Stage 5 head):

| Commit | Content |
|---|---|
| `cf6e21f` | Stages 4 and 5 recorded approved; Stage 6 design and plan |
| `b472520` | `file`: streaming load, atomic and in-place save, conflict check; 19 cases |
| `fab4de2` | Editor prompt, register, requests, Stage 6 keys, tab width, horizontal scroll |
| `545767a` | `flash-term`: renderer, input conversion, OSC 52, terminal guard and panic hook |
| `c3e86e2` | Main loop, signals, suspend, request execution, pty tests |
| (closing commit) | Review fixes and tests (column checkpoints, trace on every exit path), file bench, results, this report, DECISIONS 020–025, status |

New dependencies: `crossterm` (moved into `flash-term`), `signal-hook`
0.3; dev-only `tempfile` 3 and `rustix` 1 (pty, termios).

## C. Design and algorithms

### The core does no I/O (DECISIONS 020)

Commands that need the outside world queue a `Request` (`Save`, `Open`,
`Clipboard`, `Suspend`); the binary executes them
(`apps/flash/src/requests.rs`) and reports back through
`Editor::saved` / `Editor::opened`. Replay drops requests, so a trace can
never write a file. The load and save algorithms are still in core
(`flash_core::file`, std only) so they are tested without a terminal.

### Loading and saving (DECISIONS 021)

Load streams 64 KiB reads into the rope builder, validating UTF-8 across
read boundaries, stripping and remembering a BOM, counting line endings,
and hashing the bytes on disk (FNV-1a 64). Save resolves symlinks, checks
size, mtime, then hash against the state recorded at load or last save,
and writes atomically (temp file, fsync, permissions, owner, rename,
directory fsync) or, for hard links or an owner it cannot keep, in place
behind a backup in the same directory. Every failure before the rename
removes the temp file; an in-place failure keeps the backup and names it.
`is_modified` compares the history head with the saved node, so undoing
back to the saved text reads as unmodified.

### Rendering (DECISIONS 023)

A full redraw per frame into a reused buffer between synchronized-update
escapes, one write. Graphemes are measured per cluster; after a cluster
whose per-character width differs (ZWJ emoji) the cursor is re-positioned,
so later columns are right whether or not the terminal joins the cluster.
Control characters are drawn as U+FFFD and bidi controls dropped, so no
raw control byte from the text reaches the terminal.

### The head's column (DECISIONS 025, review finding 1)

Horizontal scroll and the status line need the primary head's display
column, which is a sum over the line from its start. Each view keeps a
`LineAnchor` with up to three checkpoints (head, left edge of the visible
part, and a rear checkpoint 1–2 KB behind the head); after a change a
checkpoint is reused only if the revision log proves the line's prefix up
to it untouched (`RevisionLog::map_untouched`) and it was and still is a
grapheme boundary on the head's line. The column is then a short walk
from the nearest one. The renderer reuses the same anchor for the head's
column and for where to start drawing the head's line.

### Main loop (DECISIONS 024)

Block for one message, drain queued messages for up to 8 ms, execute
requests after each, render once. Ctrl+Z (a request) and an external
SIGTSTP restore the terminal and `raise(SIGSTOP)`; on return the guard
re-enters and redraws. SIGTERM, SIGHUP and SIGINT restore and exit with
128 + signal. The binary refuses to start unless stdin and stdout are
terminals. `FLASH_TRACE` records every handled event, including the one
that quit (review finding 2).

## D. Tests

Counts at the closing commit: flash-core 252 (Stage 5: 213), flash-term
18, flash 6 (library) + 6 (CLI) + 6 pty (+1 ignored measurement).

- **Files** (`file/tests.rs`, 19): round trips byte-identical for LF,
  CRLF, mixed, BOM, no final newline, empty; every read chunk size; invalid
  UTF-8 refused with its offset; non-regular files refused; new file,
  existing (permissions kept), symlink, hard link (in place), read-only
  directory, changed on disk (conflict, then forced), saving over a
  directory, a simulated failure at every phase before the rename (original
  intact, temp removed), a failed directory sync, ownership fallback,
  in-place failures (original or backup kept), BOM written back, relative
  paths.
- **Editor** (`editor/commands_tests.rs`, 15, plus `editor/tests.rs`):
  prompt flows, save, open, quit and close with modified documents,
  register with matching and mismatched counts, paging, view cycling,
  horizontal scroll, tab width; the review added
  `local_edits_on_a_long_line_do_not_walk_it` and
  `checkpoints_on_another_line_are_ignored`.
- **Random events** (`editor/proptests.rs`): 200 texts × 100 events
  across every key, paste, and resize; now also checks that the focused
  view's anchor is current and that every current anchor (head column,
  line start, left-edge and rear checkpoints) equals a walk from the line
  start.
- **Change sets and the log**: `map_untouched` unit cases, a property
  (the range keeps its text and the character before it, and agrees with
  `map_pos`), and the log's chaining, eviction and future cases.
- **Renderer** (`render/tests.rs`, 15): a small terminal emulator in both
  cluster models checks lines, status, cursor, tabs, wide characters at
  both edges, control and bidi characters, selections, the prompt, tiny
  and huge screens, anchors against walking, and 150 texts × 100 random
  events (frames with and without the anchor identical).
- **Binary** (`tests/cli.rs`, `src/requests_tests.rs`, `tests/pty.rs`):
  usage errors exit 2, editing needs a terminal, invalid UTF-8 and
  directories refused, replay; request execution; in a real pty: type,
  save and quit; quitting with unsaved changes asks; a panic, SIGTERM, and
  Ctrl+Z then SIGCONT all restore the terminal; a session recorded with
  `FLASH_TRACE` ends with the key that quit and replays without touching
  the file.

## E. Commands run

All with `--offline`, at the closing commit's tree.

Gate run (2026-11-17, after the review fixes; every step passed):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings` (plain and
  `--all-features`): clean.
- `cargo test --workspace`: flash-core 252, flash-term 18, flash 6 + 6 +
  6 passed (1 ignored: the interactive measurement) (47 s).
- `cargo test -p flash-core --features small-chunks`: 252 passed (152 s).
- `PROPTEST_CASES=4096 cargo test -p flash-core --release`: 252 passed
  (70 s); with `--features small-chunks`: 252 passed (220 s).
- `cargo check` in `crates/flash-core/fuzz`: builds (re-checked after
  touching a core file, so not a cached result).

Benchmarks:

- `FLASH_BENCH_JSON=$PWD/bench/results/stage-6-latency.jsonl cargo bench -p
  flash-core --features internals --bench latency`
- `FLASH_BENCH_JSON=$PWD/bench/results/stage-6-editor.jsonl cargo bench -p
  flash --bench editor`
- `FLASH_PTY_LATENCY_OUT=$PWD/bench/results/stage-6-interactive.jsonl
  cargo test --release -p flash --test pty interactive_latency -- --ignored`
- `cargo bench -p flash-core --bench file` on ext4 and on tmpfs
  (`stage-6-file-ext4.jsonl`, `stage-6-file-tmpfs.jsonl`; run before the
  review fixes, which do not touch file code).

Also run during the review: a one-off scaled random-event run (3,000
texts × 100 events, `REAR` = 16) with every anchor checked; mutation
checks in G.

## F. Benchmarks

Machine and build as in earlier stages (release profile, `--offline`).
Results in `bench/results/stage-6-*`.

**Per-operation latency** (`benches/latency.rs`), ns, p50 / p99, against
Stage 5. 100,000 samples per row except where noted; allocations per op
are identical to Stage 5 in every row below:

| op | small | medium 1 MB | large 100 MB | one-line 1 MB | unicode 1 MB |
|---|---|---|---|---|---|
| doc commit insert char (1 view), Stage 5 | 700 / 882 | 1,774 / 2,092 | 2,549 / 2,930 | 1,780 / 2,130 | 1,798 / 2,931 |
| doc commit insert char (1 view), Stage 6 | 969 / 1,134 | 2,647 / 3,145 | 3,950 / 4,607 | 2,889 / 3,380 | 2,853 / 4,364 |
| editor handle key 'x' | 1,004 / 1,243 | 2,655 / 4,004 | 3,971 / 4,593 | 2,976 / 3,472 | 2,880 / 4,358 |
| editor key into open group (100) | 1,191 / 1,310 | 1,687 / 1,845 | 1,960 / 2,065 | 1,664 / 1,761 | 1,681 / 1,783 |
| undo char | 949 / 1,793 | 2,066 / 3,052 | 2,505 / 3,499 | 3,850 / 4,515 | 2,272 / 3,416 |
| redo char | 560 / 848 | 1,259 / 1,879 | 1,522 / 2,671 | 1,387 / 2,205 | 1,352 / 2,326 |
| editor handle Right | 401 / 461 | 1,214 / 1,590 | 1,900 / 2,776 | 1,412 / 1,854 | 1,482 / 2,720 |
| motion down (Stage 5 / Stage 6) | 1,091 / 1,084 p50 | 1,885 / 1,867 | 2,730 / 2,693 | 26.6 ms / 25.8 ms (1,000 samples) | 3,256 / 2,033 |

Every editing key now also keeps the view's horizontal scroll and line
anchor (DECISIONS 023, 025), which costs about 0.2–1 µs per key over
Stage 5 (a few more tree lookups: the line start, boundary checks, the
log check); one-cursor commit on 1 MB is **3.1 µs p99** against the
PLAN's "well under 100 µs". Before the review fix (finding 1) the same
rows on `one-line` took milliseconds and the run could not finish.

Bench method changes (review finding 4): editor rows start from clones of
1,000 editors prepared at random offsets (on `one-line`, reaching a random
offset walks the line, and 100,000 such setups took hours), and each row
stops after 10 s once it has 1,000 samples, which only `one-line` motion
down hit. Rows report their sample count in the JSON.

**Editor end to end** (`apps/flash/benches/editor.rs`: `handle_at` +
`Renderer::render` into a buffer, the Stage 5 skeleton's trace pattern),
ns, p50 / p99:

| fixture | 80×24 | 200×60 | Stage 5 80×24 |
|---|---|---|---|
| small 4 KB | 24,482 / 43,796 | 50,522 / 82,538 | 24,278 / 48,186 |
| medium 1 MB | 40,967 / 101,206 | 71,721 / 133,197 | 40,549 / 102,587 |
| unicode 1 MB | 39,756 / 117,391 | 71,445 / 149,367 | 38,901 / 118,053 |
| one-line 1 MB | 10,861 / 26.8 ms | 23,810 / 26.8 ms | 11,284 / 26.0 ms |

The full redraw with horizontal scroll, selections, and the new status
line costs the same as Stage 5's skeleton. The `one-line` tail is Down,
Up, End and Home in the pattern: vertical motion computes its goal column
and walks the target line from its start, and End is a jump (Stage 3's
motion design; the Stage 5 report attributed this tail to the status
column, which no longer walks the line). Before the review fix every
`one-line` event took about 26 ms.

**Interactive, real pty** (`tests/pty.rs` `interactive_latency`, release,
500 keys 5 ms apart into a 1 MB file, read → flush), ns:

| stage | p50 | p99 |
|---|---|---|
| total | 229,276 | 475,217 |
| queue (read to dequeue) | 90,256 | 156,425 |
| command | 27,740 | 58,141 |
| commit (inside command) | 11,884 | 40,414 |
| layout (frame built) | 79,909 | 284,778 |
| flush | 20,730 | 42,088 |

Under the PLAN's 2 ms p99 for event dequeue to flush. These include the
input thread and the pty; the queue stage is the hand-off from the input
thread until the loop dequeues the event. Before the review fixes the
same run measured 262 / 523 µs.

**Files** (`benches/file.rs`, median of runs):

| fixture | load | save (atomic, checked) ext4 | save tmpfs |
|---|---|---|---|
| medium 1 MB | 2.2 ms (~460 MB/s) | 4.1 ms | 2.6 ms |
| large 100 MB | 220 ms (~455 MB/s) | 282 ms | 257 ms |

Save includes the conflict check (stat, then a hash read of the old
file), the write, fsync of the file and the directory.

## G. Review

**Independent review: not done by a fresh reviewer** (owner's standing
rule: no delegation without an explicit request). The implementer
reviewed the branch as a separate pass with attack tests and mutation
checks. This is weaker than PLAN section 3 requires; **the owner should
decide whether to run an independent review before approving.**

Findings:

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | Important | Every event walked the head's line from its start to find its display column (for horizontal scroll and the status line): on the 1 MB `one-line` fixture a key cost milliseconds in the core (Stage 5: about 2 µs) and about 26 ms end to end. Found because the core latency bench could not finish. | Fixed: column checkpoints validated through the revision log (DECISIONS 025); `one-line` keys are back to 1–4 µs. Two bugs in the first version were found by the strengthened random test before commit (a non-boundary head used as a checkpoint; tab-dense lines falling back on every Backspace). |
| 2 | Minor | The scroll bookkeeping made every key 2–4× slower than Stage 5 on all fixtures (about 0.3–2 µs more), mostly the walk back to the view's left edge. | Partly fixed: the left edge is the line start when not scrolled, else walked forward from the previous one; about 0.2–1 µs remains (F), accepted. |
| 3 | Minor | `FLASH_TRACE` dropped the events handled in the batch that quit, so a recorded session did not end with its quitting key; an external SIGTSTP suspended before the last frame was flushed. | Fixed: events are recorded on every exit path, output flushed before suspending; `a_recorded_session_replays` (pty) records and replays a real session. This also closes Stage 4's "FLASH_TRACE not verified in a real terminal". |
| 4 | Minor | The latency bench's setups became O(line) on `one-line` (each sample jumped to a random offset), so a run took hours. | Prepared starting editors and a 10 s per-row budget (F); sample counts are in the JSON. |

Found and fixed during implementation, before this review (kept as
tests): ZWJ emoji widths drifting later columns on terminals that do not
join clusters (re-sync; `columns_after_a_disputed_cluster_are_right_on_any_terminal`);
anchors that started after zero-width graphemes at the left column
(`random_events_render_cleanly`); `flash` with no file grabbing `/dev/tty`
when run without a terminal (`editing_needs_a_terminal`).

Mutation checks (each made the code wrong, then was reverted):

| Mutation | Result |
|---|---|
| A checkpoint not required to be a grapheme boundary now | caught by `random_events_never_panic` |
| `map_untouched` allows a delete ending at the start / an insert at the start | caught by the unit test and the property |
| Walking back over a tab | caught by `random_events_never_panic` |
| Checkpoints accepted from another line | **survived** (not reachable by keys today); added `checkpoints_on_another_line_are_ignored`, now caught |
| Events of the quitting batch not recorded | caught by `a_recorded_session_replays` |
| Forward left-edge walk passes zero-width graphemes at `left` | caught (core random test and `random_events_render_cleanly`) |
| Forward left-edge walk without its stop at the head | **survived**: unreachable (the head's column is at least `left`); now a `debug_assert` |
| Reuse the old left edge without `column <= left` | **survived**: implied by `offset <= head`; folded into one `checked_sub` |
| Reuse the old left edge after any horizontal scroll | **survived**: correct, only slower (a far start); performance guard, kept |
| No `left == 0` fast path | **survived** by design: same result, slower |

Implementer calls made without the owner (cost if wrong in brackets):

- The core does no I/O; requests executed by the binary (DECISIONS 020)
  [an API shape; cheap to revisit before Stage 7 adds commands].
- A missing file opens as an empty document with that path (vim) [one
  branch in `file::load` callers].
- Failure injection through a test-only phase hook, not a storage trait
  [test seam only].
- A read-only directory fails the save; no in-place fallback there
  [one branch].
- Saving checkpoints history so typing after a save is a new node [one
  call].
- Paste distributes pieces when counts match, else pastes the joined text
  everywhere; copying with nothing selected is a message [command
  bodies].
- Tab width is an editor setting (`--tab-width`, default 4) until Stage 7
  options [one field].
- Full redraw every frame; Stage 7 benchmarks diffing (PLAN) [renderer
  internals].
- 8 ms drain budget; FNV-1a 64 content hash [constants].
- SIGTERM/SIGHUP/SIGINT exit without saving; refuse to start without a
  terminal [policy lines in `main.rs`].
- Rear checkpoint 1 KB behind the head; bench method changes in F
  [constants, bench only].

## H. Known limitations

- Emoji width: terminals disagree on some clusters (ZWJ sequences, some
  flags); later columns stay right (re-sync), but the cluster itself may
  look clipped or padded on some terminals.
- On a very long line, vertical motion and Home/End are O(line) (about
  26 ms on 1 MB): the motion layer walks from the line start (Stage 3).
  Typing, deleting, Left/Right, undo and redo near the head are not.
- A jump along a long line, an edit at least 1 KB behind the head, or an
  edit that touches every checkpoint walks the line once (DECISIONS 025).
- When the left-edge checkpoint is unavailable (a tab in the way after a
  jump), the renderer walks the head's line from its start for that
  frame.
- SIGTERM, SIGHUP, SIGINT exit without saving (atomic saving is not crash
  recovery).
- The real `fchown` path (a file owned by another user) is tested only by
  simulation; it needs root.
- The panic pty test passes through the main thread's guard drop; the
  panic hook (for panics on other threads) is not separately proven by a
  test. After a panic on the input thread, the loop could be left
  waiting for input that will not come (quit with a signal).
- The group pause is not in the trace header (Stage 5 G3).
- Opening the same file twice makes two documents; the save conflict
  check stops one from silently overwriting the other.
- Each key costs about 0.2–1 µs more in the core than in Stage 5 (F).

## I. Questions for the reviewer

1. Can any save path lose or corrupt the original file, or report success
   when the bytes on disk differ from the buffer?
2. Can any exit path (quit, error, panic on any thread, signal, suspend)
   leave the terminal in raw mode or on the alternate screen?
3. Is there a sequence of edits, undos, motions, resizes, or tab-width
   changes after which the anchor's column or start differs from walking
   the line from its start (DECISIONS 025)?
4. Can any text (control characters, escapes, bidi controls, huge or
   zero-width clusters) make the renderer emit a raw control byte or draw
   outside its row?
5. Can a trace replay write a file, touch the clipboard, or differ from
   the recorded session?

## J. Next stage

Stage 7: commands, keymaps, modes, events, fast rendering. It replaces
`editor/keys.rs` with a command registry and layered keymaps, adds
options and a TOML config, modal editing, search and replace, soft wrap,
and a renderer chosen by benchmark (where a per-line layout cache can
also remove the long-line limits in H). Started at the owner's request in
the same session ("continue the next stage in here as well"), on branch
`stage-7` on top of `stage-6`; Stage 6 approval is the owner's to record.
