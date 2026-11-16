//! Modes through the default keymap: the block cursor, extend, insert,
//! counts, sequences, the command line, and that no key is hard-coded.

use super::*;
use crate::{Event, KeyCode, KeyEvent, Modifiers, Owner, Scope, Text, Timestamp};

fn editor(s: &str) -> (Editor, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    ed.flush_events();
    (ed, view)
}

/// Presses keys written in key notation, space-separated, 10 ms apart.
fn keys(ed: &mut Editor, notation: &str) {
    for key in crate::parse_sequence(notation).unwrap() {
        let now = ed.now().as_nanos() + 10_000_000;
        ed.handle_at(&Event::Key(key), Timestamp::from_nanos(now));
    }
}

fn text(ed: &Editor, v: ViewId) -> String {
    ed.view_document(v).unwrap().1.text().to_string()
}

/// `(anchor, head)` of every range.
fn ranges(ed: &Editor, v: ViewId) -> Vec<(usize, usize)> {
    ed.selection(v)
        .unwrap()
        .ranges()
        .iter()
        .map(|r| (r.anchor(), r.head()))
        .collect()
}

fn mode(ed: &Editor, v: ViewId) -> &str {
    ed.modes().info(ed.view(v).unwrap().mode()).unwrap().name
}

#[test]
fn a_new_view_is_in_normal_mode_with_a_block_cursor() {
    let (ed, v) = editor("abc");
    assert_eq!(mode(&ed, v), "normal");
    assert_eq!(ranges(&ed, v), [(0, 1)]);
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Block);
    // An empty document keeps an empty range: there is no grapheme.
    let (ed, v) = editor("");
    assert_eq!(ranges(&ed, v), [(0, 0)]);
}

#[test]
fn normal_motions_move_the_block() {
    let (mut ed, v) = editor("ab老\ncd");
    keys(&mut ed, "l");
    assert_eq!(ranges(&ed, v), [(1, 2)]);
    keys(&mut ed, "l");
    assert_eq!(ranges(&ed, v), [(2, 5)], "a wide grapheme is one block");
    keys(&mut ed, "j");
    assert_eq!(
        ranges(&ed, v),
        [(8, 8)],
        "past the short last line: the end of the text"
    );
    keys(&mut ed, "k");
    assert_eq!(ranges(&ed, v), [(2, 5)], "back to the goal");
    keys(&mut ed, "h h h");
    assert_eq!(ranges(&ed, v), [(0, 1)], "stops at the start");
    keys(&mut ed, "g e");
    assert_eq!(ranges(&ed, v), [(8, 8)], "the end of the text is empty");
    keys(&mut ed, "g g");
    assert_eq!(ranges(&ed, v), [(0, 1)]);
    keys(&mut ed, "g l");
    assert_eq!(ranges(&ed, v), [(5, 6)], "on the line end: the newline");
    keys(&mut ed, "g h");
    assert_eq!(ranges(&ed, v), [(0, 1)]);
}

#[test]
fn counts_repeat_and_reset() {
    let (mut ed, v) = editor("abcdefghijklmnop");
    keys(&mut ed, "3 l");
    assert_eq!(ranges(&ed, v), [(3, 4)]);
    keys(&mut ed, "1 2 l");
    assert_eq!(ranges(&ed, v), [(15, 16)]);
    assert_eq!(ed.count(), None, "the command took it");
    keys(&mut ed, "2");
    assert_eq!(ed.count().map(|c| c.get()), Some(2));
    keys(&mut ed, "F5");
    assert_eq!(ed.count(), None, "an unbound key drops it");
    keys(&mut ed, "0 h");
    assert_eq!(ranges(&ed, v), [(14, 15)], "a leading 0 is no count");
}

#[test]
fn word_selection_always_advances() {
    let (mut ed, v) = editor("one two, three");
    keys(&mut ed, "w");
    assert_eq!(ranges(&ed, v), [(0, 4)], "one + space");
    keys(&mut ed, "w");
    assert_eq!(ranges(&ed, v), [(4, 7)], "two");
    keys(&mut ed, "w");
    assert_eq!(ranges(&ed, v), [(7, 9)], "comma + space");
    keys(&mut ed, "w");
    assert_eq!(ranges(&ed, v), [(9, 14)], "three");
    keys(&mut ed, "b");
    assert_eq!(ranges(&ed, v), [(14, 9)], "back over three");
    keys(&mut ed, "b");
    assert_eq!(ranges(&ed, v), [(9, 7)], "back over the comma run");
    keys(&mut ed, "2 b");
    assert_eq!(ranges(&ed, v), [(7, 0)], "two words back");
}

#[test]
fn extend_mode_keeps_the_anchor_and_flips() {
    let (mut ed, v) = editor("abcdef");
    keys(&mut ed, "l l v");
    assert_eq!(mode(&ed, v), "extend");
    keys(&mut ed, "l l");
    assert_eq!(ranges(&ed, v), [(2, 5)], "c..e");
    keys(&mut ed, "h h h h");
    assert_eq!(ranges(&ed, v), [(3, 0)], "flipped: a..c, backward");
    keys(&mut ed, "esc");
    assert_eq!(mode(&ed, v), "normal");
    keys(&mut ed, "l");
    assert_eq!(ranges(&ed, v), [(1, 2)], "moving collapses to the block");
}

#[test]
fn insert_append_and_open_lines() {
    let (mut ed, v) = editor("ab\ncd");
    keys(&mut ed, "l i X esc");
    assert_eq!(text(&ed, v), "aXb\ncd");
    assert_eq!(mode(&ed, v), "normal");
    assert_eq!(ranges(&ed, v), [(3, 2)], "the selection kept its grapheme");
    keys(&mut ed, "a Y esc");
    assert_eq!(text(&ed, v), "aXbY\ncd");
    keys(&mut ed, "A Z esc");
    assert_eq!(text(&ed, v), "aXbYZ\ncd");
    keys(&mut ed, "I W esc");
    assert_eq!(text(&ed, v), "WaXbYZ\ncd");
    keys(&mut ed, "o n esc");
    assert_eq!(text(&ed, v), "WaXbYZ\nn\ncd");
    keys(&mut ed, "O m esc");
    assert_eq!(text(&ed, v), "WaXbYZ\nm\nn\ncd");
    // Leaving insert mode ends the undo group: one undo per insert.
    keys(&mut ed, "u");
    assert_eq!(text(&ed, v), "WaXbYZ\nn\ncd");
    keys(&mut ed, "u");
    assert_eq!(text(&ed, v), "WaXbYZ\ncd");
    keys(&mut ed, "U");
    assert_eq!(text(&ed, v), "WaXbYZ\nn\ncd");
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Block);
    keys(&mut ed, "i");
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Bar);
}

#[test]
fn selecting_lines_and_everything_and_operating() {
    let (mut ed, v) = editor("one\ntwo\nthree");
    keys(&mut ed, "x");
    assert_eq!(ranges(&ed, v), [(0, 4)]);
    keys(&mut ed, "x");
    assert_eq!(ranges(&ed, v), [(0, 8)], "again: the next line too");
    keys(&mut ed, "y d");
    assert_eq!(text(&ed, v), "three");
    assert_eq!(ed.register(), ["one\ntwo\n"]);
    keys(&mut ed, "p");
    assert_eq!(text(&ed, v), "tone\ntwo\nhree", "after the block");
    keys(&mut ed, "u u %");
    assert_eq!(text(&ed, v), "one\ntwo\nthree");
    assert_eq!(ranges(&ed, v), [(0, 13)]);
    keys(&mut ed, "c X esc");
    assert_eq!(text(&ed, v), "X");
    keys(&mut ed, ";");
    assert_eq!(ranges(&ed, v), [(1, 1)], "collapsed at the end of the text");
}

#[test]
fn multiple_cursors_in_normal_mode() {
    let (mut ed, v) = editor("ab\ncd\nef");
    keys(&mut ed, "C C");
    assert_eq!(ranges(&ed, v), [(0, 1), (3, 4), (6, 7)]);
    keys(&mut ed, "l");
    assert_eq!(ranges(&ed, v), [(1, 2), (4, 5), (7, 8)]);
    keys(&mut ed, ",");
    assert_eq!(ranges(&ed, v), [(7, 8)], "the primary is the newest");
}

#[test]
fn unbound_keys_say_so_outside_insert_mode() {
    let (mut ed, v) = editor("abc");
    let fx = ed.handle(&Event::Key(KeyEvent::plain(KeyCode::F(5))));
    assert!(fx.message);
    assert_eq!(ed.message(), "F5 is not bound");
    keys(&mut ed, "i");
    let fx = ed.handle(&Event::Key(KeyEvent::plain(KeyCode::F(5))));
    assert!(!fx.message, "quiet where unbound keys are typed");
    assert_eq!(text(&ed, v), "abc");
}

#[test]
fn pending_sequences_show_and_time_out() {
    let (mut ed, v) = editor("abc\ndef");
    keys(&mut ed, "j");
    let fx = ed.handle_at(
        &Event::Key(crate::parse_key("g").unwrap()),
        Timestamp::from_nanos(1_000_000_000),
    );
    assert!(fx.pending);
    assert_eq!(ed.keymap().pending(), [crate::parse_key("g").unwrap()]);
    // Too late: `g` is dropped and `g` starts again; then `g` completes.
    ed.handle_at(
        &Event::Key(crate::parse_key("g").unwrap()),
        Timestamp::from_nanos(2_500_000_000),
    );
    assert_eq!(ranges(&ed, v), [(4, 5)], "nothing ran yet");
    ed.handle_at(
        &Event::Key(crate::parse_key("g").unwrap()),
        Timestamp::from_nanos(2_600_000_000),
    );
    assert_eq!(ranges(&ed, v), [(0, 1)]);
    // The timeout is an option.
    ed.set_option_text(Scope::Global, "sequence-timeout-ms", "5000")
        .unwrap();
    ed.handle_at(
        &Event::Key(crate::parse_key("g").unwrap()),
        Timestamp::from_nanos(3_000_000_000),
    );
    ed.handle_at(
        &Event::Key(crate::parse_key("e").unwrap()),
        Timestamp::from_nanos(6_000_000_000),
    );
    assert_eq!(ranges(&ed, v), [(7, 7)]);
}

#[test]
fn the_command_line_runs_commands_and_sets_options() {
    let (mut ed, v) = editor("abc");
    keys(&mut ed, ":");
    assert_eq!(ed.input_mode(), ModeId::PROMPT);
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Bar);
    for c in "set tab-width 8".chars() {
        ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Char(c))));
    }
    keys(&mut ed, "ret");
    assert!(ed.prompt().is_none());
    assert_eq!(ed.view(v).unwrap().options().tab_width.get(), 8);
    assert_eq!(ed.message(), "tab-width = 8");
    ed.command_line("setlocal tab-width 2");
    assert_eq!(ed.view(v).unwrap().options().tab_width.get(), 2);
    assert_eq!(ed.tab_width().get(), 8, "global unchanged");
    ed.command_line("set tab-width");
    assert_eq!(ed.message(), "tab-width = 2", "as the focused view sees it");
    ed.command_line("set tab-width 99");
    assert!(ed.message().contains("tab-width"), "{}", ed.message());
    ed.command_line("move-right");
    assert_eq!(ranges(&ed, v), [(1, 2)], "any command by name");
    ed.command_line("insert-text hello there");
    assert_eq!(
        text(&ed, v),
        "abhello therec",
        "at the head, after the block"
    );
    ed.command_line("no-such-command");
    assert!(ed.message().contains("unknown command"));
    ed.command_line("w /tmp/out.txt");
    assert!(matches!(
        &ed.take_requests()[..],
        [crate::Request::Save { .. }]
    ));
    // `:wq` quits only after the save succeeds.
    ed.command_line("wq /tmp/out.txt");
    assert!(!ed.should_quit());
    ed.command_line("q!");
    assert!(ed.should_quit());
}

/// PLAN 2.11: no key is hard-coded in Rust. Without the default keymap,
/// no key does anything (resizing still works).
#[test]
fn without_the_default_keymap_no_key_does_anything() {
    let (mut ed, v) = editor("abc\ndef");
    ed.remove_owner(Owner::DEFAULTS);
    let before = (text(&ed, v), ranges(&ed, v));
    let mut all: Vec<KeyEvent> = Vec::new();
    let codes = [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Esc,
        KeyCode::F(1),
    ];
    for c in "abcdefghijklmnopqrstuvwxyzABCZ0123456789:;,.%()".chars() {
        all.push(KeyEvent::plain(KeyCode::Char(c)));
    }
    all.extend(codes.map(KeyEvent::plain));
    for m in [Modifiers::CTRL, Modifiers::ALT, Modifiers::SHIFT] {
        for c in ['q', 's', 'z', 'y', 'c', 'v', 'x', 'w', 'o'] {
            all.push(KeyEvent::new(KeyCode::Char(c), m));
        }
        all.extend(codes.map(|c| KeyEvent::new(c, m)));
    }
    for key in all {
        let fx = ed.handle(&Event::Key(key));
        assert!(
            !fx.quit && !fx.prompt && !fx.text && !fx.selection,
            "{key:?}: {fx:?}"
        );
    }
    assert_eq!((text(&ed, v), ranges(&ed, v)), before);
    assert!(!ed.should_quit() && ed.prompt().is_none() && ed.take_requests().is_empty());
    assert!(
        ed.handle(&Event::Resize {
            width: 20,
            height: 5
        })
        .resize
    );
    // Insert mode still types: unbound printable keys are its data.
    ed.set_mode(v, ModeId::INSERT);
    ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Char('z'))));
    assert_eq!(text(&ed, v), "azbc\ndef", "at the old block's head");
}

#[test]
fn plugin_modes_register_and_go() {
    let (mut ed, v) = editor("abc");
    let flags = ModeFlags {
        insert_unbound: true,
        ..ModeFlags::default()
    };
    let m = ed
        .register_mode("scratch", Owner(8), flags, CursorShape::Underline)
        .unwrap();
    assert!(
        ed.register_mode("scratch", Owner(8), flags, CursorShape::Bar)
            .is_err()
    );
    assert!(
        ed.register_mode("bad name", Owner(8), flags, CursorShape::Bar)
            .is_err()
    );
    assert!(ed.options().id("cursor-shape-scratch").is_some());
    ed.set_mode(v, m);
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Underline);
    ed.set_option_text(Scope::View(v), "cursor-shape-scratch", "bar")
        .unwrap();
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Bar);
    ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Char('q'))));
    assert_eq!(
        text(&ed, v),
        "aqbc",
        "inserts unbound keys at the old block's head"
    );
    ed.remove_owner(Owner(8));
    assert_eq!(
        mode(&ed, v),
        "normal",
        "removing the mode returns its views"
    );
    assert!(ed.modes().id("scratch").is_none());
    assert!(ed.options().id("cursor-shape-scratch").is_none());
    // Built-in modes cannot be removed with their owner.
    ed.remove_owner(Owner::BUILTIN);
    assert!(ed.modes().id("insert").is_some());
}
