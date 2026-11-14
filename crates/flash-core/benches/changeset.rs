//! Criterion micro-benchmarks for `ChangeSet`.
//!
//! Run: `cargo bench -p flash-core --features internals --bench changeset`
//!
//! Each change set has `n` edits spread over the fixture, each replacing up
//! to 16 bytes with `"x"`. `apply` runs on a fresh snapshot of the fixture
//! every iteration, so the tree is shared and the first edit on each path
//! copies it: the realistic case, since documents keep snapshots.

mod common;

use std::hint::black_box;
use std::ops::Range;
use std::time::Duration;

use common::{Fixture, random_offsets, random_ranges};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use flash_core::internals::changeset_apply;
use flash_core::{Assoc, ChangeSet, Text};

const EDIT_COUNTS: [usize; 5] = [1, 100, 1_000, 3_000, 10_000];
const MAPPED_POSITIONS: usize = 1_000;

/// Up to `n` sorted, non-overlapping edits on `text`, each replacing at most
/// 16 bytes with `"x"`.
fn spread_edits(text: &Text, n: usize, seed: u64) -> Vec<(Range<usize>, &'static str)> {
    let mut ranges = random_ranges(text, n, 16, seed);
    ranges.sort_by_key(|r| r.start);
    let mut prev_end = 0;
    ranges.retain(|r| {
        let keep = r.start >= prev_end;
        if keep {
            prev_end = r.end;
        }
        keep
    });
    ranges.into_iter().map(|r| (r, "x")).collect()
}

fn changeset(text: &Text, n: usize, seed: u64) -> ChangeSet {
    ChangeSet::from_edits(text.byte_len(), spread_edits(text, n, seed)).unwrap()
}

fn bench_fixture(c: &mut Criterion, fx: &Fixture) {
    let text = Text::from(fx.content.as_str());
    let mut g = c.benchmark_group(format!("changeset/{}", fx.name));
    if fx.content.len() > 10 * common::MB {
        g.sample_size(10);
    }

    for n in EDIT_COUNTS {
        let edits = spread_edits(&text, n, 40);
        let cs = ChangeSet::from_edits(text.byte_len(), edits.iter().cloned()).unwrap();

        g.bench_with_input(BenchmarkId::new("build", n), &edits, |b, edits| {
            b.iter(|| ChangeSet::from_edits(text.byte_len(), edits.iter().cloned()).unwrap())
        });
        g.bench_with_input(BenchmarkId::new("apply", n), &cs, |b, cs| {
            b.iter_batched(
                || text.snapshot(),
                |mut t| {
                    changeset_apply(cs, &mut t).unwrap();
                    t
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_with_input(BenchmarkId::new("invert", n), &cs, |b, cs| {
            b.iter(|| cs.invert(&text).unwrap())
        });

        let mut after = text.snapshot();
        changeset_apply(&cs, &mut after).unwrap();
        let next = changeset(&after, n, 41);
        g.bench_with_input(
            BenchmarkId::new("compose", n),
            &(&cs, &next),
            |b, (a, n)| b.iter(|| a.compose(n).unwrap()),
        );

        let mut positions = random_offsets(&text, MAPPED_POSITIONS, 42);
        positions.sort_unstable();
        g.bench_with_input(BenchmarkId::new("map_sorted", n), &cs, |b, cs| {
            b.iter_batched_ref(
                || positions.clone(),
                |ps| cs.map_sorted(black_box(ps), Assoc::After).unwrap(),
                BatchSize::SmallInput,
            )
        });
    }
    g.finish();
}

fn changesets(c: &mut Criterion) {
    for fx in [common::medium(), common::large()] {
        bench_fixture(c, &fx);
    }
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
    targets = changesets
}
criterion_main!(benches);
