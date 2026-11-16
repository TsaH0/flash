# Stage 6 — Minimal Usable Terminal Editor Implementation Plan

**Goal:** a usable terminal editor over the headless core: safe loading
and atomic saving, prompt, register and OSC 52, suspend, a real main loop
with coalescing, a renderer with horizontal scroll and tabs, and a
terminal that is always restored.

**Spec:** `docs/superpowers/specs/2026-11-16-stage-6-design.md`,
implementing `docs/PLAN.md` Stage 6.

## Global constraints

- Edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` in `flash-core`,
  `flash-term`, and `flash_app`.
- `--offline`. New dependencies exactly: `signal-hook = "0.3"` (app);
  dev-dependencies `tempfile = "3"` (core, app) and `rustix = { version =
  "1", features = ["pty", "termios", "fs"] }` (app tests). Nothing else.
- `flash-core` depends on no terminal crate and performs no I/O outside
  `file`; `Editor::handle` never touches the disk.
- No panic reachable from input, a file, or a trace. No data loss: every
  save path leaves the original intact on failure.
- One local commit per task on `stage-6`; never push; never add
  `.claude/`, `.mcp.json`, `.gitmodules`; trailer `Co-Authored-By: Claude
  Opus 5.5 <noreply@anthropic.com>`. `set -o pipefail` on pipes.
- Gate as Stage 5, plus `cargo test -p flash-term` and the pty tests.

## Review focus

1. Every save failure point: original bytes intact, temp file removed,
   buffer still modified, backup kept when in place.
2. Symlinks and hard links: the link survives, the target gets the
   bytes.
3. Conflict detection when size and mtime match but content differs.
4. Prompt state cannot leak keys into the document, and Esc always
   leaves it.
5. Terminal restore on every exit path, including panic and signals.
6. Replay never writes a file.

## Tasks

### Task 0 — Approvals, design, plan
- [ ] STATUS and stage reports: Stages 4 and 5 approved by the owner.
  Commit with the spec and this plan.

### Task 1 — `flash_core::file`: load and save
- [ ] RED: `file/tests.rs`: round trips (LF, CRLF, mixed, BOM, no final
  newline, empty, multi-chunk), invalid UTF-8 (including split across a
  read boundary), non-regular files, missing file; save to new, existing
  (mode kept), symlink, hard link (in place), read-only directory,
  conflict (size, mtime, same-size content change), force, and injected
  failures at each phase.
- [ ] GREEN: `file/mod.rs` (`DiskState`, `Loaded`, `load`, `save`,
  `Saved`, `SaveMethod`, errors), `tempfile` dev-dependency.
- [ ] Commit `Add file loading and atomic saving`.

### Task 2 — Editor: disk state, prompt, register, requests, keys
- [ ] RED: editor tests for prompt flows, save/open/quit/close with
  modified documents, conflict → overwrite prompt, register paste
  (matching / mismatched counts), page keys, view cycling, horizontal
  scroll, tab width, Alt+Z / Alt+Y undo/redo, Ctrl+Z suspend request,
  `Transaction::paste`.
- [ ] GREEN: `Document` disk state and `is_modified`; `editor/prompt.rs`;
  requests; keys; `View::left`; tab width; `Effects::commit_ns`.
- [ ] Commit `Editor prompt, register, file requests, and Stage 6 keys`.

### Task 3 — `flash-term`
- [ ] Crate with `convert`, `Renderer` (frame buffer reuse, synchronized
  update, horizontal scroll, tabs, wide graphemes at edges, prompt line,
  status), `osc52`, `TerminalGuard`. Port and extend the skeleton tests;
  delete `skeleton.rs`.
- [ ] Commit `Add flash-term: renderer, input conversion, terminal guard`.

### Task 4 — The binary
- [ ] Input and signal threads, one channel, drain-then-render, request
  execution, suspend/resume, signals, panic hook, per-stage
  instrumentation, CLI (`--tab-width`, optional file), trace recording
  and replay through the same path.
- [ ] pty smoke tests (save and quit; panic hook; SIGTERM) and CLI tests.
- [ ] Commit `Main loop, signals, suspend, file requests, pty tests`.

### Task 5 — Benchmarks, gate, review, report
- [ ] Editor bench (ex skeleton), per-stage replay numbers, load/save
  throughput; results in `bench/results/stage-6-*`.
- [ ] Gate, review with attack tests and mutation checks, report,
  DECISIONS, STATUS.
- [ ] Commit `Stage 6 report, review fixes and tests, status`.
