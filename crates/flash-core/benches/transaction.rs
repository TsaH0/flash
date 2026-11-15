//! Criterion benchmarks for multi-cursor transactions (PLAN Stage 3).
//!
//! Run: `cargo bench -p flash-core --features internals --bench transaction`
//!
//! Each iteration builds and applies one transaction on a fresh snapshot of
//! the fixture and a copy of the selection: build, validate, apply, and the
//! new selection. `map` applies a one-edit change set with no selection, so
//! it measures mapping `n` ranges.

mod common;

use std::time::Duration;

use common::{Fixture, Layout, cursors};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use flash_core::internals::transaction_apply;
use flash_core::{ChangeSet, Text, Transaction};

const CURSORS: [usize; 5] = [1, 10, 100, 1_000, 10_000];

fn bench_fixture(c: &mut Criterion, fx: &Fixture) {
    let text = Text::from(fx.content.as_str());
    let len = text.byte_len();
    let one_edit = ChangeSet::from_edits(len, [(len / 2..len / 2, "x")]).unwrap();
    for layout in [Layout::Clustered, Layout::Spread] {
        let mut g = c.benchmark_group(format!("transaction/{}/{}", fx.name, layout.name()));
        if fx.content.len() > 10 * common::MB {
            g.sample_size(10);
        }
        for n in CURSORS {
            let sel = cursors(&text, n, layout);
            let setup = || (text.snapshot(), sel.clone());
            g.bench_with_input(BenchmarkId::new("insert", n), &n, |b, _| {
                b.iter_batched(
                    setup,
                    |(mut t, mut s)| {
                        let txn = Transaction::insert(&t, &s, "x").unwrap();
                        transaction_apply(&txn, &mut t, &mut s).unwrap();
                        (t, s)
                    },
                    BatchSize::SmallInput,
                )
            });
            g.bench_with_input(BenchmarkId::new("backspace", n), &n, |b, _| {
                b.iter_batched(
                    setup,
                    |(mut t, mut s)| {
                        let txn = Transaction::backspace(&t, &s).unwrap();
                        transaction_apply(&txn, &mut t, &mut s).unwrap();
                        (t, s)
                    },
                    BatchSize::SmallInput,
                )
            });
            let map = Transaction::new(one_edit.clone(), None);
            g.bench_with_input(BenchmarkId::new("map", n), &n, |b, _| {
                b.iter_batched(
                    setup,
                    |(mut t, mut s)| {
                        transaction_apply(&map, &mut t, &mut s).unwrap();
                        (t, s)
                    },
                    BatchSize::SmallInput,
                )
            });
        }
        g.finish();
    }
}

fn transactions(c: &mut Criterion) {
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
    targets = transactions
}
criterion_main!(benches);
