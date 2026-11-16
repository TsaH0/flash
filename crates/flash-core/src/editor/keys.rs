//! The temporary hard-coded key map (Stages 3–6).
//!
//! PLAN 2.11: no key is hard-coded in Rust after Stage 7, which replaces
//! this module with keymaps and named commands. Until then:
//!
//! - Editing: type to insert; Enter (the document's line ending), Tab,
//!   Backspace, Delete. These form one undo group until a pause, a motion
//!   or Esc, a paste, or undo/redo.
//! - Motion: arrows (Shift extends); Ctrl+arrows by word; Home/End and
//!   Ctrl+Home/End; PageUp/PageDown by a screen; Alt+Up/Down add a cursor;
//!   Esc keeps only the primary cursor.
//! - History: Alt+Z undoes; Alt+Y and Alt+Shift+Z redo.
//! - Register: Ctrl+C copies, Ctrl+X cuts, Ctrl+V pastes (copy and cut
//!   also set the system clipboard).
//! - Files and views: Ctrl+S saves (asks for a path if there is none),
//!   Ctrl+O opens, Ctrl+W closes the view, Ctrl+PageUp/PageDown switch
//!   views.
//! - Ctrl+Z suspends (owner decision 2026-11-16: terminal convention, as
//!   PLAN Stage 6 says). Ctrl+Q quits; `Editor::handle` handles it itself,
//!   because it works without a view.

use super::{Editor, Effects, KeyCode, KeyEvent, ViewId};
use crate::{Direction, Motion, Movement, Transaction};

pub(crate) fn handle_key(editor: &mut Editor, view: ViewId, key: KeyEvent) -> Effects {
    use Direction::{Backward, Forward};
    let KeyEvent { code, modifiers } = key;
    let (ctrl, alt, shift) = (modifiers.ctrl, modifiers.alt, modifiers.shift);
    let movement = if shift {
        Movement::Extend
    } else {
        Movement::Move
    };
    let tab_width = editor.tab_width();
    let grapheme = |direction| Motion::Grapheme {
        count: 1,
        direction,
    };
    let word = |direction| Motion::Word {
        count: 1,
        direction,
    };
    let vertical = |count, direction| Motion::Vertical {
        count,
        direction,
        tab_width,
    };
    let page = editor
        .view(view)
        .map_or(1, |v| usize::from(v.size().1).max(1));
    match code {
        KeyCode::Char(c) if !ctrl && !alt => {
            let mut buf = [0; 4];
            let s: &str = c.encode_utf8(&mut buf);
            editor.edit(view, true, |t, sel, _| Transaction::insert(t, sel, s))
        }
        KeyCode::Char('z') if alt && !ctrl && !shift => editor.history_step(view, false),
        KeyCode::Char('z' | 'Z') if alt && !ctrl => editor.history_step(view, true),
        KeyCode::Char('y') if alt && !ctrl => editor.history_step(view, true),
        KeyCode::Char('z') if ctrl && !alt => editor.suspend(),
        KeyCode::Char('s') if ctrl && !alt => editor.save(view),
        KeyCode::Char('o') if ctrl && !alt => editor.open_command(),
        KeyCode::Char('w') if ctrl && !alt => editor.close_view_command(view),
        KeyCode::Char('c') if ctrl && !alt => editor.copy(view, false),
        KeyCode::Char('x') if ctrl && !alt => editor.copy(view, true),
        KeyCode::Char('v') if ctrl && !alt => editor.paste_register(view),
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
        KeyCode::Up => editor.motion(view, vertical(1, Backward), movement),
        KeyCode::Down => editor.motion(view, vertical(1, Forward), movement),
        KeyCode::PageUp if ctrl => editor.cycle_view(false),
        KeyCode::PageDown if ctrl => editor.cycle_view(true),
        KeyCode::PageUp => editor.motion(view, vertical(page, Backward), movement),
        KeyCode::PageDown => editor.motion(view, vertical(page, Forward), movement),
        KeyCode::Home if ctrl => editor.motion(view, Motion::DocumentStart, movement),
        KeyCode::End if ctrl => editor.motion(view, Motion::DocumentEnd, movement),
        KeyCode::Home => editor.motion(view, Motion::LineStart, movement),
        KeyCode::End => editor.motion(view, Motion::LineEnd, movement),
        KeyCode::Esc => editor.collapse_to_primary(view),
        _ => Effects::default(),
    }
}
