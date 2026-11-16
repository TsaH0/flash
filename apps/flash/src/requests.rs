//! Carrying out the editor's requests: the only place the binary touches
//! files on the editor's behalf.

use std::io::{self, Write};

use flash_core::{Editor, Effects, Request, file};
use flash_term::osc52;

/// What a request needs from the caller beyond what [`execute`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Done; these are the effects on the editor.
    Done(Effects),
    /// Suspend the process (the caller owns the terminal).
    Suspend,
}

/// Performs `request` for `editor`: saves and opens files (reporting back
/// through `Editor::saved` / `Editor::opened`), and writes the clipboard
/// escape to `out`. Errors only if writing to `out` fails.
pub fn execute(editor: &mut Editor, request: Request, out: &mut impl Write) -> io::Result<Outcome> {
    Ok(match request {
        Request::Save { doc, path, force } => {
            let result = match editor.document(doc) {
                Some(d) => {
                    // The conflict check compares against what was loaded
                    // from (or saved to) this path; a new path must not
                    // exist yet.
                    let expected = if d.path() == Some(path.as_path()) {
                        d.disk()
                    } else {
                        None
                    };
                    file::save(&path, d.text(), d.bom(), expected, force)
                }
                None => return Ok(Outcome::Done(Effects::default())),
            };
            Outcome::Done(editor.saved(doc, path, result))
        }
        Request::Open { path } => {
            let result = file::load(&path);
            Outcome::Done(editor.opened(path, result))
        }
        Request::Clipboard { text } => match osc52::sequence(&text, osc52::MAX_TEXT) {
            Some(seq) => {
                out.write_all(&seq)?;
                Outcome::Done(Effects::default())
            }
            None => Outcome::Done(editor.notify(format!(
                "{} bytes is too large for the system clipboard; kept in the register",
                text.len()
            ))),
        },
        Request::Suspend => Outcome::Suspend,
        _ => Outcome::Done(Effects::default()),
    })
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
