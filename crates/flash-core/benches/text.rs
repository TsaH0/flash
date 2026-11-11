//! Criterion micro-benchmarks for `Text`.
//!
//! Run: `cargo bench -p flash-core --features internals --bench text`
//! UTF-16 metric overhead: run once with `-- --save-baseline plain`, then with
//! `--features internals,utf16 -- --baseline plain`.

mod common;

use std::hint::black_box;
use std::time::Duration;

use common::{Fixture, random_offsets, random_ranges};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flash_core::Text;
use flash_core::internals::{text_delete, text_insert, text_replace};

const N: usize = 1024;

fn edit_fixtures() -> Vec<Fixture> {
    vec![
        common::small(),
        common::medium(),
        common::large(),
        common::one_line(),
        common::unicode(),
    ]
}

/// Cycles through precomputed inputs so each iteration hits a new position.
struct Cycle<T> {
    items: Vec<T>,
    i: usize,
}

impl<T: Clone> Cycle<T> {
    fn new(items: Vec<T>) -> Self {
        Self { items, i: 0 }
    }
    fn next(&mut self) -> T {
        let item = self.items[self.i].clone();
        self.i = (self.i + 1) % self.items.len();
        item
    }
}

fn edits(c: &mut Criterion) {
    for fx in edit_fixtures() {
        let text = Text::from(fx.content.as_str());
        let mut g = c.benchmark_group(format!("edit/{}", fx.name));

        // Uniquely owned text: insert one char then delete it, so the fixture
        // does not grow. Reported time is for the pair.
        // Built separately so its tree shares nothing with `text`.
        let mut t = Text::from(fx.content.as_str());
        let mut offsets = Cycle::new(random_offsets(&t, N, 10));
        g.bench_function("insert+delete char (owned)", |b| {
            b.iter(|| {
                let o = offsets.next();
                text_insert(&mut t, o, "x").unwrap();
                text_delete(&mut t, o..o + 1).unwrap();
            })
        });

        // Edit on a text whose tree is shared with a snapshot: the edit must
        // copy the path it touches. This is the cost of the first edit after
        // handing a snapshot to a background worker.
        let mut offsets = Cycle::new(random_offsets(&text, N, 11));
        g.bench_function("insert char (after snapshot)", |b| {
            b.iter_batched_ref(
                || (text.snapshot(), offsets.next()),
                |(t, o)| text_insert(t, *o, "x").unwrap(),
                BatchSize::SmallInput,
            )
        });

        for (label, max_len) in [
            ("delete <=16B (after snapshot)", 16),
            ("delete <=4KB (after snapshot)", 4096),
        ] {
            let mut ranges = Cycle::new(random_ranges(&text, N, max_len, 12));
            g.bench_function(label, |b| {
                b.iter_batched_ref(
                    || (text.snapshot(), ranges.next()),
                    |(t, r)| text_delete(t, r.clone()).unwrap(),
                    BatchSize::SmallInput,
                )
            });
        }

        let mut ranges = Cycle::new(random_ranges(&text, N, 16, 13));
        g.bench_function("replace <=16B with 10B (after snapshot)", |b| {
            b.iter_batched_ref(
                || (text.snapshot(), ranges.next()),
                |(t, r)| text_replace(t, r.clone(), "replacemnt").unwrap(),
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

fn queries(c: &mut Criterion) {
    for fx in [
        common::small(),
        common::medium(),
        common::large(),
        common::unicode(),
    ] {
        let text = Text::from(fx.content.as_str());
        let mut g = c.benchmark_group(format!("query/{}", fx.name));

        let mut offsets = Cycle::new(random_offsets(&text, N, 20));
        g.bench_function("line_of_offset", |b| {
            b.iter(|| text.line_of_offset(offsets.next()).unwrap())
        });

        let lines = text.line_count();
        let mut line_idx = Cycle::new((0..N).map(|i| i * 7919 % lines).collect());
        g.bench_function("line_start", |b| {
            b.iter(|| text.line_start(line_idx.next()).unwrap())
        });
        g.bench_function("line_range", |b| {
            b.iter(|| text.line_range(line_idx.next()).unwrap())
        });

        g.bench_function("check_offset", |b| {
            b.iter(|| text.check_offset(offsets.next()))
        });
        g.bench_function("is_grapheme_boundary", |b| {
            b.iter(|| text.is_grapheme_boundary(offsets.next()))
        });

        g.bench_function("snapshot", |b| b.iter(|| black_box(text.snapshot())));
        g.finish();
    }
}

/// Documents why Text does not use crop's grapheme check: crop's cost grows
/// with the distance from the offset to the end of the rope.
fn grapheme_crop_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("grapheme_boundary_at_start");
    for fx in [common::small(), common::medium(), common::large()] {
        let rope = crop::Rope::from(fx.content.as_str());
        let text = Text::from(fx.content.as_str());
        let offset = 10;
        g.bench_with_input(BenchmarkId::new("flash Text", fx.name), &offset, |b, &o| {
            b.iter(|| text.is_grapheme_boundary(black_box(o)))
        });
        if fx.name != "large" {
            g.bench_with_input(BenchmarkId::new("crop Rope", fx.name), &offset, |b, &o| {
                b.iter(|| rope.is_grapheme_boundary(black_box(o)))
            });
        }
    }
    g.finish();
}

fn traversal(c: &mut Criterion) {
    let mut g = c.benchmark_group("traverse");
    for fx in [
        common::empty(),
        common::small(),
        common::medium(),
        common::large(),
        common::unicode(),
    ] {
        let text = Text::from(fx.content.as_str());
        g.throughput(Throughput::Bytes(fx.content.len() as u64));
        g.bench_function(BenchmarkId::new("chunks", fx.name), |b| {
            b.iter(|| text.chunks().map(str::len).sum::<usize>())
        });
        g.bench_function(BenchmarkId::new("chunks reversed", fx.name), |b| {
            b.iter(|| text.chunks().rev().map(str::len).sum::<usize>())
        });
    }
    g.finish();
}

fn build(c: &mut Criterion) {
    let mut g = c.benchmark_group("build");
    g.sample_size(10);
    for fx in [
        common::small(),
        common::medium(),
        common::large(),
        common::one_line(),
    ] {
        g.throughput(Throughput::Bytes(fx.content.len() as u64));
        g.bench_function(BenchmarkId::new("from &str", fx.name), |b| {
            b.iter(|| Text::from(black_box(fx.content.as_str())))
        });
        // Streaming in 64 KB pieces, as a file loader would.
        g.bench_function(BenchmarkId::new("builder 64KB pieces", fx.name), |b| {
            b.iter(|| {
                let mut builder = flash_core::TextBuilder::new();
                let mut rest = fx.content.as_str();
                while !rest.is_empty() {
                    let mut cut = rest.len().min(64 * 1024);
                    while !rest.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    builder.append(&rest[..cut]);
                    rest = &rest[cut..];
                }
                builder.build()
            })
        });
    }
    g.finish();
}

fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .noise_threshold(0.03)
}

criterion_group! {
    name = benches;
    config = config();
    targets = edits, queries, grapheme_crop_comparison, traversal, build
}
criterion_main!(benches);
