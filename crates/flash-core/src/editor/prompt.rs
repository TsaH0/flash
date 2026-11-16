//! The one-line prompt: file names for open and save-as, and yes/no
//! questions before overwriting a changed file or discarding changes.
//!
//! The prompt is editor state, so traces record and replay it like any
//! other input. While it is open every key goes to it; nothing reaches a
//! document.

use super::{DocumentId, ViewId};

/// What the prompt is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A path to open.
    Open,
    /// A command line: `name [argument]`.
    Command,
    /// A regex for a selection command.
    Regex(super::RegexAction),
    /// A path to save this document to.
    SaveAs(DocumentId),
    /// Yes/no: overwrite this document's file, which changed on disk.
    Overwrite(DocumentId),
    /// Yes/no: quit with unsaved changes.
    Quit,
    /// Yes/no: close this view, the last of a modified document.
    CloseView(ViewId),
}

impl PromptKind {
    /// Whether the prompt takes `y` or `n` rather than a line of text.
    pub fn is_question(self) -> bool {
        matches!(
            self,
            PromptKind::Overwrite(_) | PromptKind::Quit | PromptKind::CloseView(_)
        )
    }
}

/// An open prompt: a label, the input typed so far, and a cursor in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    kind: PromptKind,
    label: String,
    input: String,
    /// Byte offset in `input`, on a char boundary.
    cursor: usize,
}

impl Prompt {
    pub(crate) fn new(kind: PromptKind, label: impl Into<String>, input: String) -> Prompt {
        let cursor = input.len();
        Prompt {
            kind,
            label: label.into(),
            input,
            cursor,
        }
    }

    pub fn kind(&self) -> PromptKind {
        self.kind
    }

    /// The text before the input, e.g. `"open: "`.
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    /// The cursor, a byte offset in [`input`](Self::input).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Inserts `s` at the cursor, dropping control characters (so a
    /// pasted newline cannot end up in a file name).
    pub(crate) fn insert(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.input.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
    }

    /// Deletes the char before the cursor.
    pub(crate) fn backspace(&mut self) {
        if let Some(c) = self.input[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.input.remove(self.cursor);
        }
    }

    /// Deletes the char after the cursor.
    pub(crate) fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    pub(crate) fn left(&mut self) {
        if let Some(c) = self.input[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub(crate) fn right(&mut self) {
        if let Some(c) = self.input[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub(crate) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = self.input.len();
    }
}
