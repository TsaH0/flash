//! Raw input events, independent of any terminal library.
//!
//! These are what a frontend feeds [`Editor::handle`](super::Editor::handle)
//! and what a trace records (DECISIONS 016): keys with modifiers, bracketed
//! pastes, and resizes. Key releases are the frontend's to filter.

use serde::{Deserialize, Serialize};

/// One input event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A key press (or repeat).
    Key(KeyEvent),
    /// A bracketed paste: inserted as one transaction (PLAN 2.9).
    Paste { text: String },
    /// The terminal (screen) size changed.
    Resize { width: u16, height: u16 },
}

/// A key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyEvent {
    pub code: KeyCode,
    #[serde(default)]
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(code: KeyCode, modifiers: Modifiers) -> KeyEvent {
        KeyEvent { code, modifiers }
    }

    /// `code` with no modifiers.
    pub fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, Modifiers::NONE)
    }
}

/// The keys a frontend can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Esc,
    /// A function key, F1 = `F(1)`.
    F(u8),
}

/// Modifier keys held with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
    };
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    pub const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    pub const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
}
