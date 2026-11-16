# Flash status

Updated: 2026-11-17. Stages 1–6 and 7a approved by the owner (6 and 7a
on 2026-11-17). Stage 7 is gated three times (owner decision
2026-11-17): 7a done, 7b done, 7c next.

## Completed and tested

- **Workspace** (`Cargo.toml`, `crates/flash-core`, `apps/flash`): builds
  offline; fmt and clippy clean with `-D warnings`.
- **`Text`** (`crates/flash-core/src/text/mod.rs`): validated byte-offset API
  over crop 0.4.3 (vendored with `Rope::chunk_at_byte`, DECISIONS 008);
  cursor-row line model; O(log n) allocation-free grapheme-boundary check and
  counted grapheme motion (`move_graphemes`); O(1) snapshots; `pub(crate)`
  mutators.
- **Reference model and property tests** (`text/reference.rs`,
  `text/proptests.rs`): random Unicode edit sequences including invalid
  offsets, run with default and tiny (`small-chunks`) rope leaves.
- **`ChangeSet`** (`crates/flash-core/src/changeset/`): canonical builder,
  `from_edits`, validate, atomic `pub(crate)` apply, invert, compose,
  `map_pos`/`map_sorted` with a documented policy (DECISIONS 009, 010);
  property tests against a String model; fuzz target
  (`crates/flash-core/fuzz`, nightly); benchmarks (`benches/changeset.rs`).
- **Selections** (`crates/flash-core/src/selection/`): `Range`, `Selection`
  normalised by the PLAN 2.5 merge rule, primary survives, per-endpoint
  mapping through a ChangeSet (DECISIONS 011).
- **Motions** (`motion/`, `width.rs`): grapheme, word, line start/end,
  vertical with goal column, document start/end; move and extend;
  `Text::graphemes` replaces crop's buggy iterator (DECISIONS 012).
- **Transactions** (`transaction/`): insert, delete, replace, backspace,
  delete forward over every range with deterministic collision rules;
  validate-first atomic apply (DECISIONS 013); property-tested against a
  naive back-to-front model.
- **Walking skeleton** (`apps/flash`): `flash <file>`, raw-mode full-redraw
  frontend over the headless editor, multi-cursor, no save; in-process
  end-to-end latency bench driven by a trace.
- **Documents** (`crates/flash-core/src/document/`): `Document` with
  revision, bounded revision log (`map_from` for stale positions), path,
  line ending, and every view's selection; `Document::commit` is the only
  text mutator (DECISIONS 014, 015). `Transaction::apply` is crate-private.
- **Headless editor** (`editor/`): `Editor` with slotmap `DocumentId` /
  `ViewId`, views, focus, scrolling; `Editor::handle(Event) -> Effects` on
  raw input; temporary key map in `editor/keys.rs` (Stage 7 deletes it).
- **Traces** (`trace.rs`): JSON Lines record/replay of raw input
  (DECISIONS 016); `FLASH_TRACE=<file>` records, `flash --replay <trace>`
  replays headless and prints latency.
- **Standing benchmark** (`benches/document.rs`): N views × 10k
  selections, one edit, full commit (with history), and its undo.
- **History** (`document/history.rs`): branching undo tree per document;
  forward and inverse change sets, selections before and after, times;
  `undo`, `redo`, `goto` through the commit path; typing grouped by
  composition until a pause, motion, paste, undo, or checkpoint; time is
  an input (`Editor::handle_at`, trace `t_ns`) so replay groups exactly;
  chunked node storage; no cap (DECISIONS 017–019). Temporary keys
  Alt+Z / Alt+Y since Stage 6. Memory per node in `benches/history.rs`.
- **Benchmarks** (`benches/text.rs`, `benches/latency.rs`): Criterion suite
  plus per-operation p50/p95/p99/max and allocation counts on deterministic
  fixtures. Results in `bench/results/`. Interactive skeleton latency (real
  terminal, read → flush): p99 758 µs over 199 events.

- **Files** (`crates/flash-core/src/file/`): streaming load (invalid
  UTF-8 refused, BOM, dominant line ending, size/mtime/hash); atomic save
  over the resolved target, in place behind a backup for hard links or
  ownership, conflict check; failures leave the original (DECISIONS 021).
- **Terminal editor** (`crates/flash-term`, `apps/flash`): renderer with
  horizontal scroll, tabs, per-cluster width and re-sync, prompt and
  status line; terminal guard restored on exit, error, panic, signals,
  and around Ctrl+Z suspend; input and signal threads, one channel, 8 ms
  drain; save/open/quit prompts, register and OSC 52 clipboard; requests
  keep the core free of I/O (DECISIONS 020–024). Undo is Alt+Z, redo
  Alt+Y / Alt+Shift+Z (owner decision 2026-11-16). pty tests, including a
  `FLASH_TRACE` session recorded and replayed.
- **Column checkpoints** (`editor/column.rs`): the head's column without
  walking long lines (DECISIONS 025). Interactive latency through a pty:
  p99 475 µs.

- **Commands, keymaps, modes, options, config** (Stage 7a,
  `crates/flash-core/src/editor/`): named, owned commands run without
  keys; event hooks delivered in bounded batches; layered keymaps
  compiled into a trie per mode, allocation-free resolution; normal,
  insert, extend, prompt and question modes with a one-grapheme block
  cursor (PLAN 2.5); typed options resolved view → document → global →
  default; the default keymap is `default-config.toml`, applied like a
  user's `~/.config/flash/config.toml` (`--config`, `:config-reload`);
  regex selection commands over the rope (`crate::search`). No key is
  hard-coded (DECISIONS 026–032).

- **Stage 7b** (2026-11-17, [stage-7b.md](stages/stage-7b.md), DECISIONS
  033–043): carried fixes (search across chunk boundaries, undo groups
  bounded by size, hook kinds, kitty keyboard flag, read-time events,
  saves and opens on an I/O thread, idle-mode benches and frame bytes);
  the display-line layout cache (`flash_core::layout`, replacing the
  line anchor); soft wrap (`soft-wrap`, `g j`/`g k`); incremental search
  (`/ ? n N *`, `:noh`), replace (`:s`, `:%s`), large documents searched
  on a thread.

## Implemented but not fully verified

- Stage 3 had no independent fresh-context review; the owner approved it
  without one (2026-11-15). The implementer's adversarial pass is in
  [stage-3.md](stages/stage-3.md) section G.
- Stages 4 and 5 have no independent fresh-context review; the owner
  approved both without one (2026-11-16). The implementer's reviews are
  in [stage-4.md](stages/stage-4.md) and [stage-5.md](stages/stage-5.md)
  section G.
- Stages 6 and 7a have no independent fresh-context review; the
  implementer's reviews are in [stage-6.md](stages/stage-6.md) and
  [stage-7a.md](stages/stage-7a.md) section G. The owner approved both
  without one (2026-11-17).
- Interactive latency after 7a is higher than the Stage 6 run while the
  in-process benches are not; not explained (stage-7a.md F).
- File ownership preservation (`fchown`) is tested only by simulation
  (needs root).

## In progress

- Stage 7b complete on branch `stage-7`, awaiting the owner's approval;
  Stage 7c next (the owner asked for both to be completed).

## Planned

- Stage 7c (cell grid with views and overlays, renderer judged on bytes
  and cells per frame, themes, slow output, the input-thread
  experiment); Stages 8–11 as in [PLAN.md](PLAN.md).

## Deferred

- UTF-16 metric: overhead measured in Stage 1; enabling it is decided in
  Stage 10 (LSP).
- History cap: none until measurements or the Stage 7 option registry
  ask for one (DECISIONS 019).
- A jump along a huge single line (End after an edit split or shortened
  it) walks from the layout's last mark: O(distance), about 26 ms on
  1 MB (DECISIONS 040). Up/down on long lines use the layout's marks.
