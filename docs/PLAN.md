# FLASH — Master Engineering Plan (revision 3)

A fast, extensible Rust text editor built on the Crop rope, delivered in 11 gated stages.

Revision 2 replaced the original 10-stage plan
([Appendix A](#appendix-a--changes-from-revision-1)). Revision 3 (2026-11-17, after Stage 7a)
folds in a review of the measured latency and the code so far; it changes Stage 7b onward and
is listed in [Appendix B](#appendix-b--changes-from-revision-2).

---

## 0. Mission

You are the implementation engineer for Flash. You write working, tested, measured code in this
repository, one stage at a time, and stop at each stage gate for the owner's approval.

**Objective:** minimise the latency between a user's input and the visible change, while keeping
the editor correct, extensible, and simple enough to maintain. Memory may be spent (up to roughly
2x a minimal terminal editor) where it measurably reduces latency.

Optimise for demonstrated performance, correctness, and maintainability, not for cleverness.

## 1. Actual starting state (verified 2026-11-12)

The repository is a fresh `cargo new`:

- `Cargo.toml`: package `flash`, edition 2024, no dependencies.
- `src/main.rs`: three lines.
- No commits, no workspace, no Crop integration, no ChangeSet, no tests.

Earlier design discussions (ChangeSet, `Assoc`, inversion, batch-edit semantics) exist only as
ideas. **Nothing is implemented. Stage 1 starts from scratch.** Re-verify this section if the
repository has changed since the date above; never infer completion from file names.

## 2. Fixed design decisions

Challenge these only with new evidence (a benchmark, a counterexample, a dependency limitation).

### 2.1 Language and safety

Rust, edition 2024, MSRV 1.85 (Crop's MSRV). `#![forbid(unsafe_code)]` in `flash-core` until a
measured bottleneck justifies lifting it for one module, with documented invariants and tests.

### 2.2 Workspace: three crates, not five

```text
flash/
├── Cargo.toml              # workspace
├── crates/
│   ├── flash-core/         # text, changeset, selection, transaction, document, history, editor
│   └── flash-term/         # crossterm input, layout, renderer
└── apps/flash/             # binary: wiring, CLI, main loop
```

`Text` and `ChangeSet` live in the **same crate** as `Document`. This is deliberate: the rule
"only transactions mutate the rope" is enforced by `pub(crate)` on `Text`'s mutators. With a
separate `flash-text` crate the mutators would have to be `pub` and the rule would be a
convention. Add crates later only for a concrete reason (e.g. `flash-lsp`, `flash-plugin` to keep
heavy dependencies out of core).

### 2.3 Text storage: Crop 0.4.3, pinned

```rust
pub struct Text { rope: crop::Rope }
```

Facts verified against the crop 0.4.3 source, which the wrapper must account for:

| Crop behaviour | Consequence for `Text` |
|---|---|
| `insert`/`delete`/`replace`/`byte_slice`/`line_of_byte`/`byte_of_line` **panic** on out-of-bounds offsets and on non-char-boundary offsets | Every public `Text` method validates and returns `Result`. No panic may be reachable from user input, a plugin, or an LSP message. |
| `line_len()`: `""`→0, `"a"`→1, `"a\n"`→1, `"a\nb"`→2 | Crop counts *lines of content*; an editor needs *cursor rows* (`"a\n"` has two: the cursor can sit after the newline). `Text` defines its own line model on top (see Stage 1) and never leaks `line_len` semantics. |
| No char-index metric; only bytes, lines, optional UTF-16 | Consistent with byte-offset coordinates. Char counts are derived by iterating a slice when needed. |
| No multi-edit API; each edit is an independent O(log n) descent | ChangeSet application strategy (descending per-op edits vs. rebuild with `RopeBuilder`) is a benchmark question in Stage 2, with a crossover expected at high edit counts. |
| `Rope::clone` is O(1) (shared tree); rope is `Send + Sync` | Snapshots for background workers are plain clones. No locking. |
| `chunks()` is double-ended; there is no "chunk containing byte N" accessor | The vendored crop adds `Rope::chunk_at_byte` (O(log n), no allocation; DECISIONS 008). Tree-sitter's read callback and any other seek-by-offset reader use it, not `byte_slice(n..).chunks()`, which allocates a traversal stack per seek. |
| Features: `graphemes` (unicode-segmentation), `utf16-metric`, `simd` (default) | Enable `graphemes` from Stage 1. Benchmark `utf16-metric` in Stage 1 (edit and build overhead) and record the result; enable it only when LSP lands if the overhead is acceptable, otherwise convert per line. |

Do not build a custom rope. No `dyn` text trait on the hot path.

### 2.4 Coordinates

UTF-8 byte offsets are canonical. Line/column, grapheme, display column, and UTF-16 positions are
derived. Every stored offset must be a char boundary; grapheme alignment is a property of cursor
motion, not a stored invariant.

### 2.5 Selection model

**Decided (D1, 2026-11-12): selection-first modal editing** in the Kakoune/Helix style. Motions
create or extend selections; operators act on the existing selections.

- Core storage: a range is `{ anchor, head }` in byte offsets **between** characters
  (gap-indexed). Core permits empty ranges.
- Normal mode: every range covers at least one grapheme, so the "cursor" is a one-grapheme
  selection and the block is drawn over it. The exception is an empty document or the end of the
  text, where the range is empty.
- Insert mode: text is inserted at the head; the range is kept and mapped through the edit.
- The one-grapheme minimum is enforced by the mode layer (Stage 7) when entering normal mode and
  after each normal-mode command. It is not a core invariant, so plugins can define modes with
  point cursors.
- Merge rule: overlapping ranges merge. Touching non-empty ranges stay separate (adjacent
  selections such as consecutive words or lines must remain distinct). An empty range touching
  or inside another range merges into it.

### 2.6 Editing

All text changes go through a `Transaction` holding a `ChangeSet` (Retain/Delete/Insert with
`len_before`/`len_after`). Structural validity is checked at construction; char-boundary validity
is checked against the document before commit. A failed transaction changes nothing.

### 2.7 History and revisions

Branching tree of committed transactions storing forward and inverse ChangeSets. History node IDs
and document revisions are distinct; the revision advances on every effective text change
including undo and redo.

The document also keeps a bounded **revision log**: the last N `(revision, ChangeSet)` pairs. This
is what lets a result computed against revision R be mapped forward to the current revision, or
rejected when R has fallen out of the log. Without it, "transform stale results when safe" has
nothing to transform with.

### 2.8 Concurrency and the main loop

One main thread owns all mutable editor state. Everything else talks to it through **one
channel**:

```text
input thread (crossterm read) ─┐
tree-sitter worker ────────────┤
LSP reader threads ────────────┼──> mpsc<Event> ──> main loop ──> render ──> terminal
file watcher / timers ─────────┘
```

Loop policy, fixed from Stage 6:

1. Block on the channel for one event.
2. **Drain every event already queued** (bounded by a time budget), applying each.
3. Render once.

This coalescing is the main defence against input bursts, held keys, and floods of worker results.
Plain threads and channels; no async runtime in `flash-core`. Workers receive `(revision, Text
clone)` and every result carries the revision it was computed against. Worker queues are bounded
and "latest wins".

Added in revision 3:

- **The input thread is under review.** The Stage 7a pty run puts the hand-off from the input
  thread to the main loop at 86 µs p50 of a 280 µs total: the largest stage after layout, and
  pure wake-up cost. Stage 7c prototypes the alternative (the main thread blocks in one `poll`
  over the terminal fd and a wake fd that workers and the signal handler write to, then drains
  the channel) and measures both in the pty harness. Crossterm cannot add fds to its poll, so
  the alternative replaces crossterm's input half; adopt it only if the measured gain repeats
  and the parser is covered by tests. The single-owner, one-channel, drain-then-render policy
  does not change either way.
- **An event's time is when it was read**, not when the main loop dequeued it. The frontend
  passes the read timestamp to `handle_at` and records it as the trace's `t_ns`, so a stall
  on the main thread cannot change undo grouping or key-sequence timeouts.
- **File I/O is not done on the main thread.** Save runs on a worker from an O(1) text
  snapshot and reports back through the channel with the revision it saved (DECISIONS 020
  already has this shape); opening a file loads on a worker. Measured cost of doing it inline:
  282 ms to save and 220 ms to load 100 MB.

### 2.9 Terminal

Crossterm. Custom renderer: cell grid, diff against previous frame, one buffered write per frame,
synchronised-update escapes when supported. Synchronous output until measurements justify a writer
thread. Bracketed paste is enabled and a paste is **one** transaction.

The kitty keyboard protocol's disambiguate-escape-codes flag is enabled where the terminal
supports it (pushed on enter, popped on restore, suspend, and panic). Without it an escape
sequence split across reads arrives as Esc followed by literal keys, which in a modal editor
leaves insert mode and runs commands. On terminals without it this stays a documented
limitation.

The grid composites more than one source from the start: several views (splits) and overlays
(prompt, popups) draw into regions of one grid, and the diff runs over the result. The
single-full-screen-view layout of Stages 3 to 7a is temporary.

### 2.10 Plugins

Three tiers:

| Tier | Written in | What lives there |
|---|---|---|
| Core and primitives | Rust | Text, transactions, history, markers, rendering, key resolution, and every primitive that works on whole selection sets (motions, text objects, edits, search) |
| Behaviour | Luau, plus data | Keymaps, modes, options, commands composed from native primitives; user config and most plugins |
| Isolated extensions (after the 11 stages) | WASM or separate process | Heavy or untrusted work off the main thread, submitting revision-tagged transactions |

**Scripting language (D4, decided 2026-11-12): Luau via `mlua`.** Chosen for latency: among the
embeddable options it has the fastest interpreter, optional native code generation, an
incremental garbage collector built for frame-rate-bound hosts, and byte strings that carry
UTF-8 text and byte offsets across the boundary without conversion. Alternatives considered and
rejected: QuickJS (slower; UTF-16 strings force text and offset conversion), Steel (slower;
interrupt and memory-cap support unverified), WASM as the primary system (sealed modules cannot
give live, redefine-anything extensibility; kept for the isolated tier).

The command registry, keymaps, option registry, and event hooks are still built natively in
Stage 7 with dynamic, owned registrations, so Luau is an adapter over them and nothing in Stages
1 to 10 depends on it.

**Hot-path rule:** ordinary editing never enters the VM. Typing,
deleting, built-in motions, and rendering run native code only. A script runs when a
script-defined command or hook is invoked, and then it calls native primitives that operate on
whole selection sets.

Plugins never touch document internals; they call validated commands and propose revision-tagged
transactions.

### 2.11 Configurability

**Everything above the core invariants is configurable, and the defaults are just configuration.**

- **No privileged built-ins.** The default keymap, the built-in modes (normal, insert, extend),
  and the default theme are defined as data through the same registration API that user config
  and plugins use. If the defaults can express something, so can the user. No key is hard-coded
  in Rust after Stage 7 (the Stage 3 skeleton and Stage 6 editor use temporary hard-coded keys
  that Stage 7 deletes).
- **Every bind is rebindable and unbindable**, per mode, including multi-key sequences, the
  pending-sequence timeout, and keys inside prompts and popups. Every user-visible action is a
  named command, so anything the editor can do can be bound.
- **Typed option registry.** Every tunable (tab width, wrap, scroll margin, undo-group pause,
  cursor shape per mode, line-ending default, search case rules, LSP and highlighting toggles,
  and so on) is a registered option with a name, type, default, validation, and owner. Options
  resolve through scopes: view → document → language → global → default. Plugins register their
  own options the same way.
- **Behavioural policies are options, not constants**: for example whether normal mode enforces
  the one-grapheme minimum, what counts as a word, and what ends an undo group.
- **Config sources**: a declarative file (TOML) from Stage 7 for options, keys, and themes,
  reloadable at runtime without restart; a Luau init script from Stage 11 for anything
  programmable. A bad config value is reported and skipped; it never prevents startup.
- **Reading an option on the hot path is a field read**, not a string lookup: options are
  resolved into a per-view struct when they change, not when they are used.

**Not configurable, by design:** the core invariants. Text changes only through validated
transactions; offsets are char boundaries; selections are sorted and normalised by the merge
rule in 2.5; revisions are monotonic; stale async results are never applied. These are what make
configuration and plugins safe.

### 2.12 Deferred

GUI frontend, separate server process, WASM/out-of-process plugins, file-backed large-file mode,
crash-recovery journal. Keep boundaries clean; build none of these in the 11 stages.

---

## 3. Development contract

1. **One stage at a time.** Finish, present evidence, stop. No silent work on later stages.
2. **3–6 subtasks per stage, one local commit each** on a `stage-N` branch. Commits make the
   diff reviewable and give the review packet real references. Never push, tag, publish, amend
   shared history, or run destructive Git commands without explicit approval. *(Owner decision
   D2: if local commits are not wanted, each subtask instead ends with a saved patch file.)*
3. **Tests are mandatory.** Unit tests for defined behaviour and error paths; property tests
   (proptest, regression files committed) for every transformation algorithm; a `cargo-fuzz`
   target for ChangeSet from Stage 2.
4. **Verify, don't assume.** Never claim a test passed, a number improved, or a dependency API
   exists without running or reading it in this workspace.
5. **Correct baseline first, then measure alternatives, keep the simplest one that meets the
   budget.**
6. **Explain substantial algorithms** in the stage report: problem, why it is correct,
   time and space complexity, corner cases, rejected alternatives. Skip this for boilerplate.
7. **Do not change dependency versions casually.** New dependencies are listed in the stage
   report with the reason.

### Stage gate, in order

1. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, the stage's property tests at an increased case count, relevant
   benchmarks in release mode.
2. Implementer review of the full stage diff.
3. **Independent adversarial review** by a fresh-context reviewer given only the stage document
   and the code. The reviewer's deliverable is **failing tests or concrete counterexamples**, not
   opinions. Agreement between agents is not evidence of correctness.
4. Fix or explicitly accept each finding, with severity.
5. Write **one** document, `docs/stages/stage-N.md`, which is both the completion report and the
   review packet: status, commits, design decisions and algorithm explanations, invariants and
   the tests that check them, exact commands and results, benchmark numbers with machine and
   build settings, review findings and resolutions, known limitations, next stage.
6. Update `docs/DECISIONS.md` (only for real decisions: decision, alternatives, evidence,
   trade-off, date, what would reopen it) and `docs/STATUS.md`.
7. Stop for owner approval.

---

## 4. Performance measurement

### Tools

| Question | Tool |
|---|---|
| Micro-benchmarks (rope ops, ChangeSet ops, key resolution) | Criterion |
| Latency distributions (p50/p95/p99/max) over a replayed trace | Custom harness recording per-event nanoseconds into an HDR histogram |
| Allocation counts and bytes on the hot path | Counting global allocator in bench and test builds |
| Peak memory | Peak RSS from the trace harness, plus the allocator's high-water mark |
| End-to-end terminal path | Trace replay with output written to a pty or sink, timed from event read to `flush` returning |
| Terminal-side cost of a frame | Bytes written per frame and cells changed per frame, recorded per event next to the timings |
| Cost after idle | The latency harness's idle mode: a pause (default 5 ms) between samples, so each one starts with the caches and CPU state a real keystroke finds |

True on-screen latency needs external hardware and is out of scope; do not report it.

Two rules added in revision 3, both from the Stage 7a numbers:

- **`flush` returning is not the end of a frame's cost.** It measures the kernel copy; the
  terminal still has to parse the bytes and repaint what they touch. Renderer alternatives are
  therefore compared on bytes and cells per frame first, and on CPU time second. A renderer
  that is faster to build but writes a full screen per key is not the winner.
- **The pty run is the reference; hot loops are for finding regressions.** A key costs 3.5 µs
  in the hot-loop harness and 45 µs in the pty run (commit: 3.3 µs against 16.6 µs). Stage
  reports quote the pty and idle-mode numbers against the targets below, and use hot-loop
  numbers only to compare one stage with the last.

### Fixtures (generated deterministically, created in Stage 1)

- `empty`, `small` (~4 KB source), `medium` (1 MB source, ~25k lines), `large` (100 MB).
- `one-line` (1 MB with no newline) and `unicode` (CJK, combining marks, ZWJ emoji, CRLF).

`one-line` exists because every line-based algorithm (layout, highlighting, line lookup) is
O(line length); it must be slow-but-bounded, never a hang.

### Targets (hypotheses, not guarantees)

- 1 MB buffer, single-character edit: core transaction commit well under 100 µs p99; event
  dequeue to terminal flush under 2 ms p99.
- Cursor-only motion does no text mutation, no allocation proportional to document size, and
  re-renders only what changed.
- 1,000 cursors: one edit stays inside a frame (under 16 ms); 10,000 cursors is measured and
  reported.
- No hot-path stall caused by parsing, LSP, plugins, or file I/O.
- A cursor-only move writes tens of bytes, not a screen; a single-character edit writes about
  one line's worth.
- The cost of a key does not grow with how long the session, the undo group, or the history
  has become.

Results are stored as JSON under `bench/results/` with machine, commit, and build settings, so
each stage can be compared with the last. Treat single runs as noise; a regression is real when it
repeats and exceeds the run-to-run spread.

---

## 5. Stages

### Stage 1 — Workspace, `Text`, test and benchmark foundations

**Build**

- Workspace as in 2.2; crop 0.4.3 with `graphemes`; proptest; Criterion.
- `Text`: byte length, line count, byte↔line, line slice, range slice, chunk iteration, char and
  grapheme boundary checks, O(1) snapshot; `pub(crate)` insert/delete/replace. All fallible
  operations return `Result<_, TextError>`.
- **Line model**, written down and tested: number of cursor rows (`""`→1, `"a"`→1, `"a\n"`→2);
  line start offsets; line content with and without terminator; `\n` and `\r\n` are terminators,
  lone `\r` is not; text is stored byte-for-byte as loaded, never normalised.
- `String`-backed reference model with the same API.
- Deterministic fixture generator (section 4) and the counting allocator.

**Tests**: random edit sequences against the reference model across empty, ASCII, multibyte,
combining marks, emoji, CRLF (including edits that split and join `\r\n`), long lines, many short
lines; every error path; no reachable panic for any offset in `0..=len+1`.

**Benchmarks**: insert, delete, replace, byte↔line lookup, snapshot, chunk iteration, build from
file, on `small`/`medium`/`large`/`one-line`. Also: overhead of the `utf16-metric` feature.

**Accept when**: workspace builds; randomised edits match the model; line model documented and
tested; benchmark baseline stored.

### Stage 2 — ChangeSet algebra *(high risk)*

**Build**

- Representation: `Vec<Op>` where inserts hold a small-string-optimised string. Baseline first;
  measure a shared insertion buffer only if 10k-insert ChangeSets show allocation cost.
- Builder: merges adjacent compatible ops, drops empty ops, canonical ordering of adjacent
  Delete/Insert, rejects length mismatches.
- `validate(&Text)`: lengths match and every op boundary is a char boundary.
- `apply`: baseline is descending-offset per-op edits. Benchmark against a `RopeBuilder` rebuild
  and pick by edit count if a crossover exists. Application to a clone, then swap, gives
  atomicity for free.
- `invert(&original)`, `compose(a, b)` (requires `a.len_after == b.len_before`).
- `map_pos(pos, Assoc)` and `map_sorted(&mut [pos])` in one pass over ops.

**Mapping policy to define explicitly**: position at an insertion point (`Before` stays left of
the inserted text, `After` moves right); position inside a deleted range (collapses to the
deletion point); replacement (delete+insert at one point); end of document.

**Known trap**: `compose(a, b).map_pos(p)` is **not** always equal to `b.map_pos(a.map_pos(p))`
(delete-then-reinsert loses the position). Composition is guaranteed for *text* only. Anything
that tracks positions (selections, markers, the revision log) maps through the original sequence,
never through a composed ChangeSet, unless that loss is acceptable and documented.

**Properties (proptest + fuzz)**

- `retained + deleted == len_before`, `retained + inserted == len_after`.
- `apply(invert(c, t), apply(c, t)) == t`; `invert(invert(c, t), apply(c, t)) == c`.
- `apply(compose(a, b), t) == apply(b, apply(a, t))`; associativity at the text level.
- `map_sorted` equals per-position `map_pos`; mapped positions are char boundaries in the new
  text and monotonic.
- A ChangeSet that fails validation leaves the text untouched.

**Benchmarks**: build, apply, invert, compose, map for 1 / 100 / 10k ops on `medium` and `large`.

### Stage 3 — Selections, transactions, multi-cursor, and a walking skeleton

**Build**

- `Range`, `Selection` (sorted, non-overlapping, primary index; `SmallVec` inline capacity 1).
  Normalisation follows the merge rule in 2.5; the primary survives merging.
- Motions come in two forms from the start: *move* (replace each range with the motion's result)
  and *extend* (keep the anchor, move the head). Both are needed for selection-first editing.
- Selection mapping through a ChangeSet: defined per endpoint (a cursor uses `After` so typing
  advances it; a non-empty range does not absorb text inserted at its edges).
- Grapheme-aware motions: left/right, word, line start/end, up/down with a remembered display
  column, document start/end. Motions take `&Text` and are O(distance moved), never O(document).
- `Transaction { changes, selection: Option<Selection> }` and constructors that turn a selection
  plus an intent into one ChangeSet: insert at every cursor, delete ranges, replace, backspace.
  Deterministic collision rules for same-position inserts and overlapping deletes.
- **Walking skeleton**: a deliberately crude `flash <file>` binary (raw mode, full redraw each
  frame, type/delete/move/quit, no save). It is throwaway-quality UI over real core APIs. Its
  purpose is to exercise the APIs for real and to record an end-to-end latency number from this
  stage onward instead of first measuring at Stage 6.

**Tests**: normalisation cases; Unicode motion (combining marks, ZWJ emoji, CRLF); randomised
multi-selection edits against a naive model that applies edits one at a time back to front;
selection validity after every transaction; failed transaction leaves state unchanged.

**Benchmarks**: 1 / 10 / 100 / 1k / 10k cursors, clustered and spread, full transaction (build +
validate + apply + map selections).

### Stage 4 — Documents, views, revisions, headless editor

**Build**

- `Document { text, revision, revision_log, path, line_ending, ... }`; `View { doc, selection,
  scroll, mode state }`; `Editor { documents, views }` with generational IDs (`slotmap`).
- `Document::commit(txn, origin_view)` is the only way to change text: validate, apply, bump
  revision, append to the revision log, map every other view's selection.
- Revision log (2.7) with `map_from(revision, positions) -> Option<...>`.
- Headless API: `Editor::handle(Event) -> Effects`. The walking skeleton is rewired onto it.
- **Trace record/replay** (moved here from Stage 6): a serialisable event stream that drives the
  headless editor. It is both the integration-test harness and the latency benchmark driver.

**Tests**: two views on one document stay valid under random edits from either; closing a view or
document cleans up; stale IDs rejected; revisions strictly monotonic; stale-position mapping
through the log matches direct mapping.

**Standing benchmark** starts here: N views, 10k selections, one edit, complete commit. Each later
subsystem (history, markers, highlighting, plugins) is added to it as it lands.

### Stage 5 — Undo, redo, branching history *(high risk)*

**Build**

- History tree: each node holds forward and inverse ChangeSets, parent, children, selection
  before and after, timestamp.
- Undo, redo, branch on edit-after-undo, move to an arbitrary node (earlier/later by time).
- Grouping: consecutive typing transactions merge into the current node by **composition**
  (forward = compose(old, new); inverse = compose(inverse_new, inverse_old)) until a boundary:
  motion, mode change, pause over a threshold, or an explicit checkpoint.
- The view that invokes undo gets the stored selection; other views are mapped through the
  inverse ChangeSet.
- Undo and redo go through `Document::commit`, so revisions, the revision log, and cross-view
  mapping behave exactly as for ordinary edits.
- Measure bytes per history node over a long typing trace; add a cap with oldest-leaf pruning
  only if the measurement asks for it.
- *(Revision 3, fixed in Stage 7b.)* Merging by composition rebuilds the whole group's change
  set on every key, so a key costs O(group size): measured 1.7 µs per key at 10,000 keys in
  one group, 41 µs at 640,000 with a 28.7 ms worst key, 26 s in total. A group therefore also
  ends at a size bound (an option, default 64 KB of inserted text), or the merge appends to
  the head node in place; either way a test bounds the per-key cost of one long unbroken run.

**Tests**: undo all / redo all returns exact text; Unicode replacements; multi-cursor; multiple
views; branch and return; random edit/undo/redo/branch walks against a model that stores full
snapshots; grouped typing undoes as one unit and matches the ungrouped result.

### Stage 6 — Minimal usable terminal editor

**Build**

- Main loop as in 2.8: input thread, single channel, drain-then-render.
- Terminal lifecycle: raw mode, alternate screen, bracketed paste; restored on normal exit,
  on error, **and from a panic hook**; suspend/resume (Ctrl-Z) handled.
- Rendering: visible lines, selections, cursor, status line, a one-line prompt (needed for
  save-as, errors, and later search). Resize. Horizontal scroll for long lines (soft wrap is
  Stage 7). Tabs expanded to a configurable width. Display width via `unicode-width`, per
  grapheme cluster; known terminal disagreements on emoji width documented as a limitation.
- Hard-coded keys: open, type, delete, move, undo/redo, save, quit, plus copy/paste to an
  internal register and the system clipboard via OSC 52.
- **Loading**: stream into `RopeBuilder`. Invalid UTF-8: refuse to open for editing with a clear
  message (no silent lossy conversion). Strip and remember a UTF-8 BOM. Detect and remember the
  dominant line ending; new lines use it. Record size, mtime, and a content hash at load.
- **Saving**: write to a temp file in the same directory, fsync, copy permissions, rename over
  the *resolved* target (symlinks followed), fsync the directory. If the target has multiple hard
  links, or ownership cannot be preserved, fall back to an in-place write protected by a backup
  copy, and say so. Before saving, compare size/mtime/hash; on mismatch refuse and ask. A failed
  save leaves the original intact and the buffer marked modified.
- Instrumentation: timestamps at event dequeue, command done, commit done, layout done, flush
  done, feeding the trace harness.

**Tests**: save paths (new file, existing, symlink, hard link, read-only directory, changed on
disk, simulated write failure) in temp directories; load/save round-trip is byte-identical for
LF, CRLF, mixed, BOM, no final newline; pty-driven smoke test for terminal restore.

This is atomic saving, not crash recovery.

### Stage 7 — Commands, keymaps, modes, events, fast rendering

Delivered in three gates (DECISIONS 026): 7a (commands, events, keymaps, options and config,
modes, multi-selection commands; complete), 7b, and 7c. The list under **Build** is the
original scope; the revision 3 additions to 7b and 7c follow it.

**Build**

- **Command registry**: `CommandId(u32)`, semantic names, each registration has an owner and can
  be removed. Commands take a context (editor, view, count, argument) and are invocable without a
  key.
- **Event hooks**: owned subscriptions for document-changed, selection-changed, mode-changed,
  view-opened/closed, with batch payloads. Built now, with a native consumer, so Stage 11 only
  adds a Luau adapter.
- **Keymaps**: layered (mode → user → plugin → default), multi-key sequences with a pending
  state and timeout policy, compiled trie, zero allocation per key. The default keymap is loaded
  through the same path as user config; all Stage 6 hard-coded keys are removed.
- **Option registry and config** per 2.11: typed, scoped, owned options; TOML config for
  options, keys, and themes; runtime reload that recompiles the keymap trie and re-resolves
  per-view options; invalid entries reported and skipped.
- **Modes**: built-in normal, insert, and extend (select) modes per 2.5; the mode layer enforces
  the one-grapheme minimum in normal mode; mode state lives on the view. Multi-selection
  commands: split selection on regex/lines, keep/remove matching, add cursor above/below,
  rotate primary, collapse to primary.
- **Editor essentials** that a usable editor still lacks: incremental search and replace
  (regex over rope chunks with `regex-cursor` or equivalent, run off the main thread for large
  buffers), soft wrap, and a config file for keys and options.
- **Renderer**: benchmark full-grid diff against dirty-line tracking and keep the winner; cache
  per-line layout keyed by line content revision; cursor-only moves emit only cursor movement;
  reuse output buffers; minimise style transitions; scroll regions if they measurably help.
- **Slow output**: measure against a throttled pty. Add a writer thread only if it shows a
  responsiveness gain; if added, it diffs against the last frame actually emitted.

**Stage 7b, revision 3.** Before search and soft wrap, in this order:

1. *Carried fixes*, one subtask, each with a regression test. **Done 2026-11-17**
   (DECISIONS 033–039), uncommitted pending review:
   - regex search missing matches that cross a rope chunk boundary (found by the gate's
     4,096-case run after revision 3 was written; DECISIONS 033);
   - the undo-group size bound (Stage 5 note; DECISIONS 034);
   - event time taken at read (2.8; DECISIONS 037);
   - hooks keep a mask of the kinds anyone subscribes to, and nothing is queued for a kind
     nobody wants (DECISIONS 035);
   - the kitty keyboard flag (2.9; DECISIONS 036);
   - save and open on a worker (2.8; DECISIONS 038). The pty test uses a 32 MiB file
     rather than 100 MB to keep the suite fast; the property it checks (keys typed after
     Ctrl+S are drawn before the save reports back) does not depend on the size;
   - the idle mode of the latency harness and the bytes-per-frame counter (section 4;
     DECISIONS 039). The cells-per-frame counter moves to 7c, which builds the cell grid
     it counts.
2. *Display-line layout cache*, before soft wrap is built on anything else: per visible line,
   keyed by the line's content revision, tab width, and wrap width, giving its display rows
   and the byte offset ↔ display column mapping. Soft wrap, horizontal scroll, vertical
   motion, the status column, and the renderer all read it. It replaces the `LineAnchor`
   checkpoints (DECISIONS 025), which answer the same question for the primary head only;
   the 1 MB single-line fixture must stay at or below its current per-key cost.
3. Search and replace, and soft wrap, as listed above.

**Stage 7c, revision 3.**

- The cells-per-frame counter (moved from 7b: it needs the grid).
- The renderer comparison is decided by bytes and cells per frame first (section 4). The
  status line's latency readout becomes an option, off by default, so an idle frame is empty.
- The grid composites views and overlays (2.9); a second view on screen is part of the
  renderer's tests even though split commands may come later.
- The input-thread experiment (2.8), reported with pty numbers for both designs.

**Accept when**: a test config rebinds and unbinds default keys in every mode and changes
options at runtime without restart; no key is hard-coded in Rust; commands run independently of
keys; modal keymaps work; key resolution is
allocation-free and benchmarked; renderer choices are backed by numbers; search works on
`medium` without blocking input. Revision 3 adds: one unbroken typing run of 1,000,000 keys
keeps a bounded per-key cost; a 100 MB save does not delay a key; a cursor-only move and a
single-character edit meet the byte targets in section 4; the input-thread decision is
recorded in DECISIONS with both sets of numbers.

### Stage 8 — Markers and decorations

**Build**

- Semantics: ID, start/end, per-end gravity, behaviour when the range is deleted (collapse or
  invalidate), namespace, owner, opaque metadata handle, bulk clear by namespace or owner.
- Phase A: sorted-vector store updated with `map_sorted` on every commit. Range-intersection
  queries for the viewport.
- Benchmark at 1k / 10k / 100k markers in the standing benchmark.
- Phase B, only if Phase A breaks the edit budget at the required scale: an indexed structure
  with relative offsets (candidates: a B-tree with offset deltas in the style of Neovim's
  marktree, or an interval tree). Same public semantics; the vector store becomes the test
  oracle.

Recomputable syntax highlights are render cache, not markers.

### Stage 9 — Background services, Tree-sitter, highlighting

**Build**

- Worker infrastructure: bounded latest-wins request slot per document per service; results carry
  their revision; the main loop drops results older than what it has already applied.
- Tree-sitter: a small set of statically linked grammars; language detection by extension and
  first line; `InputEdit`s computed on the main thread at commit time (they need both pre-edit
  and post-edit row/column, which only the committing thread has cheaply); parsing in a worker
  from rope chunks (`chunk_at_byte`, 2.3) with a cancellation flag; old tree reused.
- Highlighting: query only the visible range plus a margin; cache per (document revision,
  line range, theme). On edit, keep existing styles for unaffected lines, shift them for the
  edited line as provisional, replace when the up-to-date parse arrives. **The queries run in
  the worker too**, which returns style spans per line for the requested range; the main
  thread only reads the cache and shifts spans. It never runs a Tree-sitter query, at render
  time or otherwise. Scrolling to lines with no cached spans draws them unstyled and
  requests them.
- Stress: edit continuously while `large` reparses; several revisions land before one parse
  finishes; saturated queues.

**Accept when**: source is highlighted; stale parses are never applied; p99 edit latency during
background parsing is recorded and within budget.

### Stage 10 — LSP

**Build**

- Server process management over stdio with reader/writer threads; JSON-RPC framing;
  initialise/shutdown; capability handling; restart on crash with backoff.
- Document sync (incremental) driven by committed ChangeSets; position-encoding negotiation
  (UTF-8 preferred, UTF-16 fallback using the choice recorded in Stage 1).
- Diagnostics stored as markers (Stage 8), rendered inline and in the status line.
- Go to definition; hover and completion, which require a **popup UI component** built in this
  stage.
- Every response is checked against the document revision it was requested for: applied if
  current, mapped forward through the revision log if the operation is position-only and the
  revision is still in the log, otherwise dropped. Text edits from a stale revision are never
  applied.
- Stress: a fake server that floods diagnostics and answers late; a server that hangs.

### Stage 11 — Luau scripting and plugins

**Build**

- **Measurement subtask first**: embed Luau through the installed `mlua` version against the
  real Stage 7 registries and record script→host and host→script call cost, the interrupt
  mechanism for runaway scripts, the memory limit, live redefinition, and collector pause
  behaviour under the standing benchmark. Report the numbers before building the full API.
- One VM on the main thread, sandboxed to the Flash API (no ambient filesystem, process, or
  network access unless granted).
- API as thin adapters over Stage 7: register commands, keymaps, modes, motions, text objects;
  read ranges in bulk; propose revision-tagged transactions; create markers; subscribe to events;
  per-view plugin state. No per-character host calls.
- Ownership: every registration, hook, marker, and pending request is tagged with the plugin;
  unload removes all of them; load/unload/reload is repeatable without leaks.
- Budgets: interrupt-based time limits, small for hooks on the keystroke path and larger for
  explicit commands. Documented limitation: the interrupt cannot pre-empt a host function, so
  every host function must itself be bounded.
- Demonstration: an alternative editing mode written entirely as a plugin.
- Design note only (no implementation): how an out-of-process extension would submit
  revision-tagged transactions.

**Tests**: register, override, unload, reload repeatedly, plugin error, runaway loop interrupted,
stale proposed edit rejected, state cleanup, standing benchmark with plugins enabled.

---

## 6. Quality bar (all stages)

- **Correctness**: no silent data loss; no stored offset off a char boundary; no partially
  applied transaction; no stale async result applied; no dangling registration; no reachable
  panic from external input.
- **Performance**: no full-document scan or copy on ordinary editing or cursor motion; no
  unbounded queue; no blocking call on the input path, file I/O included; no allocation in
  key resolution; no per-key cost that grows with session, group, or history length; no
  full-screen write for a change that touches a few cells.
- **Maintainability**: explicit ownership, no `Rc<RefCell>` state graph, no global mutable
  state, documented invariants, traits only where there are two real implementations.

## 7. After the 11 stages

Native GPU frontend; persistent server with reconnecting clients; WASM or out-of-process plugins;
file-backed large-file mode; crash-recovery journal; rope and marker-structure research. Replace
foundations only on reproducible evidence.

## 8. Decisions needed from the owner

| ID | Decision | Recommendation | Needed by |
|---|---|---|---|
| D1 | Editing model | **Decided 2026-11-12: selection-first modal.** See 2.5 | — |
| D2 | May the agent make local commits on a stage branch? | Yes: one commit per subtask, never pushed | Stage 1 |
| D3 | 11 stages (LSP split from Tree-sitter) | **Accepted with plan finalisation, 2026-11-12** | — |
| D4 | Scripting language | **Decided 2026-11-12: Luau via `mlua`.** See 2.10 | — |

---

## Appendix A — Changes from revision 1

1. **False premise removed.** Revision 1 said a Crop-based workspace with ChangeSet work already
   existed and told the agent to audit rather than rebuild. The repository is an empty
   `cargo new`. Section 1 now states the verified state.
2. **Crop specifics added** (2.3): panicking API requires a validating wrapper; `line_len`
   semantics differ from editor rows; no multi-edit API; feature flags; version pinned.
3. **Three crates instead of five**, with `Text` inside `flash-core`, so the "only transactions
   mutate text" rule is enforced by visibility. Revision 1 asked for public
   insert/delete/replace on `Text` and also forbade direct rope mutation.
4. **Selection model fixed early** (2.5, D1). Revision 1 never chose between gap-indexed and
   character-indexed ranges, which changes Stage 3 semantics.
5. **Revision log added** (2.7). Revision 1 required transforming stale async results but kept
   nothing to transform them with.
6. **Composition/position-mapping trap documented** (Stage 2).
7. **Main-loop architecture specified** (2.8): single event channel, drain-then-render,
   bracketed paste as one transaction. Revision 1 left this open until Stage 9.
8. **Walking skeleton in Stage 3 and trace replay in Stage 4.** Revision 1 produced nothing
   runnable and no end-to-end latency number until Stage 6, despite end-to-end latency being the
   project's stated objective.
9. **Stage 9 split.** Tree-sitter plus highlighting plus full LSP was three stages of work in
   one; LSP is now Stage 10 and plugins Stage 11. A popup component is listed because completion
   and hover need one.
10. **Missing editor features scheduled**: search/replace, clipboard, prompt and status line,
    soft wrap, tabs, config, panic-safe terminal restore, suspend/resume.
11. **Event hooks moved to Stage 7** so the plugin stage adapts an existing system instead of
    retrofitting one.
12. **File I/O policies decided** rather than listed as considerations: invalid UTF-8, BOM, line
    endings, symlinks, hard links, conflict detection.
13. **Measurement made concrete**: named tools, deterministic fixtures including a 1 MB single
    line, how percentiles and allocations are captured, where results are stored. Unmeasurable
    "visible presentation latency" dropped.
14. **Process trimmed.** Revision 1 repeated the same rules across four sections and required a
    ten-part report plus a separate review packet, decision log, and status file per stage. Now:
    one stage document, reviewer must deliver failing tests, local commits per subtask (revision
    1 demanded small reviewable patches while forbidding commits, which yields one large
    uncommitted diff per stage).
15. **Owner decisions folded in** (2026-11-12): selection-first modal editing (2.5);
    everything above the core invariants configurable, with defaults defined as configuration
    (2.11); three-tier extensibility with Luau as the scripting language, chosen on latency
    after comparing QuickJS, Steel, and WASM (2.10).

## Appendix B — Changes from revision 2

Made 2026-11-17 after Stage 7a, from a review of the code and the recorded numbers. Stages 1
to 7a are not reopened except for the carried fixes in Stage 7b.

1. **Undo-group merge is O(group size) per key** (Stage 5 note, fixed in 7b). Measured with
   one unbroken run of keys 1 ms apart: 1.7 µs per key at 10,000 keys, 41 µs at 640,000,
   worst key 28.7 ms. Reached by a paste through a terminal without bracketed paste, or by
   anything that types without pausing.
2. **The input thread is put to a measurement** (2.8, Stage 7c). Its hand-off is 86 µs p50 of
   280 µs in the 7a pty run. Not decided: replacing crossterm's input half has a real cost.
3. **Renderer choices are judged on bytes and cells per frame** (section 4, Stage 7c). Timing
   to `flush` returning cannot see what a full-screen write costs the terminal, and on CPU
   time alone a full redraw can beat a diff.
4. **The pty run and an idle-mode harness are the reference** (section 4). Hot-loop numbers
   were about ten times lower than the pty run for the same work.
5. **File I/O leaves the main thread** (2.8, Stage 7b): 282 ms to save and 220 ms to load
   100 MB were being spent inside the event loop.
6. **Event time is read time** (2.8). It was the dequeue time.
7. **Kitty keyboard protocol where supported** (2.9), against split escape sequences.
8. **Hooks filter by subscribed kinds** (Stage 7b). "Nothing is queued without subscribers"
   was defeated by the built-in subscriber.
9. **A display-line layout cache precedes soft wrap** (Stage 7b) and replaces the
   primary-head-only `LineAnchor` checkpoints.
10. **The cell grid composites views and overlays** (2.9, Stage 7c). The plan had no place
    where splits or popups met the renderer before Stage 10.
11. **Highlight queries run in the worker** (Stage 9), and Tree-sitter reads the rope through
    `chunk_at_byte` (2.3).
12. **New targets and quality-bar lines** (sections 4 and 6): bytes written per kind of
    change, and no per-key cost that grows with session length.

Not changed, and still open for the owner: the stage gate's independent review (section 3,
step 3) was waived for Stages 3 to 5 and has not run for 6 and 7a. Item 1 was found by a
fresh reading in minutes; running that review before 7b is recommended.
