# Stage 1 — Workspace, `Text`, test and benchmark foundations

Status: **complete, approved by the owner 2026-11-14.** Branch `stage-1`, local commits
only (nothing pushed).

## A. Status

| Requirement (PLAN Stage 1) | State |
|---|---|
| Workspace (flash-core, apps/flash; flash-term deferred to Stage 6 when it has content) | Done |
| crop 0.4.3 + `graphemes`, proptest, Criterion | Done |
| `Text`: byte_len, line count, byte↔line, line slice, range slice, chunks, char/grapheme boundary, counted grapheme motion, O(1) snapshot, `pub(crate)` mutators, `Result` errors | Done |
| Documented and tested line model | Done (`text/mod.rs` module docs) |
| String reference model | Done (`text/reference.rs`) |
| Property tests: empty, ASCII, multi-byte, combining, emoji/ZWJ, flags, CRLF split/join, long lines, many short lines, invalid offsets | Done |
| Deterministic fixtures, counting allocator | Done (`benches/common`, `benches/latency.rs`) |
| Benchmarks on small/medium/large/one-line/unicode, plus `utf16-metric` overhead | Done; results in `bench/results/` |

Deviation from the plan: `flash-term` is not created yet. An empty crate
would be noise; it arrives with the terminal code in Stage 6 (Stage 3's
walking skeleton lives in `apps/flash`).

## B. Commits

Branch `stage-1`, oldest first (`git log --oneline`):

| Commit | Content |
|---|---|
| `65f7ecb`, `6eec30b` | `.gitignore`, `docs/PLAN.md` |
| `22daac6`, `16bba36` | Workspace with `flash-core`; `apps/flash` placeholder |
| `f43fc45` | `Text`, `TextSlice`, `Chunks`, `TextBuilder`, `TextError` |
| `5967aea`, `e12c647`, `f5e6982` | Reference model; unit tests; property tests |
| `4ccbc46` | `internals` feature exposing mutators to benchmarks |
| `149b849`, `235f0de`, `615553d`, `5d08447`, `f6beaf8` | Fixtures, Criterion suite, latency/allocation harness, `bench/collect.py`, baseline results |
| `cdceba6` | Cross-thread snapshot test |
| `2e64d89`, `3b5b18e` | `docs/DECISIONS.md`, `docs/STATUS.md` |
| (closing commit) | Vendored crop with `Rope::chunk_at_byte`; allocation-free `is_grapheme_boundary`; `move_graphemes`; `len` renamed `byte_len`; `chunk_at` test; this report |

## C. Design decisions

Full entries in [DECISIONS.md](../DECISIONS.md): 001 Text inside flash-core
with `pub(crate)` mutators; 002 crop pinned `=0.4.3`; 003 cursor-row line
model; 004 own grapheme check; 005 every offset argument must be a char
boundary; 006 benchmark tooling; 007 `utf16-metric` off; 008 vendored crop
with `Rope::chunk_at_byte` and counted grapheme motion.

### Algorithm: `Text::is_grapheme_boundary`

- **Problem.** Cursor motion (Stage 3) must not move into the middle of a
  grapheme cluster. crop 0.4.3 offers `Rope::is_grapheme_boundary`, but it
  reaches the target chunk by iterating chunks backwards from the end of the
  rope, so it is O(distance to end of document).
- **Algorithm.** Reject invalid offsets; 0 and `len` are boundaries. Otherwise
  create `GraphemeCursor::new(offset, len, extended = true)`, give it the
  chunk containing `offset` (`Rope::chunk_at_byte`, DECISIONS 008), and
  whenever it returns `PreContext(end)`, feed it the chunk containing
  `end - 1`, cut at `end`.
- **Why correct.** `GraphemeCursor` is unicode-segmentation's incremental API
  designed for exactly this: it decides from the chunk containing the offset
  and asks for as much preceding context as the rules need (regional
  indicators, emoji ZWJ sequences, Indic conjuncts). Chunks are supplied
  contiguously and in order, which is its contract. Equivalence with
  `str::grapheme_indices` is property-tested, including with tiny leaves.
- **Complexity.** O(log n) per chunk looked up: O(1) chunks normally, O(k)
  for a run of k regional indicators. No heap allocation.
- **Rejected.** crop's method (O(n)); scanning a fixed window (incorrect for
  long regional-indicator runs).
- **Counted motion.** `Text::move_graphemes(offset, count, direction)` keeps
  one cursor and chunk across steps inside a chunk; a step that needs another
  chunk or context is redone with a fresh cursor (DECISIONS 008).

### Line model

`line_count = '\n' count + 1` (computed as crop's raw line metric at `len`).
`line_start(i)` is crop's `byte_of_line(i)`, safe because
`i ≤ '\n' count ≤ crop line_len`. The terminator is found by reading at most
two bytes before the next line's start. All O(log n).

## D. Invariants and how they are tested

| Invariant | Test |
|---|---|
| No public method panics for any offset, range, or line index | Proptests use offsets in `0..=len+2`, line indices `0..=line_count`; unit tests with `usize::MAX` |
| Results equal the reference model (`Result` included) | `edits_match_reference`, `edits_match_reference_long`, `builder_matches_reference` |
| Line model as documented | `line_model_*`, `edits_split_and_join_crlf`, reference model |
| Grapheme boundaries equal unicode-segmentation on `String` | Proptests on every offset; `grapheme_boundaries`; `grapheme_boundary_needs_context_across_chunks` (16 KB flag run) |
| Grapheme motion visits exactly the boundaries of unicode-segmentation | Proptests: whole-text walks plus counts 0/1/2/5 both ways from every checked offset; `move_graphemes_*` unit tests (flag runs of both parities across chunks, edited gap-buffer leaves) |
| Failed edits leave text unchanged | `failed_edits_leave_text_unchanged`; proptests compare full state after every op, valid or not |
| Snapshots independent of later edits | `snapshot_is_independent_of_later_edits`; proptest re-checks the first snapshot after all ops; cross-thread test |
| Chunks never empty and cover the text | `chunks_cover_text_and_are_never_empty`; checked after every proptest op |
| `chunk_at(b)` is the non-empty chunk containing `b`, matching `chunks()` at leaf and gap edges | `chunk_at_returns_the_chunk_containing_each_byte`; checked by injecting the `byte` vs `byte + 1` off-by-one into the vendored patch (fails with both leaf sizes) |

**Test sensitivity was checked by mutation.** Four bugs were injected one at a
time; each was caught by the property tests with a minimal shrunk input:
CRLF treated as a 1-byte terminator (`"\r\n"`), crop's line count used
directly (`"\n"`), grapheme check without context (`"e\u{301}"`), and insert
without the char-boundary check (`"😀"` plus an insert). The seeds are kept in
`proptest-regressions/`.

## E. Test results

Machine: Intel Core i7-1355U (12 threads), Linux 6.18.63, rustc 1.99.1.
Test results re-run 2026-11-14 on the closing commit's tree.
All commands used `--offline` (the sandbox blocks crates.io; all
dependencies were already in the local cache).

```
cargo fmt --all -- --check                                              ok
cargo clippy --workspace --all-targets -- -D warnings                   ok
cargo clippy --workspace --all-targets --all-features -- -D warnings    ok
cargo test --workspace                                                  26 passed
cargo test -p flash-core --release --features small-chunks              26 passed
PROPTEST_CASES=4096 cargo test -p flash-core --release                  passed
PROPTEST_CASES=4096 cargo test -p flash-core --release --features small-chunks   passed
```

## F. Benchmark results

Full data: `bench/results/stage-1-criterion.json` (72 benchmarks) and
`bench/results/stage-1-latency.jsonl`. Bench profile (release + debug
symbols), laptop on default power settings, not isolated: treat
differences under ~10% as noise.

Per-operation latency (custom harness, 100k samples each, ns):

| op | fixture | p50 | p99 | allocs/op |
|---|---|---|---|---|
| insert+delete char, unshared tree | 1 MB | 378 | 585 | 0 |
| insert+delete char, unshared tree | 100 MB | 1162 | 3109 | 1.3 |
| insert char, first edit after snapshot | 1 MB | 945 | 1326 | 18 |
| insert char, first edit after snapshot | 100 MB | 1809 | 2333 | 27 |
| replace ≤16 B after snapshot | 1 MB | 879 | 1232 | 14 |
| line_of_offset | 1 MB | 129 | 176 | 0 |
| line_range | 1 MB | 337 | 476 | 0 |
| is_grapheme_boundary (DECISIONS 008) | 1 MB | 101 | 174 | 0 |
| move_graphemes ×16 forward | 1 MB | 378 | 591 | 0 |
| snapshot + drop | any size | 26 | 28 | 0 |

Criterion highlights (mean):

- Snapshot: 14.5 ns at every size, so O(1) is confirmed.
- Chunk traversal: ~250 GiB/s (1 MB in 3.8 µs); 100 MB in 0.55 ms.
- Build from `&str`: 1 MB in 106 µs; 100 MB in 21 ms. Streaming in 64 KB
  pieces costs about 8% more.
- 1 MB single line performs the same as 1 MB of short lines for every
  operation.
- Grapheme boundary at offset 10: crop's method takes 33 ns (4 KB) and
  2,780 ns (1 MB), growing with document size; Flash's takes 133 ns and
  220 ns. crop is faster only on single-leaf texts.

The plan's target is a core transaction commit well under 100 µs p99 for
1 MB. A single rope edit is about 1 µs p99, leaving ample room for the
ChangeSet, selection, and history layers.

## G. Adversarial review

**Not done.** Gate step 3 (independent adversarial review) was not run for
this stage. The owner reviewed the closing diff and accepted closing Stage 1
without it (2026-11-14). The questions in section I remain open for a later
review.

## H. Known limitations

- `max` latencies (up to ~100 µs) are dominated by OS scheduling on a
  non-isolated laptop; they are reported but not meaningful yet.
- Timings under ~50 ns in the latency harness are bounded by `Instant::now`
  overhead.
- Invalid UTF-8, BOM, and line-ending detection are file-loading concerns
  (Stage 6); `Text` only ever holds valid UTF-8 `str` data.

## I. Questions for the reviewer

1. Can any public method of `Text`, `TextSlice`, `Chunks`, or `TextBuilder`
   panic?
2. Is there any text where `is_grapheme_boundary` disagrees with
   `unicode_segmentation` on the equivalent `String`?
3. Is the reference model obviously correct, and independent of `Text`'s
   implementation?
4. Do the benchmarks measure what their labels say?

## J. Next stage

Stage 2, ChangeSet algebra (high risk): builder with canonical form,
validation against `Text`, apply (descending per-op baseline versus a
`RopeBuilder` rebuild), invert, compose, `map_pos`/`map_sorted`, property
tests plus a fuzz target. Not started; waiting for approval.
