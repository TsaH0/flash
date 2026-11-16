# Stage 7a — Commands, Events, Keymaps, Options, Config, Modes: Implementation Plan

**Goal:** every action a named command, every key a binding in data,
every tunable an option; selection-first modal editing by default; no key
hard-coded in Rust.

**Spec:** `docs/superpowers/specs/2026-11-17-stage-7a-design.md`,
implementing the 7a part of `docs/PLAN.md` Stage 7 (owner decision
2026-11-17: Stage 7 gated as 7a, 7b, 7c).

## Global constraints

- Edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` in `flash-core`,
  `flash-term`, `flash_app`.
- `--offline`. New dependencies exactly: `regex-cursor = "0.1.5"` and
  `toml = "1"` (features `parse`, `std`; no serde) in `flash-core`.
- The core performs no I/O (DECISIONS 020): config text comes in through
  `Editor::apply_config`; reload is a `Request`.
- Typing a character in insert mode, motions, and key resolution
  allocate nothing beyond what the edit itself needs; option reads on
  the hot path are field reads.
- No panic reachable from input, config, a trace, or a command argument.
- Every Stage 3–6 behaviour stays reachable through a named command;
  tests that drove it through Stage 6 keys are ported, not deleted.
- One local commit per task on `stage-7`; never push; never add
  `.claude/`, `.mcp.json`, `.gitmodules`; trailer `Co-Authored-By: Claude
  Opus 5.5 <noreply@anthropic.com>`. `set -o pipefail` on pipes.
- Gate as Stage 6.

## Review focus

1. No key reaches an action except through the compiled keymap.
2. Pending sequences: no key is lost or run twice; timeout uses input
   time only.
3. Normal mode never leaves an empty range except at the end of the text
   or in an empty document.
4. A bad config entry never prevents startup and never discards the good
   entries; reload replaces only the user layer.
5. Owner removal leaves no dangling command, hook, mode, option, or key.
6. Hooks cannot recurse without bound.

## Tasks

### Task 1 — Option registry and per-view options

- `editor/options.rs`: `OptionId`, `OptionKind { Bool, Int { min, max },
  Choice(&'static [&'static str]) }`, `OptionValue`, registration with
  owner and doc, scopes (view, document, language, global, default),
  `set`/`unset` per scope with validation errors, `ViewOptions`
  resolved struct recomputed on change.
- Built-in options from the design; `tab-width` and
  `undo-group-pause-ms` replace `Editor::tab_width`/`set_tab_width` and
  the document's group pause (API kept as wrappers).
- Tests: validation, scope order, runtime change reaches every view's
  struct and the document's group pause.

### Task 2 — Command registry

- `editor/command.rs`: `CommandId`, `Owner`, `Context`, registration,
  `run`, `run_named`, `remove`, `remove_owner` (commands only for now).
- Built-in commands for every Stage 6 action (motions, edits, history,
  register, files, views, suspend, quit), registered by `Editor::new`.
  `keys.rs` temporarily maps keys to command IDs (deleted in Task 5).
- Tests: registry behaviour; every built-in runs by name on an empty and
  a non-empty document without panicking; Stage 6 behaviour unchanged.

### Task 3 — Event hooks

- `editor/hooks.rs`: event kinds, owned subscriptions, batching at the
  end of `handle` and of direct API calls, bounded rounds.
- Raise events from commit, selection changes, view open/close, and
  (Task 5) mode changes.
- Tests: batching, ordering, removal, bounded re-entrancy.

### Task 4 — Keymaps

- `editor/keymap.rs`: key notation parser and printer, `Bind`, layers
  per mode, trie compilation, resolver with pending state and timeout.
- Benchmark rows in `benches/latency.rs`; an allocation test with the
  counting allocator in the test build.
- Tests: notation round trip and errors; layering and unbinding;
  sequences, misses, ambiguous prefixes, timeout; property against a
  naive resolver.

### Task 5 — Modes and the default keymap

- `editor/mode.rs`: mode registry, mode per view, one-grapheme minimum,
  block-cursor motion commands, extend mode, insert/append/open-line,
  count, prompt as the `prompt` mode, the `:` command line.
- `default-config.toml` embedded and applied through the config path
  (minimal parser for `[keys.*]` and `[options]` here; Task 7 completes
  error reporting and reload). Delete `keys.rs`. Mode-change hook and
  checkpoint consumer.
- Port Stage 3–6 key-driven tests to the new keymap or to commands.
- Tests: min-grapheme property over random key streams; motions; extend
  flips; insert/append; no-hard-coded-keys test.

### Task 6 — Multi-selection commands

- `editor/select.rs`: select regex, split on regex, split on lines,
  keep/remove matching, rotate primary, keep primary, collapse; regex
  over the rope with `regex-cursor`, prompt or argument for the pattern.
- Tests: hand cases and a property (valid, normalised, inside the
  original selections).

### Task 7 — Config file, reload, frontend

- `editor/config.rs`: full error reporting (entry, reason), atomic
  apply of the user layer and user options, `Request::LoadConfig`.
- App: config path (`--config`, `$XDG_CONFIG_HOME`, `~/.config`),
  load at start and on reload; trace header config source and hash;
  `--replay --config`.
- Renderer: mode and pending keys on the status line; cursor shape per
  mode; restore the shape on exit.
- Acceptance test: a config rebinds and unbinds default keys in every
  mode and changes options at runtime without restart.

### Task 8 — Benchmarks, gate, review, report

- Latency rows (key resolution; insert-mode typing through the keymap),
  editor end-to-end bench, compared with Stage 6.
- Gate; review with attack tests and mutation checks; DECISIONS;
  `docs/stages/stage-7a.md`; STATUS.
