//! Built-in commands (owner [`Owner::BUILTIN`]): every action the editor
//! has, by name. Keys reach them only through keymaps (Stage 7); the
//! command line and plugins reach them by name.

use std::path::PathBuf;
use std::sync::Arc;

use std::num::NonZeroUsize;

use super::command::{CommandFn, Context};
use super::mode::{self, ModeId};
use super::owner::Owner;
use super::{Editor, Effects, Prompt, PromptKind, RegexAction, Request, ViewId};
use crate::{Direction, Motion, Movement, Range, Selection, Transaction};

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
        // A view named but gone is an error; no view at all, a no-op.
        Some(_) => ed.notify("no view"),
        None => Effects::default(),
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
        "row-up" => Motion::Row {
            count,
            direction: Backward,
            tab_width,
        },
        "row-down" => Motion::Row {
            count,
            direction: Forward,
            tab_width,
        },
        _ => return None,
    })
}

const MOTIONS: [&str; 14] = [
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
    "row-up",
    "row-down",
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
        "search",
        "Search forward as a pattern is typed (Enter keeps the match).",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_prompt(v, false)),
    ),
    (
        "search-backward",
        "Search backward as a pattern is typed.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_prompt(v, true)),
    ),
    (
        "search-next",
        "Select the next match of the search pattern.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_next(v, false, false)),
    ),
    (
        "search-previous",
        "Select the match of the search pattern the other way.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_next(v, true, false)),
    ),
    (
        "extend-search-next",
        "Add the next match of the search pattern as a selection.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_next(v, false, true)),
    ),
    (
        "extend-search-previous",
        "Add the match the other way as a selection.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_next(v, true, true)),
    ),
    (
        "search-selection",
        "Search for the primary selection's text.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.search_selection(v)),
    ),
    (
        "search-clear",
        "Stop highlighting the search pattern's matches.",
        |ed, _| ed.search_clear(),
    ),
    (
        "replace",
        "Replace in the selections: the argument is /pattern/replacement/.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.replace(v, false, cx.arg.unwrap_or(""))),
    ),
    (
        "replace-all",
        "Replace in the whole text: the argument is /pattern/replacement/.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.replace(v, true, cx.arg.unwrap_or(""))),
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
    ("config-reload", "Read the config file again.", |ed, _| {
        ed.request(Request::LoadConfig);
        Effects::default()
    }),
    ("previous-view", "Focus the previous view.", |ed, _| {
        ed.cycle_view(false)
    }),
    (
        "split",
        "Split the view: a second view of the document below it.",
        |ed, _| ed.split(false),
    ),
    (
        "vsplit",
        "Split the view: a second view of the document beside it.",
        |ed, _| ed.split(true),
    ),
    ("only", "Show only the focused view.", |ed, _| ed.only()),
    ("next-tile", "Focus the next view on screen.", |ed, _| {
        ed.cycle_tile(true)
    }),
    (
        "previous-tile",
        "Focus the previous view on screen.",
        |ed, _| ed.cycle_tile(false),
    ),
];

/// Commands of modes, selections, the prompt, and the command line.
static MODAL: &[(&str, &str, Body)] = &[
    ("normal-mode", "Enter normal mode.", |ed, cx| {
        on_view(ed, cx, |ed, v| ed.set_mode(v, ModeId::NORMAL))
    }),
    (
        "extend-mode",
        "Toggle extend mode (motions extend the selections).",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let to = if ed.views[v].mode == ModeId::EXTEND {
                    ModeId::NORMAL
                } else {
                    ModeId::EXTEND
                };
                ed.set_mode(v, to)
            })
        },
    ),
    ("insert-mode", "Insert before each selection.", |ed, cx| {
        on_view(ed, cx, |ed, v| {
            let fx = ed.map_ranges(v, |_, r| Ok(Range::new(r.end(), r.start())));
            fx.or(ed.set_mode(v, ModeId::INSERT))
        })
    }),
    ("append-mode", "Insert after each selection.", |ed, cx| {
        on_view(ed, cx, |ed, v| {
            let fx = ed.map_ranges(v, |_, r| Ok(Range::new(r.start(), r.end())));
            fx.or(ed.set_mode(v, ModeId::INSERT))
        })
    }),
    (
        "insert-line-start",
        "Insert at the start of each cursor's line.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.collapse_to_cursors(v);
                let fx = fx.or(ed.motion(v, Motion::LineStart, Movement::Move));
                fx.or(ed.set_mode(v, ModeId::INSERT))
            })
        },
    ),
    (
        "append-line-end",
        "Insert at the end of each cursor's line.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.collapse_to_cursors(v);
                let fx = fx.or(ed.motion(v, Motion::LineEnd, Movement::Move));
                fx.or(ed.set_mode(v, ModeId::INSERT))
            })
        },
    ),
    (
        "open-below",
        "Open a line below each cursor and insert there.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.collapse_to_cursors(v);
                let fx = fx.or(ed.motion(v, Motion::LineEnd, Movement::Move));
                let fx = fx.or(ed.set_mode(v, ModeId::INSERT));
                fx.or(ed.edit(v, true, |t, sel, ending| {
                    Transaction::insert(t, sel, ending.as_str())
                }))
            })
        },
    ),
    (
        "open-above",
        "Open a line above each cursor and insert there.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.collapse_to_cursors(v);
                let fx = fx.or(ed.motion(v, Motion::LineStart, Movement::Move));
                let fx = fx.or(ed.set_mode(v, ModeId::INSERT));
                // One edit (so `O` and what is typed next undo together): the
                // line breaks go in, each cursor stays before its own.
                fx.or(ed.edit(v, true, |t, sel, ending| {
                    let txn = Transaction::insert(t, sel, ending.as_str())?;
                    let len = ending.as_str().len();
                    let before = sel
                        .ranges()
                        .iter()
                        .enumerate()
                        .map(|(i, r)| Range::point(r.head() + i * len));
                    Ok(match Selection::new(before, sel.primary_index()) {
                        Ok(after) => Transaction::new(txn.changes().clone(), Some(after)),
                        Err(_) => txn,
                    })
                }))
            })
        },
    ),
    (
        "select-next-word",
        "Select to the next word start (count words).",
        |ed, cx| on_view(ed, cx, |ed, v| ed.select_word(v, true, cx.count())),
    ),
    (
        "select-previous-word",
        "Select back to the word start (count words).",
        |ed, cx| on_view(ed, cx, |ed, v| ed.select_word(v, false, cx.count())),
    ),
    (
        "select-line",
        "Select each selection's whole lines; again, the next line too.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.map_ranges(v, select_lines)),
    ),
    ("select-all", "Select the whole document.", |ed, cx| {
        on_view(ed, cx, |ed, v| {
            ed.reselect(v, |text, _| {
                Ok::<_, String>(Selection::single(Range::new(0, text.byte_len())))
            })
        })
    }),
    (
        "collapse",
        "Reduce each selection to its cursor.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let block = ed.block_cursor(v);
                ed.map_ranges(v, move |text, r| {
                    if block {
                        Ok(mode::block(text, mode::cursor_grapheme(text, r)?))
                    } else {
                        Ok(Range::point(r.head()))
                    }
                })
            })
        },
    ),
    (
        "delete-selection",
        "Cut every selection (to the register).",
        |ed, cx| on_view(ed, cx, |ed, v| ed.copy(v, true)),
    ),
    (
        "change-selection",
        "Cut every selection and insert in its place.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.copy(v, true);
                fx.or(ed.set_mode(v, ModeId::INSERT))
            })
        },
    ),
    (
        "paste-after",
        "Paste the register after each selection.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.map_ranges(v, |_, r| Ok(Range::point(r.end())));
                fx.or(ed.paste_register(v))
            })
        },
    ),
    (
        "paste-before",
        "Paste the register before each selection.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                let fx = ed.map_ranges(v, |_, r| Ok(Range::point(r.start())));
                fx.or(ed.paste_register(v))
            })
        },
    ),
    (
        "count",
        "Add the argument digit to the count for the next command.",
        |ed, cx| {
            let digit = cx
                .arg
                .and_then(|a| a.parse::<usize>().ok())
                .filter(|&d| d < 10);
            let (Some(digit), prev) = (digit, cx.count.map(NonZeroUsize::get)) else {
                return ed.notify("count: expected a digit");
            };
            ed.count = prev
                .unwrap_or(0)
                .checked_mul(10)
                .and_then(|n| n.checked_add(digit))
                .and_then(NonZeroUsize::new)
                .or(cx.count);
            Effects {
                pending: true,
                ..Effects::default()
            }
        },
    ),
    (
        "select-regex",
        "Select the regex's matches inside the selections.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.regex_command(v, RegexAction::Select, cx.arg)
            })
        },
    ),
    (
        "split-regex",
        "Split the selections on the regex's matches.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.regex_command(v, RegexAction::Split, cx.arg)
            })
        },
    ),
    (
        "split-lines",
        "Split the selections into their lines.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.regex_command(v, RegexAction::Split, Some(r"\r?\n"))
            })
        },
    ),
    (
        "keep-matching",
        "Keep the selections that contain a regex match.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.regex_command(v, RegexAction::Keep, cx.arg)
            })
        },
    ),
    (
        "remove-matching",
        "Remove the selections that contain a regex match.",
        |ed, cx| {
            on_view(ed, cx, |ed, v| {
                ed.regex_command(v, RegexAction::Remove, cx.arg)
            })
        },
    ),
    (
        "rotate-primary-forward",
        "Make the next selection the primary.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.rotate_primary(v, true)),
    ),
    (
        "rotate-primary-backward",
        "Make the previous selection the primary.",
        |ed, cx| on_view(ed, cx, |ed, v| ed.rotate_primary(v, false)),
    ),
    (
        "command-line",
        "Run a command by name: `name [argument]`.",
        |ed, _| ed.open_prompt(PromptKind::Command, ":"),
    ),
    ("prompt-accept", "Accept the prompt's input.", |ed, _| {
        ed.answer(true)
    }),
    ("prompt-cancel", "Close the prompt.", |ed, _| {
        ed.answer(false)
    }),
    ("prompt-yes", "Answer yes.", |ed, _| ed.answer(true)),
    ("prompt-no", "Answer no.", |ed, _| ed.answer(false)),
    (
        "prompt-backspace",
        "Delete the character before the prompt cursor.",
        |ed, _| ed.prompt_edit(Prompt::backspace),
    ),
    (
        "prompt-delete",
        "Delete the character after the prompt cursor.",
        |ed, _| ed.prompt_edit(Prompt::delete),
    ),
    ("prompt-left", "Move the prompt cursor left.", |ed, _| {
        ed.prompt_edit(Prompt::left)
    }),
    ("prompt-right", "Move the prompt cursor right.", |ed, _| {
        ed.prompt_edit(Prompt::right)
    }),
    (
        "prompt-home",
        "Move the prompt cursor to the start.",
        |ed, _| ed.prompt_edit(Prompt::home),
    ),
    (
        "prompt-end",
        "Move the prompt cursor to the end.",
        |ed, _| ed.prompt_edit(Prompt::end),
    ),
];

/// `r` widened to whole lines (with terminators); if it already is, one
/// more line.
fn select_lines(text: &crate::Text, r: Range) -> Result<Range, crate::TextError> {
    let start = text.line_start(text.line_of_offset(r.start())?)?;
    let last = if r.end() > r.start() {
        r.end() - 1
    } else {
        r.end()
    };
    let line_end = |line: usize| -> Result<usize, crate::TextError> {
        if line + 1 < text.line_count() {
            text.line_start(line + 1)
        } else {
            Ok(text.byte_len())
        }
    };
    let mut end = line_end(text.line_of_offset(last)?)?;
    if (start, end) == (r.start(), r.end()) && end < text.byte_len() {
        end = line_end(text.line_of_offset(end)?)?;
    }
    Ok(Range::new(start, end))
}

/// Registers every built-in command.
pub(crate) fn register(ed: &mut Editor) {
    for &(name, doc, body) in COMMANDS.iter().chain(MODAL) {
        let run: Arc<CommandFn> = Arc::new(body);
        let registered = ed.commands.register(name, Owner::BUILTIN, doc, run);
        debug_assert!(registered.is_ok(), "built-in {name}: {registered:?}");
    }
    for name in MOTIONS {
        for (prefix, movement) in [("move", Movement::Move), ("extend", Movement::Extend)] {
            let full = format!("{prefix}-{name}");
            let doc = format!(
                "{prefix} each selection: {name} (count repeats; in normal and extend \
                 mode the cursor is a grapheme)."
            );
            let extend = movement == Movement::Extend;
            let run: Arc<CommandFn> = Arc::new(move |ed: &mut Editor, cx: &Context<'_>| {
                on_view(ed, cx, |ed, v| match motion_of(ed, v, name, cx.count()) {
                    Some(m) => ed.mode_motion(v, m, extend),
                    None => Effects::default(),
                })
            });
            let registered = ed.commands.register(&full, Owner::BUILTIN, &doc, run);
            debug_assert!(registered.is_ok(), "built-in {full}: {registered:?}");
        }
    }
}
