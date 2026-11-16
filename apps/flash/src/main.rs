//! `flash [--tab-width N] [file]`: the terminal editor. `flash --replay
//! <trace>`: replays a recorded session without a terminal and prints its
//! latency.
//!
//! Keys (temporary until Stage 7; see `flash_core::editor::keys`): type to
//! insert; arrows, Ctrl+arrows, Home/End, PageUp/PageDown move (Shift
//! extends); Alt+Up/Down add a cursor; Esc keeps one; Alt+Z undo, Alt+Y
//! redo; Ctrl+C/X/V copy, cut, paste; Ctrl+S save, Ctrl+O open, Ctrl+W
//! close, Ctrl+PageUp/PageDown switch views; Ctrl+Z suspend; Ctrl+Q quit.
//!
//! Main loop (PLAN 2.8): an input thread and a signal thread feed one
//! channel; the loop blocks for one message, drains what is already
//! queued (up to a time budget), carries out the editor's requests, then
//! renders once.
//!
//! Environment:
//! - `FLASH_LATENCY_LOG=<file>`: on exit, append one JSON line with the
//!   per-event latency (event read → frame flushed) and its stages.
//! - `FLASH_TRACE=<file>`: record the session's input as a trace
//!   (`flash_core::trace`), replayable with `--replay`. The trace contains
//!   everything typed or pasted; the document is referenced by path,
//!   length, and hash, not copied.

use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, IsTerminal, Write};
use std::num::NonZeroU8;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event as ct;
use crossterm::terminal;
use flash_app::io::{Done, Io};
use flash_app::latency::{Stages, Timing, event_time};
use flash_app::requests::{self, Outcome, Settings};
use flash_app::searcher::Searcher;
use flash_core::trace::{
    self, DEFAULT_MAX_LINE, TRACE_VERSION, TraceEntry, TraceHeader, TraceReader, TraceSource,
    TraceWriter,
};
use flash_core::{Document, Editor, Request, Text, Timestamp, file};
use flash_term::{Renderer, convert, terminal as term};
use signal_hook::consts::signal::{SIGCONT, SIGHUP, SIGINT, SIGSTOP, SIGTERM, SIGTSTP};
use signal_hook::iterator::Signals;

const USAGE: &str = "usage: flash [--config <file>] [--tab-width N] [file]\n       \
                     flash --replay <trace> [--config <file>]\n       flash --help";

/// How long one batch may keep draining queued events before rendering.
const DRAIN_BUDGET: Duration = Duration::from_millis(8);

/// How long exit waits for a save still being written.
const IO_EXIT_WAIT: Duration = Duration::from_secs(30);

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match parse(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Replay { trace, config }) => replay(&trace, config).map(|()| Exit::Quit),
        Ok(Command::Edit {
            path,
            tab_width,
            config,
        }) => run(
            path.as_deref(),
            &Settings {
                config: Settings::config_path(config),
                tab_width,
            },
        ),
        Err(message) => {
            eprintln!("flash: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(Exit::Quit) => ExitCode::SUCCESS,
        Ok(Exit::Signal(sig)) => ExitCode::from(128u8.saturating_add(sig as u8)),
        Err(message) => {
            eprintln!("flash: {message}");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Help,
    Replay {
        trace: PathBuf,
        config: Option<PathBuf>,
    },
    Edit {
        path: Option<PathBuf>,
        tab_width: Option<NonZeroU8>,
        config: Option<PathBuf>,
    },
}

fn parse(args: &[OsString]) -> Result<Command, String> {
    let mut path = None;
    let mut tab_width = None;
    let mut config = None;
    let mut replay = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            Some("--replay") => {
                let trace = it.next().ok_or("--replay needs a trace file")?;
                replay = Some(PathBuf::from(trace));
            }
            Some("--config") => {
                let file = it.next().ok_or("--config needs a file")?;
                config = Some(PathBuf::from(file));
            }
            Some("--tab-width") => {
                let n = it.next().and_then(|n| n.to_str()?.parse::<u8>().ok());
                tab_width = Some(
                    n.and_then(NonZeroU8::new)
                        .ok_or("--tab-width needs a number from 1 to 255")?,
                );
            }
            Some(flag) if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return Err("only one file can be opened from the command line".to_owned()),
        }
    }
    match replay {
        Some(_) if path.is_some() || tab_width.is_some() => {
            Err("--replay takes only a trace file (and --config)".to_owned())
        }
        Some(trace) => Ok(Command::Replay { trace, config }),
        None => Ok(Command::Edit {
            path,
            tab_width,
            config,
        }),
    }
}

/// How the editor ended.
enum Exit {
    Quit,
    /// Terminated by this signal (exit status 128 + signal).
    Signal(i32),
}

/// One message to the main loop.
enum Msg {
    Input(ct::Event, Instant),
    InputError(io::Error),
    Signal(i32),
    /// A save or open finished on the I/O worker.
    Io(Done),
    /// A search finished on the search worker.
    Search(flash_core::SearchDone),
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// The document to start with: `path` loaded (a missing file becomes a
/// new empty document), or an empty scratch document. Refuses invalid
/// UTF-8 and non-regular files.
fn initial_document(path: Option<&Path>) -> Result<Document, String> {
    let Some(path) = path else {
        return Ok(Document::new(Text::default(), None));
    };
    match file::load(path) {
        Ok(loaded) => Ok(Document::from_loaded(loaded, path.to_path_buf())),
        Err(file::LoadError::NotFound) => {
            Ok(Document::new(Text::default(), Some(path.to_path_buf())))
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn run(path: Option<&Path>, settings: &Settings) -> Result<Exit, String> {
    if let Some(path) = path {
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
    }
    let document = initial_document(path)?;
    // crossterm would otherwise fall back to /dev/tty and take over
    // whatever terminal the process happens to have.
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("standard input and output must be a terminal".to_owned());
    }
    let (width, height) = terminal::size().unwrap_or((80, 24));
    // The config first: options such as the default line ending apply to
    // the document as it opens.
    let mut editor = Editor::new(width, height);
    let (_, config_hash) = requests::load_config(&mut editor, settings);

    let recorder = match env::var_os("FLASH_TRACE") {
        Some(trace_path) => {
            let header = TraceHeader {
                flash_trace: TRACE_VERSION,
                document: match path {
                    Some(p) => TraceSource::Path(p.display().to_string()),
                    None => TraceSource::Fixture("empty".to_owned()),
                },
                len: document.text().byte_len() as u64,
                hash: trace::content_hash(document.text()),
                size: (width, height),
                cursor: 0,
                config: config_hash,
            };
            let file = File::create(&trace_path)
                .map_err(|e| format!("{}: {e}", Path::new(&trace_path).display()))?;
            Some(TraceWriter::new(BufWriter::new(file), &header).map_err(|e| e.to_string())?)
        }
        None => None,
    };

    editor.open_document(document);

    let (tx, rx) = mpsc::channel();
    // Searches of large documents run here (latest wins).
    let searcher = {
        let tx = tx.clone();
        Searcher::spawn(move |done| tx.send(Msg::Search(done)).is_ok())
            .map_err(|e| format!("search thread: {e}"))?
    };
    // Saves and opens run here, off the main loop (PLAN 2.8).
    let mut io_worker = {
        let tx = tx.clone();
        Io::spawn(move |done| tx.send(Msg::Io(done)).is_ok())
            .map_err(|e| format!("I/O thread: {e}"))?
    };
    let mut signals = Signals::new([SIGTSTP, SIGCONT, SIGTERM, SIGHUP, SIGINT])
        .map_err(|e| format!("signal setup failed: {e}"))?;
    let signal_handle = signals.handle();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for sig in signals.forever() {
                if tx.send(Msg::Signal(sig)).is_err() {
                    break;
                }
            }
        });
    }

    term::install_panic_hook();
    let guard = term::Guard::enter().map_err(|e| format!("terminal setup failed: {e}"))?;
    thread::spawn(move || {
        loop {
            match ct::read() {
                Ok(event) => {
                    if tx.send(Msg::Input(event, Instant::now())).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::InputError(e));
                    break;
                }
            }
        }
    });

    let mut stages = Stages::default();
    let mut recording = Recording {
        writer: recorder,
        error: None,
    };
    let result = event_loop(
        &mut editor,
        &rx,
        &mut io_worker,
        &searcher,
        &mut stages,
        &mut recording,
        settings,
    );
    let Recording {
        writer: recorder,
        error: mut trace_error,
    } = recording;
    // A save still being written finishes before the process exits.
    let busy = io_worker.busy();
    let finished = io_worker.finish(IO_EXIT_WAIT);
    drop(guard);
    if busy && !finished {
        eprintln!("flash: a save was still running after {IO_EXIT_WAIT:?}; it may be incomplete");
    }
    signal_handle.close();

    if let Some(writer) = recorder {
        if let Err(e) = writer.into_inner() {
            trace_error.get_or_insert(e.to_string());
        }
    }
    if let Some(e) = trace_error {
        eprintln!("flash: trace recording stopped: {e}");
    }
    if let Some(log) = env::var_os("FLASH_LATENCY_LOG") {
        if let Some(line) = stages.to_json(r#""source":"interactive""#) {
            if let Err(e) = append_line(Path::new(&log), &line) {
                eprintln!("flash: could not write latency log: {e}");
            }
        }
    }
    result.map_err(|e| format!("terminal error: {e}"))
}

/// Restores the terminal, stops the process, and re-enters the terminal
/// when it is continued.
fn suspend() -> io::Result<()> {
    term::restore();
    signal_hook::low_level::raise(SIGSTOP)?;
    term::enter()
}

/// The trace being recorded (`FLASH_TRACE`), if any, and why recording
/// stopped, if it did.
struct Recording {
    writer: Option<TraceWriter<BufWriter<File>>>,
    error: Option<String>,
}

/// Writes `entries` to the trace (after the frame is flushed, so
/// recording is not in the latency). A write error stops recording.
fn record(entries: &mut Vec<TraceEntry>, recording: &mut Recording) {
    if let Some(writer) = &mut recording.writer {
        for entry in entries.iter() {
            if let Err(e) = writer.write(entry) {
                recording.error = Some(e.to_string());
                recording.writer = None;
                break;
            }
        }
    }
    entries.clear();
}

/// The loop: one message, then everything queued (within the budget),
/// then requests, then one frame. Every exit path records the events
/// already handled, so a trace ends with the key that quit.
fn event_loop(
    editor: &mut Editor,
    rx: &Receiver<Msg>,
    io_worker: &mut Io,
    searcher: &Searcher,
    stages: &mut Stages,
    recording: &mut Recording,
    settings: &Settings,
) -> io::Result<Exit> {
    let mut entries: Vec<TraceEntry> = Vec::new();
    let result = run_loop(
        editor,
        rx,
        Workers {
            io: io_worker,
            searcher,
        },
        stages,
        &mut entries,
        recording,
        settings,
    );
    record(&mut entries, recording);
    result
}

/// The threads the main loop hands work to.
struct Workers<'a> {
    io: &'a mut Io,
    searcher: &'a Searcher,
}

fn run_loop(
    editor: &mut Editor,
    rx: &Receiver<Msg>,
    workers: Workers<'_>,
    stages: &mut Stages,
    entries: &mut Vec<TraceEntry>,
    recording: &mut Recording,
    settings: &Settings,
) -> io::Result<Exit> {
    let session = Instant::now();
    let mut out = BufWriter::with_capacity(1 << 16, io::stdout());
    let mut renderer = Renderer::new();
    renderer.render(editor, 0, &mut out)?;
    out.flush()?;
    #[cfg(debug_assertions)]
    if env::var_os("FLASH_DEBUG_PANIC").is_some() {
        panic!("FLASH_DEBUG_PANIC: testing terminal restore");
    }
    // Per batch: (read, dequeued, command done, commit ns) per event.
    let mut batch: Vec<(Instant, Instant, Instant, u64)> = Vec::new();
    loop {
        let Ok(first) = rx.recv() else {
            return Ok(Exit::Quit);
        };
        let start = Instant::now();
        let mut redraw = false;
        let mut full_redraw = false;
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Input(raw, read_at) => {
                    let dequeued = Instant::now();
                    if let Some(event) = convert(&raw) {
                        // The time it was read, not now (PLAN 2.8).
                        let t_ns = event_time(session, read_at);
                        let commits_before = editor.last_commit_ns();
                        let fx = editor.handle_at(&event, Timestamp::from_nanos(t_ns));
                        let commit = if fx.text && editor.last_commit_ns() != commits_before {
                            editor.last_commit_ns()
                        } else {
                            0
                        };
                        batch.push((read_at, dequeued, Instant::now(), commit));
                        redraw |= fx.needs_redraw();
                        entries.push(TraceEntry { t_ns, event });
                    }
                }
                Msg::InputError(e) => return Err(e),
                Msg::Signal(SIGTSTP) => {
                    out.flush()?;
                    suspend()?;
                    full_redraw = true;
                }
                Msg::Signal(SIGCONT) => full_redraw = true,
                Msg::Signal(sig) => return Ok(Exit::Signal(sig)),
                Msg::Io(done) => redraw |= workers.io.complete(editor, done).needs_redraw(),
                Msg::Search(done) => redraw |= editor.search_done(done).needs_redraw(),
            }
            for request in editor.take_requests() {
                if let Request::Search(job) = request {
                    workers.searcher.submit(job);
                    continue;
                }
                let request = match workers.io.submit(editor, request) {
                    Ok(fx) => {
                        redraw |= fx.needs_redraw();
                        continue;
                    }
                    Err(request) => request,
                };
                match requests::execute(editor, request, &mut out, settings)? {
                    Outcome::Done(fx) => redraw |= fx.needs_redraw(),
                    Outcome::Suspend => {
                        out.flush()?;
                        suspend()?;
                        full_redraw = true;
                    }
                }
            }
            if editor.should_quit() {
                out.flush()?;
                return Ok(Exit::Quit);
            }
            if start.elapsed() < DRAIN_BUDGET {
                next = match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return Ok(Exit::Quit),
                };
            }
        }
        if full_redraw {
            // After a suspend the screen is the shell's: clear it all.
            crossterm::queue!(out, terminal::Clear(terminal::ClearType::All))?;
        }
        let (mut layout, mut flush, mut bytes) = (0, 0, 0);
        if redraw || full_redraw {
            let last = stages.last().map_or(0, |t| t.total);
            let t0 = Instant::now();
            let frame = renderer.frame(editor, last);
            let t1 = Instant::now();
            bytes = frame.len() as u64;
            out.write_all(frame)?;
            out.flush()?;
            layout = nanos(t1 - t0);
            flush = nanos(t1.elapsed());
        } else {
            out.flush()?;
        }
        let done = Instant::now();
        for (read, dequeued, handled, commit) in batch.drain(..) {
            stages.push(Timing {
                total: nanos(done - read),
                queue: nanos(dequeued - read),
                command: nanos(handled - dequeued),
                commit,
                layout,
                flush,
                bytes,
            });
        }
        record(entries, recording);
    }
}

/// Replays a trace headless: every event through the editor at its
/// recorded time, each frame rendered into a memory sink; prints one JSON
/// latency summary line. Requests are dropped: a replay never touches a
/// file or the terminal.
/// Replays a trace headless and prints latency. Keys mean what a config
/// binds them to: `config` (or the default keymap) is applied, and a
/// mismatch with the recorded config is reported on stderr.
fn replay(trace_path: &Path, config: Option<PathBuf>) -> Result<(), String> {
    let file = File::open(trace_path).map_err(|e| format!("{}: {e}", trace_path.display()))?;
    let at = |e: trace::TraceError| format!("{}: {e}", trace_path.display());
    let mut reader = TraceReader::new(BufReader::new(file), DEFAULT_MAX_LINE).map_err(at)?;
    let text = match &reader.header().document {
        TraceSource::Path(p) => {
            file::load(Path::new(p))
                .map_err(|e| format!("{p}: {e}"))?
                .text
        }
        TraceSource::Fixture(name) if name == "empty" => Text::default(),
        TraceSource::Fixture(name) => {
            return Err(format!(
                "{}: fixture trace ({name}); fixture traces replay in benches only",
                trace_path.display()
            ));
        }
    };
    let (mut editor, _) = trace::start(reader.header(), text).map_err(at)?;
    let settings = Settings {
        config,
        tab_width: None,
    };
    let (_, hash) = requests::load_config(&mut editor, &settings);
    if !editor.config_errors().is_empty() || hash != reader.header().config {
        let recorded = reader
            .header()
            .config
            .map_or("none".to_owned(), |h| format!("{h:x}"));
        let given = hash.map_or("none".to_owned(), |h| format!("{h:x}"));
        eprintln!(
            "flash: warning: the trace was recorded with config {recorded}, replaying with {given}{}",
            if editor.config_errors().is_empty() {
                ""
            } else {
                " (with skipped entries)"
            }
        );
    }
    let mut renderer = Renderer::new();
    let mut sink: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut stages = Stages::default();
    for entry in &mut reader {
        let entry = entry.map_err(at)?;
        sink.clear();
        let t0 = Instant::now();
        let mut fx = editor.handle_at(&entry.event, Timestamp::from_nanos(entry.t_ns));
        // Searches run here, synchronously, so the replay is exact; every
        // other request is dropped (a replay never touches a file).
        for request in editor.take_requests() {
            if let Request::Search(job) = request {
                fx = fx.or(editor.search_done(job.run()));
            }
        }
        let t1 = Instant::now();
        let (mut layout, mut flush, mut bytes) = (0, 0, 0);
        if fx.needs_redraw() {
            let frame = renderer.frame(&editor, 0);
            let t2 = Instant::now();
            bytes = frame.len() as u64;
            sink.extend_from_slice(frame);
            layout = nanos(t2 - t1);
            flush = nanos(t2.elapsed());
        }
        stages.push(Timing {
            total: nanos(t0.elapsed()),
            queue: 0,
            command: nanos(t1 - t0),
            commit: if fx.text { editor.last_commit_ns() } else { 0 },
            layout,
            flush,
            bytes,
        });
    }
    match stages.to_json(r#""source":"replay""#) {
        Some(line) => println!("{line}"),
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
