# Stage 7a design — commands, events, keymaps, options, config, modes

Date: 2026-11-17. Spec: `docs/PLAN.md` Stage 7, sections 2.5, 2.10,
2.11. Binding: DECISIONS 014 (commit is the only text mutator), 016
(traces are raw input), 018 (grouping; mode changes end groups), 020
(the core does no I/O).

Owner decision (2026-11-17): **Stage 7 is gated three times.**

- **7a (this design):** command registry, event hooks, keymaps, option
  registry and TOML config, modes, multi-selection commands. Ends with
  no key hard-coded in Rust.
- **7b:** incremental search and replace, soft wrap.
- **7c:** renderer benchmarks (grid diff vs dirty lines, layout cache,
  cursor-only output, style transitions, scroll regions), themes, slow
  output.

PLAN's Stage 7 acceptance list is split accordingly: 7a covers "a test
config rebinds and unbinds default keys in every mode and changes options
at runtime without restart; no key is hard-coded in Rust; commands run
independently of keys; modal keymaps work; key resolution is
allocation-free and benchmarked". Everything marked **(call)** is the
implementer's and is listed again in the stage report.

## Intent

Make every action a named command, every key a binding in data, and
every tunable an option, so that the defaults are just configuration
(PLAN 2.11) and Stage 11's Luau layer is only an adapter. Switch the
default editing model to PLAN 2.5's selection-first modal editing.

**Goals:** the 7a items above, with the Stage 6 behaviour still
reachable through commands.
**Non-goals:** search, soft wrap, themes, renderer changes beyond showing
the mode and the cursor shape (7b, 7c); Luau (Stage 11); language scope
contents (no languages until Stage 9; the scope exists and is empty).
**Anti-goals:** any per-key allocation or string lookup on the typing
path; a bad config that prevents startup or silently drops the whole
file; a key that can only be changed by editing Rust.

## Approaches

1. **Registries in `flash-core`, defaults as an embedded TOML file read
   by the same parser as user config (chosen).** Commands, hooks, modes,
   options and keymaps live in the headless editor, so traces, tests and
   replay exercise them without a terminal; the binary only reads the
   config file (DECISIONS 020) and passes the text in.
2. Registries in the binary, core keeps a fixed command enum. Keeps core
   smaller, but replay and core tests would no longer see keymaps or
   modes, and plugins (Stage 11) would need a second registry. Rejected.
3. Defaults built by Rust calls, user config by TOML. Two paths, so the
   default keymap could express things user config cannot (PLAN 2.11:
   no privileged built-ins). Rejected.

## Design

### Commands (`editor/command.rs`)

- `CommandId(u32)`: an index that is never reused; removing a command
  leaves a tombstone, so a stale ID fails cleanly.
- A registration: `name` (kebab-case, unique among live commands),
  `owner: Owner`, a doc line, and `run: Arc<dyn Fn(&mut Editor, &Context)
  -> Effects>`. `Owner` is `Owner(u32)`; `Owner::BUILTIN` is 0, config
  is 1, plugins get fresh ones. `remove_owner(owner)` removes every
  command, hook, mode, option and key layer of that owner.
- `Context { view: Option<ViewId>, count: Option<NonZeroU32>, arg:
  Option<&str> }`. Commands never read keys.
- `Editor::run(id, ctx)` and `Editor::run_named(name, ctx)` run commands
  without a key; the `:` command line uses `run_named`.
- Built-in commands (registered by `Editor::new`, owner BUILTIN) cover
  everything Stage 6 keys did plus the modal set below. Each is a small
  native function; no command allocates per call except where its
  result does (a transaction).

### Event hooks (`editor/hooks.rs`)

- Kinds: `DocumentChanged { doc, from, to: Revision }`,
  `SelectionChanged { view }`, `ModeChanged { view, from, to }`,
  `ViewOpened { view }`, `ViewClosed { view }`.
- `Editor` queues events while handling an input event (or a direct API
  call) and delivers them at the end of `handle` as one batch per
  subscriber: `Fn(&mut Editor, &[EditorEvent])`, registered with an
  owner, removable by ID. Events raised by subscribers are delivered in
  further rounds, at most 8 per `handle` (call); then the rest is dropped
  and the message says so.
- **Native consumer:** the built-in `checkpoint-on-mode-change`
  subscriber ends the undo group when a view changes mode (PLAN Stage 5
  note), so the hook path is exercised by every mode switch.

### Keymaps (`editor/keymap.rs`)

- **Notation (call, Helix-style):** `"x"`, `"C-s"`, `"A-z"`, `"S-left"`,
  `"C-A-x"`; named keys `ret esc tab backspace del left right up down
  home end pageup pagedown space F1`–`F12`; a sequence is
  space-separated: `"g g"`. Shifted letters are written as the
  character (`"U"`), matching what the terminal sends.
- **Layers:** each mode has layers, highest first: user (config), plugin
  layers (by owner), default (the embedded config). A binding maps a
  sequence to `Bind::Command { id, arg }` or `Bind::Unbound` (an explicit
  unbind that hides lower layers).
- **Compiled trie per mode**, rebuilt when any layer changes: nodes in a
  `Vec`, each with children sorted by key (binary search) and an
  optional action. Resolution keeps `(mode, node)` as the pending state
  and allocates nothing.
- **Sequences and timeout (call):** a key that continues the pending
  sequence descends; a key that does not resets the pending state and is
  resolved from the root. A node with both an action and children runs
  its action when the next key does not continue it, or when that key
  arrives after the `sequence-timeout` option (default 1000 ms). Time is
  the input time (`handle_at`), so replay is exact; there is no timer
  event, so a lone ambiguous prefix waits for the next key. The default
  keymap has no ambiguous prefixes.
- **Unbound keys:** a mode can `insert-unbound` (insert mode and the
  prompt: unbound printable characters are inserted); elsewhere an
  unbound key does nothing and the message names it.
- **Counts:** digits are bound to the `count` command (`"1" = "count
  1"`), which accumulates a count for the next command (normal and
  extend modes), so even counts are configuration.

### Modes (`editor/mode.rs`, PLAN 2.5)

- A mode is data registered through the same API plugins will use:
  `name`, `insert-unbound`, `min-grapheme`, `extend` (motions extend
  instead of move), `cursor-shape` (an option, per mode). Built-in modes
  `normal`, `insert`, `extend` (and `prompt` for the one-line prompt)
  are registered by `Editor::new` and keyed by the default config.
- The mode lives on the view; a new view starts in `normal`.
- **One-grapheme minimum** (normal and extend, option
  `normal-min-grapheme`, default true): after entering the mode and
  after each command in it, every empty range becomes the grapheme
  after it, `[h, next(h))`, or stays empty at the end of the text.
- **Block cursor model:** for a range, the *cursor grapheme* is the last
  grapheme of a forward range, the first of a backward range, and the
  one at the head of an empty range. Normal-mode motions move the block:
  the motion runs from the cursor grapheme's start, and the new range is
  the grapheme there. Extend-mode motions keep the anchor grapheme and
  move the cursor grapheme, flipping direction when they cross. Word
  motions select (`w`: from the cursor to the next word start; `b`:
  back to the word start), as in Helix.
- **Default keymap (call, Helix-like where Helix has the key):**
  `h j k l` and arrows; `w b`; `g h`/`g l` line start/end, `g g`/`g e`
  document start/end; `x` select line; `%` select all; `i a I A o O`
  enter insert; `v` extend mode; `esc` back to normal; `d c y p P`;
  `u U` undo/redo (`A-z`, `A-y`, `A-Z` kept in every mode); `;` collapse
  to the cursor grapheme; `,` keep primary; `( )` rotate primary; `C`
  and `A-C` add a cursor below/above; `s` select regex matches, `S`
  split on regex, `A-s` split on lines, `K`/`A-K` keep/remove matching;
  `:` command line; `C-s` save, `C-q` quit, `C-z` suspend, `C-o` open,
  `C-w` close view, `C-pageup`/`C-pagedown` views, `pageup`/`pagedown`
  in every non-prompt mode (conveniences kept from Stage 6, call).
- **Command line (`:`, call):** a prompt whose input is `name [arg]`;
  aliases `w`, `q`, `q!`, `wq`, `o`/`e`, `set`, `setlocal`,
  `config-reload`. Unknown names are messages.
- **Prompt as a mode:** the prompt's editing keys (`ret`, `esc`,
  `backspace`, `left`, `right`, `home`, `end`) are bindings in the
  `prompt` mode, so they are rebindable (PLAN 2.11).

### Multi-selection commands (`editor/select.rs`)

- Split selection on a regex (`S`), select regex matches inside the
  selections (`s`), keep or remove selections matching a regex (`K`,
  `A-K`), split on lines (`A-s`), add cursor above/below (`C`, `A-C`,
  already in Stage 6), rotate primary (`(`, `)`), collapse to the cursor
  grapheme (`;`), keep primary (`,`).
- Regex over the rope with `regex-cursor` 0.1.5 (in the offline cache),
  searching only inside each selection's byte range; the regex comes
  from a prompt (or the command's argument) and is compiled once per
  command. Results are normalised by the 2.5 merge rule; an empty
  result keeps the old selection and says so.

### Options (`editor/options.rs`, PLAN 2.11)

- A registration: `name`, `owner`, doc, `kind` (`Bool`, `Int { min,
  max }`, `Choice(&[&str])`), default. Values: `bool`, `i64`, choice
  index.
- **Scopes** view → document → language → global → default. `set`
  writes global, `setlocal` writes the focused view; documents have a
  scope through the API; language is empty until Stage 9.
- **Hot path is a field read:** each view holds a resolved
  `ViewOptions` struct (tab width, group pause, sequence timeout,
  min-grapheme, cursor shapes, default line ending) recomputed when any
  option, scope, or the view's document changes. Plugin options resolve
  through the registry (a lookup), not the struct.
- Built-in options (call): `tab-width` (1–16, 4), `undo-group-pause-ms`
  (0–60000, 1000), `sequence-timeout-ms` (0–60000, 1000),
  `normal-min-grapheme` (true), `default-line-ending` (`lf`/`crlf`, for
  new documents), `cursor-shape-normal|insert|extend|prompt`
  (`block`/`bar`/`underline`; block, bar, block, bar). The Stage 6
  `--tab-width` flag sets the global value.

### Config (`editor/config.rs`, PLAN 2.11)

- TOML, parsed with `toml` 1.x (offline cache) into a value tree and
  walked by hand, so every entry is checked separately: `[options]`
  `name = value`; `[keys.<mode>]` `"seq" = "command [arg]"` or `false`
  (unbind). Unknown options, wrong types, bad key notation, unknown
  commands or modes are reported (file, entry, reason) and skipped;
  the rest applies. Themes are 7c.
- The default keymap and option defaults are `default-config.toml`,
  embedded with `include_str!` and applied by `Editor::new` through the
  same function as user config, into the default layer.
- `Editor::apply_config(source, text) -> Vec<ConfigError>` replaces the
  user layer and user option values atomically (parse and check
  everything first, then swap). The binary reads
  `$XDG_CONFIG_HOME/flash/config.toml` (else `~/.config/flash/…`, or
  `--config <path>`); a missing file is no error. Reload at runtime:
  the `config-reload` command queues `Request::LoadConfig`, the binary
  reads the file and calls `apply_config` (DECISIONS 020).
- Traces stay raw input (DECISIONS 016); the header gains the config
  source and a hash of its text, and `--replay` warns when it replays
  against different config.

### Editor plumbing

- `Editor::handle`: resize; else resolve the key in the focused view's
  mode (or the prompt mode while a prompt is open); run the command with
  the count and argument; enforce the mode's minimum; deliver hooks;
  scroll. `keys.rs` is deleted; the Stage 6 key functions become the
  bodies of named commands.
- `Effects` gains `mode: bool` and `pending: bool` (a sequence is
  pending).
- The renderer shows the mode and pending keys on the status line and
  sets the cursor shape from the view's resolved options (crossterm
  `SetCursorStyle`), restored on exit.

## Tests

- Commands: register, run by ID and name, remove, stale IDs, owner
  removal, every built-in runs without a key on an empty and a non-empty
  document.
- Hooks: batching, ordering, removal, re-entrant rounds bounded, the
  mode-change checkpoint.
- Keymaps: notation round trip and errors; layers and unbinding; trie
  resolution of single keys, sequences, misses, ambiguous prefixes and
  timeout; a property test against a naive resolver; zero allocations
  per resolved key (counting allocator test).
- Modes: min-grapheme after every command in normal mode (property over
  random key streams), block-cursor motions, extend flips, insert and
  append, mode-change events.
- Multi-selection: each command against hand cases and a property
  (results valid, normalised, inside the original selections).
- Options: scopes, validation, runtime changes reach the view struct.
- Config (PLAN acceptance): a test config rebinds and unbinds default
  keys in **every** mode and changes options at runtime without restart;
  bad entries reported and skipped; reload replaces only the user layer.
- No hard-coded keys: a test runs the editor with an empty default
  keymap and checks no key does anything but resize.
- Existing suites: Stage 3–6 tests are ported to commands (their key
  sequences change with the default keymap; behaviour must not).

## Benchmarks

- Key resolution: single key, two-key sequence, miss; p50/p99 and
  allocations (must be zero) in `benches/latency.rs`.
- The core latency rows and the editor end-to-end bench, compared with
  Stage 6, through the new keymap (typing in insert mode must not get
  slower by more than run-to-run spread).

## New dependencies

`regex-cursor` 0.1.5 (with `regex-automata`, `regex-syntax`, `memchr`,
`log`), `toml` 1.x (parser only, no serde derive). All in the offline
cache.
