//! The standing benchmark (PLAN Stage 4): N views of one document, 10,000
//! selections each, one edit, complete commit.
//!
//! Run: `cargo bench -p flash-core --bench document`
//!
//! Each iteration builds `Transaction::insert(.., "x")` from the origin
//! view's selection and commits it through `Editor::commit` on a fresh
//! clone of the editor (text snapshot-shared): validate, map every other
//! view's 10,000 ranges, apply, bump the revision, log the change set.
//! `origin = one` is a one-cursor edit with 10,000-range bystanders;
//! `origin = 10k` is a 10,000-cursor edit. Later stages add their
//! subsystems (history, markers, highlighting, plugins) to this benchmark.

mod common;

use std::time::Duration;

use common::{Fixture, Layout, cursors};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use flash_core::{Editor, LineEnding, Selection, Text, Transaction, ViewId};

const VIEWS: [usize; 3] = [1, 2, 8];
const SELECTIONS: usize = 10_000;

/// An editor showing `fx` in `views` views: the origin (returned) with
/// `origin` cursors, every other view with `SELECTIONS` spread cursors.
fn editor(text: &Text, views: usize, origin: &Selection) -> (Editor, ViewId) {
    let mut ed = Editor::new(200, 60);
    let doc = ed.open(text.snapshot(), None);
    let bystanders = cursors(text, SELECTIONS, Layout::Spread);
    let mut origin_view = None;
    for i in 0..views {
        let view = ed.open_view(doc).unwrap();
        if i == 0 {
            ed.set_selection(view, origin.clone()).unwrap();
            origin_view = Some(view);
        } else {
            ed.set_selection(view, bystanders.clone()).unwrap();
        }
    }
    let origin_view = origin_view.unwrap();
    ed.set_focus(origin_view).unwrap();
    (ed, origin_view)
}

fn bench_fixture(c: &mut Criterion, fx: &Fixture) {
    let text = Text::from(fx.content.as_str());
    let middle = text.line_start(text.line_count() / 2).unwrap();
    let origins = [
        ("one", Selection::point(middle)),
        ("10k", cursors(&text, SELECTIONS, Layout::Spread)),
    ];
    let mut g = c.benchmark_group(format!("commit/{}", fx.name));
    if fx.content.len() > 10 * common::MB {
        g.sample_size(10);
    }
    for (label, origin) in &origins {
        for views in VIEWS {
            let (base, view) = editor(&text, views, origin);
            g.bench_with_input(
                BenchmarkId::new(format!("origin {label}"), views),
                &views,
                |b, _| {
                    b.iter_batched(
                        || base.clone(),
                        |mut ed| {
                            let doc = ed.view_document(view).unwrap().1;
                            let txn =
                                Transaction::insert(doc.text(), doc.selection(view).unwrap(), "x")
                                    .unwrap();
                            ed.commit(view, txn).unwrap();
                            ed
                        },
                        BatchSize::LargeInput,
                    )
                },
            );
        }
    }
    g.finish();

    let mut g = c.benchmark_group(format!("line_ending_detect/{}", fx.name));
    if fx.content.len() > 10 * common::MB {
        g.sample_size(10);
    }
    g.bench_function("detect", |b| b.iter(|| LineEnding::detect(&text)));
    g.finish();
}

fn documents(c: &mut Criterion) {
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
    targets = documents
}
criterion_main!(benches);
