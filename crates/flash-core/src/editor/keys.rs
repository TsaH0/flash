//! The temporary hard-coded key map, moved from the Stage 3 skeleton.
//!
//! PLAN 2.11: no key is hard-coded in Rust after Stage 7, which replaces
//! this module with keymaps and named commands. Until then the bindings are
//! exactly the skeleton's: type to insert; Enter (the document's line
//! ending), Tab, Backspace, Delete; arrows move (Shift extends);
//! Ctrl+arrows by word; Home/End and Ctrl+Home/End; Alt+Up/Down add a
//! cursor; Esc keeps only the primary cursor; Ctrl+Z undoes, Ctrl+Y and
//! Ctrl+Shift+Z redo. Ctrl+Q is handled by `Editor::handle` itself,
//! because it works without a view. Typing keys (characters, Enter, Tab,
//! Backspace, Delete) form one undo group until a pause, a motion or Esc,
//! a paste, or undo/redo.
//!
//! Ctrl+Z conflicts with suspend, which PLAN Stage 6 also puts on Ctrl-Z;
//! Stage 6 must move one of them (Stage 7 makes both rebindable).

use std::num::NonZeroU8;

use super::{Editor, Effects, KeyCode, KeyEvent, ViewId};
use crate::{Direction, Motion, Movement, Transaction};

/// Tab width for vertical motion; an option from Stage 7 (PLAN 2.11).
pub(crate) const TAB: NonZeroU8 = NonZeroU8::new(4).unwrap();

pub(crate) fn handle_key(editor: &mut Editor, view: ViewId, key: KeyEvent) -> Effects {
    use Direction::{Backward, Forward};
    let KeyEvent { code, modifiers } = key;
    let (ctrl, alt) = (modifiers.ctrl, modifiers.alt);
    let movement = if modifiers.shift {
        Movement::Extend
    } else {
        Movement::Move
    };
    let grapheme = |direction| Motion::Grapheme {
        count: 1,
        direction,
    };
    let word = |direction| Motion::Word {
        count: 1,
        direction,
    };
    let vertical = |direction| Motion::Vertical {
        count: 1,
        direction,
        tab_width: TAB,
    };
    match code {
        KeyCode::Char(c) if !ctrl && !alt => {
            let mut buf = [0; 4];
            let s: &str = c.encode_utf8(&mut buf);
            editor.edit(view, true, |t, sel, _| Transaction::insert(t, sel, s))
        }
        KeyCode::Char('z' | 'Z') if ctrl && !alt => editor.history_step(view, modifiers.shift),
        KeyCode::Char('y') if ctrl && !alt => editor.history_step(view, true),
        KeyCode::Enter => editor.edit(view, true, |t, sel, ending| {
            Transaction::insert(t, sel, ending.as_str())
        }),
        KeyCode::Tab => editor.edit(view, true, |t, sel, _| Transaction::insert(t, sel, "\t")),
        KeyCode::Backspace => editor.edit(view, true, |t, sel, _| Transaction::backspace(t, sel)),
        KeyCode::Delete => editor.edit(view, true, |t, sel, _| Transaction::delete_forward(t, sel)),
        KeyCode::Left if ctrl => editor.motion(view, word(Backward), movement),
        KeyCode::Right if ctrl => editor.motion(view, word(Forward), movement),
        KeyCode::Left => editor.motion(view, grapheme(Backward), movement),
        KeyCode::Right => editor.motion(view, grapheme(Forward), movement),
        KeyCode::Up if alt => editor.add_cursor(view, Backward),
        KeyCode::Down if alt => editor.add_cursor(view, Forward),
        KeyCode::Up => editor.motion(view, vertical(Backward), movement),
        KeyCode::Down => editor.motion(view, vertical(Forward), movement),
        KeyCode::Home if ctrl => editor.motion(view, Motion::DocumentStart, movement),
        KeyCode::End if ctrl => editor.motion(view, Motion::DocumentEnd, movement),
        KeyCode::Home => editor.motion(view, Motion::LineStart, movement),
        KeyCode::End => editor.motion(view, Motion::LineEnd, movement),
        KeyCode::Esc => editor.collapse_to_primary(view),
        _ => Effects::default(),
    }
}
