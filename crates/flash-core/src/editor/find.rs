//! Search and replace (PLAN Stage 7b).
//!
//! `/` and `?` open a prompt; as it is typed, the next (or previous) match
//! from where the search started is selected, wrapping around the text.
//! Enter keeps it and makes the pattern the current one; Esc restores the
//! selection. `n`/`N` repeat it from the primary selection (in extend mode
//! adding the match as a new selection), `*` makes the selected text the
//! pattern, and `:s/pattern/replacement/` (in the selections) or
//! `:%s/…/…/` (the whole text) replace with `$1`/`${name}` captures, as
//! one undo step.
//!
//! A document of [`INLINE_MAX`] bytes or more is searched off the main
//! thread: the editor queues a [`Request::Search`] with a snapshot of the
//! text (O(1)) and its revision, and the frontend runs it
//! ([`SearchJob::run`]) and reports back with [`Editor::search_done`]. Only
//! the latest search's result is applied; one computed against an older
//! revision is mapped forward through the revision log, or dropped if it
//! cannot be. A replay runs searches synchronously, so traces stay exact.

use std::fmt;
use std::ops::Range as Span;

use super::{DocumentId, Editor, Effects, PromptKind, Request, ViewId};
use crate::changeset::{Assoc, ChangeSet};
use crate::document::Revision;
use crate::search::{Matcher, escape};
use crate::selection::{Range, Selection};
use crate::text::Text;
use crate::transaction::Transaction;

#[cfg(test)]
mod tests;

/// Documents at least this long are searched off the main thread.
pub const INLINE_MAX: usize = 256 * 1024;

/// A search to run off the main thread.
#[derive(Clone)]
pub struct SearchJob {
    pub id: u64,
    pub doc: DocumentId,
    pub revision: Revision,
    /// A snapshot of the document's text at `revision`.
    pub text: Text,
    pub matcher: Matcher,
    /// Search from here: the first match starting at or after it, or
    /// (backward) the last starting before it.
    pub from: usize,
    pub backward: bool,
}

impl SearchJob {
    /// Runs the search (any thread). O(bytes searched).
    pub fn run(&self) -> SearchDone {
        SearchDone {
            id: self.id,
            doc: self.doc,
            revision: self.revision,
            found: find(&self.matcher, &self.text, self.from, self.backward),
        }
    }
}

impl fmt::Debug for SearchJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchJob")
            .field("id", &self.id)
            .field("doc", &self.doc)
            .field("revision", &self.revision)
            .field("from", &self.from)
            .field("backward", &self.backward)
            .finish_non_exhaustive()
    }
}

/// Jobs are equal if they are the same request (the snapshot and pattern
/// follow from it).
impl PartialEq for SearchJob {
    fn eq(&self, other: &Self) -> bool {
        (self.id, self.doc, self.revision, self.from, self.backward)
            == (
                other.id,
                other.doc,
                other.revision,
                other.from,
                other.backward,
            )
    }
}

impl Eq for SearchJob {}

/// A finished [`SearchJob`]: the match and whether the search wrapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchDone {
    pub id: u64,
    pub doc: DocumentId,
    pub revision: Revision,
    pub found: Option<(Span<usize>, bool)>,
}

fn find(
    matcher: &Matcher,
    text: &Text,
    from: usize,
    backward: bool,
) -> Option<(Span<usize>, bool)> {
    if backward {
        matcher.previous_before(text, from)
    } else {
        matcher.next_after(text, from)
    }
}

/// The search state of the editor.
#[derive(Debug, Clone, Default)]
pub(crate) struct Search {
    /// The current pattern (the last accepted, or the one being typed).
    pattern: String,
    matcher: Option<Matcher>,
    /// Whether the current search goes backward.
    backward: bool,
    /// Whether to highlight the current pattern's matches.
    highlight: bool,
    /// While the search prompt is open: the input last searched for, the
    /// selection to restore, and the pattern to restore on Esc.
    typed: String,
    origin: Option<(ViewId, Selection)>,
    previous: Option<(String, Option<Matcher>, bool)>,
    /// The search whose result is awaited.
    pending: Option<Pending>,
    next_id: u64,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    id: u64,
    view: ViewId,
    extend: bool,
    /// An incremental search: restore the origin if nothing matches.
    incremental: bool,
}

impl Editor {
    /// The pattern whose matches a renderer highlights, if any.
    pub fn search_highlight(&self) -> Option<&Matcher> {
        self.search
            .matcher
            .as_ref()
            .filter(|_| self.search.highlight)
    }

    /// The current search pattern ("" before the first search).
    pub fn search_pattern(&self) -> &str {
        &self.search.pattern
    }

    /// Opens the search prompt (`/`, or `?` with `backward`).
    pub(crate) fn search_prompt(&mut self, view: ViewId, backward: bool) -> Effects {
        let Some(selection) = self.selection(view).cloned() else {
            return Effects::default();
        };
        self.search.previous = Some((
            self.search.pattern.clone(),
            self.search.matcher.clone(),
            self.search.backward,
        ));
        self.search.origin = Some((view, selection));
        self.search.typed.clear();
        self.search.backward = backward;
        self.open_prompt(
            PromptKind::Search { backward },
            if backward { "?" } else { "/" },
        )
    }

    /// After an event while the search prompt is open: searches again if
    /// the input changed.
    pub(crate) fn search_incremental(&mut self) -> Effects {
        let Some(input) = self
            .prompt
            .as_ref()
            .filter(|p| matches!(p.kind(), PromptKind::Search { .. }))
            .map(|p| p.input())
        else {
            return Effects::default();
        };
        if input == self.search.typed {
            return Effects::default();
        }
        self.search.typed = input.to_owned();
        let Some((view, origin)) = self.search.origin.clone() else {
            return Effects::default();
        };
        self.search.pending = None;
        if self.search.typed.is_empty() {
            self.search.matcher = None;
            return self.restore(view, origin);
        }
        match Matcher::new(&self.search.typed) {
            Ok(m) => {
                self.search.pattern = self.search.typed.clone();
                self.search.matcher = Some(m);
                self.search.highlight = true;
                let backward = self.search.backward;
                self.run_search(view, from_of(&origin, backward), backward, false, true)
            }
            // Not yet a valid pattern: say why, keep what is selected.
            Err(e) => self.notify(e),
        }
    }

    /// The search prompt closed: `accept` keeps the match, else the
    /// selection and the pattern from before the prompt come back.
    pub(crate) fn search_closed(&mut self, accept: bool) -> Effects {
        let origin = self.search.origin.take();
        let previous = self.search.previous.take();
        self.search.typed.clear();
        if accept && self.search.matcher.is_some() {
            return Effects::default();
        }
        self.search.pending = None;
        if let Some((pattern, matcher, backward)) = previous {
            self.search.pattern = pattern;
            self.search.matcher = matcher;
            self.search.backward = backward;
        }
        match origin {
            Some((view, selection)) => self.restore(view, selection),
            None => Effects::default(),
        }
    }

    /// `n` (or `N` with `reverse`): the next match of the current pattern
    /// from the primary selection; with `extend`, added as a new primary
    /// selection.
    pub(crate) fn search_next(&mut self, view: ViewId, reverse: bool, extend: bool) -> Effects {
        if self.search.matcher.is_none() {
            return self.notify("no search pattern");
        }
        let Some(selection) = self.selection(view).cloned() else {
            return Effects::default();
        };
        let backward = self.search.backward != reverse;
        self.search.highlight = true;
        self.run_search(view, from_of(&selection, backward), backward, extend, false)
    }

    /// `*`: the primary selection's text, escaped, becomes the pattern.
    pub(crate) fn search_selection(&mut self, view: ViewId) -> Effects {
        let Some((_, doc)) = self.view_document(view) else {
            return Effects::default();
        };
        let Some(r) = doc.selection(view).map(|s| *s.primary()) else {
            return Effects::default();
        };
        let Ok(slice) = doc.text().slice(r.start()..r.end()) else {
            return Effects::default();
        };
        let selected = slice.to_string();
        if selected.is_empty() {
            return self.notify("nothing selected to search for");
        }
        let pattern = escape(&selected);
        match Matcher::new(&pattern) {
            Ok(m) => {
                self.search.matcher = Some(m);
                self.search.pattern = pattern;
                self.search.backward = false;
                self.search.highlight = true;
                let message = format!("search: {}", self.search.pattern);
                self.notify(message)
            }
            Err(e) => self.notify(e),
        }
    }

    /// Stops highlighting matches (the pattern stays for `n`).
    pub(crate) fn search_clear(&mut self) -> Effects {
        let was = std::mem::take(&mut self.search.highlight);
        Effects {
            selection: was,
            ..Effects::default()
        }
    }

    /// Searches `view`'s document from `from` with the current pattern,
    /// `backward` or forward: at once for a short document, else by a
    /// request.
    fn run_search(
        &mut self,
        view: ViewId,
        from: usize,
        backward: bool,
        extend: bool,
        incremental: bool,
    ) -> Effects {
        let (Some(matcher), Some((doc_id, doc))) = (
            self.search.matcher.clone(),
            self.views
                .get(view)
                .and_then(|v| Some((v.doc, self.documents.get(v.doc)?))),
        ) else {
            return Effects::default();
        };
        self.search.next_id += 1;
        let id = self.search.next_id;
        self.search.pending = Some(Pending {
            id,
            view,
            extend,
            incremental,
        });
        let job = SearchJob {
            id,
            doc: doc_id,
            revision: doc.revision(),
            text: doc.text().clone(),
            matcher,
            from,
            backward,
        };
        if doc.text().byte_len() < INLINE_MAX {
            let done = job.run();
            self.search_done(done)
        } else {
            self.request(Request::Search(job));
            Effects {
                requests: true,
                ..self.notify("searching…")
            }
        }
    }

    /// Reports a finished search (from [`Request::Search`]). Applied only
    /// if it is the latest search and its view still shows the document;
    /// a match computed against an older revision is mapped forward, or
    /// dropped if the log no longer reaches back to it.
    pub fn search_done(&mut self, done: SearchDone) -> Effects {
        let Some(pending) = self.search.pending.filter(|p| p.id == done.id) else {
            return Effects::default();
        };
        self.search.pending = None;
        let view = pending.view;
        if self.views.get(view).map(|v| v.doc) != Some(done.doc) {
            return Effects::default();
        }
        let Some(doc) = self.documents.get(done.doc) else {
            return Effects::default();
        };
        let found = match done.found {
            Some((span, wrapped)) => {
                let mut ends = [span.start, span.end];
                // Start after text inserted at it, end before: the match
                // keeps its own text.
                let mapped = if done.revision == doc.revision() {
                    Ok(())
                } else {
                    doc.map_from(done.revision, &mut ends[..1], Assoc::After)
                        .and_then(|()| doc.map_from(done.revision, &mut ends[1..], Assoc::Before))
                };
                match mapped {
                    Ok(()) if ends[0] < ends[1] => Some((ends[0]..ends[1], wrapped)),
                    _ => return self.notify("the text changed during the search; search again"),
                }
            }
            None => None,
        };
        let Some((span, wrapped)) = found else {
            let message = format!("no match for {}", self.search.pattern);
            let mut fx = self.notify(message);
            if pending.incremental {
                if let Some((v, origin)) = self.search.origin.clone() {
                    fx = fx.or(self.restore(v, origin));
                }
            }
            return fx;
        };
        let Some(current) = self.selection(view).cloned() else {
            return Effects::default();
        };
        let range = Range::new(span.start, span.end);
        let next = if pending.extend {
            current.with_range(range)
        } else {
            Selection::single(range)
        };
        let mut fx = match self.set_selection(view, next) {
            Ok(()) => Effects {
                selection: true,
                ..Effects::default()
            },
            Err(e) => return self.notify(e),
        };
        fx = fx.or(if wrapped {
            self.notify("search wrapped around")
        } else {
            self.notify("")
        });
        fx
    }

    /// Puts `selection` back on `view` (if still valid).
    fn restore(&mut self, view: ViewId, selection: Selection) -> Effects {
        match self.set_selection(view, selection) {
            Ok(()) => Effects {
                selection: true,
                ..Effects::default()
            },
            Err(_) => Effects::default(),
        }
    }

    /// `:s/pattern/replacement/` in the selections, or the whole text with
    /// `whole`: every match replaced, `$1`/`${name}`/`$$` expanded, as one
    /// undo step.
    pub(crate) fn replace(&mut self, view: ViewId, whole: bool, spec: &str) -> Effects {
        let Some((pattern, replacement)) = parse_substitution(spec) else {
            return self.notify("usage: s/pattern/replacement/ (any delimiter; flag i)");
        };
        let matcher = match Matcher::new(&pattern) {
            Ok(m) => m,
            Err(e) => return self.notify(e),
        };
        let mut count = 0;
        let mut error = None;
        let fx = self.edit(view, false, |text, sel, _| {
            let spans: Vec<Span<usize>> = if whole {
                std::iter::once(0..text.byte_len()).collect()
            } else {
                sel.ranges().iter().map(|r| r.start()..r.end()).collect()
            };
            let mut edits = Vec::new();
            for span in spans {
                match matcher.replacements(text, span, &replacement) {
                    Ok(e) => edits.extend(e),
                    Err(e) => error = Some(e.to_string()),
                }
            }
            count = edits.len();
            let changes = ChangeSet::from_edits(text.byte_len(), edits)
                .map_err(crate::transaction::TransactionError::from)?;
            Ok(Transaction::new(changes, None))
        });
        if let Some(e) = error {
            return fx.or(self.notify(e));
        }
        let message = match count {
            0 if whole => format!("no match for {pattern}"),
            0 => format!("no match for {pattern} in the selections (:%s for the whole text)"),
            1 => "1 replacement".to_owned(),
            n => format!("{n} replacements"),
        };
        self.search.pattern = pattern;
        self.search.matcher = Some(matcher);
        fx.or(self.notify(message))
    }
}

/// Where a search from `selection` starts: after the primary selection
/// going forward (so a match at the cursor is skipped), at its start
/// going backward.
fn from_of(selection: &Selection, backward: bool) -> usize {
    let p = selection.primary();
    if backward { p.start() } else { p.end() }
}

/// Parses `/pattern/replacement/flags` (the first char is the delimiter;
/// a backslash before it makes it literal; flag `i` ignores case).
fn parse_substitution(spec: &str) -> Option<(String, String)> {
    let mut chars = spec.chars();
    let delim = chars
        .next()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace() && *c != '\\')?;
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in chars {
        if escaped {
            if c != delim {
                parts.last_mut()?.push('\\');
            }
            parts.last_mut()?.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delim {
            parts.push(String::new());
        } else {
            parts.last_mut()?.push(c);
        }
    }
    if escaped {
        parts.last_mut()?.push('\\');
    }
    let (pattern, replacement, flags) = match parts.len() {
        2 => (parts[0].clone(), parts[1].clone(), String::new()),
        3 => (parts[0].clone(), parts[1].clone(), parts[2].clone()),
        _ => return None,
    };
    if pattern.is_empty() || flags.chars().any(|f| f != 'i' && f != 'g') {
        return None;
    }
    let pattern = if flags.contains('i') {
        format!("(?i){pattern}")
    } else {
        pattern
    };
    Some((pattern, replacement))
}
