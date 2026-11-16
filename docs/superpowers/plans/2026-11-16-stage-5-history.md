# Stage 5 — Undo, Redo, Branching History Implementation Plan

**Goal:** a branching undo tree per document, recorded by
`Document::commit`, with undo, redo, and atomic jumps that themselves go
through commit; typing grouped by composition; time supplied as input so
replay stays deterministic.

**Spec:** `docs/superpowers/specs/2026-11-16-stage-5-design.md`,
implementing `docs/PLAN.md` Stage 5. DECISIONS 009, 010, 013, 014, 015
are binding.

## Global constraints

- Edition 2024, MSRV 1.85; `#![forbid(unsafe_code)]` in `flash-core`.
- `--offline`; no new dependencies.
- No panic reachable from any public method. Unreachable errors:
  `debug_assert!` then return the error.
- Validate and compute everything (inverse, compositions, mapped
  selections) before the text changes.
- One local commit per task on `stage-5`. Never push. Never add
  `.claude/`, `.mcp.json`, `.gitmodules`. Messages end with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Gate as Stage 4: fmt; clippy plain and `--all-features` with
  `-D warnings`; workspace tests; `small-chunks`; `PROPTEST_CASES=4096`
  release plain and `small-chunks`; fuzz crate `cargo check`.
- Run piped commands with `set -o pipefail`.

## Review focus

1. Undo/redo after merges: the composed inverse applied to the head text
   gives exactly the parent's text (property against snapshots).
2. Jumps across branches are one commit; a failed jump changes nothing.
3. `last_child` after undo, redo, jump, and new branches (redo follows
   the branch last visited).
4. Groups never merge across views, pauses, motions, pastes, undo, or
   checkpoints; a merge only ever extends the newest node.
5. Time going backwards in a trace: no panic, times stay monotonic.

## Files

| File | Responsibility |
|---|---|
| `crates/flash-core/src/document/history.rs` | `Timestamp`, `NodeId`, `Node`, `History`, `Record`, lookups, `heap_bytes` |
| `crates/flash-core/src/document/mod.rs` | `commit_with`, `undo`, `redo`, `goto`, `checkpoint`; group closing in `set_selection` |
| `crates/flash-core/src/document/{history_tests,history_proptests}.rs` | Cases and the snapshot-model walk |
| `crates/flash-core/src/editor/{mod,keys}.rs` | `now`, `handle_at`, grouping policy, undo/redo commands and keys |
| `crates/flash-core/src/trace.rs` | `replay` passes `t_ns` |
| `apps/flash/src/main.rs` | `handle_at` with the session clock; key help |
| `crates/flash-core/benches/{document,latency,history}.rs` | Standing bench undo rows, latency rows, memory measurement |

## Tasks

### Task 0 — Branch, approval, design and plan
- [ ] `stage-5` from `61c46aa`. STATUS: Stage 5 in progress at the
  owner's request; Stage 4's approval status is left for the owner to
  record. Commit spec and plan.

### Task 1 — History tree and document integration
- [ ] RED: `document/history_tests.rs`: undo/redo single edit; undo all /
  redo all exact text per step; branch on edit-after-undo; goto across
  branches; redo follows last visited branch; grouping merges, and each
  boundary (pause, other view, `group: false`, `set_selection`,
  `checkpoint`, undo) splits; identity commits record nothing; bystander
  views mapped through the inverse; invoking view gets stored selection;
  revisions advance and the log records undo; failed goto changes
  nothing; time clamping; `earlier`/`later`/`at_time`.
- [ ] GREEN: `history.rs` + `Document` methods. Commit computes the
  inverse with `ChangeSet::invert` before applying.
- [ ] Commit `Add branching undo history to documents`.

### Task 2 — Properties
- [ ] `history_proptests.rs`: snapshot-model random walk (1–3 views,
  grouped/ungrouped edits with random time steps, undo, redo, goto,
  earlier, later, checkpoint, selection change); grouped vs ungrouped
  equivalence; run with `small-chunks` too.
- [ ] Commit `Property-test history against a snapshot model`.

### Task 3 — Editor, keys, traces, binary
- [ ] RED in `editor/tests.rs`: Ctrl+Z/Ctrl+Y/Ctrl+Shift+Z; typed word is
  one undo step; motion splits groups; paste is its own step; pause via
  `handle_at` splits; messages at the ends; random events include undo
  keys; trace replay reproduces grouping (`trace_tests.rs`).
- [ ] GREEN: editor `now`, `handle_at`, `undo`/`redo`/`goto`/`checkpoint`;
  keys; `trace::replay` with `t_ns`; binary uses `handle_at`.
- [ ] Commit `Undo and redo in the editor, keys, and timed replay`.

### Task 4 — Benchmarks and measurement
- [ ] Standing bench undo rows; latency rows; `benches/history.rs` (live
  bytes per node over long typing traces). Run, save results under
  `bench/results/stage-5-*`. Decide on a cap from the numbers.
- [ ] Commit `Benchmark history and measure bytes per node`.

### Task 5 — Gate, review, report
- [ ] Full gate; whole-branch review with attack tests and mutation
  checks; fixes; `docs/stages/stage-5.md`; DECISIONS; STATUS.
- [ ] Commit `Stage 5 report, review fixes and tests, status`.
