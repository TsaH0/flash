//! Modes (PLAN 2.5, Stage 7): data, registered through the same API
//! plugins will use, held per view.
//!
//! A mode's flags say how keys and selections behave in it:
//! `insert_unbound` (an unbound printable key is inserted), `min_grapheme`
//! (every range covers at least one grapheme, PLAN 2.5's normal mode), and
//! `extend` (motions extend instead of move). Keys come from the mode's
//! keymap layers; the cursor shape from the `cursor-shape-<mode>` option.
//!
//! # The block cursor
//!
//! In a `min_grapheme` mode the cursor is a grapheme, drawn as a block: the
//! last grapheme of a forward range, the first of a backward one, the one
//! at the head of an empty range (only at the end of the text). Motions
//! there run from the cursor grapheme's start; a move makes the range that
//! grapheme alone, an extend keeps the anchor grapheme and spans to the new
//! cursor grapheme (flipping direction when they cross).

use std::collections::HashMap;
use std::fmt;

#[cfg(test)]
mod tests;

use super::owner::Owner;
use super::{Editor, Effects, OptionId, OptionKind, OptionValue, ViewId};
use crate::motion::{Class, class};
use crate::selection::{Range, Selection};
use crate::text::{Direction, Text, TextError};
use crate::{Motion, Movement};

/// A registered mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModeId(pub(crate) u32);

impl ModeId {
    pub const NORMAL: ModeId = ModeId(0);
    pub const INSERT: ModeId = ModeId(1);
    pub const EXTEND: ModeId = ModeId(2);
    /// The one-line text prompt (file names, the command line).
    pub const PROMPT: ModeId = ModeId(3);
    /// A yes/no question in the prompt line.
    pub const QUESTION: ModeId = ModeId(4);
}

/// How a mode treats keys and selections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeFlags {
    pub insert_unbound: bool,
    pub min_grapheme: bool,
    pub extend: bool,
}

/// How the terminal cursor is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

const SHAPES: &[&str] = &["block", "bar", "underline"];

/// Error from registering a mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeError {
    Duplicate(String),
    BadName(String),
}

impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModeError::Duplicate(n) => write!(f, "mode {n:?} already exists"),
            ModeError::BadName(n) => write!(f, "bad mode name {n:?}"),
        }
    }
}

impl std::error::Error for ModeError {}

#[derive(Debug, Clone)]
struct ModeDef {
    name: String,
    owner: Owner,
    flags: ModeFlags,
    cursor_shape: OptionId,
}

/// One mode as listed by [`Modes::iter`].
#[derive(Debug, Clone, Copy)]
pub struct ModeInfo<'a> {
    pub id: ModeId,
    pub name: &'a str,
    pub owner: Owner,
    pub flags: ModeFlags,
}

/// The mode registry.
#[derive(Debug, Clone, Default)]
pub struct Modes {
    defs: Vec<Option<ModeDef>>,
    by_name: HashMap<String, ModeId>,
}

impl Modes {
    pub fn id(&self, name: &str) -> Option<ModeId> {
        self.by_name.get(name).copied()
    }

    pub fn info(&self, id: ModeId) -> Option<ModeInfo<'_>> {
        let d = self.defs.get(id.0 as usize)?.as_ref()?;
        Some(ModeInfo {
            id,
            name: &d.name,
            owner: d.owner,
            flags: d.flags,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = ModeInfo<'_>> {
        (0..self.defs.len()).filter_map(|i| self.info(ModeId(i as u32)))
    }

    /// The flags of `id`; a removed mode behaves as normal mode.
    pub fn flags(&self, id: ModeId) -> ModeFlags {
        self.info(id)
            .or_else(|| self.info(ModeId::NORMAL))
            .map_or_else(ModeFlags::default, |i| i.flags)
    }

    fn cursor_option(&self, id: ModeId) -> Option<OptionId> {
        Some(self.defs.get(id.0 as usize)?.as_ref()?.cursor_shape)
    }
}

impl Editor {
    /// The mode registry.
    pub fn modes(&self) -> &Modes {
        &self.modes
    }

    /// Registers a mode and its `cursor-shape-<name>` option.
    pub fn register_mode(
        &mut self,
        name: &str,
        owner: Owner,
        flags: ModeFlags,
        cursor: CursorShape,
    ) -> Result<ModeId, ModeError> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ModeError::BadName(name.to_owned()));
        }
        if self.modes.by_name.contains_key(name) {
            return Err(ModeError::Duplicate(name.to_owned()));
        }
        let option = self
            .options
            .register(
                &format!("cursor-shape-{name}"),
                owner,
                "Cursor shape in this mode: block, bar, or underline.",
                OptionKind::Choice(SHAPES),
                OptionValue::Choice(cursor as u16),
            )
            .map_err(|_| ModeError::Duplicate(name.to_owned()))?;
        let id = ModeId(u32::try_from(self.modes.defs.len()).unwrap_or(u32::MAX));
        self.modes.defs.push(Some(ModeDef {
            name: name.to_owned(),
            owner,
            flags,
            cursor_shape: option,
        }));
        self.modes.by_name.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Removes the modes of `owner`: their views return to normal mode and
    /// their key layers go.
    pub(crate) fn remove_modes_of(&mut self, owner: Owner) {
        let ids: Vec<ModeId> = self
            .modes
            .iter()
            .filter(|m| m.owner == owner && m.id > ModeId::QUESTION)
            .map(|m| m.id)
            .collect();
        for id in ids {
            if let Some(def) = self
                .modes
                .defs
                .get_mut(id.0 as usize)
                .and_then(Option::take)
            {
                self.modes.by_name.remove(&def.name);
            }
            self.keymap.remove_mode(id);
            let views: Vec<ViewId> = self
                .views
                .iter()
                .filter(|(_, v)| v.mode == id)
                .map(|(k, _)| k)
                .collect();
            for v in views {
                self.set_mode(v, ModeId::NORMAL);
            }
        }
    }

    /// Registers the built-in modes at their `ModeId` constants.
    pub(crate) fn register_builtin_modes(&mut self) {
        let modes = [
            (
                "normal",
                ModeFlags {
                    min_grapheme: true,
                    ..ModeFlags::default()
                },
                CursorShape::Block,
            ),
            (
                "insert",
                ModeFlags {
                    insert_unbound: true,
                    ..ModeFlags::default()
                },
                CursorShape::Bar,
            ),
            (
                "extend",
                ModeFlags {
                    min_grapheme: true,
                    extend: true,
                    ..ModeFlags::default()
                },
                CursorShape::Block,
            ),
            (
                "prompt",
                ModeFlags {
                    insert_unbound: true,
                    ..ModeFlags::default()
                },
                CursorShape::Bar,
            ),
            ("question", ModeFlags::default(), CursorShape::Block),
        ];
        for (i, (name, flags, cursor)) in modes.into_iter().enumerate() {
            let id = self.register_mode(name, Owner::BUILTIN, flags, cursor);
            debug_assert_eq!(id, Ok(ModeId(i as u32)));
        }
    }

    /// The mode keys resolve in: the prompt's while one is open, else the
    /// focused view's.
    pub fn input_mode(&self) -> ModeId {
        match &self.prompt {
            Some(p) if p.kind().is_question() => ModeId::QUESTION,
            Some(_) => ModeId::PROMPT,
            None => self
                .focus
                .and_then(|v| self.views.get(v))
                .map_or(ModeId::NORMAL, |v| v.mode),
        }
    }

    /// The cursor shape for the input mode, as `view` resolves the mode's
    /// option.
    pub fn cursor_shape(&self, view: Option<ViewId>) -> CursorShape {
        let shape = self
            .modes
            .cursor_option(self.input_mode())
            .and_then(|o| self.option(o, view));
        match shape {
            Some(OptionValue::Choice(1)) => CursorShape::Bar,
            Some(OptionValue::Choice(2)) => CursorShape::Underline,
            _ => CursorShape::Block,
        }
    }

    /// Puts `view` in `mode`: ends the undo group (through the built-in
    /// mode-change hook), drops a pending key sequence, and in a
    /// `min_grapheme` mode widens empty ranges.
    pub fn set_mode(&mut self, view: ViewId, mode: ModeId) -> Effects {
        let Some(v) = self.views.get_mut(view) else {
            return Effects::default();
        };
        if self.modes.info(mode).is_none() {
            return self.notify(format_args!("no mode #{}", mode.0));
        }
        let from = v.mode;
        v.mode = mode;
        self.keymap.cancel();
        let mut fx = Effects::default();
        if from != mode {
            fx.mode = true;
            // The hook checkpoints the undo group (PLAN Stage 5); the
            // checkpoint is also made here so it holds without hooks.
            if let Ok(doc) = self.document_of_mut(view) {
                doc.checkpoint();
            }
            self.emit(super::EditorEvent::ModeChanged {
                view,
                from,
                to: mode,
            });
        }
        fx.or(self.enforce_min(view))
    }

    /// In a `min_grapheme` mode (and with the option on), widens every
    /// empty range to the grapheme after it (empty only at the end of the
    /// text). Allocates only when some range is empty.
    pub(crate) fn enforce_min(&mut self, view: ViewId) -> Effects {
        let Some(v) = self.views.get(view) else {
            return Effects::default();
        };
        if !self.modes.flags(v.mode).min_grapheme || !v.options.normal_min_grapheme {
            return Effects::default();
        }
        let Some(doc) = self.documents.get(v.doc) else {
            return Effects::default();
        };
        let Some(sel) = doc.selection(view) else {
            return Effects::default();
        };
        let text = doc.text();
        let len = text.byte_len();
        if !sel.ranges().iter().any(|r| r.is_empty() && r.head() < len) {
            return Effects::default();
        }
        let ranges = sel.ranges().iter().map(|&r| {
            if r.is_empty() && r.head() < len {
                block(text, r.head()).with_goal_column(r.goal_column())
            } else {
                r
            }
        });
        let primary = sel.primary_index();
        match Selection::new(ranges, primary) {
            Ok(next) => {
                if let Ok(doc) = self.document_of_mut(view) {
                    if doc.set_selection(view, next).is_ok() {
                        self.emit(super::EditorEvent::SelectionChanged { view });
                        return Effects {
                            selection: true,
                            ..Effects::default()
                        };
                    }
                }
                Effects::default()
            }
            Err(_) => Effects::default(),
        }
    }

    /// Applies `motion` to `view` the way its mode does: block moves or
    /// extends in a `min_grapheme` mode, point moves elsewhere; `extend`
    /// (or an extending mode) extends.
    pub(crate) fn mode_motion(&mut self, view: ViewId, motion: Motion, extend: bool) -> Effects {
        let Some(v) = self.views.get(view) else {
            return Effects::default();
        };
        let extend = extend || self.modes.flags(v.mode).extend;
        if !self.block_cursor(view) {
            let movement = if extend {
                Movement::Extend
            } else {
                Movement::Move
            };
            return self.motion(view, motion, movement);
        }
        self.map_ranges(view, |text, r| {
            let (a, c) = (anchor_grapheme(text, r)?, cursor_grapheme(text, r)?);
            let from = Range::point(c).with_goal_column(r.goal_column());
            let moved = motion.apply_range(text, from, Movement::Move)?;
            let h = moved.head();
            let next = if extend {
                span(text, a, h)
            } else {
                block(text, h)
            };
            Ok(next.with_goal_column(moved.goal_column()))
        })
    }

    /// Whether `view`'s cursor is a grapheme (a `min_grapheme` mode with
    /// the option on).
    pub(crate) fn block_cursor(&self, view: ViewId) -> bool {
        self.views
            .get(view)
            .is_some_and(|v| self.modes.flags(v.mode).min_grapheme && v.options.normal_min_grapheme)
    }

    /// Where `view`'s primary cursor is drawn: the start of the cursor
    /// grapheme with a block cursor, else the head. Scrolling keeps it
    /// visible and the view's [`LineAnchor`](super::LineAnchor) describes
    /// it.
    pub fn cursor_position(&self, view: ViewId) -> Option<usize> {
        let v = self.views.get(view)?;
        let doc = self.documents.get(v.doc)?;
        let r = *doc.selection(view)?.primary();
        if self.block_cursor(view) {
            cursor_grapheme(doc.text(), r).ok()
        } else {
            Some(r.head())
        }
    }

    /// Replaces each range with a point at its cursor: the cursor
    /// grapheme's start with a block cursor, else the head.
    pub(crate) fn collapse_to_cursors(&mut self, view: ViewId) -> Effects {
        let block = self.block_cursor(view);
        self.map_ranges(view, move |text, r| {
            Ok(Range::point(if block {
                cursor_grapheme(text, r)?
            } else {
                r.head()
            }))
        })
    }

    /// Selects from each cursor to the next word start (`forward`) or back
    /// to the word start (Kakoune's `w`/`b`): if the cursor grapheme and
    /// the next (previous) one differ in class, it starts from that one,
    /// so repeating always advances. `count` words.
    pub(crate) fn select_word(&mut self, view: ViewId, forward: bool, count: usize) -> Effects {
        self.map_ranges(view, |text, r| {
            let mut c = cursor_grapheme(text, r)?;
            if forward {
                let n = next(text, c);
                if n < text.byte_len() && grapheme_class(text, c) != grapheme_class(text, n) {
                    c = n;
                }
                let word = Motion::Word {
                    count,
                    direction: Direction::Forward,
                };
                let end = word
                    .apply_range(text, Range::point(c), Movement::Move)?
                    .head();
                Ok(if end > c {
                    Range::new(c, end)
                } else {
                    block(text, c)
                })
            } else {
                if c > 0 {
                    let p = prev(text, c);
                    if grapheme_class(text, p) != grapheme_class(text, c) {
                        c = p;
                    }
                }
                let end = next(text, c);
                let word = Motion::Word {
                    count,
                    direction: Direction::Backward,
                };
                let start = word
                    .apply_range(text, Range::point(end), Movement::Move)?
                    .head();
                Ok(Range::new(end, start))
            }
        })
    }

    /// Replaces every range of `view` with `f`'s result (validated and
    /// normalised); a motion boundary for undo grouping.
    pub(crate) fn map_ranges(
        &mut self,
        view: ViewId,
        f: impl Fn(&Text, Range) -> Result<Range, TextError>,
    ) -> Effects {
        self.reselect(view, |text, sel| {
            let ranges = sel
                .ranges()
                .iter()
                .map(|&r| f(text, r))
                .collect::<Result<Vec<Range>, TextError>>()
                .map_err(|e| e.to_string())?;
            Selection::new(ranges, sel.primary_index()).map_err(|e| e.to_string())
        })
    }
}

/// The start of the grapheme after `pos` (or `pos` at the end).
pub(crate) fn next(text: &Text, pos: usize) -> usize {
    text.move_graphemes(pos, 1, Direction::Forward)
        .unwrap_or(pos)
}

/// The start of the grapheme before `pos` (or `pos` at the start).
pub(crate) fn prev(text: &Text, pos: usize) -> usize {
    text.move_graphemes(pos, 1, Direction::Backward)
        .unwrap_or(pos)
}

/// The range covering the grapheme at `pos` (empty at the end).
pub(crate) fn block(text: &Text, pos: usize) -> Range {
    Range::new(pos, next(text, pos))
}

/// The start of `r`'s cursor grapheme.
pub(crate) fn cursor_grapheme(text: &Text, r: Range) -> Result<usize, TextError> {
    text.check_offset(r.head())?;
    Ok(if r.head() > r.anchor() {
        prev(text, r.head()).max(r.anchor())
    } else {
        r.head()
    })
}

/// The start of `r`'s anchor grapheme (the end it does not move).
fn anchor_grapheme(text: &Text, r: Range) -> Result<usize, TextError> {
    text.check_offset(r.anchor())?;
    Ok(if r.head() < r.anchor() {
        prev(text, r.anchor()).max(r.head())
    } else {
        r.anchor()
    })
}

/// The range from the grapheme at `a` to the grapheme at `c`, both
/// included, with its head on `c`'s side.
fn span(text: &Text, a: usize, c: usize) -> Range {
    if c >= a {
        Range::new(a, next(text, c))
    } else {
        Range::new(next(text, a), c)
    }
}

fn grapheme_class(text: &Text, pos: usize) -> Option<Class> {
    let mut g = text.graphemes(pos, Direction::Forward).ok()?;
    g.next().map(|g| class(&g))
}
