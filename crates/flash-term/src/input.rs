//! Crossterm events to core events.

use crossterm::event::{self as ct, KeyEventKind, KeyModifiers};
use flash_core::{Event, KeyCode, KeyEvent, Modifiers};

/// Converts a crossterm event to a core event. `None` for key releases and
/// for events the editor does not use (mouse, focus, unmapped keys).
pub fn convert(event: &ct::Event) -> Option<Event> {
    match event {
        ct::Event::Key(key) if key.kind != KeyEventKind::Release => {
            let mut modifiers = Modifiers {
                shift: key.modifiers.contains(KeyModifiers::SHIFT),
                ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
                alt: key.modifiers.contains(KeyModifiers::ALT),
            };
            let code = match key.code {
                ct::KeyCode::Char(c) => KeyCode::Char(c),
                ct::KeyCode::Enter => KeyCode::Enter,
                ct::KeyCode::Tab => KeyCode::Tab,
                ct::KeyCode::BackTab => {
                    modifiers.shift = true;
                    KeyCode::Tab
                }
                ct::KeyCode::Backspace => KeyCode::Backspace,
                ct::KeyCode::Delete => KeyCode::Delete,
                ct::KeyCode::Left => KeyCode::Left,
                ct::KeyCode::Right => KeyCode::Right,
                ct::KeyCode::Up => KeyCode::Up,
                ct::KeyCode::Down => KeyCode::Down,
                ct::KeyCode::Home => KeyCode::Home,
                ct::KeyCode::End => KeyCode::End,
                ct::KeyCode::PageUp => KeyCode::PageUp,
                ct::KeyCode::PageDown => KeyCode::PageDown,
                ct::KeyCode::Esc => KeyCode::Esc,
                ct::KeyCode::F(n) => KeyCode::F(n),
                _ => return None,
            };
            Some(Event::Key(KeyEvent::new(code, modifiers)))
        }
        ct::Event::Paste(text) => Some(Event::Paste { text: text.clone() }),
        ct::Event::Resize(width, height) => Some(Event::Resize {
            width: *width,
            height: *height,
        }),
        _ => None,
    }
}
