//! Built-in commands (owner [`Owner::BUILTIN`]): every action the editor
//! has, by name. Keys reach them only through keymaps (Stage 7); the
//! command line and plugins reach them by name.

use std::path::PathBuf;
use std::sync::Arc;

use super::command::{CommandFn, Context};
use super::owner::Owner;
use super::{Editor, Effects, Request, ViewId};
use crate::{Direction, Motion, Movement, Transaction};

type Body = fn(&mut Editor, &Context<'_>) -> Effects;

/// The view a command acts on: the context's, else the focused one.
fn view(ed: &Editor, cx: &Context<'_>) -> Option<ViewId> {
    cx.view.or(ed.focus)
}

fn on_view(
    ed: &mut Editor,
    cx: &Context<'_>,
    f: impl FnOnce(&mut Editor, ViewId) -> Effects,
) -> Effects {
    match view(ed, cx) {
        Some(v) if ed.views.contains_key(v) => f(ed, v),
        _ => ed.notify("no view"),
    }
}

fn tab(ed: &Editor, v: ViewId) -> std::num::NonZeroU8 {
    ed.views
        .get(v)
        .map_or(Editor::DEFAULT_TAB_WIDTH, |view| view.options.tab_width)
}

fn page(ed: &Editor, v: ViewId) -> usize {
    ed.views
        .get(v)
        .map_or(1, |view| usize::from(view.height).max(1))
}

/// The motion a named motion command applies, for `count` repeats.
fn motion_of(ed: &Editor, v: ViewId, name: &str, count: usize) -> Option<Motion> {
    use Direction::{Backward, Forward};
    let tab_width = tab(ed, v);
    Some(match name {
        "left" => Motion::Grapheme {
            count,
            direction: Backward,
        },
        "right" => Motion::Grapheme {
            count,
            direction: Forward,
        },
        "up" => Motion::Vertical {
            count,
            direction: Backward,
            tab_width,
        },
        "down" => Motion::Vertical {
            count,
            direction: Forward,
            tab_width,
        },
        "word-left" => Motion::Word {
            count,
            direction: Backward,
        },
        "word-right" => Motion::Word {
            count,
            direction: Forward,
        },
        "line-start" => Motion::LineStart,
        "line-end" => Motion::LineEnd,
        "document-start" => Motion::DocumentStart,
        "document-end" => Motion::DocumentEnd,
        "page-up" => Motion::Vertical {
            count: count.saturating_mul(page(ed, v)),
            direction: Backward,
            tab_width,
        },
        "page-down" => Motion::Vertical {
            count: count.saturating_mul(page(ed, v)),
            direction: Forward,
            tab_width,
        },
        _ => return None,
    })
}

const MOTIONS: [&str; 12] = [
    "left",
    "right",
    "up",
    "down",
    "word-left",
    "word-right",
    "line-start",
    "line-end",
    "document-start",
    "document-end",
    "page-up",
    "page-down",
];

static COMMANDS: &[(&str, &str, Body)] = &[
    (
        "insert-newline",
        "Insert the document's line ending at each cursor.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.edit(v, true, |t, sel, ending| {
                    Transaction::insert(t, sel, ending.as_str())
                })
            })
        },
    ),
    ("insert-tab", "Insert a tab at each cursor.", |ed, cx| {
        on_view(ed, cx, |ed, v| {
            ed.edit(v, true, |t, sel, _| Transaction::insert(t, sel, "\t"))
        })
    }),
    (
        "insert-text",
        "Insert the argument at each cursor.",
        |ed, cx| {
            let text = cx.arg.unwrap_or("");
            on_view(ed, cx, |ed, v| {
                ed.edit(v, false, |t, sel, _| Transaction::insert(t, sel, text))
            })
        },
    ),
    (
        "delete-backward",
        "Delete the grapheme before each cursor, or the selection.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.edit(v, true, |t, sel, _| Transaction::backspace(t, sel))
            })
        },
    ),
    (
        "delete-forward",
        "Delete the grapheme after each cursor, or the selection.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.edit(v, true, |t, sel, _| Transaction::delete_forward(t, sel))
            })
        },
    ),
    ("undo", "Undo the last change.", |ed, cx| {
        on_view(ed, cx, |ed, v| ed.history_step(v, false))
    }),
    ("redo", "Redo the last undone change.", |ed, cx| {
        on_view(ed, cx, |ed, v| ed.history_step(v, true))
    }),
    (
        "copy",
        "Copy every selection to the register and the clipboard.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.copy(v, false)),
    ),
    (
        "cut",
        "Cut every selection to the register and the clipboard.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.copy(v, true)),
    ),
    ("paste", "Paste the register at every cursor.", |ed, cx| {
        on_view(ed, cx, |ed, v| ed.paste_register(v))
    }),
    (
        "add-cursor-below",
        "Add a cursor on the line below the last one.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.add_cursor(v, Direction::Forward)),
    ),
    (
        "add-cursor-above",
        "Add a cursor on the line above the first one.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.add_cursor(v, Direction::Backward)),
    ),
    (
        "keep-primary",
        "Keep only the primary selection.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.collapse_to_primary(v)),
    ),
    (
        "save",
        "Save the document (to the argument's path, if given).",
        |ed, cx| {
            on_view(ed, cx, |ed, v| match cx.arg.filter(|a| !a.is_empty()) {
                Some(path) => match ed.view_doc_id(v) {
                    Ok(doc) => {
                        ed.request(Request::Save {
                            doc,
                            path: PathBuf::from(path),
                            force: false,
                        });
                        Effects::default()
                    }
                    Err(e) => ed.notify(e),
                },
                None => ed.save(v),
            })
        },
    ),
    (
        "open",
        "Open the argument's path, or ask for one.",
        |ed, cx| match cx.arg.filter(|a| !a.is_empty()) {
            Some(path) => {
                ed.request(Request::Open {
                    path: PathBuf::from(path),
                });
                Effects::default()
            }
            None => ed.open_command(),
        },
    ),
    (
        "close-view",
        "Close the view (asks if it holds the last copy of unsaved changes).",
        |ed, cx| on_view(ed, cx, |ed, v| ed.close_view_command(v)),
    ),
    ("quit", "Quit (asks if anything is unsaved).", |ed, _| {
        ed.quit_command()
    }),
    ("quit-force", "Quit without saving.", |ed, _| ed.quit_now()),
    (
        "suspend",
        "Suspend the editor (the terminal's Ctrl+Z).",
        |ed, _| ed.suspend(),
    ),
    ("next-view", "Focus the next view.", |ed, _| {
        ed.cycle_view(true)
    }),
    ("previous-view", "Focus the previous view.", |ed, _| {
        ed.cycle_view(false)
    }),
];

/// Registers every built-in command.
pub(crate) fn register(ed: &mut Editor) {
    for &(name, doc, body) in COMMANDS {
        let run: Arc<CommandFn> = Arc::new(body);
        let registered = ed.commands.register(name, Owner::BUILTIN, doc, run);
        debug_assert!(registered.is_ok(), "built-in {name}: {registered:?}");
    }
    for name in MOTIONS {
        for (prefix, movement) in [("move", Movement::Move), ("extend", Movement::Extend)] {
            let full = format!("{prefix}-{name}");
            let doc = format!("{prefix} each selection: {name} (count repeats).");
            let run: Arc<CommandFn> = Arc::new(move |ed: &mut Editor, cx: &Context<'_>| {
                on_view(ed, cx, |ed, v| match motion_of(ed, v, name, cx.count()) {
                    Some(m) => ed.motion(v, m, movement),
                    None => Effects::default(),
                })
            });
            let registered = ed.commands.register(&full, Owner::BUILTIN, &doc, run);
            debug_assert!(registered.is_ok(), "built-in {full}: {registered:?}");
        }
    }
}
