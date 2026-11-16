//! Carrying out the editor's requests: the only place the binary touches
//! files on the editor's behalf.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::num::NonZeroU8;
use std::path::PathBuf;

use flash_core::{Editor, Effects, OptionId, OptionValue, Request, Text, file, trace};
use flash_term::{osc52, terminal as term};

/// The user's config file and the command line's overrides of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub config: Option<PathBuf>,
    /// `--tab-width`: wins over the config, also after a reload.
    pub tab_width: Option<NonZeroU8>,
}

impl Settings {
    /// The config file: `cli` (`--config`), else
    /// `$XDG_CONFIG_HOME/flash/config.toml`, else
    /// `$HOME/.config/flash/config.toml`.
    pub fn config_path(cli: Option<PathBuf>) -> Option<PathBuf> {
        let nonempty =
            |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
        cli.or_else(|| {
            nonempty(env::var_os("XDG_CONFIG_HOME")).map(|d| d.join("flash/config.toml"))
        })
        .or_else(|| nonempty(env::var_os("HOME")).map(|h| h.join(".config/flash/config.toml")))
    }
}

/// Reads the config file and applies it as the user layer (a missing file
/// clears it; an unreadable one is reported and changes nothing), then the
/// command line's overrides, then the terminal options (the keyboard
/// protocol flag, pushed or popped at once if the terminal is entered).
/// Returns the effects and the [`trace::content_hash`] of the text
/// applied, if any.
pub fn load_config(editor: &mut Editor, settings: &Settings) -> (Effects, Option<u64>) {
    let (fx, hash) = match &settings.config {
        Some(path) => match fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let hash = trace::content_hash(&Text::from(text.as_str()));
                    let source = path.display().to_string();
                    (editor.load_config(&source, Some(&text)), Some(hash))
                }
                Err(_) => (
                    editor.notify(format!("{}: not UTF-8; config not loaded", path.display())),
                    None,
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (editor.load_config("", None), None),
            Err(e) => (editor.notify(format!("{}: {e}", path.display())), None),
        },
        None => (editor.load_config("", None), None),
    };
    if let Some(n) = settings.tab_width {
        editor.set_tab_width(n);
    }
    let kitty = !matches!(
        editor.option(OptionId::KITTY_KEYBOARD, None),
        Some(OptionValue::Bool(false))
    );
    let fx = match term::set_kitty_keyboard(kitty) {
        Ok(()) => fx,
        Err(e) => fx.or(editor.notify(format_args!("keyboard protocol: {e}"))),
    };
    (fx, hash)
}

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
pub fn execute(
    editor: &mut Editor,
    request: Request,
    out: &mut impl Write,
    settings: &Settings,
) -> io::Result<Outcome> {
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
        Request::LoadConfig => Outcome::Done(load_config(editor, settings).0),
        _ => Outcome::Done(Effects::default()),
    })
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
