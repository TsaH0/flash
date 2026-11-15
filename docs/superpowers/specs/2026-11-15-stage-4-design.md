# Stage 4 design — documents, views, revisions, headless editor, trace replay

Date: 2026-11-15. Spec: `docs/PLAN.md` Stage 4, sections 2.6–2.8, 2.11, 3, 4, 6.

## Intent

Turn the Stage 3 skeleton's loose `Text` + `Selection` into a headless core
that owns all editing state and is driven by one entry point, so that:

- "only transactions mutate text" holds for the whole editor, not just
  `Text` (`Transaction::apply` becomes `pub(crate)`);
- several views of one document stay valid under edits from any of them;
- results computed against an older revision can be mapped forward or
  rejected (the revision log, PLAN 2.7);
- a recorded event stream replays deterministically through the editor, as
  both the integration-test harness and the latency benchmark driver.

**Goals:** the PLAN Stage 4 build list and tests; the standing benchmark.
**Non-goals:** undo/history (Stage 5), file saving and the load policy
(Stage 6), modes and keymaps (Stage 7), any UI for splits.
**Anti-goals:** a hot path that gets slower than Stage 3 by more than noise
for a single-view edit; a trace format that can panic or allocate unboundedly
on malformed input; core depending on a terminal crate.

## Approach (owner decision, 2026-11-15)

**Trace events are raw input (option A):** key + modifiers, paste, resize,
in a core-defined, terminal-independent type. The temporary hard-coded
keymap moves from the skeleton into the core editor (Stage 7 replaces it
with keymaps), so traces exercise the whole input path, including key
resolution once it exists. Rejected: command-level events (stable across
rebinding but skip key resolution, so the standing benchmark would
under-measure). Commands remain an internal layer Stage 7 exposes.

## Design

### Revisions and the revision log (`document/revision.rs`)

- `Revision(u64)`, starting at 0. A commit whose change set is not the
  identity increments it by one; a selection-only commit does not.
- `RevisionLog`: a `VecDeque<(Revision, ChangeSet)>`; entry `(r, c)` maps
  revision `r - 1` to `r`. Bounded by entry count (default 1,024) **and**
  total op count (default 2^20), evicting oldest first and always keeping
  the newest entry. The op bound exists because one 10k-cursor edit holds
  ~20k ops; 1,024 of them would be hundreds of MB.
- `map_from(rev, &mut [usize] sorted, Assoc) -> Result<(), MapError>`: maps
  through each entry after `rev` in order — never a composed change set
  (DECISIONS 009). `rev == current` is the identity. Errors: `Evicted`
  (older than the log reaches), `Future` (newer than current), or a
  change-set error (position past the old length). Slice unchanged on error.
  Cost O(Σ ops + k · entries).
- `Document::map_from` additionally validates every result against the
  current text (a position that was not a char boundary at `rev` cannot be
  detected otherwise), so callers never receive an invalid offset.

### Document (`document/mod.rs`)

```text
Document { text, revision, log, path: Option<PathBuf>, line_ending,
           selections: SecondaryMap<ViewId, Selection> }
```

**Selections live in the document, keyed by view** (Helix's layout), not in
`View` as PLAN lists them. Reason: the document can then own the invariant
"every view's selection is valid for my text" privately, and `commit` needs
no access to the editor's view table. `View` keeps everything else.

`Document::commit(view, Transaction) -> Result<Revision, CommitError>`, the
only text mutator:

1. Validate the change set against the text; compute the origin view's new
   selection (the transaction's, checked against the post-change text
   without building it, or the current one mapped — the existing
   `Transaction` logic, refactored into a `pub(crate) prepare`).
2. Map every other view's selection through the change set (valid by the
   invariant, so mapping can only fail on a bug: `debug_assert` + error).
3. Only now apply the text, store the selections, bump the revision, and
   move the change set into the log (no clone).

All-or-nothing: any error before step 3 changes nothing. Unknown view →
error.

`LineEnding { Lf, Crlf }`, detected at open by counting `\n` and `\r\n`
(ties → Lf); Enter inserts the document's line ending.

### Views and the editor (`editor/`)

- IDs: `slotmap` generational keys `DocumentId`, `ViewId`. A key from a
  closed document or view is rejected even after its slot is reused.
- `View { doc, top: usize, size: (u16, u16) }`. Mode state is deferred to
  Stage 7, which defines modes (ruling).
- `Editor { documents, views, focus: Option<ViewId>, screen, message }`:
  `open(text, path) -> DocumentId`; `open_view(doc) -> ViewId` (cursor at 0,
  becomes focused); `close_view` (removes its selection; focus moves to
  another view or none; the document stays open); `close_document` (closes
  its views); `set_focus`; `set_selection`; `commit(view, txn)`; read
  accessors. All take IDs and return `Result<_, EditorError>`.
- `Editor::handle(&Event) -> Effects` acts on the focused view (none → no-op).
  It never panics; a failed command leaves state unchanged and sets the
  message. After every event the focused view scrolls so its primary head's
  line is visible.
- `Event { Key(KeyEvent), Paste(String), Resize { width, height } }`;
  `KeyEvent { code: KeyCode, modifiers: Modifiers }`; serde-serialisable.
  Key releases are filtered by the frontend. Resize sets the screen and
  gives the focused view `(width, height - 1)` (one status row: the
  skeleton's layout, temporary).
- `Effects { text, selection, scroll, resize, message, quit }` (bools) with
  `needs_redraw()`.
- Key map: the skeleton's keys, moved verbatim to `editor/keys.rs`, marked
  temporary (PLAN 2.11; deleted in Stage 7).

### Trace record and replay (`trace.rs`)

JSON Lines, read and written with serde_json. Line 1 is a header:
`{"flash_trace":1,"document":{"path":…}|{"fixture":…},"len":…,"hash":…,
"size":[w,h],"cursor":…}`; each later line is
`{"t_ns":…,"event":{…}}` (time since recording started, kept for
coalescing replay in Stage 6). The core reads and writes traces and replays
them into an `Editor`; resolving the document source and verifying
`len`/FNV-1a `hash` is the caller's job. Reading is line-bounded
(configurable maximum line length, default 16 MiB, for large pastes) and
returns errors, never panics.

### Frontend (`apps/flash`)

- `skeleton.rs` keeps only rendering (`render(&Editor, …)`, reading the
  focused view) and `crossterm::Event → flash_core::Event` conversion.
- `main.rs`: `FLASH_TRACE=<file>` records the session; `flash --replay
  <trace>` replays a trace headless with rendering into a sink and prints
  the latency summary as JSON.

### Tests (PLAN Stage 4)

- Two views on one document stay valid under random edits/motions from
  either (property): text equals a String model; origin selection is the
  transaction's result; each other view's selection is its old one mapped.
- Close view / close document clean up (selections, focus); stale IDs are
  rejected, including after slot reuse.
- Revisions strictly monotonic; identity commits don't advance them.
- `map_from` equals sequential `map_pos` through the recorded change sets;
  eviction and future revisions are rejected; bounds hold.
- `handle` never panics over 20,000 random events; the skeleton's key tests
  move to the core.
- Trace: write/read round-trip (property); replaying a recorded random
  session reproduces text, selections, and revision exactly; malformed
  traces return errors.

### Benchmarks

- **Standing benchmark** (`benches/document.rs`, Criterion): 1 / 2 / 8
  views, each other view holding 10,000 spread selections, one edit from
  a one-cursor origin view and from a 10k-cursor origin view; full commit.
- Latency rows: single-view commit (compare with Stage 3 `txn insert`);
  headless trace replay per event.
- `apps/flash` skeleton bench: the Stage 3 20-event pattern, now a trace
  replayed through `Editor` plus render (compare with Stage 3).
- Stage 3 `transaction` Criterion bench keeps measuring `Transaction` via
  `internals`, so its numbers stay comparable.

## New dependencies

`slotmap 1` (generational IDs; named by PLAN), `serde 1` with `derive` and
`serde_json 1` (trace format; both already in the lock file through
criterion). All resolve offline.

## Left to the builder

Exact error enum variants, file layout within `document/` and `editor/`,
default bounds' constants, how the property tests generate event streams.
