//! `flash <file>`: the Stage 3 walking skeleton in a real terminal.
//!
//! Keys: type to insert; Enter, Tab, Backspace, Delete; arrows move (Shift
//! extends); Ctrl+arrows move by word; Home/End and Ctrl+Home/End; Alt+Up /
//! Alt+Down add a cursor; Esc keeps only the primary cursor; Ctrl+Q quits.
//! There is no save.
//!
//! If `FLASH_LATENCY_LOG` names a file, one JSON line summarising the
//! per-event latency (event read → frame flushed) is appended to it on exit.

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::panic;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use flash_app::latency::summarize;
use flash_app::skeleton::Skeleton;
use flash_core::Text;

const USAGE: &str = "usage: flash <file>";

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        [path] => match run(Path::new(path)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("flash: {message}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let mut skeleton = Skeleton::new(
        path.display().to_string(),
        Text::from(content),
        width,
        height,
    );

    let guard = TerminalGuard::enter().map_err(|e| format!("terminal setup failed: {e}"))?;
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let mut samples = Vec::new();
    let result = event_loop(&mut skeleton, &mut samples);
    drop(guard);

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

/// Reads events until quit; each event's latency (read returned → flush
/// returned) goes into `samples`.
fn event_loop(skeleton: &mut Skeleton, samples: &mut Vec<u64>) -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout());
    skeleton.render(&mut out)?;
    out.flush()?;
    loop {
        let event = event::read()?;
        let start = Instant::now();
        skeleton.handle(&event);
        if skeleton.should_quit() {
            return Ok(());
        }
        skeleton.render(&mut out)?;
        out.flush()?;
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        skeleton.set_last_latency(nanos);
        samples.push(nanos);
    }
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
