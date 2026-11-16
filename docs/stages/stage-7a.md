# Stage 7a — Commands, events, keymaps, options, config, modes

Status: **complete; awaiting owner approval** (no independent review was
run). Branch `stage-7` (on top of `stage-6`, which also awaits approval),
local commits only (nothing pushed). Design:
[2026-11-17-stage-7a-design.md](../superpowers/specs/2026-11-17-stage-7a-design.md).
Plan: [2026-11-17-stage-7a-commands-keymaps-modes.md](../superpowers/plans/2026-11-17-stage-7a-commands-keymaps-modes.md).

The owner asked to "continue the next stage in here as well" and decided
one question (2026-11-17): **Stage 7 is gated three times** (DECISIONS
026). This is the first gate. The design's choices marked **(call)** are
the implementer's and are listed in section G.

## A. Status

PLAN Stage 7 acceptance, the part this gate covers:

| Requirement | State |
|---|---|
| A test config rebinds and unbinds default keys in every mode and changes options at runtime without restart | Done: `config/tests.rs` `a_config_rebinds_and_unbinds_in_every_mode_and_sets_options_at_runtime` (normal, insert, extend, prompt, question); end to end in a pty with `:config-reload` |
| No key is hard-coded in Rust | Done: `editor/keys.rs` deleted; `Editor::handle` resolves every key through the compiled keymap; with the default layer removed no key does anything (`without_the_default_keymap_no_key_does_anything`) |
| Commands run independently of keys | Done: `run`, `run_named`, the `:` command line; every built-in runs by name (`every_builtin_runs_without_a_key`) |
| Modal keymaps work | Done: normal, insert, extend, prompt, question (DECISIONS 029) |
| Key resolution is allocation-free and benchmarked | Done: `tests/alloc.rs` (zero allocations), latency rows (F) |
| Command registry with owners, removable | Done (DECISIONS 027) |
| Event hooks with batch payloads and a native consumer | Done (DECISIONS 031) |
| Layered keymaps, multi-key sequences, pending state, timeout, compiled trie | Done (DECISIONS 028) |
| Option registry, scopes, TOML config, runtime reload, invalid entries reported and skipped | Done; the language scope waits for languages (Stage 9) (DECISIONS 030) |
| Modes per 2.5 with the one-grapheme minimum; multi-selection commands | Done (DECISIONS 029, 032) |
| Search and replace, soft wrap | Stage 7b |
| Renderer benchmarks and choices, themes, slow output | Stage 7c (this gate only adds the mode and pending keys to the status line and the cursor shape) |

## B. Commits

Branch `stage-7`, from `00c76c5` (Stage 6 head):

| Commit | Content |
|---|---|
| `89d31cb` | Stage 7a design and plan; `regex-cursor`, `toml` |
| `c52b710` | Option registry, scopes, per-view resolved options |
| `053f02f` | Command registry and built-in commands |
| `7a7d36d` | Event hooks, batched and bounded |
| `9dee6d4` | Keymaps: notation, layers, trie, sequences |
| `f3ccaa2` | Modes, the default keymap as config, `keys.rs` deleted, Stage 3–6 tests ported to insert mode |
| `120d5e2` | Multi-selection commands, `crate::search` (**committed with failing renderer tests and two lints**: see G, finding 9) |
| `796068a` | Block cursor drawn on its grapheme, zero-width left-edge fix, renderer tests ported |
| `3416422` | Config file, reload, trace config hash, mode and cursor shape on screen |
| `61b91cc` | Allocation-free key resolution and normal motions; latency rows |
| (closing commit) | Review fixes and tests, results, this report, DECISIONS 026–032, status |

New dependencies: `regex-cursor` 0.1.5 (with `regex-automata`,
`regex-syntax`, `memchr`, `log`) and `toml` 1.x (features `parse`,
`std`, `serde`: the value tree is behind `serde`; no derive), all from
the offline cache.

## C. Design

### Commands and hooks

Every action is a named command with an owner (DECISIONS 027): the
Stage 6 actions, `move-*`/`extend-*` motions with counts, the modal
commands (`insert-mode`, `append-mode`, `open-below`, `select-line`,
`select-next-word`, ...), prompt editing, the selection commands, and
`config-reload`. Events queue while the editor works and are delivered
in batches at the outermost call, at most eight rounds deep (DECISIONS
031).

### Keys

Each mode has key layers (user config, plugins, default config),
compiled into a trie; resolution keeps the pending sequence in the
keymap and uses input time for the sequence timeout, so a trace replays
exactly (DECISIONS 028). The default keymap is `default-config.toml`,
read through the same code as a user's file. Normal mode is Helix-like
where Helix has the key; insert mode keeps every Stage 6 key.

### Modes

Selection-first modal editing (PLAN 2.5, DECISIONS 029): in normal and
extend mode the cursor is a one-grapheme selection; motions move or
extend it; operators act on the selections; empty ranges are widened
after every command. The block's grapheme is what scrolling, the status
column, and the terminal cursor follow.

### Options and config

Options resolve view → document → global → default, with the built-ins
a view needs kept resolved in the view (DECISIONS 030). The config file
(`--config`, else `$XDG_CONFIG_HOME/flash/config.toml` or
`~/.config/flash/config.toml`) is checked entry by entry; `:config-reload`
re-reads it. Traces record its hash.

## D. Tests

FILL-COUNTS

New in 7a (core unless noted):

- **Options** (`options/tests.rs`, 5): built-ins at their IDs, parsing
  and validation, owners and tombstones, scope order through the editor,
  group pause and line ending reaching documents.
- **Commands** (`command/tests.rs`, 4): registration, running by ID and
  name with a context, removal, owners; every built-in by name on empty
  and non-empty documents, with and without a view.
- **Hooks** (`hooks/tests.rs`, 6): one batch per key, kinds, nesting,
  removal, the round limit, direct API calls.
- **Keymaps** (`keymap/tests.rs`, 4): notation round trip and errors,
  layering and unbinding, sequences, ambiguous prefixes, timeouts, and a
  property test against a naive buffer resolver.
- **Modes** (`mode/tests.rs`, 15): block motions, counts, word
  selection progress, extend flips, `i a I A o O` with undo, `x % d c y
  p`, multiple cursors, unbound keys, timeouts, the command line, no
  hard-coded keys, plugin modes, the native hook consumer, and hostile
  patterns, counts, and command lines.
- **Selections** (`select/tests.rs`, 6; `search.rs`, 2): each command and
  a property (results valid, normalised, inside the old selections);
  the rope matcher against the `regex` crate on random Unicode text.
- **Config** (`config/tests.rs`, 7): the default config, the PLAN
  acceptance config, bad entries, reload layering, default options, and
  hostile configs (100,000-deep nesting, 20,000 bindings, control
  characters, a sequence of six keys).
- **Allocations** (`tests/alloc.rs`): zero allocations for 5,000 key
  resolutions and for normal- and insert-mode motions through
  `Editor::handle`.
- **Random events** (`editor/proptests.rs`): the modal keys added; after
  every event, normal and extend mode have no empty range except at the
  end of the text; anchors still checked; zero-width pieces added. A
  one-off scaled run (3,000 texts × 100 events) passed.
- **Trace replay** (`trace_tests.rs`): now also compares mode, pending
  keys, and count, and allows both sessions to have closed the view.
- **Renderer** (flash-term, 19): `normal_mode_draws_the_block_and_shows_the_mode`
  (cursor on the block's grapheme, mode and pending keys, cursor style);
  the emulator understands cursor-style escapes.
- **Binary** (CLI 7, pty 7 + 1 ignored): `--config` errors, replay
  config warnings, `a_config_file_binds_keys_and_reloads_at_runtime`;
  every pty session checks the cursor shape is reset on exit and never
  reads the user's own config file.

## E. Commands run

FILL

## F. Benchmarks

Results in `bench/results/stage-7a-*`. Machine and build as before.

**Key resolution and normal-mode keys** (`benches/latency.rs`, 100,000
samples), ns p50 / p99, allocations per op:

| op | small | medium 1 MB | large 100 MB | one-line 1 MB | unicode 1 MB |
|---|---|---|---|---|---|
| key resolve `l` | 46 / 50, 0 | 47 / 51, 0 | 46 / 55, 0 | 46 / 118, 0 | 47 / 53, 0 |
| key resolve `g g` (2nd key) | 33 / 35, 0 | 34 / 36, 0 | 34 / 50, 0 | 34 / 62, 0 | 34 / 37, 0 |
| key resolve miss | 37 / 41, 0 | 39 / 43, 0 | 39 / 47, 0 | 37 / 41, 0 | 38 / 48, 0 |
| editor normal `l` (block) | 696 / 1,049, 0 | 1,873 / 2,455, 0 | 2,462 / 3,225, 0 | 2,072 / 2,763, 0 | 2,282 / 3,905, 0 |
| editor normal `w` (select word) | 1,113 / 1,709, 0 | 2,448 / 3,202, 0 | 3,065 / 3,966, 0 | 2,743 / 4,042, 0 | 3,342 / 5,944, 0 |

**The Stage 6 rows through the keymap** (insert mode, as Stage 6 typed),
ns p50 / p99, Stage 6 → 7a:

| op | small | medium 1 MB | large 100 MB | one-line 1 MB |
|---|---|---|---|---|
| doc commit insert char (1 view) | 969 / 1,134 → 1,302 / 1,706 | 2,647 / 3,145 → 3,305 / 4,119 | 3,950 / 4,607 → 5,212 / 7,180 | 2,889 / 3,380 → 3,970 / 9,076 |
| editor handle key 'x' | 1,004 / 1,243 → 1,418 / 1,847 | 2,655 / 4,004 → 3,456 / 4,292 | 3,971 / 4,593 → 5,335 / 6,753 | 2,976 / 3,472 → 3,882 / 7,208 |
| editor handle Right | 401 / 461 → 640 / 933 | 1,214 / 1,590 → 1,585 / 2,089 | 1,900 / 2,776 → 2,569 / 3,603 | 1,412 / 1,854 → 1,892 / 4,225 |
| undo char | 949 / 1,793 → 1,076 / 1,916 | 2,066 / 3,052 → 2,199 / 3,212 | 2,505 / 3,499 → 2,682 / 3,896 | 3,850 / 4,515 → 3,996 / 7,790 |

A key now costs about 0.1–1.3 µs more at p50 (keymap dispatch, the
command call, hook bookkeeping, the mode check); one-cursor commit on
1 MB is **4.1 µs p99**, against PLAN's "well under 100 µs". The 7a
insert-mode rows show one more allocation per op than Stage 6: each
sample starts from a fresh clone of the editor, whose hook queue has no
capacity yet, so the first queued event allocates once. In a running
editor the queue keeps its capacity: `tests/alloc.rs` counts zero
allocations for the same keys after warm-up.

**Editor end to end** (`apps/flash/benches/editor.rs`, in insert mode as
Stage 6), ns p50 / p99:

| fixture | 80×24 7a | 80×24 Stage 6 | 200×60 7a | 200×60 Stage 6 |
|---|---|---|---|---|
| small 4 KB | 25,569 / 44,628 | 24,482 / 43,796 | 51,953 / 169,008 | 50,522 / 82,538 |
| medium 1 MB | 42,006 / 104,361 | 40,967 / 101,206 | 73,375 / 133,979 | 71,721 / 133,197 |
| unicode 1 MB | 40,451 / 121,398 | 39,756 / 117,391 | 72,824 / 151,049 | 71,445 / 149,367 |
| one-line 1 MB | 11,333 / 26.8 ms | 10,861 / 26.8 ms | 24,497 / 26.8 ms | 23,810 / 26.8 ms |

Within run-to-run spread except small 200×60 p99 (169 µs against 83 µs;
its max was 357 µs in this run): one run, not repeated, so not
established as a regression.

**Interactive, real pty** (500 keys 5 ms apart into 1 MB, release),
three runs, ns:

| | total p50 / p99 | command p50 / p99 | layout p50 |
|---|---|---|---|
| Stage 6 (closing run) | 229,276 / 475,217 | 27,740 / 58,141 | 79,909 |
| 7a run 1 | 280,140 / 474,803 | 45,162 / 95,639 | 108,272 |
| 7a run 2 | 280,263 / 631,198 | 46,313 / 104,889 | 110,094 |
| 7a run 3 | 295,708 / 567,568 | 45,913 / 93,659 | 113,214 |

Still under PLAN's 2 ms p99. The command and layout stages are
consistently higher than the Stage 6 run, which the in-process benches
(above) do not show. **Unexplained and unverified:** an A/B run of the
Stage 6 binary on the same machine now was not done (the rebuild in a
scratch directory was not permitted), so machine state and a real
regression in the interactive path are not separated.

## G. Review

**Independent review: not done by a fresh reviewer** (owner's standing
rule: no delegation without an explicit request). The implementer
reviewed the branch as a separate pass with attack tests and mutation
checks. This is weaker than PLAN section 3 requires; **the owner should
decide whether to run an independent review before approving.**

Findings (found during implementation or the review pass; all fixed
unless marked):

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | Important | A hook subscriber that ran a command delivered events recursively, bypassing the round limit (26 rounds in the test). | Fixed: nested calls queue while subscribers run (`hooks_that_keep_causing_events_stop_after_the_round_limit`). |
| 2 | Important | Widening a new view's cursor from a deferred hook changed a selection behind the command, which ended another view's undo group (a Stage 6 test caught the split). | Fixed: done synchronously in `open_view`; DECISIONS 031. |
| 3 | Important | In normal mode, `I A o O` and add-cursor worked from the head; a block on a newline has its head on the next line, so they acted on the wrong line. | Fixed: they start from the cursor grapheme. |
| 4 | Important | Scrolling, the status column, and the terminal cursor followed the head, one grapheme past the block. | Fixed: `Editor::cursor_position` drives all three (`normal_mode_draws_the_block_and_shows_the_mode`). |
| 5 | Important | A left-edge checkpoint reused at exactly the left column skipped zero-width graphemes before it; the renderer's random test showed a frame different from the walk. | Fixed (`a_start_reused_at_the_left_column_takes_in_zero_width_graphemes`); zero-width pieces added to the core random test. |
| 6 | Minor | Alt+Shift+z normalised to `A-z` (undo) instead of `A-Z` (redo). | Fixed: Shift folds into the character. |
| 7 | Minor | `O` was two undo steps (the newline, then a motion). | Fixed: one edit with the cursor placed before the new line. |
| 8 | Minor | `tab-width` allowed 1–16 while `--tab-width` allows 1–255, so a larger CLI value was silently dropped; removing the built-in owner was allowed. | Fixed: 1–255; removing the built-in owner does nothing. |
| 9 | Process | `120d5e2` was committed with two clippy errors and failing renderer tests: the commit was chained after the checks instead of gated on them, and since `f3ccaa2` `cargo test --workspace` stopped at the first failing crate, so flash-term's tests had not run. | Fixed forward in `796068a` (findings 4 and 5 came from those tests); every later commit was gated on fmt, clippy, and the whole test suite with `--no-fail-fast`. |
| 10 | Open | Interactive latency is higher than the Stage 6 run (F). | Not explained (no A/B). |

Attack tests, kept (none found a fault): hostile configs (100,000-deep
nesting, 20,000 bindings, control characters and spaces as keys,
non-string bindings, a BOM, a six-key sequence); a regex past the size
limit; counts past `usize` overflow on every counted motion; empty and
odd command lines. Also checked by reading: counted word, grapheme and
vertical motions stop at the text's edges (no hang on huge counts);
regex matching is linear in the bytes searched.

Mutation checks (each made the code wrong, then was reverted):

| Mutation | Result |
|---|---|
| No sequence timeout / no Shift folding / no minimum after commands / unbound keys typed in every mode / unbinding ignored / user layer below plugins | caught |
| Reload keeps old globals / keeps the old key layer | caught by `config/tests.rs` |
| Extend mode does not extend / `w` without the progress rule | caught by `mode/tests.rs` |
| Split drops the tail / document scope before view scope / events never queued | caught |
| `map_ranges` collects into a `Vec` | caught by `tests/alloc.rs` |
| Closing a view keeps the pending sequence and count | **survived**; added `closing_a_view_drops_pending_keys_and_count`, now caught |

Implementer calls made without the owner (cost if wrong in brackets):

- Default keymap Helix-like where Helix has the key, Stage 6 keys kept
  in insert mode and as `C-s`/`C-q`/`C-z`/`C-o`/`C-w` in normal mode
  [data in `default-config.toml`].
- `w`/`b` select words (Kakoune's rule); `p` pastes after the selection,
  cursor after the paste [command bodies].
- Unbound keys reported outside modes that type them [one branch].
- Sequence timeout on input time, no timer event; default 1000 ms
  [an option].
- Config walked by hand over `toml`'s value tree; a reload clears `:set`
  values; `--tab-width` re-applied after each load [config.rs].
- The hook round limit 8; mode-change checkpoint done directly, not by
  a hook; the native consumer drops pending keys on view close
  [DECISIONS 031].
- Language scope deferred to Stage 9 [one more scope].
- The `:` aliases `w q q! wq o e set setlocal` [command_line].
- Benchmarks keep Stage 6's insert-mode patterns for comparison; new
  normal-mode rows [bench code].

## H. Known limitations

- A lone ambiguous key prefix waits for the next key (no timer event);
  the default keymap has none.
- Typing via `insert-text`, `a`, or a paste in normal mode goes at the
  head, after the block.
- A config reload clears global option values set with `:set`.
- `:wq` quits only if the save succeeds at once; after a conflict
  question, the overwrite does not quit.
- Options for a plugin mode's cursor shape in the user's config are
  skipped if the config loads before the plugin registers the mode.
- No language scope for options yet (Stage 9).
- Interactive latency is higher than the Stage 6 run, unexplained (F).
- Stage 6 limitations stand (vertical motion and Home/End on huge
  lines, emoji width disagreements, SIGTERM loses unsaved changes).

## I. Questions for the reviewer

1. Can any key reach an action except through the compiled keymap?
2. Can a pending sequence lose a key or run one twice, including across
   a mode change, a prompt opening, or a view closing?
3. Is there a command sequence after which a normal-mode view holds an
   empty range away from the end of the text?
4. Can a config file (any bytes) prevent startup, panic, or discard
   valid entries next to invalid ones? Does a reload ever keep a stale
   binding?
5. Can removing an owner leave a dangling command, mode, option, key
   layer, or hook?
6. Can hooks recurse without bound or change editing state behind a
   command (as finding 2 did)?

## J. Next stage

Stage 7b: incremental search and replace (regex over the rope through
`crate::search`, off the main thread for large buffers so input never
blocks) and soft wrap. Then 7c: renderer benchmarks (grid diff against
dirty lines, a per-line layout cache, cursor-only output), themes, and
slow output. 7b starts after the owner approves 7a (and Stage 6).
