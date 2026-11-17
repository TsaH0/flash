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

- Stage 7b committed (`3ef069c`), awaiting the owner's approval; still
  to run: the 4,096-case `small-chunks` property gate on the final tree.
- **Stage 7c, uncommitted in the working tree (2026-11-17, resume
  here).** Written and passing `cargo test --workspace` and clippy:
  themes (`flash_core::theme`, `[theme]` in config and defaults); split
  views (`editor/tiles.rs`: `split`, `vsplit`, `only`, `next-tile`,
  bound under Space); the cell-grid renderer (`flash-term/src/grid.rs`,
  `emit.rs`: full / rows / cells strategies, `FLASH_RENDER`; the random
  render test checks diffed frames against an emulated screen); the
  `status-latency` option (off by default); cells per frame in the
  latency log. Written but **not yet compiled or run**: the input-thread
  experiment (`apps/flash/src/source.rs`, `FLASH_INPUT=main`, rustix
  poll and wake pipe) and the pty measurement
  `input_and_output_paths` (pty-side key-to-output delays, both input
  designs, each strategy, and a throttled 256 KB/s link). Next: build,
  run the measurements, decide the default strategy and input design
  (DECISIONS 044, 045), the 7c report, the full gate, commits.

## Stage 7 review — bugs and performance fixes to do

All twelve findings below are **open; fixes not applied**. Review covered
committed 7a/7b and the uncommitted 7c working tree. The existing suite
(`cargo test --offline --workspace --no-fail-fast`) passed: 388 tests,
2 ignored. Four initial regression probes and six follow-up probes
reproduced the ten correctness bugs; release-mode measurements confirmed
the two performance hotspots.

- [ ] **High priority: bound and cache search highlighting.**
  `crates/flash-term/src/render.rs`, `draw_view` / `find`: each frame
  searches up to 64 KiB per visible unwrapped line and builds fresh match
  lists, even when nothing changed. On an 80×24 screen with 24 lines of
  70,000 `a` characters, pattern `.` increased unchanged-frame composition
  from roughly 0.1–0.2 ms to 80 ms (release, mean of 10 frames; repeated).
  Cache by document revision, pattern and visible range, and bound work
  on the foreground thread. Add a highlight-enabled latency regression.
- [ ] **High priority: preserve literal regex delimiters in substitution.**
  `crates/flash-core/src/editor/find.rs`, `parse_substitution`: on
  `a|b a b`, `:%s|a\|b|X|` produces `X|X X X`, not `X a b`.
  Removing the escape before a metacharacter delimiter turns literal `|`
  into regex alternation and replaces unintended text. Preserve the
  pattern's regex semantics; cover metacharacter delimiters in tests.
- [ ] **Invalidate pending searches when the pattern changes.**
  `crates/flash-core/src/editor/find.rs`, `search_selection` / `search_done`:
  in a document above the background-search threshold, type `/needle`,
  accept before completion, then press `*` on the original selection
  (`a`). The pattern becomes `a`, but completion of the old job still
  selects `needle`. Cancel/invalidate pending work on every pattern change,
  including selection-search and replacement, and test delayed completion.
- [ ] **Reject stale matches whose text was edited.**
  `crates/flash-core/src/editor/find.rs`, `search_done`: start a background
  search for `needle`, accept it, replace `needle` with `xxxxxx`, then
  deliver the old result. It selects `xxxxxx`. Mapping endpoints alone
  does not prove a match survived unchanged. Check the matched span for
  intervening edits or revalidate the match before applying the result.
- [ ] **Preserve whole-document regex context in highlighting.**
  `crates/flash-term/src/render.rs`, `find`, and
  `crates/flash-core/src/search.rs`, `Matcher::find_in`: searching `^foo`
  in `x\nfoo\n` correctly reports no match, but the renderer underlines
  `foo`. Highlighting searches isolated slices, so anchors and word
  boundaries can differ from search navigation. Use context-preserving
  matching and test anchors, boundaries and viewport edges.
- [ ] **Stop backward search before scanning an unnecessary suffix.**
  `crates/flash-core/src/search.rs`, `Matcher::previous_before`: it always
  scans the entire document. With `a ` repeated, searching backward from
  byte 3 returns the match at byte 2 but takes approximately 8–9 µs for
  1 KiB, 710 µs for 64 KiB and 14 ms for 1 MiB (release, mean of three
  searches; repeated). Stop once the preceding match is established;
  continue through the remainder only when wraparound is needed.

- [ ] **High priority: resolve keys again after an ambiguous prefix changes mode.**
  `crates/flash-core/src/editor/mod.rs`, `key`: bind normal-mode `g` to
  `insert-mode` while retaining the default `g g` sequence. On `abc`,
  typing `gx` enters insert mode but runs normal-mode `x` (select line)
  instead of inserting `x`; the text stays `abc`, not `xabc`. Resolve
  the second key in the mode active after the prefix action, including
  when that action opens a prompt.
- [ ] **High priority: refresh layouts of every visible split after option changes.**
  `crates/flash-core/src/editor/mod.rs`, `refresh_options`: split a view,
  then enable global `soft-wrap`. The unfocused pane's resolved option
  becomes true but its layout still has `wrap: None`; only the focused
  pane is re-scrolled and reconfigured. Update all visible panes' layouts
  and test soft-wrap and tab-width changes across splits.
- [ ] **High priority: keep split option caches consistent with scoped values.**
  `crates/flash-core/src/editor/mod.rs`, `split`: set the original view's
  local tab width to 2, then split. The new pane's copied `ViewOptions`
  uses 2, but its option registry reports the global value 4. An unrelated
  option refresh can unexpectedly change its rendering. Either copy the
  view-scoped overrides or resolve the new view's options from its own
  scopes; test consistency before and after a refresh.
- [ ] **Restore highlight visibility when cancelling a search.**
  `crates/flash-core/src/editor/find.rs`, `search_prompt` / `search_closed`:
  search for `foo`, clear highlights, start searching for `bar`, then
  press Esc. The old `foo` highlights reappear. The saved search state
  omits highlight visibility; preserve and restore it alongside the
  previous pattern, matcher and direction.
- [ ] **Give Unicode characters and special keys disjoint identifiers.**
  `crates/flash-core/src/editor/keymap.rs`, `sort_key`: typing U+100000
  in insert mode inserts a newline instead of that character. Special-key
  identifiers start at `0x100000`, inside the valid Unicode scalar range,
  so other nearby characters also collide. Use disjoint tagged ranges
  and test supplementary-plane characters against special keys.
- [ ] **Prevent split-coordinate overflow at the maximum screen width.**
  `crates/flash-core/src/editor/tiles.rs`, `place`: resize to width 65,535
  and run `vsplit` 18 times. Once a zero-width pane is at the right edge,
  `area.x + left + 1` overflows before the subsequent clamp, panicking in
  debug builds. This is a hostile-trace/API edge case. Use checked or
  saturating coordinate arithmetic and test repeated splits at maximum
  dimensions as well as tiny rectangles.

Review probes and benchmark source are temporarily available at
`/tmp/flash-stage7-review-biBSfs/` (`src/lib.rs`, `src/main.rs`, and
`tests/additional.rs`);
these are not committed regression tests and may disappear. Recreate or
port them into the relevant crate tests when implementing the fixes.

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
