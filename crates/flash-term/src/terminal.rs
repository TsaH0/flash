//! Terminal lifecycle: raw mode, alternate screen, bracketed paste, and the
//! kitty keyboard protocol's disambiguation flag.
//!
//! [`enter`] and [`restore`] are idempotent enough to call around a
//! suspend; [`Guard`] restores on drop (normal exit and error paths);
//! [`install_panic_hook`] restores before the default panic message, so
//! the message is readable and the shell is usable.
//!
//! # Keyboard protocol
//!
//! Without the kitty protocol an escape sequence split across reads (a
//! slow link, a loaded machine) arrives as Esc followed by literal keys,
//! which in a modal editor leaves insert mode and runs commands. With its
//! disambiguate-escape-codes flag, Esc and modified keys arrive as
//! unambiguous `CSI … u` sequences. The flag is pushed unconditionally (no
//! support query: crossterm's blocks for up to 2 s on a terminal that does
//! not answer); terminals without the protocol ignore the push, a private
//! CSI sequence. [`set_kitty_keyboard`] turns it off (the `kitty-keyboard`
//! option), and every restore path pops it if it was pushed.

use std::io::{self, Write};
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

/// Whether [`enter`] pushes the keyboard flag.
static KITTY_WANTED: AtomicBool = AtomicBool::new(true);
/// Whether the flag is pushed now (so restore pops exactly once).
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);
/// Whether the terminal is entered (between [`enter`] and [`restore`]).
static ENTERED: AtomicBool = AtomicBool::new(false);

/// Raw mode, the alternate screen, bracketed paste, and (unless turned off)
/// the keyboard flag on.
pub fn enter() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    write_enter(&mut out, KITTY_WANTED.load(Ordering::Relaxed))?;
    if KITTY_WANTED.load(Ordering::Relaxed) {
        KITTY_PUSHED.store(true, Ordering::Relaxed);
    }
    ENTERED.store(true, Ordering::Relaxed);
    out.flush()
}

/// Best-effort restore of everything [`enter`] changed, plus the cursor;
/// safe to call more than once and when [`enter`] did not run.
pub fn restore() {
    ENTERED.store(false, Ordering::Relaxed);
    let pushed = KITTY_PUSHED.swap(false, Ordering::Relaxed);
    let mut out = io::stdout();
    let _ = write_restore(&mut out, pushed);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

/// Sets whether the keyboard flag is used; while the terminal is entered,
/// pushes or pops it at once (a config reload takes effect immediately).
pub fn set_kitty_keyboard(on: bool) -> io::Result<()> {
    KITTY_WANTED.store(on, Ordering::Relaxed);
    if !ENTERED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut out = io::stdout();
    if on && !KITTY_PUSHED.swap(true, Ordering::Relaxed) {
        execute!(out, push_flags())?;
    } else if !on && KITTY_PUSHED.swap(false, Ordering::Relaxed) {
        execute!(out, PopKeyboardEnhancementFlags)?;
    }
    Ok(())
}

fn push_flags() -> PushKeyboardEnhancementFlags {
    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
}

/// The escapes [`enter`] writes.
fn write_enter(out: &mut impl Write, kitty: bool) -> io::Result<()> {
    queue!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    if kitty {
        queue!(out, push_flags())?;
    }
    Ok(())
}

/// The escapes [`restore`] writes: the keyboard flag is popped first, while
/// still on the alternate screen (kitty keeps a stack per screen).
fn write_restore(out: &mut impl Write, pushed: bool) -> io::Result<()> {
    if pushed {
        queue!(out, PopKeyboardEnhancementFlags)?;
    }
    queue!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    const PUSH: &str = "\u{1b}[>1u";
    const POP: &str = "\u{1b}[<1u";

    fn text(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn enter_pushes_the_flag_only_when_wanted() {
        let on = text(|o| write_enter(o, true));
        assert!(on.contains("\u{1b}[?1049h") && on.ends_with(PUSH), "{on:?}");
        let off = text(|o| write_enter(o, false));
        assert!(!off.contains(PUSH), "{off:?}");
    }

    #[test]
    fn restore_pops_before_leaving_the_alternate_screen() {
        let on = text(|o| write_restore(o, true));
        let pop = on.find(POP).expect("popped");
        assert!(pop < on.find("\u{1b}[?1049l").unwrap(), "{on:?}");
        assert!(!text(|o| write_restore(o, false)).contains(POP));
    }
}
