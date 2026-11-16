//! Multi-selection commands (PLAN Stage 7): select, split, keep, or
//! remove by regex inside the selections; split on lines; rotate the
//! primary.
//!
//! Patterns are searched only inside each selection, over the rope
//! (`crate::search`). Results are normalised by the PLAN 2.5 merge rule
//! (touching selections stay separate). A command that would leave no
//! selection changes nothing and says so.

use super::{Editor, Effects, PromptKind, ViewId};
use crate::search::Matcher;
use crate::selection::{Range, Selection};

#[cfg(test)]
mod tests;

/// What a regex selection command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexAction {
    /// Select every match inside the selections.
    Select,
    /// Split the selections on the matches (the matches are dropped).
    Split,
    /// Keep the selections that contain a match.
    Keep,
    /// Remove the selections that contain a match.
    Remove,
}

impl RegexAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RegexAction::Select => "select: ",
            RegexAction::Split => "split: ",
            RegexAction::Keep => "keep: ",
            RegexAction::Remove => "remove: ",
        }
    }
}

impl Editor {
    /// Applies `action` with `pattern` to `view`'s selections, or asks for
    /// the pattern.
    pub(crate) fn regex_command(
        &mut self,
        view: ViewId,
        action: RegexAction,
        pattern: Option<&str>,
    ) -> Effects {
        match pattern.filter(|p| !p.is_empty()) {
            Some(p) => self.regex_selection(view, action, p),
            None => self.open_prompt(PromptKind::Regex(action), action.label()),
        }
    }

    pub(crate) fn regex_selection(
        &mut self,
        view: ViewId,
        action: RegexAction,
        pattern: &str,
    ) -> Effects {
        let matcher = match Matcher::new(pattern) {
            Ok(m) => m,
            Err(e) => return self.notify(e),
        };
        self.reselect(view, |text, sel| {
            let mut out: Vec<Range> = Vec::new();
            let mut primary = None;
            for (i, r) in sel.ranges().iter().enumerate() {
                let before = out.len();
                let span = r.start()..r.end();
                match action {
                    RegexAction::Select => {
                        for m in matcher.find_in(text, span).map_err(|e| e.to_string())? {
                            out.push(Range::new(m.start, m.end));
                        }
                    }
                    RegexAction::Split => {
                        let mut at = r.start();
                        for m in matcher.find_in(text, span).map_err(|e| e.to_string())? {
                            if m.start > at {
                                out.push(Range::new(at, m.start));
                            }
                            at = m.end;
                        }
                        if r.end() > at {
                            out.push(Range::new(at, r.end()));
                        }
                    }
                    RegexAction::Keep | RegexAction::Remove => {
                        let hit = matcher.is_match_in(text, span).map_err(|e| e.to_string())?;
                        if hit == (action == RegexAction::Keep) {
                            out.push(*r);
                        }
                    }
                }
                if i == sel.primary_index() && out.len() > before {
                    primary = Some(before);
                }
            }
            if out.is_empty() {
                return Err(match action {
                    RegexAction::Select => format!("no matches for {pattern:?}"),
                    _ => "that would leave no selection".to_owned(),
                });
            }
            Selection::new(out, primary.unwrap_or(0)).map_err(|e| e.to_string())
        })
    }

    /// Makes the next (`forward`) or previous selection the primary.
    pub(crate) fn rotate_primary(&mut self, view: ViewId, forward: bool) -> Effects {
        self.reselect(view, |_, sel| {
            let n = sel.ranges().len();
            let i = sel.primary_index();
            let next = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            Selection::new(sel.ranges().iter().copied(), next).map_err(|e| e.to_string())
        })
    }
}
