//! End-to-end latency of the editor without a terminal.
//!
//! Replays a fixed key trace (written and read back through
//! `flash_core::trace`) through `Editor::handle_at` + `Renderer::render` into an
//! in-memory sink (cleared per event) and reports p50/p95/p99/max per
//! fixture and viewport. This is the dequeue-to-flush path minus the
//! terminal write itself (PLAN section 4, "End-to-end terminal path").
//!
//! Run: `cargo bench -p flash --bench editor`
//! Output: a table on stdout and JSON lines in `$FLASH_BENCH_JSON` if set.

#[path = "../../../crates/flash-core/benches/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::io::{Cursor, Write};
use std::time::Instant;

use common::Fixture;
use flash_app::latency::summarize;
use flash_core::trace::{
    self, DEFAULT_MAX_LINE, TRACE_VERSION, TraceEntry, TraceHeader, TraceReader, TraceSource,
    TraceWriter,
};
use flash_core::{Event, KeyCode, KeyEvent, Modifiers, Text, Timestamp};
use flash_term::Renderer;

const REPEATS: usize = 100;

/// The 20-event pattern: type, delete, move, newline, add a cursor.
fn pattern() -> Vec<Event> {
    let key = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
    let plain = |code| key(code, Modifiers::NONE);
    let mut v: Vec<Event> = (0..8).map(|_| plain(KeyCode::Char('x'))).collect();
    v.extend([
        plain(KeyCode::Backspace),
        plain(KeyCode::Backspace),
        plain(KeyCode::Right),
        plain(KeyCode::Left),
        plain(KeyCode::Down),
        plain(KeyCode::Down),
        plain(KeyCode::Up),
        key(KeyCode::Right, Modifiers::CTRL),
        plain(KeyCode::End),
        plain(KeyCode::Home),
        plain(KeyCode::Enter),
        key(KeyCode::Down, Modifiers::ALT),
    ]);
    v
}

/// The pattern repeated `REPEATS` times as a trace starting from the
/// middle line of `text`, written and read back so the bench drives the
/// editor from a real trace.
fn trace_for(fx: &Fixture, text: &Text, width: u16, height: u16) -> (TraceHeader, Vec<TraceEntry>) {
    let middle = text.line_start(text.line_count() / 2).unwrap();
    let header = TraceHeader {
        flash_trace: TRACE_VERSION,
        document: TraceSource::Fixture(fx.name.to_owned()),
        len: text.byte_len() as u64,
        hash: trace::content_hash(text),
        size: (width, height),
        cursor: middle as u64,
        config: None,
    };
    let events = pattern();
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    for (i, event) in events
        .iter()
        .cycle()
        .take(events.len() * REPEATS)
        .enumerate()
    {
        writer
            .write(&TraceEntry {
                // 100 ms apart: typing speed, so edits group (Stage 5).
                t_ns: i as u64 * 100_000_000,
                event: event.clone(),
            })
            .unwrap();
    }
    let bytes = writer.into_inner().unwrap();
    let reader = TraceReader::new(Cursor::new(bytes), DEFAULT_MAX_LINE).unwrap();
    let header = reader.header().clone();
    let entries = reader.collect::<Result<Vec<_>, _>>().unwrap();
    (header, entries)
}

fn run(fx: &Fixture, width: u16, height: u16) -> Vec<u64> {
    let text = Text::from(fx.content.as_str());
    let (header, entries) = trace_for(fx, &text, width, height);
    let (mut editor, _) = trace::start(&header, text).unwrap();
    let mut renderer = Renderer::new();
    let mut sink: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut samples = Vec::with_capacity(entries.len());
    // The pattern types: enter insert mode first (untimed), so the run
    // measures what Stage 6's did.
    editor.handle_at(
        &flash_core::Event::Key(KeyEvent::plain(KeyCode::Char('i'))),
        Timestamp::ZERO,
    );
    for entry in &entries {
        sink.clear();
        let start = Instant::now();
        editor.handle_at(&entry.event, Timestamp::from_nanos(entry.t_ns));
        renderer.render(&editor, 0, &mut sink).unwrap();
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
