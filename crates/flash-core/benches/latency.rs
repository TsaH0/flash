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
use flash_core::internals::{
    changeset_apply, text_delete, text_insert, text_replace, transaction_apply,
};
use std::num::NonZeroU8;

use flash_core::{
    ChangeSet, Direction, Editor, Event, KeyCode, KeyEvent, ModeId, Motion, Movement, Range,
    Selection, Text, Timestamp, Transaction, ViewId, parse_key,
};

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
/// Per-operation time budget and the fewest samples kept (see `measure`).
const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
const MIN_SAMPLES: usize = 1_000;
/// Distinct starting editors for the editor operations (see `run_fixture`).
const PREPARED: usize = 1_000;

struct Report {
    fixture: &'static str,
    op: &'static str,
    p50: u64,
    p95: u64,
    p99: u64,
    max: u64,
    samples: usize,
    allocs_per_op: f64,
    bytes_per_op: f64,
}

/// Runs `op(i)` ITERS times after a warm-up, timing each call, but stops
/// after BUDGET once it has MIN_SAMPLES (an operation that takes
/// milliseconds, such as a vertical motion on `one-line`, would otherwise
/// take most of an hour); the report says how many samples it took.
/// `setup(i)` runs untimed and uncounted before each call.
fn measure<S>(
    fixture: &'static str,
    name: &'static str,
    mut setup: impl FnMut(usize) -> S,
    mut op: impl FnMut(&mut S),
) -> Report {
    let started = Instant::now();
    for i in 0..ITERS / 10 {
        if started.elapsed() > BUDGET / 10 {
            break;
        }
        let mut s = setup(i);
        op(&mut s);
    }
    let started = Instant::now();
    let mut samples = Vec::with_capacity(ITERS);
    let (mut allocs, mut bytes) = (0u64, 0u64);
    for i in 0..ITERS {
        if samples.len() >= MIN_SAMPLES && started.elapsed() > BUDGET {
            break;
        }
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
        samples: samples.len(),
        allocs_per_op: allocs as f64 / samples.len() as f64,
        bytes_per_op: bytes as f64 / samples.len() as f64,
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
    // Stage 3: one-cursor transactions (build + validate + apply + new
    // selection) on a snapshot-shared tree, and motions of one range.
    out.push(measure(
        name,
        "txn insert char (1 cursor)",
        |i| (base.snapshot(), Selection::point(offsets[i])),
        |(t, s)| {
            let txn = Transaction::insert(t, s, "x").unwrap();
            transaction_apply(&txn, t, s).unwrap();
        },
    ));
    out.push(measure(
        name,
        "txn backspace (1 cursor)",
        |i| (base.snapshot(), Selection::point(offsets[i])),
        |(t, s)| {
            let txn = Transaction::backspace(t, s).unwrap();
            transaction_apply(&txn, t, s).unwrap();
        },
    ));
    // Stage 4: the same edit through Document::commit (via the editor),
    // and keys through Editor::handle. Each sample starts from a clone of a
    // base editor (untimed): text snapshot-shared, empty revision log.
    let editor_with = |views: usize| {
        let mut ed = Editor::new(80, 24);
        let doc = ed.open(base.snapshot(), None);
        let ids: Vec<ViewId> = (0..views).map(|_| ed.open_view(doc).unwrap()).collect();
        // Insert mode: typing and point motions, as in Stage 6.
        for &id in &ids {
            ed.set_mode(id, ModeId::INSERT);
        }
        ed.set_focus(ids[0]).unwrap();
        (ed, ids)
    };
    let (one_view, one_ids) = editor_with(1);
    let (two_views, two_ids) = editor_with(2);
    // Editors already at the first PREPARED random offsets (Stage 6: a
    // view scrolls to its head, and on `one-line` reaching a random offset
    // walks the line, which 100,000 setups would repeat for hours). Each
    // sample clones one: O(1), text shared.
    let one_at: Vec<Editor> = (0..PREPARED)
        .map(|i| {
            let mut ed = one_view.clone();
            ed.set_selection(one_ids[0], Selection::point(offsets[i]))
                .unwrap();
            ed
        })
        .collect();
    let two_at: Vec<Editor> = (0..PREPARED)
        .map(|i| {
            let mut ed = two_views.clone();
            ed.set_selection(two_ids[0], Selection::point(offsets[i]))
                .unwrap();
            ed.set_selection(two_ids[1], Selection::point(offsets[i + 1]))
                .unwrap();
            ed
        })
        .collect();
    out.push(measure(
        name,
        "doc commit insert char (1 view)",
        |i| one_at[i % PREPARED].clone(),
        |ed| {
            let v = one_ids[0];
            let doc = ed.view_document(v).unwrap().1;
            let txn = Transaction::insert(doc.text(), doc.selection(v).unwrap(), "x").unwrap();
            ed.commit(v, txn).unwrap();
        },
    ));
    out.push(measure(
        name,
        "doc commit insert char (2 views)",
        |i| two_at[i % PREPARED].clone(),
        |ed| {
            let v = two_ids[0];
            let doc = ed.view_document(v).unwrap().1;
            let txn = Transaction::insert(doc.text(), doc.selection(v).unwrap(), "x").unwrap();
            ed.commit(v, txn).unwrap();
        },
    ));
    for (label, key) in [
        ("editor handle key 'x'", KeyCode::Char('x')),
        ("editor handle Right", KeyCode::Right),
    ] {
        let event = Event::Key(KeyEvent::plain(key));
        out.push(measure(
            name,
            label,
            |i| one_at[i % PREPARED].clone(),
            |ed| {
                black_box(ed.handle(&event));
            },
        ));
    }
    // Stage 7: key resolution alone (the normal-mode keymap trie), and
    // keys through the keymap in normal mode (block cursor, mode minimum,
    // hooks), from the same random offsets.
    for (label, keys) in [
        ("key resolve 'l'", &["l"][..]),
        ("key resolve 'g g' (2nd key)", &["g", "g"][..]),
        ("key resolve miss", &["F5"][..]),
    ] {
        let keys: Vec<KeyEvent> = keys.iter().map(|k| parse_key(k).unwrap()).collect();
        let (last, prefix) = keys.split_last().unwrap();
        out.push(measure(
            name,
            label,
            |_| {
                let mut map = one_view.keymap().clone();
                for &k in prefix {
                    map.feed(ModeId::NORMAL, k, Timestamp::ZERO, u64::MAX);
                }
                map
            },
            |map| {
                black_box(map.feed(ModeId::NORMAL, *last, Timestamp::ZERO, u64::MAX));
            },
        ));
    }
    for (label, key) in [
        ("editor normal 'l' (block)", "l"),
        ("editor normal 'w' (select word)", "w"),
    ] {
        let event = Event::Key(parse_key(key).unwrap());
        out.push(measure(
            name,
            label,
            |i| {
                let mut ed = one_at[i % PREPARED].clone();
                ed.set_mode(one_ids[0], ModeId::NORMAL);
                ed
            },
            |ed| {
                black_box(ed.handle(&event));
            },
        ));
    }
    // Stage 5: history. Typing into an open undo group composes the new
    // edit into the group (100 earlier keystrokes); undo and redo of one
    // character commit the stored change sets.
    let mut grouped = one_view.clone();
    let v = one_ids[0];
    grouped
        .set_selection(v, Selection::point(offsets[0]))
        .unwrap();
    for i in 0..100u64 {
        grouped.handle_at(
            &Event::Key(KeyEvent::plain(KeyCode::Char('x'))),
            Timestamp::from_nanos(i),
        );
    }
    let typed = Event::Key(KeyEvent::plain(KeyCode::Char('y')));
    out.push(measure(
        name,
        "editor key into open group (100)",
        |_| grouped.clone(),
        |ed| {
            black_box(ed.handle_at(&typed, Timestamp::from_nanos(100)));
        },
    ));
    let with_edit = |i: usize| {
        let mut ed = one_at[i % PREPARED].clone();
        let doc = ed.view_document(v).unwrap().1;
        let txn = Transaction::insert(doc.text(), doc.selection(v).unwrap(), "x").unwrap();
        ed.commit(v, txn).unwrap();
        ed
    };
    out.push(measure(name, "undo char", with_edit, |ed| {
        ed.undo(v).unwrap().unwrap();
    }));
    out.push(measure(
        name,
        "redo char",
        |i| {
            let mut ed = with_edit(i);
            ed.undo(v).unwrap();
            ed
        },
        |ed| {
            ed.redo(v).unwrap().unwrap();
        },
    ));
    let tab = NonZeroU8::new(4).unwrap();
    for (label, motion) in [
        (
            "motion right",
            Motion::Grapheme {
                count: 1,
                direction: Direction::Forward,
            },
        ),
        (
            "motion down",
            Motion::Vertical {
                count: 1,
                direction: Direction::Forward,
                tab_width: tab,
            },
        ),
        (
            "motion word forward",
            Motion::Word {
                count: 1,
                direction: Direction::Forward,
            },
        ),
    ] {
        out.push(measure(
            name,
            label,
            |i| Range::point(offsets[i]),
            |r| {
                black_box(motion.apply_range(&base, *r, Movement::Move).unwrap());
            },
        ));
    }
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
        "{:<10} {:<34} {:>8} {:>8} {:>8} {:>10} {:>8} {:>10}",
        "fixture", "op (ns)", "p50", "p95", "p99", "max", "allocs", "bytes"
    );
    let mut json = std::env::var_os("FLASH_BENCH_JSON")
        .map(|p| std::fs::File::create(p).expect("create JSON output"));
    for fx in &fixtures {
        for r in run_fixture(fx) {
            println!(
                "{:<10} {:<34} {:>8} {:>8} {:>8} {:>10} {:>8.2} {:>10.1}",
                r.fixture, r.op, r.p50, r.p95, r.p99, r.max, r.allocs_per_op, r.bytes_per_op
            );
            if let Some(f) = json.as_mut() {
                writeln!(
                    f,
                    r#"{{"fixture":"{}","op":"{}","samples":{},"p50_ns":{},"p95_ns":{},"p99_ns":{},"max_ns":{},"allocs_per_op":{:.3},"bytes_per_op":{:.1}}}"#,
                    r.fixture, r.op, r.samples, r.p50, r.p95, r.p99, r.max, r.allocs_per_op, r.bytes_per_op
                )
                .unwrap();
            }
        }
    }
}
