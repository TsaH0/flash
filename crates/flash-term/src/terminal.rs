//! Terminal lifecycle: raw mode, alternate screen, bracketed paste.
//!
//! [`enter`] and [`restore`] are idempotent enough to call around a
//! suspend; [`Guard`] restores on drop (normal exit and error paths);
//! [`install_panic_hook`] restores before the default panic message, so
//! the message is readable and the shell is usable.

use std::io::{self, Write};
use std::panic;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};

/// Raw mode, the alternate screen, and bracketed paste on.
pub fn enter() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    out.flush()
}

/// Best-effort restore of everything [`enter`] changed, plus the cursor;
/// safe to call more than once and when [`enter`] did not run.
pub fn restore() {
    let mut out = io::stdout();
    let _ = execute!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    );
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

/// Restores the terminal when dropped.
pub struct Guard(());

impl Guard {
    /// [`enter`]s the terminal; if that fails partway, restores it.
    pub fn enter() -> io::Result<Guard> {
        match enter() {
            Ok(()) => Ok(Guard(())),
            Err(e) => {
                restore();
                Err(e)
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Chains a panic hook that restores the terminal first.
pub fn install_panic_hook() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        default(info);
    }));
}
