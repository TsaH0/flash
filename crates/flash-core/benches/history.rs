//! History memory and latency over long typing sessions (PLAN Stage 5:
//! "measure bytes per history node over a long typing trace").
//!
//! Each scenario drives a fresh editor on the `medium` fixture through
//! `Editor::handle_at` with synthetic session times, and reports:
//! - live heap bytes added by the session (a counting allocator: allocated
//!   minus freed), which includes the revision log (bounded at 1,024
//!   entries) as well as history;
//! - `History::heap_bytes()` (the history's own estimate) and nodes, so
//!   bytes per node and per event;
//! - p50/p99/max per event (ns), to show cost does not grow with history.
//!
//! Run: `cargo bench -p flash-core --bench history`
//! Output: a table on stdout and JSON lines in `$FLASH_BENCH_JSON` if set.

mod common;

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::hint::black_box;
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
use std::time::Instant;

use common::{Layout, Rng, cursors};
use flash_core::{Editor, Event, KeyCode, KeyEvent, Modifiers, Text, Timestamp};

struct Live;

static LIVE: AtomicI64 = AtomicI64::new(0);

// SAFETY: every method delegates to `System` with the caller's arguments
// unchanged, so `System`'s guarantees hold; the counter is a side effect.
unsafe impl GlobalAlloc for Live {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as i64, Relaxed);
        // SAFETY: forwarded from the caller, who upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        LIVE.fetch_sub(layout.size() as i64, Relaxed);
        // SAFETY: `ptr` was allocated by `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as i64 - layout.size() as i64, Relaxed);
        // SAFETY: forwarded from the caller, who upholds `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Live = Live;

const MS: u64 = 1_000_000;

/// A typing session: words of 2–8 letters at 80 ms per key, a space after
/// each, Enter every 12 words, a pause of `pause_ms` after every sentence
/// (8 words), a Backspace 1 key in 20 and an arrow key 1 in 40.
fn session(events: usize, pause_ms: u64, seed: u64) -> Vec<(u64, Event)> {
    let mut rng = Rng::new(seed);
    let key = |code| Event::Key(KeyEvent::new(code, Modifiers::NONE));
    let mut out = Vec::with_capacity(events);
    let mut t = 0;
    let mut words = 0;
    while out.len() < events {
        for _ in 0..2 + rng.below(7) {
            t += 80 * MS;
            let event = match rng.below(40) {
                0 => key(KeyCode::Left),
                1 => key(KeyCode::Right),
                2 | 3 => key(KeyCode::Backspace),
                _ => key(KeyCode::Char((b'a' + rng.below(26) as u8) as char)),
            };
            out.push((t, event));
        }
        words += 1;
        t += 80 * MS;
        let end = if words % 12 == 0 {
            KeyCode::Enter
        } else {
            KeyCode::Char(' ')
        };
        out.push((t, key(end)));
        if words % 8 == 0 {
            t += pause_ms * MS;
        }
    }
    out.truncate(events);
    out
}

struct Report {
    scenario: String,
    events: usize,
    nodes: usize,
    live_bytes: i64,
    history_bytes: usize,
    p50: u64,
    p99: u64,
    max: u64,
}

fn run(scenario: String, text: &Text, cursor_count: usize, events: &[(u64, Event)]) -> Report {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(text.snapshot(), None);
    let view = ed.open_view(doc).unwrap();
    let start_sel = if cursor_count == 1 {
        flash_core::Selection::point(text.line_start(text.line_count() / 2).unwrap())
    } else {
        cursors(text, cursor_count, Layout::Spread)
    };
    ed.set_selection(view, start_sel).unwrap();
    let live0 = LIVE.load(Relaxed);
    let history0 = ed.document(doc).unwrap().history().heap_bytes();
    let mut samples = Vec::with_capacity(events.len());
    for (t, event) in events {
        let start = Instant::now();
        black_box(ed.handle_at(event, Timestamp::from_nanos(*t)));
        samples.push(start.elapsed().as_nanos() as u64);
    }
    let live = LIVE.load(Relaxed) - live0;
    let history = ed.document(doc).unwrap().history();
    samples.sort_unstable();
    let pct = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
    Report {
        scenario,
        events: events.len(),
        nodes: history.len() - 1,
        live_bytes: live,
        history_bytes: history.heap_bytes() - history0,
        p50: pct(0.5),
        p99: pct(0.99),
        max: *samples.last().unwrap(),
    }
}

fn main() {
    // `cargo bench` passes `--bench`; ignore arguments.
    let fx = common::medium();
    let text = Text::from(fx.content.as_str());
    let mut reports = Vec::new();
    // Single cursor grouped by sentence pauses; then the worst case, every
    // key its own step (keys 2 s apart); then multi-cursor worst cases.
    for (label, events, pause_ms) in [
        ("1 cursor, grouped", 100_000, 1_500),
        ("1 cursor, grouped", 1_000_000, 1_500),
    ] {
        let s = session(events, pause_ms, 7);
        reports.push(run(format!("{label}, {events} keys"), &text, 1, &s));
    }
    let ungrouped: Vec<(u64, Event)> = session(100_000, 0, 7)
        .into_iter()
        .enumerate()
        .map(|(i, (_, e))| (i as u64 * 2_000 * MS, e))
        .collect();
    reports.push(run(
        "1 cursor, every key a step, 100000 keys".to_owned(),
        &text,
        1,
        &ungrouped,
    ));
    for (cursors, events) in [(100, 10_000), (10_000, 200)] {
        let s: Vec<(u64, Event)> = session(events, 0, 11)
            .into_iter()
            .enumerate()
            .map(|(i, (_, e))| (i as u64 * 2_000 * MS, e))
            .collect();
        reports.push(run(
            format!("{cursors} cursors, every key a step, {events} keys"),
            &text,
            cursors,
            &s,
        ));
    }

    println!(
        "{:<48} {:>8} {:>8} {:>12} {:>12} {:>10} {:>9} {:>9} {:>10}",
        "scenario",
        "events",
        "nodes",
        "live B",
        "history B",
        "B/node",
        "p50 ns",
        "p99 ns",
        "max ns"
    );
    let mut json = std::env::var_os("FLASH_BENCH_JSON")
        .map(|p| std::fs::File::create(p).expect("create JSON output"));
    for r in &reports {
        let per_node = r.history_bytes as f64 / r.nodes.max(1) as f64;
        println!(
            "{:<48} {:>8} {:>8} {:>12} {:>12} {:>10.1} {:>9} {:>9} {:>10}",
            r.scenario,
            r.events,
            r.nodes,
            r.live_bytes,
            r.history_bytes,
            per_node,
            r.p50,
            r.p99,
            r.max
        );
        if let Some(f) = json.as_mut() {
            writeln!(
                f,
                r#"{{"fixture":"{}","scenario":"{}","events":{},"nodes":{},"live_bytes":{},"history_bytes":{},"bytes_per_node":{:.1},"p50_ns":{},"p99_ns":{},"max_ns":{}}}"#,
                fx.name, r.scenario, r.events, r.nodes, r.live_bytes, r.history_bytes, per_node, r.p50, r.p99, r.max
            )
            .unwrap();
        }
    }
}
