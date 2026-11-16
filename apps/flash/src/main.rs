//! `flash <file>`: the walking skeleton in a real terminal, driven by the
//! headless editor. `flash --replay <trace>`: replays a recorded session
//! without a terminal and prints its latency.
//!
//! Keys: type to insert; Enter, Tab, Backspace, Delete; arrows move (Shift
//! extends); Ctrl+arrows move by word; Home/End and Ctrl+Home/End; Alt+Up /
//! Alt+Down add a cursor; Esc keeps only the primary cursor; Ctrl+Z undoes;
//! Ctrl+Y or Ctrl+Shift+Z redoes; Ctrl+Q quits. There is no save.
//!
//! Environment:
//! - `FLASH_LATENCY_LOG=<file>`: on exit, append one JSON line summarising
//!   the per-event latency (event read → frame flushed).
//! - `FLASH_TRACE=<file>`: record the session's input as a trace
//!   (`flash_core::trace`), replayable with `--replay`. The trace contains
//!   everything typed or pasted; the document itself is referenced by path,
//!   length, and hash, not copied.

use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::panic;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use flash_app::latency::summarize;
use flash_app::skeleton::{Skeleton, convert};
use flash_core::trace::{
    self, DEFAULT_MAX_LINE, TRACE_VERSION, TraceEntry, TraceHeader, TraceReader, TraceSource,
    TraceWriter,
};
use flash_core::{Editor, Text, Timestamp};

const USAGE: &str = "usage: flash <file>\n       flash --replay <trace>";

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [flag, trace] if flag == "--replay" => replay(Path::new(trace)),
        [path] if !path.to_string_lossy().starts_with("--") => run(Path::new(path)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("flash: {message}");
            ExitCode::FAILURE
        }
    }
}

fn now_ns(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn run(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for var in ["FLASH_TRACE", "FLASH_LATENCY_LOG"] {
        if let Some(out) = env::var_os(var) {
            if same_file(Path::new(&out), path) {
                return Err(format!(
                    "{var}={}: is the document; refusing to write to it",
                    Path::new(&out).display()
                ));
            }
        }
    }
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let text = Text::from(content);
    let name = path.display().to_string();

    let mut recorder = match env::var_os("FLASH_TRACE") {
        Some(trace_path) => {
            let header = TraceHeader {
                flash_trace: TRACE_VERSION,
                document: TraceSource::Path(name.clone()),
                len: text.byte_len() as u64,
                hash: trace::content_hash(&text),
                size: (width, height),
                cursor: 0,
            };
            let file = File::create(&trace_path)
                .map_err(|e| format!("{}: {e}", Path::new(&trace_path).display()))?;
            Some(TraceWriter::new(BufWriter::new(file), &header).map_err(|e| e.to_string())?)
        }
        None => None,
    };

    let mut editor = Editor::new(width, height);
    let doc = editor.open(text, Some(path.to_path_buf()));
    editor.open_view(doc).map_err(|e| e.to_string())?;
    let mut skeleton = Skeleton::new(name);

    let guard = TerminalGuard::enter().map_err(|e| format!("terminal setup failed: {e}"))?;
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let mut samples = Vec::new();
    let mut trace_error = None;
    let result = event_loop(
        &mut editor,
        &mut skeleton,
        &mut samples,
        &mut recorder,
        &mut trace_error,
    );
    drop(guard);

    if let Some(writer) = recorder {
        if let Err(e) = writer.into_inner() {
            trace_error.get_or_insert(e.to_string());
        }
    }
    if let Some(e) = trace_error {
        eprintln!("flash: trace recording stopped: {e}");
    }
    if let Some(log) = env::var_os("FLASH_LATENCY_LOG") {
        if let Some(summary) = summarize(&samples) {
            let line = summary.to_json(r#""source":"interactive""#);
            if let Err(e) = append_line(Path::new(&log), &line) {
                eprintln!("flash: could not write latency log: {e}");
            }
        }
    }
    result.map_err(|e| format!("terminal error: {e}"))
}

/// Reads events until quit. Each handled event's latency (read returned →
/// flush returned) goes into `samples`; each is recorded to the trace after
/// the frame is flushed, so recording is not part of the latency. A trace
/// write error stops recording and is reported on exit.
fn event_loop(
    editor: &mut Editor,
    skeleton: &mut Skeleton,
    samples: &mut Vec<u64>,
    recorder: &mut Option<TraceWriter<BufWriter<File>>>,
    trace_error: &mut Option<String>,
) -> io::Result<()> {
    let session = Instant::now();
    let mut out = BufWriter::new(io::stdout());
    skeleton.render(editor, &mut out)?;
    out.flush()?;
    loop {
        let raw = event::read()?;
        let start = Instant::now();
        let Some(event) = convert(&raw) else {
            continue;
        };
        let t_ns = now_ns(session);
        let effects = editor.handle_at(&event, Timestamp::from_nanos(t_ns));
        if !effects.quit && effects.needs_redraw() {
            skeleton.render(editor, &mut out)?;
            out.flush()?;
        }
        let nanos = now_ns(start);
        skeleton.set_last_latency(nanos);
        samples.push(nanos);
        if let Some(writer) = recorder {
            if let Err(e) = writer.write(&TraceEntry { t_ns, event }) {
                *trace_error = Some(e.to_string());
                *recorder = None;
            }
        }
        if editor.should_quit() {
            return Ok(());
        }
    }
}

/// Replays a trace headless: every event through the editor, each frame
/// rendered into a memory sink; prints one JSON latency summary line.
fn replay(trace_path: &Path) -> Result<(), String> {
    let file = File::open(trace_path).map_err(|e| format!("{}: {e}", trace_path.display()))?;
    let at = |e: trace::TraceError| format!("{}: {e}", trace_path.display());
    let mut reader = TraceReader::new(BufReader::new(file), DEFAULT_MAX_LINE).map_err(at)?;
    let doc_path = match &reader.header().document {
        TraceSource::Path(p) => p.clone(),
        TraceSource::Fixture(name) => {
            return Err(format!(
                "{}: fixture trace ({name}); fixture traces replay in benches only",
                trace_path.display()
            ));
        }
    };
    let content = fs::read_to_string(&doc_path).map_err(|e| format!("{doc_path}: {e}"))?;
    let (mut editor, _) = trace::start(reader.header(), Text::from(content)).map_err(at)?;
    let skeleton = Skeleton::new(doc_path);

    let mut sink: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut samples = Vec::new();
    for entry in &mut reader {
        let entry = entry.map_err(at)?;
        sink.clear();
        let start = Instant::now();
        let now = Timestamp::from_nanos(entry.t_ns);
        if editor.handle_at(&entry.event, now).needs_redraw() {
            skeleton
                .render(&editor, &mut sink)
                .map_err(|e| e.to_string())?;
        }
        samples.push(now_ns(start));
    }
    match summarize(&samples) {
        Some(summary) => println!("{}", summary.to_json(r#""source":"replay""#)),
        None => println!(r#"{{"source":"replay","events":0}}"#),
    }
    Ok(())
}

/// Whether `a` and `b` name the same existing file: equal canonical paths
/// (symlinks resolved) or, on Unix, the same device and inode (hard links).
fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        if x == y {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(x), Ok(y)) = (fs::metadata(a), fs::metadata(b)) {
            return x.dev() == y.dev() && x.ino() == y.ino();
        }
    }
    false
}

fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// Raw mode, alternate screen, and bracketed paste for its lifetime.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<TerminalGuard> {
        terminal::enable_raw_mode()?;
        let guard = TerminalGuard;
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Best-effort terminal restore; safe to call more than once.
fn restore_terminal() {
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}
