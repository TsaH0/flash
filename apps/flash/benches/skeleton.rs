//! End-to-end latency of the walking skeleton without a terminal.
//!
//! Replays a fixed key trace through `Skeleton::handle` + `Skeleton::render`
//! into an in-memory sink (cleared per event) and reports p50/p95/p99/max per
//! fixture and viewport. This is the dequeue-to-flush path minus the
//! terminal write itself (PLAN section 4, "End-to-end terminal path").
//!
//! Run: `cargo bench -p flash --bench skeleton`
//! Output: a table on stdout and JSON lines in `$FLASH_BENCH_JSON` if set.

#[path = "../../../crates/flash-core/benches/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use common::Fixture;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use flash_app::latency::summarize;
use flash_app::skeleton::Skeleton;
use flash_core::{Range, Selection, Text};

const REPEATS: usize = 100;

/// The 20-event pattern: type, delete, move, newline, add a cursor.
fn pattern() -> Vec<Event> {
    let key = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
    let plain = |code| key(code, KeyModifiers::NONE);
    let mut v: Vec<Event> = (0..8).map(|_| plain(KeyCode::Char('x'))).collect();
    v.extend([
        plain(KeyCode::Backspace),
        plain(KeyCode::Backspace),
        plain(KeyCode::Right),
        plain(KeyCode::Left),
        plain(KeyCode::Down),
        plain(KeyCode::Down),
        plain(KeyCode::Up),
        key(KeyCode::Right, KeyModifiers::CONTROL),
        plain(KeyCode::End),
        plain(KeyCode::Home),
        plain(KeyCode::Enter),
        key(KeyCode::Down, KeyModifiers::ALT),
    ]);
    v
}

fn run(fx: &Fixture, width: u16, height: u16) -> Vec<u64> {
    let text = Text::from(fx.content.as_str());
    let middle = text.line_start(text.line_count() / 2).unwrap();
    let mut skeleton = Skeleton::new(fx.name.to_owned(), text, width, height);
    skeleton
        .set_selection(Selection::single(Range::point(middle)))
        .unwrap();
    let events = pattern();
    let mut sink: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut samples = Vec::with_capacity(events.len() * REPEATS);
    for event in events.iter().cycle().take(events.len() * REPEATS) {
        sink.clear();
        let start = Instant::now();
        skeleton.handle(event);
        skeleton.render(&mut sink).unwrap();
        sink.flush().unwrap();
        samples.push(start.elapsed().as_nanos() as u64);
        black_box(&sink);
    }
    samples
}

fn main() {
    // `cargo bench` passes `--bench`; ignore arguments.
    let fixtures = [
        common::small(),
        common::medium(),
        common::one_line(),
        common::unicode(),
    ];
    println!(
        "{:<10} {:<9} {:>8} {:>8} {:>8} {:>10}",
        "fixture", "viewport", "p50 ns", "p95 ns", "p99 ns", "max ns"
    );
    let mut json = std::env::var_os("FLASH_BENCH_JSON")
        .map(|p| std::fs::File::create(p).expect("create JSON output"));
    for fx in &fixtures {
        for (w, h) in [(80, 24), (200, 60)] {
            let s = summarize(&run(fx, w, h)).expect("trace is non-empty");
            let viewport = format!("{w}x{h}");
            println!(
                "{:<10} {:<9} {:>8} {:>8} {:>8} {:>10}",
                fx.name, viewport, s.p50, s.p95, s.p99, s.max
            );
            if let Some(f) = json.as_mut() {
                let fields = format!(
                    r#""fixture":"{}","viewport":"{viewport}","op":"skeleton event""#,
                    fx.name
                );
                writeln!(f, "{}", s.to_json(&fields)).unwrap();
            }
        }
    }
}
