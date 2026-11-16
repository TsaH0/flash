//! Load and save throughput on the `medium` (1 MB) and `large` (100 MB)
//! fixtures, in a temp directory (PLAN Stage 6).
//!
//! Each run: `file::load` (stream, validate UTF-8, hash, count line
//! endings), and `file::save` atomically (temp file, fsync, rename, fsync
//! the directory) with the conflict check (hash of the old file). The
//! page cache is warm after the first load; fsync dominates saves.
//!
//! Run: `cargo bench -p flash-core --bench file`
//! Output: a table on stdout and JSON lines in `$FLASH_BENCH_JSON` if set.

mod common;

use std::io::Write;
use std::time::Instant;

use flash_core::file;

const RUNS: usize = 7;

fn median(mut v: Vec<u64>) -> u64 {
    v.sort_unstable();
    v[v.len() / 2]
}

fn main() {
    let dir = std::env::temp_dir().join(format!("flash-file-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut json = std::env::var_os("FLASH_BENCH_JSON")
        .map(|p| std::fs::File::create(p).expect("create JSON output"));
    println!(
        "{:<8} {:<26} {:>12} {:>10}",
        "fixture", "op", "median µs", "MB/s"
    );
    for fx in [common::medium(), common::large()] {
        let path = dir.join(fx.name);
        std::fs::write(&path, &fx.content).unwrap();
        let mb = fx.content.len() as f64 / (1 << 20) as f64;
        let mut loads = Vec::new();
        let mut loaded = None;
        for _ in 0..RUNS {
            let t = Instant::now();
            let l = file::load(&path).unwrap();
            loads.push(t.elapsed().as_nanos() as u64);
            loaded = Some(l);
        }
        let loaded = loaded.unwrap();
        let mut saves = Vec::new();
        let mut disk = loaded.disk;
        for _ in 0..RUNS {
            let t = Instant::now();
            let saved = file::save(&path, &loaded.text, loaded.bom, Some(&disk), false).unwrap();
            saves.push(t.elapsed().as_nanos() as u64);
            disk = saved.disk;
        }
        for (op, ns) in [
            ("load", median(loads)),
            ("save (atomic, checked)", median(saves)),
        ] {
            let rate = mb / (ns as f64 / 1e9);
            println!("{:<8} {:<26} {:>12} {:>10.0}", fx.name, op, ns / 1000, rate);
            if let Some(f) = json.as_mut() {
                writeln!(
                    f,
                    r#"{{"fixture":"{}","op":"{op}","median_ns":{ns},"mb_per_s":{rate:.1}}}"#,
                    fx.name
                )
                .unwrap();
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
