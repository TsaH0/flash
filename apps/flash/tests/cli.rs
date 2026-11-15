//! The `flash` binary's non-interactive paths: usage, errors, and
//! `--replay`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use flash_core::trace::{
    TRACE_VERSION, TraceEntry, TraceHeader, TraceSource, TraceWriter, content_hash,
};
use flash_core::{Event, KeyCode, KeyEvent, Text};

fn flash() -> Command {
    Command::new(env!("CARGO_BIN_EXE_flash"))
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_trace(path: &PathBuf, header: &TraceHeader, events: &[Event]) {
    let mut w = TraceWriter::new(Vec::new(), header).unwrap();
    for (i, event) in events.iter().enumerate() {
        w.write(&TraceEntry {
            t_ns: i as u64 * 1_000,
            event: event.clone(),
        })
        .unwrap();
    }
    fs::write(path, w.into_inner().unwrap()).unwrap();
}

#[test]
fn usage_and_help() {
    let out = flash().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = flash().arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--replay"));
}

#[test]
fn replay_prints_latency_summary() {
    let dir = scratch("replay_prints_latency_summary");
    let doc = dir.join("doc.txt");
    fs::write(&doc, "hello\nworld\n").unwrap();
    let header = TraceHeader {
        flash_trace: TRACE_VERSION,
        document: TraceSource::Path(doc.display().to_string()),
        len: 12,
        hash: content_hash(&Text::from("hello\nworld\n")),
        size: (80, 24),
        cursor: 0,
    };
    let events = [
        Event::Key(KeyEvent::plain(KeyCode::Char('x'))),
        Event::Key(KeyEvent::plain(KeyCode::Down)),
        Event::Key(KeyEvent::plain(KeyCode::Backspace)),
    ];
    let trace = dir.join("session.jsonl");
    write_trace(&trace, &header, &events);

    let out = flash().arg("--replay").arg(&trace).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout.trim();
    assert!(
        line.starts_with(r#"{"source":"replay","events":3,"p50_ns":"#),
        "{line}"
    );
    assert_eq!(stdout.lines().count(), 1);
    // The document on disk is never modified by a replay.
    assert_eq!(fs::read_to_string(&doc).unwrap(), "hello\nworld\n");
}

#[test]
fn replay_rejects_bad_traces() {
    let dir = scratch("replay_rejects_bad_traces");
    let doc = dir.join("doc.txt");
    fs::write(&doc, "abc").unwrap();
    let header = TraceHeader {
        flash_trace: TRACE_VERSION,
        document: TraceSource::Path(doc.display().to_string()),
        len: 3,
        hash: 0,
        size: (80, 24),
        cursor: 0,
    };
    let mismatch = dir.join("mismatch.jsonl");
    write_trace(&mismatch, &header, &[]);
    let fixture = dir.join("fixture.jsonl");
    write_trace(
        &fixture,
        &TraceHeader {
            document: TraceSource::Fixture("medium".to_owned()),
            ..header.clone()
        },
        &[],
    );
    let garbage = dir.join("garbage.jsonl");
    fs::write(&garbage, "not json\n").unwrap();
    for (path, expect) in [
        (&mismatch, "does not match"),
        (&fixture, "fixture"),
        (&garbage, "line 1"),
        (&dir.join("missing.jsonl"), "missing.jsonl"),
    ] {
        let out = flash().arg("--replay").arg(path).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}: {stderr}", path.display());
        assert!(stderr.contains(expect), "{}: {stderr}", path.display());
    }
}

/// `FLASH_TRACE` or `FLASH_LATENCY_LOG` naming the document itself (or a
/// symlink to it) must not truncate or append to it: the skeleton cannot
/// save, so the file would be lost.
#[test]
fn output_files_never_clobber_the_document() {
    let dir = scratch("output_files_never_clobber_the_document");
    let doc = dir.join("doc.txt");
    let link = dir.join("link.txt");
    fs::write(&doc, "precious\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&doc, &link).unwrap();
    #[cfg(not(unix))]
    let link = doc.clone();
    for var in ["FLASH_TRACE", "FLASH_LATENCY_LOG"] {
        for target in [&doc, &link] {
            let out = flash()
                .arg(&doc)
                .env(var, target)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{var}={}: {stderr}",
                target.display()
            );
            assert!(stderr.contains("is the document"), "{var}: {stderr}");
            assert_eq!(fs::read_to_string(&doc).unwrap(), "precious\n", "{var}");
        }
    }
}
