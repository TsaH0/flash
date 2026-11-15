# Stage 4 — Documents, Views, Revisions, Headless Editor, Trace Replay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless `Editor` owning documents and views behind generational IDs, where `Document::commit` is the only text mutator, a bounded revision log maps stale positions forward, `Editor::handle(Event) -> Effects` drives everything from raw input, and recorded JSON-Lines traces replay deterministically for tests and latency benchmarks.

**Architecture:** Three new `flash-core` modules: `document/` (`Revision`, `RevisionLog`, `Document`, `LineEnding`), `editor/` (`DocumentId`/`ViewId`, `View`, `Editor`, core `Event`/`KeyEvent`, the temporary key map, `Effects`), and `trace.rs` (serde JSON Lines record/read/replay). `apps/flash` keeps only rendering and crossterm→core event conversion, plus trace recording and `--replay`.

**Tech Stack:** Rust 2024, crop 0.4.3 (vendored), `slotmap` 1, `serde` 1 (`derive`), `serde_json` 1, proptest, Criterion, crossterm 0.29 (app only).

**Spec:** `docs/superpowers/specs/2026-11-15-stage-4-design.md` (approved 2026-11-15), implementing `docs/PLAN.md` Stage 4. DECISIONS 009, 010, 011, 013 are binding.

## Global Constraints

- Edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` in `flash-core` and `flash_app`.
- Build and test `--offline`. New dependencies, exactly: root `[workspace.dependencies]` gets `slotmap = "1"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`; `flash-core` uses all three; `apps/flash` uses `serde_json`. Nothing else.
- No panic reachable from any public method for any input, including a malformed trace. Unreachable mutator errors: `debug_assert!` then return the error.
- Atomicity: validate everything, then mutate (DECISIONS 010). Never map positions through a composed ChangeSet (009).
- `flash-core` depends on no terminal crate. Hard-coded keys (`editor/keys.rs`) are temporary (PLAN 2.11; Stage 7 deletes them).
- One local commit per task on branch `stage-4`. Never push. Never add `.claude/`, `.mcp.json`, `.gitmodules`. Messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Gate: `cargo fmt --all -- --check`; `cargo clippy --offline --workspace --all-targets -- -D warnings` (plain and `--all-features`); `cargo test --offline --workspace`; `cargo test --offline -p flash-core --features small-chunks`; `PROPTEST_CASES=4096 cargo test --offline -p flash-core --release` (plain and `small-chunks`).

## Review Focus

1. **Stale IDs after slot reuse.** Close a view, open a new one (reusing the slot): the old `ViewId` must be rejected by every method, never alias the new view. Pinned in Task 3 (`stale_ids_rejected_after_reuse`).
2. **Closing the focused view / a document with several views.** Focus moves to a remaining view or becomes `None`; `handle` with no focus is a no-op that returns empty `Effects`. Pinned in Task 3 (`close_focused_view_moves_focus`, `handle_without_focus_is_noop`).
3. **Malformed or hostile trace input.** Truncated JSON, unknown event kinds, a wrong version, an oversized line, a huge resize, `t_ns` going backwards, invalid cursor offset: errors, never panics. Pinned in Task 4 (`read_rejects_malformed`, `replay_rejects_bad_header`).
4. **Revision log at its bounds.** Mapping from exactly the oldest reachable revision works; one older is `Evicted`; a single change set larger than the op bound is still kept as the newest entry. Pinned in Task 1 (`log_bounds_and_eviction`).
5. **Edits from a non-focused view.** `Editor::commit` from a background view must map the focused view's selection and keep its scroll valid. Pinned in Task 3 (`commit_from_background_view_maps_focused`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/flash-core/src/document/mod.rs` | `Document`, `CommitError`, `LineEnding`, `commit`, `map_from`, view-selection table |
| `crates/flash-core/src/document/revision.rs` | `Revision`, `RevisionLog`, `MapError` |
| `crates/flash-core/src/document/{tests,proptests}.rs` | Commit, multi-view, log cases and properties |
| `crates/flash-core/src/editor/mod.rs` | `Editor`, `View`, `EditorError`, `Effects`, `handle`, scrolling |
| `crates/flash-core/src/editor/ids.rs` | `DocumentId`, `ViewId` (`slotmap::new_key_type!`) |
| `crates/flash-core/src/editor/event.rs` | `Event`, `KeyEvent`, `KeyCode`, `Modifiers` (serde) |
| `crates/flash-core/src/editor/keys.rs` | Temporary hard-coded key map (moved from the skeleton) |
| `crates/flash-core/src/editor/{tests,proptests}.rs` | Lifecycle, keys, never-panic, two-view property |
| `crates/flash-core/src/trace.rs` (+ `trace_tests.rs`) | `TraceHeader`, `TraceEntry`, `TraceWriter`, `TraceReader`, `TraceError`, `replay` |
| `crates/flash-core/src/transaction/mod.rs` | Split `apply` into `pub(crate) prepare` + apply; `apply` becomes `pub(crate)` |
| `crates/flash-core/src/internals.rs` | Add `transaction_apply` for benches |
| `crates/flash-core/benches/document.rs` | Standing benchmark |
| `crates/flash-core/benches/{latency,transaction}.rs` | Switch to `internals::transaction_apply`; new rows |
| `apps/flash/src/skeleton.rs` | `render(&mut Editor, …)`, `convert(crossterm Event)` |
| `apps/flash/src/main.rs` | Editor wiring, `FLASH_TRACE`, `--replay` |
| `apps/flash/benches/skeleton.rs` | Pattern as a trace, replayed through `Editor` + render |

---

### Task 0: Branch and record Stage 3's interactive number

- [ ] `git status` shows only untracked `.claude/`, `.gitmodules`, `.mcp.json`, `bench/results/stage-3-interactive.jsonl`; `HEAD` is `stage-3` at `9135723`.
- [ ] `git switch -c stage-4`.
- [ ] In `docs/stages/stage-3.md` section F replace "The latency log … no interactive number yet." with the measured line: owner session, 199 events, read → flush p50 339 µs, p95 548 µs, p99 758 µs, max 819 µs (`bench/results/stage-3-interactive.jsonl`). Remove the matching bullet from STATUS "Implemented but not fully verified"; mark Stage 3 approved 2026-11-15 without independent review.
- [ ] Commit `Record Stage 3 interactive latency; Stage 4 design and plan` (results file, stage-3.md, STATUS.md, the spec, this plan).

---

### Task 1: Revisions and the revision log

**Files:** Create `src/document/{mod.rs (module skeleton only), revision.rs, tests.rs}`; modify `src/lib.rs`.

**Produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(u64);
impl Revision { pub const ZERO: Revision; pub fn get(self) -> u64; pub(crate) fn next(self) -> Revision; }

pub struct RevisionLog { entries: VecDeque<(Revision, ChangeSet)>, ops: usize, max_entries: usize, max_ops: usize }
impl RevisionLog {
    pub const DEFAULT_MAX_ENTRIES: usize = 1024;
    pub const DEFAULT_MAX_OPS: usize = 1 << 20;
    pub fn new() -> RevisionLog;                                     // defaults
    pub fn with_limits(max_entries: NonZeroUsize, max_ops: usize) -> RevisionLog;
    pub(crate) fn push(&mut self, revision: Revision, changes: ChangeSet); // revision == last + 1 (debug_assert)
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    /// Oldest revision positions can be mapped from: first entry's revision - 1, or `current` when empty.
    pub fn oldest(&self, current: Revision) -> Revision;
    pub fn map_from(&self, from: Revision, current: Revision, positions: &mut [usize], assoc: Assoc) -> Result<(), MapError>;
}
#[non_exhaustive] pub enum MapError { Evicted { from: Revision, oldest: Revision }, Future { from: Revision, current: Revision }, ChangeSet(ChangeSetError), Text(TextError) }
```

Push evicts from the front while `entries.len() > max_entries || ops > max_ops`, but never the entry just pushed. `map_from` maps a scratch copy (`SmallVec<[usize; 8]>`) through each entry with revision `> from`, in order, then writes back, so the slice is unchanged on error. `Text` is used by Task 2's validation.

- [ ] **Step 1: Failing tests** (`document/tests.rs`): `map_from_current_is_identity`; `map_from_matches_sequential_map_pos` (three hand change sets on `"hello world"`, positions `[0, 3, 5, 11]`, `After` and `Before`, compared with chained `map_pos`); `map_from_future_rejected` (`Future`); `log_bounds_and_eviction`: `with_limits(3, usize::MAX)`, push revisions 1..=5 → `len() == 3`, `oldest(5) == Revision(2)`, mapping from 2 works, from 1 is `Evicted { from: 1, oldest: 2 }`; `with_limits(100, 4)`: pushing a 3-op then a 9-op change set keeps only the 9-op one; `map_from_error_leaves_positions` (position past old length → `ChangeSet(_)`, slice unchanged).
- [ ] **Step 2:** `cargo test --offline -p flash-core document` → FAIL (module missing).
- [ ] **Step 3:** Implement; `pub mod document;` in `lib.rs`, re-export `Revision, RevisionLog, MapError`.
- [ ] **Step 4:** Tests pass; clippy clean.
- [ ] **Step 5:** Commit `Add revisions and the bounded revision log`.

---

### Task 2: Document and commit

**Files:** Create `src/editor/ids.rs` (with `src/editor/mod.rs` declaring only `mod ids; pub use ids::*`), `src/document/proptests.rs`; modify `src/document/mod.rs`, `src/transaction/mod.rs`, manifests (`slotmap`), `docs/DECISIONS.md` (014, 015).

**Consumes:** Task 1. `Transaction`, `Selection::{validate, map}`.

**Produces:**

```rust
slotmap::new_key_type! { pub struct DocumentId; pub struct ViewId; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding { #[default] Lf, Crlf }
impl LineEnding { pub fn detect(text: &Text) -> LineEnding; pub fn as_str(self) -> &'static str; }

pub struct Document { text, revision, log: RevisionLog, path: Option<PathBuf>, line_ending, selections: SecondaryMap<ViewId, Selection> }
impl Document {
    pub fn new(text: Text, path: Option<PathBuf>) -> Document;        // detects line ending
    pub fn with_log(self, log: RevisionLog) -> Document;
    pub fn text(&self) -> &Text; pub fn revision(&self) -> Revision; pub fn log(&self) -> &RevisionLog;
    pub fn path(&self) -> Option<&Path>; pub fn line_ending(&self) -> LineEnding;
    pub fn selection(&self, view: ViewId) -> Option<&Selection>;
    pub fn views(&self) -> impl Iterator<Item = ViewId> + '_;
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), CommitError>; // validated
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, CommitError>;
    pub fn map_from(&self, from: Revision, positions: &mut [usize], assoc: Assoc) -> Result<(), MapError>; // + validate against text → MapError::Text
    pub(crate) fn add_view(&mut self, view: ViewId);                  // Selection::point(0)
    pub(crate) fn remove_view(&mut self, view: ViewId) -> bool;
}
#[non_exhaustive] pub enum CommitError { NoSuchView(ViewId), Transaction(TransactionError) }
// transaction/mod.rs
impl Transaction {
    /// Validates against `text` and returns the selection `selection` becomes. No mutation.
    pub(crate) fn prepare(&self, text: &Text, selection: &Selection) -> Result<Selection, TransactionError>;
    pub(crate) fn into_parts(self) -> (ChangeSet, Option<Selection>);
}
```

`LineEnding::detect`: one pass over `text.chunks()`, counting `\n` and `\n` preceded by `\r` (carry the last byte across chunks); `Crlf` iff CRLF count > LF-only count.

`commit`: `prepare` for the origin; for every other view `sel.map(changes)` collected into a `SmallVec<[(ViewId, Selection); 4]>`; then `changes.apply(text)` (debug_assert on error), store selections, and if `!changes.is_identity()` bump revision and `log.push(revision, changes)`. Returns the (possibly unchanged) revision. Keep `Transaction::apply` `pub` in this task (the skeleton still uses it; Task 4 narrows it), implemented as `prepare` + apply.

- [ ] **Step 1: Failing tests** (`document/tests.rs`): `commit_bumps_revision_and_logs`; `identity_commit_keeps_revision` (selection-only transaction sets the selection, revision unchanged, log unchanged); `commit_maps_other_views` (views A at 0, B at 3 on `"abc"`; A inserts `"xy"` at 0 → B at 5); `failed_commit_changes_nothing` (bad change set length and bad transaction selection: text, revision, log, every selection unchanged); `commit_unknown_view_rejected`; `map_from_validates_against_text` (a hand-made position that maps inside `"é"` → `MapError::Text`); `line_ending_detection` (`""`/`"a\nb"` → Lf, `"a\r\nb\r\nc\n"` → Crlf, a CRLF split across chunks with `small-chunks`).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement as specified.
- [ ] **Step 4: Property** (`document/proptests.rs`) `views_stay_valid_under_commits`: text from `text_strategy(8)`, 1–3 views with `selection_for`, 1–12 steps of `(view index, Intent)` reusing `transaction::proptests` `intent()`/`build` (make them `pub(crate)`). After each step: text equals `apply_ops` on a `String` model; origin selection equals `txn.prepare(...)` result; every other view's selection equals its previous `map(changes)`; every selection validates and `assert_normalised`; revision increased by one iff the change set was not the identity. At the end, `map_from(Revision::ZERO, ...)` on 3 random boundaries of the initial text equals chaining `map_pos` through the recorded change sets.
- [ ] **Step 5:** Tests pass (plain and `small-chunks`); clippy.
- [ ] **Step 6:** DECISIONS 014 "Documents own per-view selections; commit is the only text mutator" (alternatives: selections on `View` with commit on `Editor`; a `views: &mut` parameter), 015 "Revision log bounds and stale-position mapping" (entry and op bounds, sequential mapping, re-validation; reopen if Stage 9/10 measurements show workers falling out of the log).
- [ ] **Step 7:** Commit `Add Document with commit, per-view selections, and the revision log`.

---

### Task 3: Headless editor

**Files:** Create `src/editor/{event.rs, keys.rs, tests.rs, proptests.rs}`; modify `src/editor/mod.rs`, `src/lib.rs`, manifests (`serde`).

**Consumes:** Task 2.

**Produces:**

```rust
// event.rs — all derive Debug, Clone, PartialEq, Eq, Serialize, Deserialize
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event { Key(KeyEvent), Paste { text: String }, Resize { width: u16, height: u16 } }
pub struct KeyEvent { pub code: KeyCode, #[serde(default)] pub modifiers: Modifiers }
#[serde(rename_all = "snake_case")]
pub enum KeyCode { Char(char), Enter, Tab, Backspace, Delete, Left, Right, Up, Down, Home, End, PageUp, PageDown, Esc, F(u8) }
#[derive(Copy, Default)] pub struct Modifiers { #[serde(default)] pub shift: bool, pub ctrl: bool, pub alt: bool }
impl KeyEvent { pub fn new(code: KeyCode, modifiers: Modifiers) -> KeyEvent; pub fn plain(code: KeyCode) -> KeyEvent; }

// mod.rs
pub struct View { doc: DocumentId, top: usize, width: u16, height: u16 }
impl View { pub fn doc(&self) -> DocumentId; pub fn top(&self) -> usize; pub fn size(&self) -> (u16, u16); }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects { pub text: bool, pub selection: bool, pub scroll: bool, pub resize: bool, pub message: bool, pub quit: bool }
impl Effects { pub fn needs_redraw(&self) -> bool; }   // any field except quit
#[non_exhaustive] pub enum EditorError { NoSuchDocument(DocumentId), NoSuchView(ViewId), Commit(CommitError), Selection(TextError) }
pub struct Editor { documents: SlotMap<DocumentId, Document>, views: SlotMap<ViewId, View>, focus: Option<ViewId>, screen: (u16, u16), message: String, quit: bool }
impl Editor {
    pub fn new(width: u16, height: u16) -> Editor;
    pub fn open(&mut self, text: Text, path: Option<PathBuf>) -> DocumentId;
    pub fn open_view(&mut self, doc: DocumentId) -> Result<ViewId, EditorError>;  // focuses it; size = (w, h - 1)
    pub fn close_view(&mut self, view: ViewId) -> Result<(), EditorError>;
    pub fn close_document(&mut self, doc: DocumentId) -> Result<(), EditorError>;
    pub fn set_focus(&mut self, view: ViewId) -> Result<(), EditorError>;
    pub fn focus(&self) -> Option<ViewId>;
    pub fn document(&self, doc: DocumentId) -> Option<&Document>;
    pub fn view(&self, view: ViewId) -> Option<&View>;
    pub fn view_document(&self, view: ViewId) -> Option<(&View, &Document)>;
    pub fn selection(&self, view: ViewId) -> Option<&Selection>;
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), EditorError>;
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, EditorError>;
    pub fn message(&self) -> &str; pub fn should_quit(&self) -> bool; pub fn screen(&self) -> (u16, u16);
    pub fn handle(&mut self, event: &Event) -> Effects;
}
// keys.rs (temporary)
pub(crate) fn handle_key(editor: &mut Editor, view: ViewId, key: KeyEvent) -> Effects;
```

Key map: exactly the Stage 3 skeleton's (`apps/flash/src/skeleton.rs` `key`), Enter inserts `document.line_ending().as_str()`, tab width 4. `close_view` with the focused view focuses the first remaining view in `views` iteration order, else `None`. `close_document` removes the document and every view whose `doc` is it. After every `handle` with a focused view, scroll it: `top` moves the minimal amount so the primary head's line is within `top..top + height` (no change when `height == 0`); set `Effects.scroll` if `top` changed. Errors become `message` (cleared by the next successful command; `Effects.message` when it changes).

- [ ] **Step 1: Failing tests** (`editor/tests.rs`), porting every `skeleton_tests.rs` key test that does not render (typing/moving/deleting, shift extends, alt+down adds a cursor, paste is one insert, ctrl+q quits, esc collapses) to `Editor::handle`, plus: `enter_uses_document_line_ending` (`"a\r\nb"` → Enter inserts `"\r\n"`); `stale_ids_rejected_after_reuse`; `close_focused_view_moves_focus`; `close_document_closes_its_views` (its selections gone, other document untouched); `handle_without_focus_is_noop` (`Effects::default()`); `commit_from_background_view_maps_focused` (two views of one doc, commit from the non-focused one, focused selection mapped, `handle` afterwards still valid); `scroll_follows_primary_head` (40-line text, height 11 → after 15 Downs `top == 6`; after Ctrl+Home `top == 0`); `resize_sets_view_size` (`Resize{100, 30}` → focused view size `(100, 29)`, `Resize{0,0}` → `(0, 0)` no panic); `effects_report_changes` (typing → text+selection; Right → selection only; failed command → message).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4: Properties** (`editor/proptests.rs`): `random_events_never_panic` — 20,000 events (keys from the full `KeyCode` set with random modifiers, pastes from `text_strategy(2)` with BEL/ESC/CSI bytes, resizes in `0..=300 × 0..=100`) over two views of one Unicode text, randomly switching focus via `set_focus` every 50 events; after each event every view's selection validates and the focused view's primary head line is in `top..top+height` when `height > 0`.
- [ ] **Step 5:** Tests pass; clippy.
- [ ] **Step 6:** Commit `Add the headless editor with views, events, and effects`.

---

### Task 4: Trace record and replay; skeleton rewired onto the editor

**Files:** Create `src/trace.rs`, `src/trace_tests.rs`; modify `src/lib.rs`, `src/transaction/mod.rs` (`apply` → `pub(crate)`), `src/internals.rs`, `benches/{transaction,latency}.rs` (use `internals::transaction_apply`; `transaction` bench gets `required-features = ["internals"]`), `apps/flash/{Cargo.toml, src/skeleton.rs, src/skeleton_tests.rs, src/main.rs, src/lib.rs}`, `docs/DECISIONS.md` (016).

**Consumes:** Task 3.

**Produces:**

```rust
// trace.rs
pub const TRACE_VERSION: u32 = 1;
pub const DEFAULT_MAX_LINE: usize = 16 << 20;
#[serde(rename_all = "snake_case")] pub enum TraceSource { Path(String), Fixture(String) }
pub struct TraceHeader { pub flash_trace: u32, pub document: TraceSource, pub len: u64, pub hash: u64, pub size: (u16, u16), pub cursor: u64 }
pub struct TraceEntry { pub t_ns: u64, pub event: Event }
pub fn content_hash(text: &Text) -> u64;                              // FNV-1a 64 over chunks
pub struct TraceWriter<W: Write> { .. }
impl<W: Write> TraceWriter<W> { pub fn new(out: W, header: &TraceHeader) -> Result<Self, TraceError>; pub fn write(&mut self, entry: &TraceEntry) -> Result<(), TraceError>; pub fn into_inner(self) -> Result<W, TraceError>; }
pub struct TraceReader<R: BufRead> { .. }
impl<R: BufRead> TraceReader<R> {
    pub fn new(input: R, max_line: usize) -> Result<Self, TraceError>;   // reads + checks header (version)
    pub fn header(&self) -> &TraceHeader;
}
impl<R: BufRead> Iterator for TraceReader<R> { type Item = Result<TraceEntry, TraceError>; }
/// Opens `text` as the trace's document with one view at `header.cursor` and screen `header.size`, after checking len and hash.
pub fn start(header: &TraceHeader, text: Text) -> Result<(Editor, ViewId), TraceError>;
#[non_exhaustive] pub enum TraceError { Io(io::ErrorKind, String), Json { line: usize, message: String }, LineTooLong { line: usize, max: usize }, Version(u32), Mismatch { what: &'static str }, Cursor(TextError) }
// internals.rs
pub fn transaction_apply(txn: &Transaction, text: &mut Text, selection: &mut Selection) -> Result<(), TransactionError>;
// apps/flash skeleton.rs
pub fn convert(event: &crossterm::event::Event) -> Option<flash_core::Event>;   // None for releases, mouse, focus
pub struct Skeleton { name: String, last_latency_ns: u64 }
impl Skeleton { pub fn new(name: String) -> Skeleton; pub fn set_last_latency(&mut self, nanos: u64); pub fn render(&self, editor: &Editor, out: &mut impl Write) -> io::Result<()>; }
```

Reading: `read_until(b'\n')` into a reused buffer, failing with `LineTooLong` once the buffer exceeds `max_line` (use `Read::take(max_line + 1)` per line so a hostile file cannot allocate past the limit); blank lines skipped; JSON errors carry the 1-based line number. `t_ns` is not required to be monotonic (replay ignores it today). `render` reads only the focused view (scrolling now happens in the editor); it draws nothing when there is no focus. `main.rs`: builds an `Editor`, opens the file, converts each crossterm event, times `read` → `flush` as before; `FLASH_TRACE=<file>` writes a header (`Path`, len, hash, terminal size, cursor 0) and one entry per converted event; `flash --replay <trace>` loads the header's `Path` document, `start`s, replays every entry through `handle` + `render` into a `Vec<u8>` sink, and prints `Summary::to_json(r#""source":"replay""#)`; a `Fixture` source is an error in the binary ("fixture traces replay in benches only").

- [ ] **Step 1: Failing tests** (`trace_tests.rs`): `round_trip` (property: header + random entries written then read back equal); `replay_reproduces_session` (property: random 200-event session on `text_strategy(8)` recorded to a `Vec<u8>` while being applied, then read and replayed into a fresh `start` → identical text, selection, revision, top); `read_rejects_malformed` (truncated JSON, unknown `kind`, version 2, line longer than `max_line = 64`, missing header → the matching `TraceError`, no panic); `replay_rejects_bad_header` (wrong len, wrong hash → `Mismatch`; cursor inside `"é"` → `Cursor`); `event_json_shape` (`{"kind":"key","code":{"char":"x"},"modifiers":{"shift":false,"ctrl":true,"alt":false}}` round-trips; `modifiers` may be omitted).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `trace.rs`; narrow `Transaction::apply` to `pub(crate)`; add `internals::transaction_apply`; switch benches.
- [ ] **Step 4:** Rewire `apps/flash`: `skeleton_tests.rs` keeps only render tests (control characters, tiny terminals, wide chars, scrolling, file name) now driven through `Editor` + `Skeleton::render`, plus `convert_maps_keys` (shift/ctrl/alt, release → `None`, paste, resize) and the 20,000-event `random_events_never_panic` render variant.
- [ ] **Step 5:** `cargo test --offline --workspace`, clippy (plain and `--all-features`) pass; `cargo build --offline -p flash`; `flash --replay` on a trace written by a test helper prints one JSON line.
- [ ] **Step 6:** DECISIONS 016 "Trace format: raw input events as JSON Lines" (owner decision A; serde_json; bounded lines; header verifies content; alternatives: command-level events, a hand-written line format, bincode).
- [ ] **Step 7:** Commit `Add trace record and replay; rewire the skeleton onto the editor`.

---

### Task 5: Benchmarks

**Files:** Create `benches/document.rs`; modify `crates/flash-core/Cargo.toml` (`[[bench]] document`), `benches/latency.rs`, `apps/flash/benches/skeleton.rs`, `bench/results/stage-4-*`.

- [ ] **Step 1: Standing benchmark** `benches/document.rs` (Criterion, `medium` and `large`): group `commit/<fixture>`, parameters `views ∈ {1, 2, 8}` × origin `{one, 10k}`: one document, origin view with one cursor (or 10,000 spread cursors via `common::cursors`), every other view with 10,000 spread cursors; each iteration commits `Transaction::insert(.., "x")` from the origin on a fresh `Document` clone (setup clones the document; `Document: Clone`, O(views · k)). Also `LineEnding::detect` per fixture.
- [ ] **Step 2: Latency rows** in `benches/latency.rs`: `doc commit insert char (1 view)` (compare with Stage 3 `txn insert char`), `doc commit insert char (2 views, 1 cursor each)`, `editor handle key 'x'`, `editor handle Right`.
- [ ] **Step 3:** `apps/flash/benches/skeleton.rs`: build the Stage 3 20-event pattern as `Vec<TraceEntry>`, round-trip it through `TraceWriter`/`TraceReader` once (so the bench exercises the trace path), and time `handle` + `render` per event as before.
- [ ] **Step 4:** Run and store: `cargo bench --offline -p flash-core --bench document -- --save-baseline stage-4` then `bench/collect.py stage-4 bench/results/stage-4-criterion.json`; `FLASH_BENCH_JSON=bench/results/stage-4-latency.jsonl cargo bench --offline -p flash-core --features internals --bench latency`; `FLASH_BENCH_JSON=bench/results/stage-4-skeleton.jsonl cargo bench --offline -p flash --bench skeleton`.
- [ ] **Step 5:** Commit `Benchmark documents, the editor, and trace replay`.

---

### Task 6: Gate, review, report

- [ ] Run every gate command in Global Constraints; record outputs.
- [ ] Implementer adversarial pass over `git diff stage-3..` with the Review Focus list and PLAN 6 quality bar; add any attack tests that find nothing as kept regression tests, fix anything they find with a test. Independent fresh-context review only if the owner asks (their standing rule: no delegation without an explicit request); say so in the report.
- [ ] Write `docs/stages/stage-4.md` (sections A–J as in Stage 3), update `docs/STATUS.md`.
- [ ] Commit `Stage 4 report, review tests, status, and results`.
