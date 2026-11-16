# Stage 6 design — minimal usable terminal editor

Date: 2026-11-16. Spec: `docs/PLAN.md` Stage 6, sections 2.2, 2.8, 2.9,
2.11, 3, 4, 6. Binding: DECISIONS 014 (commit is the only text mutator),
016 (traces are raw input), 017–018 (history, time as input).

Owner decisions (2026-11-16): **Ctrl+Z suspends** (PLAN, terminal
convention); undo moves to Alt+Z, redo to Alt+Y and Alt+Shift+Z.
Stages 4 and 5 approved. Everything marked **(call)** is the
implementer's and is listed again in the stage report.

## Intent

Turn the walking skeleton into an editor someone can use on real files:
open, edit, undo, copy and paste, save safely, quit without losing work,
in a terminal that is always restored. Keep the core headless and the
latency path measurable end to end.

**Goals:** the PLAN Stage 6 build list and tests.
**Non-goals:** keymaps, modes, options registry, search, soft wrap, a
diffing renderer (all Stage 7); crash recovery; file watching.
**Anti-goals:** any path that loses data silently (a save that destroys
the original on failure, a quit that drops unsaved changes without being
asked, a lossy UTF-8 load); a terminal left in raw mode; replay writing
files; core depending on a terminal crate.

## Approaches to file I/O (call)

1. **Requests (chosen).** The core never touches the disk, the clipboard,
   or the terminal. Commands that need the outside world (save, open,
   system clipboard, suspend) queue a `Request`; `Effects::requests`
   says some are pending; the binary performs them and reports back
   (`Editor::saved`, `Editor::opened`). Replay performs none, so a trace
   can never write files, and the core stays deterministic. The load and
   save algorithms still live in core (`flash_core::file`, std only) so
   they are unit-tested without a terminal.
2. Core performs I/O through a storage trait (real and in-memory). Two
   real implementations, but replay would need a fake disk to stay
   harmless, and every command would block inside `handle`.
3. Core performs I/O directly. Replay could overwrite files. Rejected.

## Design

### Crates (PLAN 2.2)

- `flash-core`: adds `file` (load, save, disk state), document disk
  state and modified tracking, the prompt, the register, requests, new
  keys.
- **`flash-term` (new):** crossterm → core event conversion, the
  renderer, OSC 52 encoding, and the terminal guard (raw mode, alternate
  screen, bracketed paste, restore). Replaces `apps/flash/src/skeleton.rs`.
- `apps/flash`: CLI, input thread, main loop, signals, request
  execution, latency log, trace recording and replay.

### Loading (`file::load`)

Stream the file in 64 KiB reads into `TextBuilder`, validating UTF-8
incrementally (an incomplete sequence at a read boundary is carried to
the next read). Invalid UTF-8 → `LoadError::InvalidUtf8 { offset }`, no
lossy conversion. A leading BOM is stripped and remembered. Line endings
are counted while streaming (dominant one; ties → LF, as
`LineEnding::detect`). Size, mtime, and FNV-1a 64 of the bytes on disk
(BOM included) are recorded as `DiskState`. Refuses anything that is not
a regular file (directories, devices, FIFOs). A missing file opens as an
empty document with that path and no disk state (call: vim's behaviour).

### Saving (`file::save`)

`save(path, text, bom, expected: Option<&DiskState>, force) ->
Result<Saved, SaveError>`:

1. Resolve the target: `canonicalize` if it exists (follows symlinks),
   else the canonical parent plus the file name.
2. Conflict check unless `force`: with `expected`, the target's size and
   mtime must match and then its hash (read once); without, the target
   must not exist. Mismatch → `SaveError::Conflict`, nothing written.
3. If the target has more than one hard link, or its owner or group
   differs from a new file's and `fchown` fails → **in-place**: copy the
   target to a backup file in the same directory (fsync), truncate and
   write the target, fsync, then remove the backup. `Saved::method` says
   so, and the message reports it. If the in-place write fails, the
   backup is kept and its path is in the error.
4. Otherwise **atomic**: create `.<name>.flash-<pid>-<n>.tmp` in the
   target's directory (`create_new`), write BOM and chunks, `sync_all`,
   copy permissions, `fchown` if needed, rename over the target, fsync
   the directory. Any failure before the rename removes the temp file;
   the original is untouched.
5. Return the new `DiskState` (size and mtime from the written file, hash
   computed while writing).

Tests inject failures through a test-only phase hook (call: a seam, not
a trait). Read-only directories fail with a clear error; no fallback
(call: in-place needs a backup in the same directory too).

### Documents: disk state and modified

`Document` gains `disk: Option<DiskState>`, `bom: bool`, and
`saved: Option<NodeId>` (the history node whose text is on disk).
`is_modified()` is `head != saved`, so undoing back to the saved state
reads as unmodified. Saving checkpoints history, so typing after a save
starts a new node (call; otherwise a merge into the saved node would hide
a modification).

### Editor: prompt, register, requests, keys

- **Prompt** (core, so traces cover it): one line with a label, input,
  and a cursor; kinds `Open`, `SaveAs`, `Overwrite` (yes/no after a
  conflict), `Quit` (unsaved changes). Keys go to the prompt while it is
  open: characters, Backspace, Left/Right/Home/End, Enter, Esc.
- **Register:** copy (Ctrl+C) stores each range's text; cut (Ctrl+X)
  stores and deletes; paste (Ctrl+V) inserts piece *i* at range *i*
  when the counts match, else the joined text at every range (call:
  Helix/Kakoune behaviour); a paste is its own undo step. Copy and cut
  also request the system clipboard (OSC 52) with the joined text.
  Nothing selected → message, nothing copied (call).
- **Requests:** `Save { doc, path, force }`, `Open { path }`,
  `Clipboard { text }`, `Suspend`. `Editor::saved(doc, result)` records
  success (disk state, path, saved node, message) or shows the error;
  `Conflict` opens the Overwrite prompt. `Editor::opened(path, result)`
  opens a document and focused view, or shows the error.
- **Keys** (temporary until Stage 7): Ctrl+S save (prompts for a path if
  the document has none), Ctrl+O open (prompt), Ctrl+W close view
  (asks if it is the last view of a modified document), Ctrl+Q quit (asks
  if any document is modified), Ctrl+Z suspend, Alt+Z undo, Alt+Y and
  Alt+Shift+Z redo, Ctrl+C/X/V copy/cut/paste, PageUp/PageDown by a
  screen, Ctrl+PageUp/PageDown previous/next view.
- **Views:** `View` gains `left` (first visible display column); the
  focused view scrolls horizontally so the primary head is visible
  (O(line length) on the head's line, as the status column already is).
  Tab width is an editor setting (`--tab-width`, default 4) used by
  motions, scrolling, and rendering (call: an option in Stage 7).

### Rendering (`flash-term`)

Full redraw per frame into a reused buffer, wrapped in synchronized-
update escapes, one write per frame (call: Stage 7 benchmarks diffing).
Visible lines from `top`, columns from `left`; graphemes measured with
`unicode-width` (existing `grapheme_width`), tabs expanded to the tab
width, control characters drawn as U+FFFD; a wide grapheme cut by either
edge is drawn as spaces. Selections reversed, secondary cursors
reversed, the primary cursor is the terminal cursor. Status line: name,
modified marker, cursor count, line:column, latency, message. The prompt
replaces the status line while open. Emoji width disagreements between
terminals are a documented limitation.

### Main loop (`apps/flash`, PLAN 2.8)

- An input thread runs `crossterm::event::read` and sends `Msg::Input`
  over one `mpsc` channel; a signal thread (`signal-hook`) sends
  `Msg::Signal` for SIGTSTP, SIGCONT, SIGTERM, SIGHUP, SIGINT.
- The loop blocks for one message, then drains every message already
  queued for up to 8 ms (call), applying each, then executes pending
  requests, then renders once if anything needs it.
- Suspend (Ctrl+Z or an external SIGTSTP): restore the terminal,
  `raise(SIGSTOP)`; on return (or SIGCONT) re-enter raw mode and the
  alternate screen and redraw. SIGTERM/SIGHUP/SIGINT: restore and exit
  (status 128 + signal); unsaved changes are not saved (not crash
  recovery).
- The terminal guard restores on normal exit, on error (drop), and from
  the panic hook.
- Instrumentation per event: dequeue, command done, commit done (from
  `Effects::commit_ns`, measured in core with `Instant`; time stays an
  input for state), layout done (frame built), flush done. The latency
  log gains per-stage percentiles; `--replay` reports the same stages.

### CLI

`flash [--tab-width N] [file]` (no file: an empty scratch document),
`flash --replay <trace>`. Invalid UTF-8 or a non-regular file: refuse
with a message, exit 1.

## Tests (PLAN Stage 6)

- Save paths in temp directories: new file, existing file (permissions
  kept), symlink (target written, link kept), hard link (in place, both
  names see it), read-only directory (error, original intact, still
  modified), changed on disk (conflict; force overwrites), simulated
  write failures at each phase (original intact, temp removed, backup
  kept for in-place).
- Load/save round-trip byte-identical for LF, CRLF, mixed, BOM, no final
  newline, empty; invalid UTF-8 at a read boundary refused; non-regular
  files refused.
- Editor: prompt flows, save/open/quit/close with modified documents,
  register (matching and mismatched counts), page and view keys,
  horizontal scroll, tab width.
- Renderer: tabs, wide graphemes at both edges, horizontal scroll,
  prompt line, selections; no raw control byte ever written; random
  events never panic.
- pty smoke test: run `flash` under a pseudo-terminal in a new session,
  type, save, quit; the file is saved and the terminal attributes,
  alternate screen, and cursor are restored; also after a panic
  (debug-only test hook) and after SIGTERM.

## Benchmarks

The skeleton bench becomes the editor bench (same trace pattern plus
horizontal scroll and the prompt); per-stage instrumentation in
`--replay`; load and save throughput on `medium` and `large`.

## New dependencies

`signal-hook` 0.3 (in the lock file via crossterm) for signals;
`tempfile` 3 (in the lock file via criterion) and `rustix` with `pty`,
`termios` features (in the lock file via crossterm) as dev-dependencies
for tests.
