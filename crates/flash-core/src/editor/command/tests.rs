//! The command registry and the built-in commands, run without keys.

use std::cell::Cell;
use std::num::NonZeroUsize;
use std::rc::Rc;

use super::*;
use crate::{OptionKind, OptionValue, Request, Selection, Text};

fn editor(s: &str) -> (Editor, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    // Point cursors, as the motion commands give in insert mode.
    crate::editor::tests::insert_mode(&mut ed, view);
    (ed, view)
}

fn text(ed: &Editor, v: ViewId) -> String {
    ed.view_document(v).unwrap().1.text().to_string()
}

fn head(ed: &Editor, v: ViewId) -> usize {
    ed.selection(v).unwrap().primary().head()
}

#[test]
fn register_run_and_remove() {
    let (mut ed, view) = editor("abc");
    let seen = Rc::new(Cell::new((None, 0, String::new())));
    let s = Rc::clone(&seen);
    let plugin = Owner(9);
    let id = ed
        .register_command("probe", plugin, "records its context", move |_, cx| {
            s.set((cx.view, cx.count(), cx.arg.unwrap_or("").to_owned()));
            Effects::default()
        })
        .unwrap();
    assert_eq!(ed.commands().id("probe"), Some(id));
    let info = ed.commands().info(id).unwrap();
    assert_eq!(
        (info.name, info.owner, info.doc),
        ("probe", plugin, "records its context")
    );
    let cx = Context {
        view: Some(view),
        count: NonZeroUsize::new(3),
        arg: Some("x y"),
    };
    ed.run(id, &cx);
    assert_eq!(seen.take(), (Some(view), 3, "x y".to_owned()));
    ed.run_named("probe", &Context::default());
    assert_eq!(seen.take(), (None, 1, String::new()));

    for bad in ["", "two words", "tab\there"] {
        assert_eq!(
            ed.register_command(bad, plugin, "", |_, _| Effects::default()),
            Err(CommandError::BadName(bad.to_owned()))
        );
    }
    assert_eq!(
        ed.register_command("probe", plugin, "", |_, _| Effects::default()),
        Err(CommandError::Duplicate("probe".to_owned()))
    );
    ed.remove_command(id).unwrap();
    assert_eq!(ed.remove_command(id), Err(CommandError::Unknown(id)));
    // A stale ID or unknown name changes nothing and says so.
    let fx = ed.run(id, &Context::default());
    assert!(fx.message && ed.message().contains("no command"));
    let fx = ed.run_named("probe", &Context::default());
    assert!(fx.message && ed.message().contains("unknown command"));
    assert_eq!(text(&ed, view), "abc");
    // IDs are never reused.
    let again = ed
        .register_command("probe", plugin, "", |_, _| Effects::default())
        .unwrap();
    assert_ne!(again, id);
}

#[test]
fn removing_an_owner_removes_its_commands_and_options() {
    let (mut ed, _) = editor("");
    let plugin = Owner(5);
    let builtins = ed.commands().iter().count();
    ed.register_command("p-one", plugin, "", |_, _| Effects::default())
        .unwrap();
    ed.register_command("p-two", plugin, "", |_, _| Effects::default())
        .unwrap();
    ed.register_option(
        "p-opt",
        plugin,
        "",
        OptionKind::Bool,
        OptionValue::Bool(true),
    )
    .unwrap();
    assert_eq!(ed.commands().iter().count(), builtins + 2);
    ed.remove_owner(plugin);
    assert_eq!(ed.commands().iter().count(), builtins);
    assert_eq!(ed.commands().id("p-one"), None);
    assert_eq!(ed.options().id("p-opt"), None);
    assert!(ed.commands().iter().all(|c| c.owner == Owner::BUILTIN));
}

/// Every built-in runs by name, with and without a count and an argument,
/// on an empty document, a non-empty one, and with no view at all,
/// without panicking; the selection stays valid.
#[test]
fn every_builtin_runs_without_a_key() {
    let names: Vec<String> = Editor::new(1, 1)
        .commands()
        .iter()
        .map(|c| c.name.to_owned())
        .collect();
    assert!(names.len() > 40, "{names:?}");
    for name in &names {
        for content in ["", "héllo wörld\n\tsecond 老虎 line\r\nthird"] {
            for cx in [
                Context::default(),
                Context {
                    view: None,
                    count: NonZeroUsize::new(3),
                    arg: Some("/tmp/arg"),
                },
            ] {
                let (mut ed, view) = editor(content);
                let middle = (0..=content.len() / 2)
                    .rev()
                    .find(|&i| content.is_char_boundary(i))
                    .unwrap_or(0);
                ed.set_selection(view, Selection::point(middle)).unwrap();
                ed.run_named(name, &cx);
                if let Some((_, d)) = ed.view_document(view) {
                    let sel = d.selection(view).unwrap();
                    assert_eq!(sel.validate(d.text()), Ok(()), "{name} on {content:?}");
                }
            }
        }
        let mut empty = Editor::new(80, 24);
        empty.run_named(name, &Context::default());
    }
}

#[test]
fn builtins_do_what_their_names_say() {
    let (mut ed, v) = editor("one two three");
    let cx = |count: usize, arg: Option<&'static str>| Context {
        view: None,
        count: NonZeroUsize::new(count),
        arg,
    };
    ed.run_named("move-right", &cx(3, None));
    assert_eq!(head(&ed, v), 3);
    ed.run_named("move-word-right", &cx(1, None));
    assert_eq!(head(&ed, v), 4);
    ed.run_named("extend-line-end", &cx(1, None));
    assert_eq!(ed.selection(v).unwrap().primary().anchor(), 4);
    assert_eq!(head(&ed, v), 13);
    ed.run_named("move-document-start", &cx(1, None));
    ed.run_named("insert-text", &cx(1, Some(">> ")));
    assert_eq!(text(&ed, v), ">> one two three");
    ed.run_named("undo", &cx(1, None));
    assert_eq!(text(&ed, v), "one two three");
    ed.run_named("redo", &cx(1, None));
    assert_eq!(text(&ed, v), ">> one two three");
    ed.run_named("insert-newline", &cx(1, None));
    ed.run_named("delete-backward", &cx(1, None));
    assert_eq!(text(&ed, v), ">> one two three");

    // File commands queue requests; with an argument they skip the prompt.
    ed.run_named("save", &cx(1, Some("out.txt")));
    ed.run_named("open", &cx(1, Some("in.txt")));
    let requests = ed.take_requests();
    assert!(
        matches!(&requests[0], Request::Save { path, force: false, .. } if path.ends_with("out.txt"))
    );
    assert!(matches!(&requests[1], Request::Open { path } if path.ends_with("in.txt")));
    let fx = ed.run_named("save", &cx(1, None));
    assert!(fx.prompt && ed.prompt().is_some(), "no path yet: asks");
    // An explicit view that does not exist is reported, not used.
    let gone = {
        let doc = ed.open(Text::from("x"), None);
        let w = ed.open_view(doc).unwrap();
        ed.close_view(w).unwrap();
        w
    };
    let fx = ed.run_named(
        "move-right",
        &Context {
            view: Some(gone),
            ..Context::default()
        },
    );
    assert!(fx.message);
    assert_eq!(ed.message(), "no view");
    ed.run_named("quit-force", &cx(1, None));
    assert!(ed.should_quit());
}
