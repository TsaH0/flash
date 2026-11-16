//! The headless editor: documents and views behind generational IDs.
//!
//! [`Editor`] owns every open [`Document`] and [`View`]. Frontends feed it
//! raw input through [`Editor::handle`], which returns the [`Effects`] the
//! frontend must act on (redraw, quit); everything else is read through
//! accessors. Text changes only through [`Document::commit`] (via
//! [`Editor::commit`] or the key map), so every view of a document stays
//! valid (DECISIONS 014).
//!
//! The layout is the skeleton's, and temporary: each view fills the
//! screen except its last row, which the frontend uses for a status line.
//! Only the focused view is shown and scrolled.
//!
//! Time is an input ([`Editor::handle_at`]): the editor never reads a
//! clock for state, so replaying a trace with its recorded times groups
//! undo steps exactly as the session did. Typing keys commit as one undo
//! group until a pause, a motion, a paste, or an undo (PLAN Stage 5).
//!
//! The editor performs no I/O. Saving, opening, the system clipboard, and
//! suspending are [`Request`]s the frontend carries out (Stage 6); a
//! one-line [`Prompt`] asks for file names and confirmations.

use std::convert::Infallible;
use std::fmt;
use std::num::{NonZeroU8, NonZeroUsize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use slotmap::SlotMap;

use crate::document::{
    CommitError, Document, History, LineEnding, NodeId, Record, Revision, Timestamp,
};
use crate::layout::{Layout, LayoutConfig, Location};
use crate::motion::{Motion, Movement};
use crate::selection::{Range, Selection};
use crate::text::{Direction, Text, TextError};
use crate::transaction::{Transaction, TransactionError};

mod builtin;
mod command;
mod commands;
mod config;
mod event;
mod find;
mod hooks;
mod ids;
mod keymap;
mod mode;
mod options;
mod owner;
mod prompt;
mod select;

pub use command::{CommandError, CommandFn, CommandId, CommandInfo, Commands, Context};
pub use commands::{Request, SaveSnapshot};
pub use config::{ConfigError, DEFAULT_CONFIG};
pub use event::{Event, KeyCode, KeyEvent, Modifiers};
pub use find::{INLINE_MAX as SEARCH_INLINE_MAX, SearchDone, SearchJob};
pub use hooks::{EditorEvent, EventKinds, HookFn, HookId, MAX_ROUNDS};
pub use ids::{DocumentId, ViewId};
pub use keymap::{
    Action, Bind, KeyError, KeySeq, Keymap, Resolved, Step, key_name, normalize, parse_key,
    parse_sequence,
};
pub use mode::{CursorShape, ModeError, ModeFlags, ModeId, ModeInfo, Modes};
pub use options::{
    OptionError, OptionId, OptionInfo, OptionKind, OptionValue, Options, Scope, ViewOptions,
};
pub use owner::Owner;
pub use prompt::{Prompt, PromptKind};
pub use select::RegexAction;

#[cfg(test)]
mod commands_tests;
#[cfg(test)]
pub(crate) mod proptests;
#[cfg(test)]
mod tests;

/// A window onto one document. Its selection lives in the document
/// (DECISIONS 014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    doc: DocumentId,
    /// First visible line, and with soft wrap its first visible row.
    top: usize,
    top_row: usize,
    /// First visible display column (horizontal scroll; 0 with soft wrap).
    left: usize,
    /// Where lines are drawn (the display-line cache).
    layout: Layout,
    /// Where the primary cursor was at the last scroll.
    cursor: Option<Location>,
    /// Text area in cells (the screen minus the status row).
    width: u16,
    height: u16,
    /// The view's resolved built-in options.
    options: ViewOptions,
    mode: ModeId,
}

impl View {
    pub fn doc(&self) -> DocumentId {
        self.doc
    }

    /// The first visible line.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The first visible display column.
    pub fn left(&self) -> usize {
        self.left
    }

    /// With soft wrap, the first visible row of the [`top`](Self::top)
    /// line (0 without).
    pub fn top_row(&self) -> usize {
        self.top_row
    }

    /// The view's display-line cache.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Where the primary cursor was at the last scroll: current while the
    /// layout's revision is the document's and the offset is the cursor's
    /// ([`Editor::cursor_position`]).
    pub fn cursor(&self) -> Option<&Location> {
        self.cursor.as_ref()
    }

    /// The text area, `(width, height)` in cells.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The view's built-in options, resolved (PLAN 2.11: a field read).
    pub fn options(&self) -> &ViewOptions {
        &self.options
    }

    /// The view's mode.
    pub fn mode(&self) -> ModeId {
        self.mode
    }
}

/// What handling one event changed, so the frontend knows what to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects {
    /// The focused view's document text changed.
    pub text: bool,
    /// The focused view's selection changed.
    pub selection: bool,
    /// The focused view scrolled.
    pub scroll: bool,
    /// The screen size changed.
    pub resize: bool,
    /// The status message changed.
    pub message: bool,
    /// The prompt opened, changed, or closed.
    pub prompt: bool,
    /// Requests are waiting ([`Editor::take_requests`]).
    pub requests: bool,
    /// The editor should exit.
    pub quit: bool,
    /// The focused view's mode changed.
    pub mode: bool,
    /// A key sequence or a count is pending (shown on the status line).
    pub pending: bool,
}

impl Effects {
    /// Both sets of changes.
    pub fn or(self, other: Effects) -> Effects {
        Effects {
            text: self.text || other.text,
            selection: self.selection || other.selection,
            scroll: self.scroll || other.scroll,
            resize: self.resize || other.resize,
            message: self.message || other.message,
            prompt: self.prompt || other.prompt,
            requests: self.requests || other.requests,
            quit: self.quit || other.quit,
            mode: self.mode || other.mode,
            pending: self.pending || other.pending,
        }
    }

    /// Whether anything visible changed.
    pub fn needs_redraw(&self) -> bool {
        self.text
            || self.selection
            || self.scroll
            || self.resize
            || self.message
            || self.prompt
            || self.mode
            || self.pending
    }
}

/// All editing state: open documents, views, focus, screen size.
#[derive(Debug, Clone)]
pub struct Editor {
    documents: SlotMap<DocumentId, Document>,
    views: SlotMap<ViewId, View>,
    focus: Option<ViewId>,
    screen: (u16, u16),
    message: String,
    quit: bool,
    /// The time of the event being handled (the last one given).
    now: Timestamp,
    prompt: Option<Prompt>,
    /// Copied text, one piece per range.
    register: Vec<String>,
    requests: Vec<Request>,
    options: Options,
    commands: Commands,
    hooks: hooks::Hooks,
    modes: Modes,
    keymap: Keymap,
    /// A count typed before a command.
    count: Option<std::num::NonZeroUsize>,
    /// `:wq`: quit once the pending save succeeds.
    quit_after_save: bool,
    /// Search state: the pattern, highlighting, an incremental search.
    search: find::Search,
    config_errors: Vec<ConfigError>,
    /// Wall time of the last commit, for instrumentation only.
    last_commit_ns: u64,
}

impl Editor {
    /// Columns per tab stop unless set otherwise (an option from Stage 7).
    pub const DEFAULT_TAB_WIDTH: NonZeroU8 = NonZeroU8::new(4).unwrap();

    /// An editor with no documents, for a `width` × `height` screen.
    pub fn new(width: u16, height: u16) -> Editor {
        let mut editor = Editor {
            documents: SlotMap::with_key(),
            views: SlotMap::with_key(),
            focus: None,
            screen: (width, height),
            message: String::new(),
            quit: false,
            now: Timestamp::ZERO,
            prompt: None,
            register: Vec::new(),
            requests: Vec::new(),
            options: Options::with_builtins(),
            commands: Commands::default(),
            hooks: hooks::Hooks::default(),
            modes: Modes::default(),
            keymap: Keymap::default(),
            count: None,
            quit_after_save: false,
            search: find::Search::default(),
            config_errors: Vec::new(),
            last_commit_ns: 0,
        };
        builtin::register(&mut editor);
        editor.register_builtin_modes();
        // The native hook consumer: when a view closes, a key sequence or
        // count typed for it does not carry over to the next view.
        editor.subscribe(EventKinds::VIEW, Owner::BUILTIN, |ed, events| {
            if events
                .iter()
                .any(|e| matches!(e, EditorEvent::ViewClosed { .. }))
            {
                ed.keymap.cancel();
                ed.count = None;
            }
            Effects::default()
        });
        let errors = editor.apply_config(Owner::DEFAULTS, "default config", DEFAULT_CONFIG);
        debug_assert!(errors.is_empty(), "default config: {errors:?}");
        editor
    }

    /// The key bindings.
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// The count typed so far for the next command, if any.
    pub fn count(&self) -> Option<std::num::NonZeroUsize> {
        self.count
    }

    /// Opens `text` as a new document with no views. A text without a
    /// line break gets the `default-line-ending` option's line ending.
    pub fn open(&mut self, text: Text, path: Option<PathBuf>) -> DocumentId {
        let document = if text.line_count() > 1 {
            Document::new(text, path)
        } else {
            Document::with_line_ending(text, path, self.default_line_ending())
        };
        self.insert_document(document)
    }

    /// Adds `document`, applying the document-scoped options.
    pub(crate) fn insert_document(&mut self, mut document: Document) -> DocumentId {
        document.set_group_pause(self.group_pause(None));
        document.set_group_max_bytes(self.group_max_bytes(None));
        self.documents.insert(document)
    }

    fn default_line_ending(&self) -> LineEnding {
        match self.options.get(OptionId::DEFAULT_LINE_ENDING, None, None) {
            Some(OptionValue::Choice(1)) => LineEnding::Crlf,
            _ => LineEnding::Lf,
        }
    }

    fn group_pause(&self, doc: Option<DocumentId>) -> Duration {
        match self.options.get(OptionId::UNDO_GROUP_PAUSE_MS, None, doc) {
            Some(OptionValue::Int(ms)) => Duration::from_millis(u64::try_from(ms).unwrap_or(0)),
            _ => History::DEFAULT_GROUP_PAUSE,
        }
    }

    fn group_max_bytes(&self, doc: Option<DocumentId>) -> usize {
        match self.options.get(OptionId::UNDO_GROUP_MAX_BYTES, None, doc) {
            Some(OptionValue::Int(n)) => usize::try_from(n).unwrap_or(0),
            _ => History::DEFAULT_GROUP_MAX_BYTES,
        }
    }

    /// Opens a view of `doc` with a cursor at 0 and focuses it.
    pub fn open_view(&mut self, doc: DocumentId) -> Result<ViewId, EditorError> {
        let (width, height) = self.view_area();
        let document = self
            .documents
            .get_mut(doc)
            .ok_or(EditorError::NoSuchDocument(doc))?;
        let options = self.options.resolve(None, Some(doc));
        let view = self.views.insert(View {
            doc,
            top: 0,
            top_row: 0,
            left: 0,
            layout: Layout::new(LayoutConfig {
                tab_width: options.tab_width,
                wrap: None,
            }),
            cursor: None,
            width,
            height,
            options,
            mode: ModeId::NORMAL,
        });
        document.add_view(view);
        self.focus = Some(view);
        self.emit(EditorEvent::ViewOpened { view });
        self.enforce_min(view);
        Ok(view)
    }

    /// Closes `view`. If it was focused, focus moves to another view, or to
    /// none. Its document stays open.
    pub fn close_view(&mut self, view: ViewId) -> Result<(), EditorError> {
        let removed = self
            .views
            .remove(view)
            .ok_or(EditorError::NoSuchView(view))?;
        if let Some(doc) = self.documents.get_mut(removed.doc) {
            doc.remove_view(view);
        }
        self.options.forget_view(view);
        self.emit(EditorEvent::ViewClosed { view });
        self.refocus();
        Ok(())
    }

    /// Closes `doc` and every view of it.
    pub fn close_document(&mut self, doc: DocumentId) -> Result<(), EditorError> {
        let removed = self
            .documents
            .remove(doc)
            .ok_or(EditorError::NoSuchDocument(doc))?;
        for view in removed.views() {
            self.views.remove(view);
            self.options.forget_view(view);
            self.emit(EditorEvent::ViewClosed { view });
        }
        self.options.forget_document(doc);
        self.refocus();
        Ok(())
    }

    /// Focuses `view` and scrolls it to its primary cursor.
    pub fn set_focus(&mut self, view: ViewId) -> Result<(), EditorError> {
        if !self.views.contains_key(view) {
            return Err(EditorError::NoSuchView(view));
        }
        self.focus = Some(view);
        self.scroll(view);
        Ok(())
    }

    pub fn focus(&self) -> Option<ViewId> {
        self.focus
    }

    pub fn document(&self, doc: DocumentId) -> Option<&Document> {
        self.documents.get(doc)
    }

    pub fn documents(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.documents.iter()
    }

    pub fn view(&self, view: ViewId) -> Option<&View> {
        self.views.get(view)
    }

    pub fn views(&self) -> impl Iterator<Item = (ViewId, &View)> {
        self.views.iter()
    }

    /// `view` and the document it shows.
    pub fn view_document(&self, view: ViewId) -> Option<(&View, &Document)> {
        let v = self.views.get(view)?;
        Some((v, self.documents.get(v.doc)?))
    }

    /// The selection of `view`.
    pub fn selection(&self, view: ViewId) -> Option<&Selection> {
        self.view_document(view)?.1.selection(view)
    }

    /// Replaces the selection of `view` if it is valid for its text.
    pub fn set_selection(&mut self, view: ViewId, selection: Selection) -> Result<(), EditorError> {
        self.document_of_mut(view)?
            .set_selection(view, selection)
            .map_err(EditorError::Commit)?;
        self.emit(EditorEvent::SelectionChanged { view });
        self.scroll(view);
        Ok(())
    }

    /// Commits `txn` to the document of `view`, on its behalf, as its own
    /// undo step at the current time ([`Document::commit_with`]). Returns
    /// the document's revision afterwards.
    pub fn commit(&mut self, view: ViewId, txn: Transaction) -> Result<Revision, EditorError> {
        let record = Record::step(self.now);
        let doc = self.document_of_mut(view)?;
        let before = doc.revision();
        let revision = doc
            .commit_with(view, txn, record)
            .map_err(EditorError::Commit)?;
        self.emit_document(view, before, true);
        self.scroll_focus();
        Ok(revision)
    }

    /// Undoes the last change to `view`'s document, on its behalf
    /// ([`Document::undo`]). `None` if there is nothing to undo.
    pub fn undo(&mut self, view: ViewId) -> Result<Option<Revision>, EditorError> {
        let doc = self.document_of_mut(view)?;
        let before = doc.revision();
        let result = doc.undo(view);
        self.emit_document(view, before, result.as_ref().is_ok_and(Option::is_some));
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Redoes the change last undone in `view`'s document
    /// ([`Document::redo`]). `None` if there is nothing to redo.
    pub fn redo(&mut self, view: ViewId) -> Result<Option<Revision>, EditorError> {
        let doc = self.document_of_mut(view)?;
        let before = doc.revision();
        let result = doc.redo(view);
        self.emit_document(view, before, result.as_ref().is_ok_and(Option::is_some));
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Moves `view`'s document to history node `node` ([`Document::goto`]).
    pub fn goto(&mut self, view: ViewId, node: NodeId) -> Result<Option<Revision>, EditorError> {
        let doc = self.document_of_mut(view)?;
        let before = doc.revision();
        let result = doc.goto(view, node);
        self.emit_document(view, before, result.as_ref().is_ok_and(Option::is_some));
        self.scroll_focus();
        result.map_err(EditorError::Commit)
    }

    /// Ends the open undo group of `view`'s document.
    pub fn checkpoint(&mut self, view: ViewId) -> Result<(), EditorError> {
        self.document_of_mut(view)?.checkpoint();
        Ok(())
    }

    /// The time of the event being handled: the last one given to
    /// [`handle_at`](Self::handle_at).
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// The open prompt, if any.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The register: the pieces last copied or cut.
    pub fn register(&self) -> &[String] {
        &self.register
    }

    /// The global tab width (a view may override it: see
    /// [`View::options`]).
    pub fn tab_width(&self) -> NonZeroU8 {
        self.options.resolve(None, None).tab_width
    }

    /// Sets the global `tab-width` option.
    pub fn set_tab_width(&mut self, width: NonZeroU8) {
        let _ = self.set_option(
            Scope::Global,
            OptionId::TAB_WIDTH,
            OptionValue::Int(i64::from(width.get())),
        );
    }

    /// The option registry and every scope's values.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Registers an option (see [`Options::register`]).
    pub fn register_option(
        &mut self,
        name: &str,
        owner: Owner,
        doc: &str,
        kind: OptionKind,
        default: OptionValue,
    ) -> Result<OptionId, OptionError> {
        self.options.register(name, owner, doc, kind, default)
    }

    /// Sets option `id` at `scope` and re-resolves every view's options.
    pub fn set_option(
        &mut self,
        scope: Scope,
        id: OptionId,
        value: OptionValue,
    ) -> Result<(), OptionError> {
        self.check_scope(scope)?;
        self.options.set(scope, id, value)?;
        self.refresh_options();
        Ok(())
    }

    /// Parses `value` for the option named `name` and sets it at `scope`.
    pub fn set_option_text(
        &mut self,
        scope: Scope,
        name: &str,
        value: &str,
    ) -> Result<(), OptionError> {
        let id = self
            .options
            .id(name)
            .ok_or_else(|| OptionError::Unknown(name.to_owned()))?;
        let value = self.options.parse(id, value)?;
        self.set_option(scope, id, value)
    }

    /// Removes the value of option `id` at `scope`.
    pub fn unset_option(&mut self, scope: Scope, id: OptionId) -> Result<(), OptionError> {
        self.check_scope(scope)?;
        self.options.unset(scope, id)?;
        self.refresh_options();
        Ok(())
    }

    /// The value of option `id` as seen by `view` (view, document, global,
    /// default), or the global value without a view.
    pub fn option(&self, id: OptionId, view: Option<ViewId>) -> Option<OptionValue> {
        let doc = view.and_then(|v| self.views.get(v)).map(|v| v.doc);
        self.options.get(id, view, doc)
    }

    fn check_scope(&self, scope: Scope) -> Result<(), OptionError> {
        let exists = match scope {
            Scope::Global => true,
            Scope::View(v) => self.views.contains_key(v),
            Scope::Document(d) => self.documents.contains_key(d),
        };
        if exists {
            Ok(())
        } else {
            Err(OptionError::NoSuchScope)
        }
    }

    /// Re-resolves every view's options and every document's undo-group
    /// pause and size bound,
    /// then re-scrolls the focused view (the tab width may have changed).
    pub(crate) fn refresh_options(&mut self) {
        for (id, view) in &mut self.views {
            view.options = self.options.resolve(Some(id), Some(view.doc));
        }
        let groups: Vec<(DocumentId, Duration, usize)> = self
            .documents
            .keys()
            .map(|d| (d, self.group_pause(Some(d)), self.group_max_bytes(Some(d))))
            .collect();
        for (d, pause, max_bytes) in groups {
            if let Some(doc) = self.documents.get_mut(d) {
                doc.set_group_pause(pause);
                doc.set_group_max_bytes(max_bytes);
            }
        }
        if let Some(view) = self.focus {
            self.scroll(view);
        }
    }

    /// Nanoseconds the last key-driven commit took (instrumentation; not
    /// editor state, and not reproduced by replay).
    pub fn last_commit_ns(&self) -> u64 {
        self.last_commit_ns
    }

    /// The status message: the last command's error, or empty.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Shows `message` on the status line (the frontend reporting on a
    /// request, e.g. a copy too large for the system clipboard).
    pub fn notify(&mut self, message: impl fmt::Display) -> Effects {
        Effects {
            message: self.report(Some(message)),
            ..Effects::default()
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The screen size, `(width, height)`.
    pub fn screen(&self) -> (u16, u16) {
        self.screen
    }

    /// [`handle`](Self::handle)s `event` as happening at `now` (nanoseconds
    /// since an epoch the frontend chooses; a trace's `t_ns`). Undo groups
    /// end when typing pauses for longer than the document's group pause.
    pub fn handle_at(&mut self, event: &Event, now: Timestamp) -> Effects {
        self.now = now;
        self.handle(event)
    }

    /// Applies one input event to the focused view (resizes and Ctrl+Q also
    /// without one) and scrolls the focused view so its primary cursor's
    /// line is visible. Never panics; a failed command changes nothing and
    /// sets the message. The event happens at the last time given to
    /// [`handle_at`](Self::handle_at).
    pub fn handle(&mut self, event: &Event) -> Effects {
        self.delivering(|ed| ed.handle_inner(event))
    }

    fn handle_inner(&mut self, event: &Event) -> Effects {
        let mut fx = match event {
            Event::Resize { width, height } => {
                self.screen = (*width, *height);
                let (w, h) = self.view_area();
                for view in self.views.values_mut() {
                    (view.width, view.height) = (w, h);
                }
                Effects {
                    resize: true,
                    ..Effects::default()
                }
            }
            // A paste is one transaction and its own undo step (PLAN 2.9);
            // into the prompt, its text (not into a yes/no question).
            Event::Paste { text } => match self.prompt.as_mut() {
                Some(p) if !p.kind().is_question() => {
                    p.insert(text);
                    Effects {
                        prompt: true,
                        ..Effects::default()
                    }
                }
                Some(_) => Effects::default(),
                None => match self.focus {
                    Some(view) => {
                        self.edit(view, false, |t, sel, _| Transaction::insert(t, sel, text))
                    }
                    None => Effects::default(),
                },
            },
            Event::Key(key) => self.key(*key),
        };
        // The search prompt's input changed: search again.
        fx = fx.or(self.search_incremental());
        if let Some(view) = self.focus {
            if self.prompt.is_none() {
                fx = fx.or(self.enforce_min(view));
            }
            fx.scroll |= self.scroll(view);
        }
        fx.requests = !self.requests.is_empty();
        fx
    }

    /// Resolves `key` in the input mode's keymap and runs what it is bound
    /// to; an unbound printable key is typed in a mode that inserts
    /// unbound keys. No key is handled anywhere else (PLAN 2.11).
    fn key(&mut self, key: KeyEvent) -> Effects {
        let mode = self.input_mode();
        let timeout_ms = self
            .focus
            .and_then(|v| self.views.get(v))
            .map_or(ViewOptions::default().sequence_timeout_ms, |v| {
                v.options.sequence_timeout_ms
            });
        let resolved = self
            .keymap
            .feed(mode, key, self.now, timeout_ms.saturating_mul(1_000_000));
        let mut fx = Effects::default();
        if let Some(action) = resolved.first {
            fx = fx.or(self.run_action(&action));
        }
        match resolved.step {
            Step::Run(action) => fx = fx.or(self.run_action(&action)),
            Step::Pending => fx.pending = true,
            Step::Unbound => fx = fx.or(self.unbound(mode, key)),
        }
        fx
    }

    fn run_action(&mut self, action: &Action) -> Effects {
        let count = self.count.take();
        let cx = Context {
            view: None,
            count,
            arg: action.arg.as_deref(),
        };
        let fx = self.run(action.command, &cx);
        if self.count.is_some() {
            Effects {
                pending: true,
                ..fx
            }
        } else {
            fx
        }
    }

    fn unbound(&mut self, mode: ModeId, key: KeyEvent) -> Effects {
        self.count = None;
        let printable = match key.code {
            KeyCode::Char(c) if !key.modifiers.ctrl && !key.modifiers.alt => Some(c),
            _ => None,
        };
        match printable {
            Some(c) if self.modes.flags(mode).insert_unbound => {
                let mut buf = [0; 4];
                let s: &str = c.encode_utf8(&mut buf);
                match (self.prompt.as_mut(), self.focus) {
                    (Some(p), _) => {
                        p.insert(s);
                        Effects {
                            prompt: true,
                            ..Effects::default()
                        }
                    }
                    (None, Some(view)) => {
                        self.edit(view, true, |t, sel, _| Transaction::insert(t, sel, s))
                    }
                    (None, None) => Effects::default(),
                }
            }
            // Where unbound keys are typed, other unbound keys are quiet.
            _ if self.modes.flags(mode).insert_unbound => Effects::default(),
            _ => self.notify(format_args!("{} is not bound", key_name(key))),
        }
    }

    // ---- commands used by the key map ----

    /// Builds a transaction for `view` with `make` and commits it at the
    /// current time; `group`: as typing, which may join the open undo group.
    pub(crate) fn edit(
        &mut self,
        view: ViewId,
        group: bool,
        make: impl FnOnce(&Text, &Selection, LineEnding) -> Result<Transaction, TransactionError>,
    ) -> Effects {
        let record = Record {
            time: self.now,
            group,
        };
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        let before = doc.revision();
        let mut commit_ns = None;
        let result = match doc.selection(view) {
            Some(sel) => make(doc.text(), sel, doc.line_ending())
                .map_err(CommitError::from)
                .and_then(|txn| {
                    let start = Instant::now();
                    let r = doc.commit_with(view, txn, record);
                    commit_ns = Some(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    r
                }),
            None => Err(CommitError::NoSuchView(view)),
        };
        if let Some(ns) = commit_ns {
            self.last_commit_ns = ns;
        }
        let changed = result.as_ref().is_ok_and(|&after| after != before);
        if changed {
            self.emit_document(view, before, true);
        }
        Effects {
            text: changed,
            selection: changed,
            message: self.report(result.err()),
            ..Effects::default()
        }
    }

    /// Applies `motion` to every range of `view`'s selection, with the
    /// view's layout answering column questions on long lines.
    pub(crate) fn motion(&mut self, view: ViewId, motion: Motion, movement: Movement) -> Effects {
        self.with_layout(view, |ed, layout| {
            ed.reselect(view, |text, sel| {
                motion.apply_with(text, sel, movement, layout)
            })
        })
    }

    /// Runs `f` with `view`'s layout brought up to date with its document
    /// (taken out of the view meanwhile, so `f` may use the editor).
    pub(crate) fn with_layout<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut Editor, &mut Layout) -> R,
    ) -> R {
        let mut layout = match self.views.get_mut(view) {
            Some(v) => {
                let config = v.layout_config();
                let mut layout = std::mem::replace(&mut v.layout, Layout::new(config));
                layout.set_config(config);
                if let Some(doc) = self.documents.get(v.doc) {
                    layout.sync(doc);
                }
                layout
            }
            None => Layout::new(LayoutConfig {
                tab_width: Self::DEFAULT_TAB_WIDTH,
                wrap: None,
            }),
        };
        let result = f(self, &mut layout);
        if let Some(v) = self.views.get_mut(view) {
            v.layout = layout;
        }
        result
    }

    /// Adds a cursor one line below the last range (or above the first), at
    /// its goal column, as the new primary.
    pub(crate) fn add_cursor(&mut self, view: ViewId, direction: Direction) -> Effects {
        let tab_width = self
            .views
            .get(view)
            .map_or(Self::DEFAULT_TAB_WIDTH, |v| v.options.tab_width);
        let block = self.block_cursor(view);
        self.with_layout(view, |ed, layout| {
            ed.reselect(view, |text, sel| {
                let ranges = sel.ranges();
                let from = match direction {
                    Direction::Forward => ranges[ranges.len() - 1],
                    Direction::Backward => ranges[0],
                };
                let motion = Motion::Vertical {
                    count: 1,
                    direction,
                    tab_width,
                };
                let at = if block {
                    mode::cursor_grapheme(text, from)?
                } else {
                    from.head()
                };
                let start = Range::point(at).with_goal_column(from.goal_column());
                let range = motion.apply_range_with(text, start, Movement::Move, layout)?;
                Ok::<_, TextError>(sel.with_range(range))
            })
        })
    }

    /// Keeps only the primary selection.
    pub(crate) fn collapse_to_primary(&mut self, view: ViewId) -> Effects {
        self.reselect(view, |_, sel| Ok::<_, Infallible>(sel.primary_only()))
    }

    /// Undo (`redo` false) or redo for the key map: reports a change, or
    /// that there is nothing to do, in the message.
    pub(crate) fn history_step(&mut self, view: ViewId, redo: bool) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        let before = doc.revision();
        let result = if redo { doc.redo(view) } else { doc.undo(view) };
        let (moved, changed) = match result {
            Ok(Some(after)) => (true, after != before),
            _ => (false, false),
        };
        self.emit_document(view, before, moved);
        let message = match result {
            Ok(Some(_)) => None,
            Ok(None) if redo => Some("already at the newest change".to_owned()),
            Ok(None) => Some("already at the oldest change".to_owned()),
            Err(e) => Some(e.to_string()),
        };
        Effects {
            text: changed,
            selection: moved,
            message: self.report(message),
            ..Effects::default()
        }
    }

    /// Replaces `view`'s selection with `make`'s result, validated. Ends
    /// the open undo group even if the selection does not change: a motion
    /// is a boundary.
    fn reselect<E: fmt::Display>(
        &mut self,
        view: ViewId,
        make: impl FnOnce(&Text, &Selection) -> Result<Selection, E>,
    ) -> Effects {
        let Ok(doc) = self.document_of_mut(view) else {
            return Effects::default();
        };
        doc.checkpoint();
        let Some(current) = doc.selection(view) else {
            return Effects::default();
        };
        let result = match make(doc.text(), current) {
            Ok(next) if next == *current => Ok(false),
            Ok(next) => doc
                .set_selection(view, next)
                .map(|()| true)
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let changed = result.as_ref().is_ok_and(|&c| c);
        if changed {
            self.emit(EditorEvent::SelectionChanged { view });
        }
        Effects {
            selection: changed,
            message: self.report(result.err()),
            ..Effects::default()
        }
    }

    /// Queues the events of a change to `view`'s document since revision
    /// `before` (see [`Editor::emit_change`]).
    fn emit_document(&mut self, view: ViewId, before: Revision, selection: bool) {
        if let Some(doc) = self.views.get(view).map(|v| v.doc) {
            let after = self.documents.get(doc).map_or(before, Document::revision);
            self.emit_change(view, doc, before, after, selection);
        }
    }

    /// Sets the message to `error`, or clears it. Returns whether it changed.
    fn report(&mut self, error: Option<impl fmt::Display>) -> bool {
        let next = error.map(|e| e.to_string()).unwrap_or_default();
        let changed = next != self.message;
        self.message = next;
        changed
    }

    // ---- internals ----

    fn document_of_mut(&mut self, view: ViewId) -> Result<&mut Document, EditorError> {
        let doc = self
            .views
            .get(view)
            .ok_or(EditorError::NoSuchView(view))?
            .doc;
        self.documents
            .get_mut(doc)
            .ok_or(EditorError::NoSuchDocument(doc))
    }

    /// The text area of a view on the current screen: all but the status row.
    fn view_area(&self) -> (u16, u16) {
        (self.screen.0, self.screen.1.saturating_sub(1))
    }

    fn scroll_focus(&mut self) {
        if let Some(focus) = self.focus {
            self.scroll(focus);
        }
    }

    /// Focus the first remaining view if the focused one is gone.
    fn refocus(&mut self) {
        if self.focus.is_some_and(|v| !self.views.contains_key(v)) {
            self.focus = self.views.keys().next();
            if let Some(view) = self.focus {
                self.scroll(view);
            }
        }
    }

    /// Scrolls `view` by the least amount that shows its primary cursor
    /// (its row, and without wrapping its column), after bringing the
    /// view's [`Layout`] up to date; records where the cursor is
    /// ([`View::cursor`]). Returns whether the view scrolled. O(log n +
    /// view size), plus walks from the layout's nearest marks (see
    /// [`crate::layout`]).
    fn scroll(&mut self, view: ViewId) -> bool {
        // The primary cursor: the block's grapheme in normal mode.
        let Some(head) = self.cursor_position(view) else {
            return false;
        };
        let Some(v) = self.views.get_mut(view) else {
            return false;
        };
        let Some(doc) = self.documents.get(v.doc) else {
            return false;
        };
        let text = doc.text();
        v.layout.set_config(v.layout_config());
        v.layout.sync(doc);
        let Some(loc) = v.layout.locate(text, head) else {
            return false;
        };
        let (width, height) = (usize::from(v.width), usize::from(v.height));
        let (old_top, old_row, old_left) = (v.top, v.top_row, v.left);
        if v.options.soft_wrap {
            v.left = 0;
            v.scroll_rows(text, (loc.line, loc.row), height);
        } else {
            v.top_row = 0;
            let column = loc.column;
            if width > 0 {
                if column < v.left {
                    v.left = column;
                } else if column >= v.left + width {
                    v.left = column + 1 - width;
                }
            }
            if height > 0 {
                if loc.line < v.top {
                    v.top = loc.line;
                } else if loc.line >= v.top + height {
                    v.top = loc.line + 1 - height;
                }
            }
            // Visible long lines get marks up to the left column, so a
            // renderer starts drawing them near it.
            v.layout.extend_visible(text, v.top, height, v.left);
        }
        v.cursor = Some(loc);
        (v.top, v.top_row, v.left) != (old_top, old_row, old_left)
    }
}

impl View {
    /// The layout this view's options and size ask for.
    fn layout_config(&self) -> LayoutConfig {
        LayoutConfig {
            tab_width: self.options.tab_width,
            wrap: if self.options.soft_wrap {
                NonZeroUsize::new(usize::from(self.width).max(1))
            } else {
                None
            },
        }
    }

    /// Wrapped: scrolls by whole rows so row `cursor` (line, row) is one of
    /// the `height` rows from `(top, top_row)`.
    fn scroll_rows(&mut self, text: &Text, cursor: (usize, usize), height: usize) {
        if height == 0 {
            return;
        }
        // Rows may have gone (an edit shortened the top line).
        self.top = self.top.min(text.line_count().saturating_sub(1));
        let top_rows = self.layout.row_count(text, self.top).unwrap_or(1);
        self.top_row = self.top_row.min(top_rows - 1);
        if cursor < (self.top, self.top_row) {
            (self.top, self.top_row) = cursor;
            return;
        }
        // Rows from the top to the cursor, if fewer than `height`.
        let mut rows = 0;
        let mut visible = true;
        for line in self.top..=cursor.0 {
            if rows >= height || line - self.top >= height {
                visible = false;
                break;
            }
            let first = if line == self.top { self.top_row } else { 0 };
            if line == cursor.0 {
                rows += cursor.1 - first;
            } else {
                rows += self.layout.row_count(text, line).unwrap_or(1) - first;
            }
        }
        if visible && rows < height {
            return;
        }
        // The cursor's row becomes the last visible one.
        let (mut line, mut row) = cursor;
        let mut back = height - 1;
        while back > 0 {
            if row > 0 {
                let step = row.min(back);
                row -= step;
                back -= step;
            } else if line > 0 {
                line -= 1;
                row = self.layout.row_count(text, line).unwrap_or(1) - 1;
                back -= 1;
            } else {
                break;
            }
        }
        (self.top, self.top_row) = (line, row);
    }
}

/// Error returned by [`Editor`] methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditorError {
    /// No open document has this ID (it was closed, or never existed).
    NoSuchDocument(DocumentId),
    /// No open view has this ID.
    NoSuchView(ViewId),
    /// The commit or selection change was rejected.
    Commit(CommitError),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchDocument(d) => write!(f, "no such document: {d:?}"),
            Self::NoSuchView(v) => write!(f, "no such view: {v:?}"),
            Self::Commit(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for EditorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Commit(e) => Some(e),
            _ => None,
        }
    }
}
