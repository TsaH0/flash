//! Per-operation latency distribution and allocation counts for `Text`.
//!
//! Criterion reports means; the editor cares about tails. This harness times
//! every operation individually and reports p50/p95/p99/max, plus the number
//! of heap allocations and bytes allocated per operation, using a counting
//! global allocator.
//!
//! Run: `cargo bench -p flash-core --features internals --bench latency`
//! Output: a table on stdout and JSON lines in `$FLASH_BENCH_JSON` if set.
//!
//! Timer resolution and the cost of `Instant::now` (~20 ns) are included in
//! every sample; treat results under ~50 ns as an upper bound.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use common::{Fixture, random_offsets, random_ranges};
use flash_core::internals::{changeset_apply, text_delete, text_insert, text_replace};
use flash_core::{ChangeSet, Direction, Text};

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method delegates to `System` with the caller's arguments
// unchanged, so `System`'s guarantees hold; the counters are side effects only.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size() as u64, Relaxed);
        // SAFETY: forwarded from the caller, who upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(new_size as u64, Relaxed);
        // SAFETY: forwarded from the caller, who upholds `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const ITERS: usize = 100_000;

struct Report {
    fixture: &'static str,
    op: &'static str,
    p50: u64,
    p95: u64,
    p99: u64,
    max: u64,
    allocs_per_op: f64,
    bytes_per_op: f64,
}

/// Runs `op(i)` ITERS times after a warm-up, timing each call. `setup(i)` runs
/// untimed and uncounted before each call.
fn measure<S>(
    fixture: &'static str,
    name: &'static str,
    mut setup: impl FnMut(usize) -> S,
    mut op: impl FnMut(&mut S),
) -> Report {
    for i in 0..ITERS / 10 {
        let mut s = setup(i);
        op(&mut s);
    }
    let mut samples = Vec::with_capacity(ITERS);
    let (mut allocs, mut bytes) = (0u64, 0u64);
    for i in 0..ITERS {
        let mut s = setup(i);
        let (a0, b0) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
        let start = Instant::now();
        op(&mut s);
        let elapsed = start.elapsed().as_nanos() as u64;
        allocs += ALLOCS.load(Relaxed) - a0;
        bytes += BYTES.load(Relaxed) - b0;
        samples.push(elapsed);
        drop(black_box(s));
    }
    samples.sort_unstable();
    let pct = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
    Report {
        fixture,
        op: name,
        p50: pct(0.50),
        p95: pct(0.95),
        p99: pct(0.99),
        max: *samples.last().unwrap(),
        allocs_per_op: allocs as f64 / ITERS as f64,
        bytes_per_op: bytes as f64 / ITERS as f64,
    }
}

fn run_fixture(fx: &Fixture) -> Vec<Report> {
    let name = fx.name;
    let base = Text::from(fx.content.as_str());
    let offsets = random_offsets(&base, ITERS, 30);
    let ranges = random_ranges(&base, ITERS, 16, 31);
    let lines = base.line_count();
    let mut out = Vec::new();

    // Uniquely owned: insert then delete one char (pair).
    // Built separately so its tree shares nothing with `base`.
    let mut owned = Text::from(fx.content.as_str());
    out.push(measure(
        name,
        "insert+delete char (owned)",
        |i| offsets[i],
        |&mut o| {
            text_insert(&mut owned, o, "x").unwrap();
            text_delete(&mut owned, o..o + 1).unwrap();
        },
    ));

    out.push(measure(
        name,
        "insert char (after snapshot)",
        |i| (base.snapshot(), offsets[i]),
        |(t, o)| text_insert(t, *o, "x").unwrap(),
    ));
    out.push(measure(
        name,
        "delete <=16B (after snapshot)",
        |i| (base.snapshot(), ranges[i].clone()),
        |(t, r)| text_delete(t, r.clone()).unwrap(),
    ));
    out.push(measure(
        name,
        "replace <=16B (after snapshot)",
        |i| (base.snapshot(), ranges[i].clone()),
        |(t, r)| text_replace(t, r.clone(), "replacemnt").unwrap(),
    ));
    out.push(measure(
        name,
        "line_of_offset",
        |i| offsets[i],
        |&mut o| {
            black_box(base.line_of_offset(o).unwrap());
        },
    ));
    out.push(measure(
        name,
        "line_range",
        |i| i * 7919 % lines,
        |&mut l| {
            black_box(base.line_range(l).unwrap());
        },
    ));
    out.push(measure(
        name,
        "is_grapheme_boundary",
        |i| offsets[i],
        |&mut o| {
            black_box(base.is_grapheme_boundary(o));
        },
    ));
    for (label, count, direction) in [
        ("move_graphemes 1 fwd", 1, Direction::Forward),
        ("move_graphemes 16 fwd", 16, Direction::Forward),
        ("move_graphemes 16 back", 16, Direction::Backward),
    ] {
        out.push(measure(
            name,
            label,
            |i| offsets[i],
            |&mut o| {
                black_box(base.move_graphemes(o, count, direction).unwrap());
            },
        ));
    }
    // One edit as a change set: validate + apply, on a snapshot-shared tree.
    let single_edits: Vec<ChangeSet> = ranges
        .iter()
        .map(|r| ChangeSet::from_edits(base.byte_len(), [(r.clone(), "x")]).unwrap())
        .collect();
    out.push(measure(
        name,
        "changeset 1 edit apply (after snapshot)",
        |i| (base.snapshot(), &single_edits[i]),
        |(t, c)| changeset_apply(c, t).unwrap(),
    ));
    out.push(measure(
        name,
        "snapshot+drop",
        |_| (),
        |_| {
            drop(black_box(base.snapshot()));
        },
    ));
    out
}

fn main() {
    // `cargo bench` passes `--bench`; ignore arguments.
    let fixtures = [
        common::small(),
        common::medium(),
        common::large(),
        common::one_line(),
        common::unicode(),
    ];
    println!(
        "{:<10} {:<32} {:>8} {:>8} {:>8} {:>10} {:>8} {:>10}",
        "fixture", "op (ns)", "p50", "p95", "p99", "max", "allocs", "bytes"
    );
    let mut json = std::env::var_os("FLASH_BENCH_JSON")
        .map(|p| std::fs::File::create(p).expect("create JSON output"));
    for fx in &fixtures {
        for r in run_fixture(fx) {
            println!(
                "{:<10} {:<32} {:>8} {:>8} {:>8} {:>10} {:>8.2} {:>10.1}",
                r.fixture, r.op, r.p50, r.p95, r.p99, r.max, r.allocs_per_op, r.bytes_per_op
            );
            if let Some(f) = json.as_mut() {
                writeln!(
                    f,
                    r#"{{"fixture":"{}","op":"{}","p50_ns":{},"p95_ns":{},"p99_ns":{},"max_ns":{},"allocs_per_op":{:.3},"bytes_per_op":{:.1}}}"#,
                    r.fixture, r.op, r.p50, r.p95, r.p99, r.max, r.allocs_per_op, r.bytes_per_op
                )
                .unwrap();
            }
        }
    }
}
