//! Event hooks (PLAN Stage 7): owned subscriptions to what happens in the
//! editor, delivered in batches.
//!
//! The editor queues events as documents, selections, modes, and views
//! change, and delivers them when the outermost [`Editor::handle`] or
//! [`Editor::run`] returns (or on [`Editor::flush_events`] after direct
//! API calls). Each subscriber gets the whole batch, once, if it holds any
//! kind it asked for. Only kinds some subscriber wants are queued at all,
//! so an edit costs nothing here while nobody listens for edits. Events
//! raised by subscribers are delivered in
//! further rounds, at most [`MAX_ROUNDS`]; the rest are dropped and the
//! message says so, so hooks cannot recurse without bound.
//!
//! Text changes report the document (`DocumentChanged`); every view of it
//! had its selection mapped. `SelectionChanged` names the view whose
//! selection a command set.

use std::fmt;
use std::sync::Arc;

use super::owner::Owner;
use super::{DocumentId, Editor, Effects, ViewId};
use crate::document::Revision;

#[cfg(test)]
mod tests;

/// Rounds of delivery per outermost call.
pub const MAX_ROUNDS: usize = 8;

/// Something that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditorEvent {
    /// `doc`'s text went from revision `from` to `to`.
    DocumentChanged {
        doc: DocumentId,
        from: Revision,
        to: Revision,
    },
    /// A command set `view`'s selection.
    SelectionChanged {
        view: ViewId,
    },
    /// `view` changed mode (mode IDs as in [`super::ModeId`]).
    ModeChanged {
        view: ViewId,
        from: super::ModeId,
        to: super::ModeId,
    },
    ViewOpened {
        view: ViewId,
    },
    ViewClosed {
        view: ViewId,
    },
}

impl EditorEvent {
    pub fn kind(&self) -> EventKinds {
        match self {
            EditorEvent::DocumentChanged { .. } => EventKinds::DOCUMENT,
            EditorEvent::SelectionChanged { .. } => EventKinds::SELECTION,
            EditorEvent::ModeChanged { .. } => EventKinds::MODE,
            EditorEvent::ViewOpened { .. } | EditorEvent::ViewClosed { .. } => EventKinds::VIEW,
        }
    }
}

/// A set of event kinds a subscriber wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventKinds(u8);

impl EventKinds {
    pub const DOCUMENT: EventKinds = EventKinds(1);
    pub const SELECTION: EventKinds = EventKinds(2);
    pub const MODE: EventKinds = EventKinds(4);
    pub const VIEW: EventKinds = EventKinds(8);
    pub const ALL: EventKinds = EventKinds(15);
    pub const NONE: EventKinds = EventKinds(0);

    pub fn contains(self, other: EventKinds) -> bool {
        self.0 & other.0 == other.0
    }

    fn intersects(self, other: EventKinds) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for EventKinds {
    type Output = EventKinds;
    fn bitor(self, rhs: EventKinds) -> EventKinds {
        EventKinds(self.0 | rhs.0)
    }
}

/// A subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HookId(pub(crate) u32);

/// A subscriber's body: the batch, and what it changed.
pub type HookFn = dyn Fn(&mut Editor, &[EditorEvent]) -> Effects;

#[derive(Clone)]
struct Sub {
    owner: Owner,
    kinds: EventKinds,
    run: Arc<HookFn>,
}

/// The subscriptions, the queued events, and the delivery depth.
#[derive(Clone, Default)]
pub(crate) struct Hooks {
    subs: Vec<Option<Sub>>,
    pub(crate) queue: Vec<EditorEvent>,
    /// Nesting of `handle`/`run`; delivery happens when it returns to 0.
    pub(crate) depth: u32,
    /// The kinds some live subscriber wants: only these are queued.
    wanted: EventKinds,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
            .field("subscribers", &self.subs.iter().flatten().count())
            .field("queued", &self.queue.len())
            .finish()
    }
}

impl Hooks {
    pub(crate) fn remove_owner(&mut self, owner: Owner) {
        for slot in &mut self.subs {
            if slot.as_ref().is_some_and(|s| s.owner == owner) {
                *slot = None;
            }
        }
        self.recompute_wanted();
    }

    fn recompute_wanted(&mut self) {
        self.wanted = self
            .subs
            .iter()
            .flatten()
            .fold(EventKinds::NONE, |k, s| k | s.kinds);
    }
}

impl Editor {
    /// Subscribes `run` to batches holding any of `kinds`.
    pub fn subscribe(
        &mut self,
        kinds: EventKinds,
        owner: Owner,
        run: impl Fn(&mut Editor, &[EditorEvent]) -> Effects + 'static,
    ) -> HookId {
        let id = HookId(u32::try_from(self.hooks.subs.len()).unwrap_or(u32::MAX));
        self.hooks.subs.push(Some(Sub {
            owner,
            kinds,
            run: Arc::new(run),
        }));
        self.hooks.wanted = self.hooks.wanted | kinds;
        id
    }

    /// Ends a subscription. `false` if it was not live.
    pub fn unsubscribe(&mut self, id: HookId) -> bool {
        let removed = self
            .hooks
            .subs
            .get_mut(id.0 as usize)
            .and_then(Option::take)
            .is_some();
        self.hooks.recompute_wanted();
        removed
    }

    /// Queues `event` if some subscriber wants its kind. O(1).
    pub(crate) fn emit(&mut self, event: EditorEvent) {
        if self.hooks.wanted.intersects(event.kind()) {
            self.hooks.queue.push(event);
        }
    }

    /// Queues the events of a text change of `doc` (if its revision moved)
    /// made through `view`, whose selection was set.
    pub(crate) fn emit_change(
        &mut self,
        view: ViewId,
        doc: DocumentId,
        from: Revision,
        to: Revision,
        selection: bool,
    ) {
        if from != to {
            self.emit(EditorEvent::DocumentChanged { doc, from, to });
        }
        if selection || from != to {
            self.emit(EditorEvent::SelectionChanged { view });
        }
    }

    /// Delivers the queued events now (after direct API calls; `handle`
    /// and `run` deliver on their own). Returns what subscribers changed.
    pub fn flush_events(&mut self) -> Effects {
        let mut fx = Effects::default();
        for _ in 0..MAX_ROUNDS {
            if self.hooks.queue.is_empty() {
                return fx;
            }
            let batch = std::mem::take(&mut self.hooks.queue);
            let kinds = batch.iter().fold(EventKinds::NONE, |k, e| k | e.kind());
            for i in 0..self.hooks.subs.len() {
                let body = match self.hooks.subs.get(i) {
                    Some(Some(s)) if s.kinds.intersects(kinds) => Arc::clone(&s.run),
                    _ => continue,
                };
                // Nested `handle`/`run` calls from a subscriber queue
                // their events for the next round instead of delivering.
                self.hooks.depth += 1;
                fx = fx.or(body(self, &batch));
                self.hooks.depth -= 1;
            }
            // Reuse the batch's storage when nothing new was queued.
            if self.hooks.queue.is_empty() {
                let mut batch = batch;
                batch.clear();
                self.hooks.queue = batch;
            }
        }
        if !self.hooks.queue.is_empty() {
            let dropped = self.hooks.queue.len();
            self.hooks.queue.clear();
            fx = fx.or(self.notify(format_args!(
                "hooks: {dropped} events dropped after {MAX_ROUNDS} rounds"
            )));
        }
        fx
    }

    /// Runs `f` as an outermost-or-nested call: events are delivered when
    /// the outermost one returns.
    pub(crate) fn delivering(&mut self, f: impl FnOnce(&mut Editor) -> Effects) -> Effects {
        self.hooks.depth += 1;
        let fx = f(self);
        self.hooks.depth -= 1;
        if self.hooks.depth == 0 {
            fx.or(self.flush_events())
        } else {
            fx
        }
    }
}
