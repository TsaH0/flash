//! Config files: the default keymap, the PLAN Stage 7 acceptance test
//! (rebind and unbind in every mode, options at runtime), bad entries, and
//! reloading.

use super::*;
use crate::{
    CursorShape, Event, KeyCode, KeyEvent, ModeId, Scope, Selection, Text, Timestamp, ViewId,
    parse_key,
};

fn editor(s: &str) -> (Editor, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    (ed, view)
}

fn press(ed: &mut Editor, notation: &str) {
    for key in parse_sequence(notation).unwrap() {
        let now = ed.now().as_nanos() + 10_000_000;
        ed.handle_at(&Event::Key(key), Timestamp::from_nanos(now));
    }
}

fn head(ed: &Editor, v: ViewId) -> usize {
    ed.selection(v).unwrap().primary().head()
}

fn text(ed: &Editor, v: ViewId) -> String {
    ed.view_document(v).unwrap().1.text().to_string()
}

#[test]
fn the_default_config_is_clean_and_binds_every_built_in_mode() {
    let mut ed = Editor::new(80, 24);
    let errors = ed.apply_config(Owner::DEFAULTS, "default config", DEFAULT_CONFIG);
    assert_eq!(errors, []);
    for mode in ["normal", "insert", "extend", "prompt", "question"] {
        let id = ed.modes().id(mode).unwrap();
        let bindings = ed.keymap().bindings(id);
        assert!(!bindings.is_empty(), "{mode}");
        for (_, action) in bindings {
            assert!(ed.commands().info(action.command).is_some(), "{mode}");
        }
    }
    // The user layer starts empty.
    let (ed, _) = editor("");
    assert!(ed.config_errors().is_empty());
}

/// PLAN Stage 7 acceptance: a config rebinds and unbinds default keys in
/// every mode and changes options at runtime, without a restart.
#[test]
fn a_config_rebinds_and_unbinds_in_every_mode_and_sets_options_at_runtime() {
    let (mut ed, v) = editor("abcdef\nxyz");
    let config = r#"
        [options]
        tab-width = 8
        cursor-shape-insert = "underline"
        sequence-timeout-ms = 0

        [keys.normal]
        q = "move-right"
        h = false
        "g x" = "move-document-end"

        [keys.insert]
        "C-e" = "move-line-end"
        ret = false

        [keys.extend]
        l = "move-left"

        [keys.prompt]
        "C-a" = "prompt-home"
        esc = false

        [keys.question]
        y = false
        o = "prompt-yes"
    "#;
    let fx = ed.load_config("test.toml", Some(config));
    assert!(fx.message);
    assert_eq!(ed.config_errors(), []);
    assert_eq!(ed.message(), "config: loaded test.toml");
    // Options changed at runtime.
    assert_eq!(ed.view(v).unwrap().options().tab_width.get(), 8);
    assert_eq!(ed.view(v).unwrap().options().sequence_timeout_ms, 0);

    // Normal: q rebound, h unbound, a new sequence; the rest kept.
    press(&mut ed, "q");
    assert_eq!(head(&ed, v), 2, "q moves right (block 1..2)");
    press(&mut ed, "h");
    assert_eq!(ed.message(), "h is not bound");
    assert_eq!(head(&ed, v), 2);
    press(&mut ed, "l");
    assert_eq!(head(&ed, v), 3, "an untouched default");

    // Extend: l rebound to move left.
    press(&mut ed, "v l");
    assert_eq!(ed.selection(v).unwrap().primary().anchor(), 3);
    assert_eq!(head(&ed, v), 1, "extended left: block 1..3, backward");
    press(&mut ed, "esc");

    // Insert: C-e added, ret unbound (quiet: insert mode).
    press(&mut ed, "i");
    assert_eq!(ed.cursor_shape(Some(v)), CursorShape::Underline);
    press(&mut ed, "C-e");
    assert_eq!(head(&ed, v), 6, "line end");
    let fx = ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Enter)));
    assert!(!fx.text);
    assert_eq!(text(&ed, v), "abcdef\nxyz");
    press(&mut ed, "esc");

    // Prompt: C-a added, esc unbound (ret still accepts).
    press(&mut ed, ": e c h o");
    press(&mut ed, "C-a");
    assert_eq!(ed.prompt().unwrap().cursor(), 0);
    press(&mut ed, "esc");
    assert!(ed.prompt().is_some(), "esc no longer cancels");
    press(&mut ed, "backspace backspace backspace backspace");
    press(&mut ed, "end ret");
    assert!(ed.prompt().is_none());

    // Question: y unbound, o says yes.
    press(&mut ed, "i Z esc C-q");
    assert!(ed.prompt().is_some(), "unsaved: asks");
    press(&mut ed, "y");
    assert!(!ed.should_quit(), "y no longer answers");
    press(&mut ed, "o");
    assert!(ed.should_quit());
}

#[test]
fn bad_entries_are_reported_and_skipped() {
    let (mut ed, v) = editor("abc");
    let config = r#"
        bogus = 1

        [options]
        tab-width = 2
        no-such-option = true
        undo-group-pause-ms = "fast"
        normal-min-grapheme = 3
        default-line-ending = "cr"
        sequence-timeout-ms = -5

        [keys.normal]
        q = "move-right"
        "S-a" = "move-left"
        "C-" = "move-left"
        z = "no-such-command"
        x = 3
        "g" = "select-line extra args are an argument"

        [keys.nomode]
        a = "undo"

        [keys.insert]
        "C-e" = "move-line-end"
    "#;
    let fx = ed.load_config("bad.toml", Some(config));
    assert!(fx.message);
    let entries: Vec<&str> = ed
        .config_errors()
        .iter()
        .map(|e| e.entry.as_str())
        .collect();
    assert_eq!(
        entries,
        // Sections and entries in sorted order (TOML tables are sorted).
        [
            "bogus",
            "keys.nomode",
            "keys.normal.\"C-\"",
            "keys.normal.\"S-a\"",
            "keys.normal.\"x\"",
            "keys.normal.\"z\"",
            "options.default-line-ending",
            "options.no-such-option",
            "options.normal-min-grapheme",
            "options.sequence-timeout-ms",
            "options.undo-group-pause-ms",
        ],
        "{:#?}",
        ed.config_errors()
    );
    assert!(ed.config_errors().iter().all(|e| e.source == "bad.toml"));
    assert!(
        ed.message()
            .starts_with("config: 11 entries skipped; first: bad.toml: bogus: ")
    );
    // The good entries applied.
    assert_eq!(ed.view(v).unwrap().options().tab_width.get(), 2);
    press(&mut ed, "q");
    assert_eq!(head(&ed, v), 2);
    // Not valid TOML at all: reported, and the user layer is cleared.
    ed.load_config("broken.toml", Some("[keys.normal\nq = "));
    assert_eq!(ed.config_errors().len(), 1);
    assert!(ed.config_errors()[0].reason.contains("not valid TOML"));
    assert_eq!(ed.view(v).unwrap().options().tab_width.get(), 4);
}

#[test]
fn reloading_replaces_only_the_user_layer() {
    let (mut ed, v) = editor("abcdef");
    ed.load_config(
        "a.toml",
        Some("[options]\ntab-width = 8\n[keys.normal]\nq = \"move-right\"\nl = false\n"),
    );
    press(&mut ed, "q");
    assert_eq!(head(&ed, v), 2);
    // A plugin layer and a :set value.
    let plugin = Owner(20);
    let normal = ModeId::NORMAL;
    let undo = ed.commands().id("undo").unwrap();
    ed.keymap.set_layer(
        plugin,
        normal,
        vec![(
            parse_sequence("Q").unwrap(),
            crate::Bind::Command {
                id: undo,
                arg: None,
            },
        )],
    );
    ed.set_option(
        Scope::View(v),
        crate::OptionId::TAB_WIDTH,
        crate::OptionValue::Int(3),
    )
    .unwrap();
    // Reload with a different file: a.toml's q and l go, b.toml's w goes in.
    ed.load_config("b.toml", Some("[keys.normal]\nw = \"move-left\"\n"));
    assert_eq!(
        ed.keymap().lookup(normal, &parse_sequence("q").unwrap()),
        None
    );
    assert!(
        ed.keymap()
            .lookup(normal, &parse_sequence("l").unwrap())
            .is_some(),
        "default back"
    );
    assert_eq!(
        ed.keymap()
            .lookup(normal, &parse_sequence("w").unwrap())
            .map(|a| a.command),
        ed.commands().id("move-left")
    );
    assert!(
        ed.keymap()
            .lookup(normal, &parse_sequence("Q").unwrap())
            .is_some(),
        "plugin kept"
    );
    assert_eq!(ed.tab_width().get(), 4, "a.toml's global value gone");
    assert_eq!(
        ed.view(v).unwrap().options().tab_width.get(),
        3,
        "view value kept"
    );
    // No file at all: back to the defaults.
    let fx = ed.load_config("", None);
    assert!(!fx.message || ed.message().is_empty());
    assert!(
        ed.keymap()
            .lookup(normal, &parse_sequence("w").unwrap())
            .is_some()
    );
    assert_eq!(
        ed.keymap()
            .lookup(normal, &parse_sequence("w").unwrap())
            .map(|a| a.command),
        ed.commands().id("select-next-word")
    );
    // The reload command asks the frontend to read the file.
    ed.run_named("config-reload", &crate::Context::default());
    assert_eq!(ed.take_requests(), [crate::Request::LoadConfig]);
    let _ = (Selection::point(0), parse_key("x"));
}

/// The default config's `[options]` set defaults, not global values: a
/// user reload does not undo them.
#[test]
fn default_options_are_defaults() {
    let mut ed = Editor::new(80, 24);
    let errors = ed.apply_config(Owner::DEFAULTS, "defaults", "[options]\ntab-width = 6\n");
    assert_eq!(errors, []);
    assert_eq!(ed.tab_width().get(), 6);
    ed.load_config("", None);
    assert_eq!(ed.tab_width().get(), 6);
    assert_eq!(
        ed.options()
            .info(crate::OptionId::TAB_WIDTH)
            .unwrap()
            .default,
        crate::OptionValue::Int(6)
    );
}

/// Review attack tests: hostile configs never panic and never prevent
/// the editor from working.
#[test]
fn hostile_configs_are_reported_not_fatal() {
    let (mut ed, v) = editor("abc");
    let deep = format!("x = {}{}", "[".repeat(100_000), "]".repeat(100_000));
    let huge_keys: String = (0..20_000)
        .map(|i| format!("\"F{}\" = \"undo\"\n", i % 30))
        .collect();
    for config in [
        deep,
        format!("[keys.normal]\n{huge_keys}"),
        "[keys.normal]\n\"\\u0007\" = \"undo\"\n\"\\u001b[A\" = \"undo\"\n\" \" = \"undo\"\n"
            .to_owned(),
        "[keys.normal]\nq = \"\"\nw = \"   \"\ne = [\"undo\"]\nr = { a = 1 }\n".to_owned(),
        "[keys]\nnormal = 3\n[options]\ntab-width = 1e9\n".to_owned(),
        "\u{feff}[options]\ntab-width = 3\n".to_owned(),
        "[keys.normal]\n\"g g g g g g g g\" = \"undo\"\n".to_owned(),
    ] {
        ed.load_config("hostile.toml", Some(&config));
        // Still an editor: the defaults answer.
        press(&mut ed, "esc l h");
        assert!(ed.view(v).is_some());
    }
    // A sequence longer than four keys works (stored on the heap).
    ed.load_config(
        "long.toml",
        Some("[keys.normal]\n\"Q Q Q Q Q Q\" = \"move-right\"\n"),
    );
    assert_eq!(ed.config_errors(), []);
    press(&mut ed, "Q Q Q Q Q Q");
    assert_eq!(head(&ed, v), 2);
}

#[test]
fn hostile_patterns_counts_and_command_lines() {
    let (mut ed, v) = editor("abc def\nghi");
    // A pattern too large for the regex size limit is a message.
    ed.run_named(
        "select-regex",
        &crate::Context {
            arg: Some("(a{1000}){1000}"),
            ..Default::default()
        },
    );
    assert!(ed.message().contains("bad pattern"), "{}", ed.message());
    // Counts past usize overflow stop growing; huge counts end at the edges.
    press(&mut ed, "9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 l");
    assert_eq!(head(&ed, v), 11, "the end of the text");
    press(&mut ed, "9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 b");
    press(&mut ed, "9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 w");
    press(&mut ed, "9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 j");
    press(
        &mut ed,
        "9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 pagedown",
    );
    for line in [
        "",
        " ",
        ":",
        "set",
        "set  ",
        "setlocal x",
        "w",
        "q extra",
        "e",
        "count x",
        "count 12",
    ] {
        ed.command_line(line);
    }
    assert!(!ed.should_quit() || ed.documents().count() > 0);
    let _ = Selection::point(0);
}
