//! Commands that reach outside the core (files, the system clipboard,
//! suspend) or ask the user first (the prompt), plus the register.
//!
//! The core never performs I/O: these commands queue a [`Request`] and
//! the frontend carries it out, reporting back through
//! [`Editor::saved`] and [`Editor::opened`]. A replayed trace performs no
//! requests, so it can never write a file.

use std::path::{Path, PathBuf};

use super::prompt::{Prompt, PromptKind};
use super::{Context, DocumentId, Editor, EditorError, Effects, OptionError, Scope, ViewId};
use crate::document::Document;
use crate::file::{LoadError, Loaded, SaveError, SaveMethod, Saved};
use crate::text::Text;
use crate::transaction::Transaction;

/// Something the editor needs the frontend to do.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Request {
    /// Save `doc` to `path` (`file::save` with the document's text, BOM,
    /// and disk state), then call [`Editor::saved`]. `force` skips the
    /// conflict check (the user said to overwrite).
    Save {
        doc: DocumentId,
        path: PathBuf,
        force: bool,
    },
    /// Load `path` (`file::load`), then call [`Editor::opened`].
    Open { path: PathBuf },
    /// Put `text` on the system clipboard (OSC 52).
    Clipboard { text: String },
    /// Suspend the process (restore the terminal, stop, resume).
    Suspend,
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

impl Editor {
    /// Takes the requests queued since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// The requests waiting to be taken.
    pub fn requests(&self) -> &[Request] {
        &self.requests
    }

    /// Opens `document` in a new view and focuses it.
    pub fn open_document(&mut self, document: Document) -> ViewId {
        let doc = self.insert_document(document);
        // The document was just inserted, so opening a view cannot fail.
        match self.open_view(doc) {
            Ok(view) => view,
            Err(e) => unreachable!("fresh document rejected: {e}"),
        }
    }

    /// Reports the outcome of a [`Request::Save`] of `doc` to `path`. On
    /// success the document records the path and disk state and is no
    /// longer modified. A [`SaveError::Conflict`] asks whether to
    /// overwrite. The message says what happened (including an in-place
    /// save).
    pub fn saved(
        &mut self,
        doc: DocumentId,
        path: PathBuf,
        result: Result<Saved, SaveError>,
    ) -> Effects {
        let Some(document) = self.documents.get_mut(doc) else {
            // Closed while saving: the file is written, nothing to record.
            return Effects::default();
        };
        let message = match result {
            Ok(saved) => {
                document.mark_saved(path.clone(), saved.disk);
                let how = match saved.method {
                    SaveMethod::Atomic => String::new(),
                    SaveMethod::InPlaceHardLinks(n) => {
                        format!(" in place (the file has {n} hard links)")
                    }
                    SaveMethod::InPlaceOwnership => {
                        " in place (to keep the file's owner)".to_owned()
                    }
                };
                let sync = if saved.dir_synced {
                    ""
                } else {
                    "; the directory could not be flushed"
                };
                format!(
                    "saved {}{how}, {} bytes{sync}",
                    display(&path),
                    saved.disk.size
                )
            }
            Err(SaveError::Conflict) => {
                self.prompt = Some(Prompt::new(
                    PromptKind::Overwrite(doc),
                    format!("{} changed on disk; overwrite it? (y/n) ", display(&path)),
                    String::new(),
                ));
                String::new()
            }
            Err(e) => format!("{}: {e}", display(&path)),
        };
        let fx = Effects {
            message: self.report(Some(message).filter(|m| !m.is_empty())),
            prompt: true,
            text: true,
            ..Effects::default()
        };
        // `:wq`: quit once the save went through.
        if std::mem::take(&mut self.quit_after_save)
            && self.documents.get(doc).is_some_and(|d| !d.is_modified())
        {
            return fx.or(self.quit_command());
        }
        fx
    }

    /// Reports the outcome of a [`Request::Open`] of `path`: opens the
    /// file in a new focused view, a missing file as a new empty document,
    /// or shows the error.
    pub fn opened(&mut self, path: PathBuf, result: Result<Loaded, LoadError>) -> Effects {
        let message = match result {
            Ok(loaded) => {
                self.open_document(Document::from_loaded(loaded, path));
                None
            }
            Err(LoadError::NotFound) => {
                let message = format!("{}: new file", display(&path));
                self.open_document(Document::new(Text::default(), Some(path)));
                Some(message)
            }
            Err(e) => Some(format!("{}: {e}", display(&path))),
        };
        let mut fx = Effects {
            text: true,
            selection: true,
            scroll: true,
            message: self.report(message),
            ..Effects::default()
        };
        if let Some(view) = self.focus {
            fx.scroll |= self.scroll(view);
        }
        fx
    }

    // ---- key commands ----

    pub(crate) fn request(&mut self, request: Request) {
        self.requests.push(request);
    }

    pub(crate) fn open_prompt(&mut self, kind: PromptKind, label: impl Into<String>) -> Effects {
        self.prompt = Some(Prompt::new(kind, label, String::new()));
        Effects {
            prompt: true,
            message: self.report(None::<String>),
            ..Effects::default()
        }
    }

    /// Saves `view`'s document to its path, or asks for one.
    pub(crate) fn save(&mut self, view: ViewId) -> Effects {
        let Ok(doc) = self.view_doc_id(view) else {
            return Effects::default();
        };
        match self.documents[doc].path().map(Path::to_path_buf) {
            Some(path) => {
                self.request(Request::Save {
                    doc,
                    path,
                    force: false,
                });
                Effects::default()
            }
            None => self.open_prompt(PromptKind::SaveAs(doc), "save as: "),
        }
    }

    /// Asks for a path to open.
    pub(crate) fn open_command(&mut self) -> Effects {
        self.open_prompt(PromptKind::Open, "open: ")
    }

    /// Quits, asking first if any document is modified.
    pub(crate) fn quit_command(&mut self) -> Effects {
        let modified = self.documents.values().filter(|d| d.is_modified()).count();
        if modified == 0 {
            return self.quit_now();
        }
        let s = if modified == 1 { "" } else { "s" };
        self.open_prompt(
            PromptKind::Quit,
            format!("{modified} document{s} with unsaved changes; quit anyway? (y/n) "),
        )
    }

    pub(crate) fn quit_now(&mut self) -> Effects {
        self.quit = true;
        Effects {
            quit: true,
            ..Effects::default()
        }
    }

    /// Closes `view`, asking first if it is the last view of a modified
    /// document. Closing a document's last view closes the document;
    /// closing the editor's last view quits.
    pub(crate) fn close_view_command(&mut self, view: ViewId) -> Effects {
        let Ok(doc) = self.view_doc_id(view) else {
            return Effects::default();
        };
        let document = &self.documents[doc];
        if document.views().count() == 1 && document.is_modified() {
            let name = document
                .path()
                .map(display)
                .unwrap_or_else(|| "this document".to_owned());
            return self.open_prompt(
                PromptKind::CloseView(view),
                format!("{name} has unsaved changes; close it anyway? (y/n) "),
            );
        }
        self.close_view_now(view)
    }

    fn close_view_now(&mut self, view: ViewId) -> Effects {
        let Ok(doc) = self.view_doc_id(view) else {
            return Effects::default();
        };
        let _ = self.close_view(view);
        if self
            .documents
            .get(doc)
            .is_some_and(|d| d.views().next().is_none())
        {
            let _ = self.close_document(doc);
        }
        if self.views.is_empty() {
            return self.quit_now();
        }
        Effects {
            text: true,
            selection: true,
            scroll: true,
            ..Effects::default()
        }
    }

    /// Focuses the next (or previous) view.
    pub(crate) fn cycle_view(&mut self, forward: bool) -> Effects {
        let ids: Vec<ViewId> = self.views.keys().collect();
        let Some(i) = self.focus.and_then(|f| ids.iter().position(|&v| v == f)) else {
            return Effects::default();
        };
        if ids.len() < 2 {
            return Effects::default();
        }
        let next = if forward {
            (i + 1) % ids.len()
        } else {
            (i + ids.len() - 1) % ids.len()
        };
        let _ = self.set_focus(ids[next]);
        Effects {
            text: true,
            selection: true,
            scroll: true,
            ..Effects::default()
        }
    }

    pub(crate) fn suspend(&mut self) -> Effects {
        self.request(Request::Suspend);
        Effects::default()
    }

    /// Copies every range's text into the register (and the system
    /// clipboard, joined by the document's line ending); `cut` also
    /// deletes it, as its own undo step.
    pub(crate) fn copy(&mut self, view: ViewId, cut: bool) -> Effects {
        let Ok(doc) = self.view_doc_id(view) else {
            return Effects::default();
        };
        let document = &self.documents[doc];
        let Some(selection) = document.selection(view) else {
            return Effects::default();
        };
        if selection.ranges().iter().all(|r| r.is_empty()) {
            return Effects {
                message: self.report(Some("nothing selected")),
                ..Effects::default()
            };
        }
        let text = document.text();
        let pieces: Vec<String> = selection
            .ranges()
            .iter()
            .map(|r| {
                text.slice(r.start()..r.end())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();
        let joined = pieces.join(document.line_ending().as_str());
        let n = pieces.len();
        self.register = pieces;
        self.request(Request::Clipboard { text: joined });
        let s = if n == 1 { "" } else { "s" };
        let mut fx = Effects {
            message: self.report(Some(format!(
                "{} {n} selection{s}",
                if cut { "cut" } else { "copied" }
            ))),
            ..Effects::default()
        };
        if cut {
            let edit = self.edit(view, false, |t, sel, _| Transaction::delete(t, sel));
            fx.text |= edit.text;
            fx.selection |= edit.selection;
            fx.message |= edit.message;
        }
        fx
    }

    /// Pastes the register: piece `i` at range `i` when the counts match,
    /// else all pieces joined at every range. Its own undo step.
    pub(crate) fn paste_register(&mut self, view: ViewId) -> Effects {
        if self.register.is_empty() {
            return Effects {
                message: self.report(Some("nothing to paste")),
                ..Effects::default()
            };
        }
        let register = std::mem::take(&mut self.register);
        let fx = self.edit(view, false, |t, sel, ending| {
            let k = sel.ranges().len();
            if register.len() == k {
                Transaction::paste(t, sel, &register)
            } else {
                let joined = register.join(ending.as_str());
                Transaction::paste(t, sel, &vec![joined; k])
            }
        });
        self.register = register;
        fx
    }

    /// Handles an event while the prompt is open.
    /// Applies `edit` to the open prompt's input.
    pub(crate) fn prompt_edit(&mut self, edit: fn(&mut Prompt)) -> Effects {
        match self.prompt.as_mut() {
            Some(p) if !p.kind().is_question() => {
                edit(p);
                Effects {
                    prompt: true,
                    ..Effects::default()
                }
            }
            _ => Effects::default(),
        }
    }

    /// Runs a command line: `name [argument]`, with the aliases `w`
    /// (save), `q`, `q!`, `wq`, `o`/`e` (open), `set` and `setlocal`
    /// (`name value`; without a value, show it).
    pub(crate) fn command_line(&mut self, line: &str) -> Effects {
        let line = line.trim();
        let (name, arg) = match line.split_once(' ') {
            Some((name, arg)) => (name, Some(arg.trim()).filter(|a| !a.is_empty())),
            None => (line, None),
        };
        let cx = |arg| Context {
            view: None,
            count: None,
            arg,
        };
        match name {
            "" => Effects::default(),
            "w" => self.run_named("save", &cx(arg)),
            "q" => self.run_named("quit", &cx(None)),
            "q!" => self.run_named("quit-force", &cx(None)),
            "wq" => {
                self.quit_after_save = true;
                self.run_named("save", &cx(arg))
            }
            "o" | "e" => self.run_named("open", &cx(arg)),
            "set" | "setlocal" => {
                let scope = if name == "set" {
                    Scope::Global
                } else {
                    match self.focus {
                        Some(v) => Scope::View(v),
                        None => return self.notify("setlocal: no view"),
                    }
                };
                let (option, value) = match arg.and_then(|a| a.split_once(' ')) {
                    Some((o, v)) => (o, Some(v.trim())),
                    None => (arg.unwrap_or(""), None),
                };
                match value {
                    Some(value) => match self.set_option_text(scope, option, value) {
                        Ok(()) => self.notify(format_args!("{option} = {value}")),
                        Err(e) => self.notify(e),
                    },
                    None => match self.options.id(option) {
                        Some(id) => {
                            let value = self.option(id, self.focus);
                            let shown =
                                value.map_or_else(String::new, |v| self.options.format(id, v));
                            self.notify(format_args!("{option} = {shown}"))
                        }
                        None => self.notify(OptionError::Unknown(option.to_owned())),
                    },
                }
            }
            _ => self.run_named(name, &cx(arg)),
        }
    }

    /// Closes the prompt with `yes` (Enter, `y`) or no (Esc, `n`) and acts
    /// on it.
    pub(crate) fn answer(&mut self, yes: bool) -> Effects {
        let Some(prompt) = self.prompt.take() else {
            return Effects::default();
        };
        let mut fx = Effects {
            prompt: true,
            ..Effects::default()
        };
        let input = prompt.input().to_owned();
        match prompt.kind() {
            _ if !yes => {
                fx.message = self.report(None::<String>);
            }
            PromptKind::Open if !input.is_empty() => {
                self.request(Request::Open { path: input.into() });
            }
            PromptKind::SaveAs(doc) if !input.is_empty() => {
                self.request(Request::Save {
                    doc,
                    path: input.into(),
                    force: false,
                });
            }
            PromptKind::Command => {
                fx = fx.or(self.command_line(&input));
            }
            PromptKind::Open | PromptKind::SaveAs(_) => {}
            PromptKind::Overwrite(doc) => {
                if let Some(path) = self.documents.get(doc).and_then(|d| d.path()) {
                    let path = path.to_path_buf();
                    self.request(Request::Save {
                        doc,
                        path,
                        force: true,
                    });
                }
            }
            PromptKind::Quit => return self.quit_now(),
            PromptKind::CloseView(view) => {
                let closed = self.close_view_now(view);
                fx.text |= closed.text;
                fx.quit |= closed.quit;
            }
        }
        fx
    }

    pub(crate) fn view_doc_id(&self, view: ViewId) -> Result<DocumentId, EditorError> {
        self.views
            .get(view)
            .map(|v| v.doc)
            .ok_or(EditorError::NoSuchView(view))
    }
}
