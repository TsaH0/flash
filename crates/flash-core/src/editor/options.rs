//! The typed option registry (PLAN 2.11).
//!
//! Every tunable is a registered option with a name, an owner, a kind, and
//! a default. A value resolves through scopes, most specific first: view,
//! document, global, default. (PLAN's language scope arrives with
//! languages in Stage 9.) Reading an option on the hot path is a field
//! read: the editor resolves the options it needs into each view's
//! [`ViewOptions`] whenever any option value changes, never per key.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU8;

use slotmap::SecondaryMap;

use super::ids::{DocumentId, ViewId};
use super::owner::Owner;

/// A registered option. IDs are never reused: a removed option's ID stays
/// invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OptionId(pub(crate) u32);

impl OptionId {
    pub const TAB_WIDTH: OptionId = OptionId(0);
    pub const UNDO_GROUP_PAUSE_MS: OptionId = OptionId(1);
    pub const SEQUENCE_TIMEOUT_MS: OptionId = OptionId(2);
    pub const NORMAL_MIN_GRAPHEME: OptionId = OptionId(3);
    pub const DEFAULT_LINE_ENDING: OptionId = OptionId(4);
}

/// What values an option takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    /// An integer in `min..=max`.
    Int {
        min: i64,
        max: i64,
    },
    /// One of these names, stored as its index.
    Choice(&'static [&'static str]),
}

/// An option value. A `Choice` holds the index of the chosen name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    Choice(u16),
}

/// Where a value is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    View(ViewId),
    Document(DocumentId),
    Global,
}

/// The built-in options a view reads on the hot path, resolved for that
/// view (view, then its document, then global, then default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewOptions {
    pub tab_width: NonZeroU8,
    pub sequence_timeout_ms: u64,
    pub normal_min_grapheme: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            tab_width: NonZeroU8::new(4).unwrap(),
            sequence_timeout_ms: 1000,
            normal_min_grapheme: true,
        }
    }
}

/// Error from registering, setting, or parsing an option.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionError {
    /// No live option has this name or ID.
    Unknown(String),
    /// A live option already has this name.
    Duplicate(String),
    /// The value does not fit the option's kind.
    Invalid { name: String, reason: String },
    /// The view or document of the scope does not exist.
    NoSuchScope,
}

impl fmt::Display for OptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OptionError::Unknown(name) => write!(f, "unknown option {name:?}"),
            OptionError::Duplicate(name) => write!(f, "option {name:?} already exists"),
            OptionError::Invalid { name, reason } => write!(f, "option {name}: {reason}"),
            OptionError::NoSuchScope => write!(f, "no such view or document"),
        }
    }
}

impl std::error::Error for OptionError {}

#[derive(Debug, Clone)]
struct Def {
    name: String,
    owner: Owner,
    doc: String,
    kind: OptionKind,
    default: OptionValue,
    global: Option<OptionValue>,
}

/// One option as listed by [`Options::iter`].
#[derive(Debug, Clone, Copy)]
pub struct OptionInfo<'a> {
    pub id: OptionId,
    pub name: &'a str,
    pub owner: Owner,
    pub doc: &'a str,
    pub kind: OptionKind,
    pub default: OptionValue,
}

/// The registry and every scope's values.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Indexed by `OptionId`; `None` once removed.
    defs: Vec<Option<Def>>,
    by_name: HashMap<String, OptionId>,
    views: SecondaryMap<ViewId, Vec<(OptionId, OptionValue)>>,
    documents: SecondaryMap<DocumentId, Vec<(OptionId, OptionValue)>>,
}

impl Options {
    /// The registry with the built-in options (owner [`Owner::BUILTIN`]),
    /// at the IDs of the `OptionId` constants.
    pub(crate) fn with_builtins() -> Options {
        let mut o = Options::default();
        let builtins: [(&str, &str, OptionKind, OptionValue); 5] = [
            (
                "tab-width",
                "Columns per tab stop.",
                OptionKind::Int { min: 1, max: 16 },
                OptionValue::Int(4),
            ),
            (
                "undo-group-pause-ms",
                "Typing after a pause this long starts a new undo step.",
                OptionKind::Int {
                    min: 0,
                    max: 60_000,
                },
                OptionValue::Int(1000),
            ),
            (
                "sequence-timeout-ms",
                "A key arriving this long after a pending key starts a new sequence.",
                OptionKind::Int {
                    min: 0,
                    max: 60_000,
                },
                OptionValue::Int(1000),
            ),
            (
                "normal-min-grapheme",
                "Normal and extend mode keep every selection at least one grapheme wide.",
                OptionKind::Bool,
                OptionValue::Bool(true),
            ),
            (
                "default-line-ending",
                "Line ending of new documents.",
                OptionKind::Choice(&["lf", "crlf"]),
                OptionValue::Choice(0),
            ),
        ];
        for (i, (name, doc, kind, default)) in builtins.into_iter().enumerate() {
            let id = o.register(name, Owner::BUILTIN, doc, kind, default);
            debug_assert_eq!(id, Ok(OptionId(i as u32)));
        }
        o
    }

    /// Registers an option. Fails if the name is taken or the default does
    /// not fit `kind`.
    pub fn register(
        &mut self,
        name: &str,
        owner: Owner,
        doc: &str,
        kind: OptionKind,
        default: OptionValue,
    ) -> Result<OptionId, OptionError> {
        if self.by_name.contains_key(name) {
            return Err(OptionError::Duplicate(name.to_owned()));
        }
        check(name, kind, default)?;
        let id = OptionId(
            u32::try_from(self.defs.len()).map_err(|_| OptionError::Invalid {
                name: name.to_owned(),
                reason: "too many options".to_owned(),
            })?,
        );
        self.defs.push(Some(Def {
            name: name.to_owned(),
            owner,
            doc: doc.to_owned(),
            kind,
            default,
            global: None,
        }));
        self.by_name.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Removes an option and every value set for it.
    pub fn remove(&mut self, id: OptionId) -> Result<(), OptionError> {
        let def = self
            .defs
            .get_mut(id.0 as usize)
            .and_then(Option::take)
            .ok_or_else(|| OptionError::Unknown(format!("#{}", id.0)))?;
        self.by_name.remove(&def.name);
        for values in self.views.values_mut().chain(self.documents.values_mut()) {
            values.retain(|&(o, _)| o != id);
        }
        Ok(())
    }

    /// Removes every option registered by `owner`.
    pub fn remove_owner(&mut self, owner: Owner) {
        let ids: Vec<OptionId> = self
            .iter()
            .filter(|o| o.owner == owner)
            .map(|o| o.id)
            .collect();
        for id in ids {
            let _ = self.remove(id);
        }
    }

    pub fn id(&self, name: &str) -> Option<OptionId> {
        self.by_name.get(name).copied()
    }

    fn def(&self, id: OptionId) -> Result<&Def, OptionError> {
        self.defs
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| OptionError::Unknown(format!("#{}", id.0)))
    }

    pub fn info(&self, id: OptionId) -> Option<OptionInfo<'_>> {
        let d = self.def(id).ok()?;
        Some(OptionInfo {
            id,
            name: &d.name,
            owner: d.owner,
            doc: &d.doc,
            kind: d.kind,
            default: d.default,
        })
    }

    /// Every live option, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = OptionInfo<'_>> {
        (0..self.defs.len()).filter_map(|i| self.info(OptionId(i as u32)))
    }

    /// Parses `text` as a value of option `id`: `true`/`false`/`on`/`off`
    /// for booleans, a decimal integer, or one of the choice names.
    pub fn parse(&self, id: OptionId, text: &str) -> Result<OptionValue, OptionError> {
        let d = self.def(id)?;
        let invalid = |reason: String| OptionError::Invalid {
            name: d.name.clone(),
            reason,
        };
        let value = match d.kind {
            OptionKind::Bool => match text {
                "true" | "on" | "yes" => OptionValue::Bool(true),
                "false" | "off" | "no" => OptionValue::Bool(false),
                _ => return Err(invalid(format!("expected true or false, got {text:?}"))),
            },
            OptionKind::Int { .. } => OptionValue::Int(
                text.parse()
                    .map_err(|_| invalid(format!("expected an integer, got {text:?}")))?,
            ),
            OptionKind::Choice(names) => OptionValue::Choice(
                names
                    .iter()
                    .position(|&n| n == text)
                    .and_then(|i| u16::try_from(i).ok())
                    .ok_or_else(|| invalid(format!("expected one of {names:?}, got {text:?}")))?,
            ),
        };
        check(&d.name, d.kind, value)?;
        Ok(value)
    }

    /// Writes `value` at `scope`. The caller checks that the scope's view
    /// or document exists.
    pub(crate) fn set(
        &mut self,
        scope: Scope,
        id: OptionId,
        value: OptionValue,
    ) -> Result<(), OptionError> {
        let d = self.def(id)?;
        check(&d.name, d.kind, value)?;
        let slot = match scope {
            Scope::Global => {
                if let Some(Some(d)) = self.defs.get_mut(id.0 as usize) {
                    d.global = Some(value);
                }
                return Ok(());
            }
            Scope::View(v) => entry(&mut self.views, v),
            Scope::Document(d) => entry(&mut self.documents, d),
        }
        .ok_or(OptionError::NoSuchScope)?;
        match slot.iter_mut().find(|(o, _)| *o == id) {
            Some((_, v)) => *v = value,
            None => slot.push((id, value)),
        }
        Ok(())
    }

    /// Removes the value set at `scope`, so the next scope applies.
    pub(crate) fn unset(&mut self, scope: Scope, id: OptionId) -> Result<(), OptionError> {
        self.def(id)?;
        match scope {
            Scope::Global => {
                if let Some(Some(d)) = self.defs.get_mut(id.0 as usize) {
                    d.global = None;
                }
            }
            Scope::View(v) => {
                if let Some(values) = self.views.get_mut(v) {
                    values.retain(|&(o, _)| o != id);
                }
            }
            Scope::Document(d) => {
                if let Some(values) = self.documents.get_mut(d) {
                    values.retain(|&(o, _)| o != id);
                }
            }
        }
        Ok(())
    }

    /// Forgets a closed view's or document's values.
    pub(crate) fn forget_view(&mut self, view: ViewId) {
        self.views.remove(view);
    }

    pub(crate) fn forget_document(&mut self, doc: DocumentId) {
        self.documents.remove(doc);
    }

    /// The value of `id` for `view` showing `doc`: view, document, global,
    /// default. `None` for an unknown option.
    pub fn get(
        &self,
        id: OptionId,
        view: Option<ViewId>,
        doc: Option<DocumentId>,
    ) -> Option<OptionValue> {
        let d = self.def(id).ok()?;
        let find = |values: Option<&Vec<(OptionId, OptionValue)>>| {
            values.and_then(|v| v.iter().find(|(o, _)| *o == id).map(|&(_, x)| x))
        };
        find(view.and_then(|v| self.views.get(v)))
            .or_else(|| find(doc.and_then(|d| self.documents.get(d))))
            .or(d.global)
            .or(Some(d.default))
    }

    /// The built-in options for `view` showing `doc`.
    pub(crate) fn resolve(&self, view: Option<ViewId>, doc: Option<DocumentId>) -> ViewOptions {
        let int = |id| match self.get(id, view, doc) {
            Some(OptionValue::Int(n)) => n,
            _ => 0,
        };
        ViewOptions {
            tab_width: u8::try_from(int(OptionId::TAB_WIDTH))
                .ok()
                .and_then(NonZeroU8::new)
                .unwrap_or(ViewOptions::default().tab_width),
            sequence_timeout_ms: u64::try_from(int(OptionId::SEQUENCE_TIMEOUT_MS)).unwrap_or(0),
            normal_min_grapheme: self.get(OptionId::NORMAL_MIN_GRAPHEME, view, doc)
                != Some(OptionValue::Bool(false)),
        }
    }

    /// Formats a value of `id` as [`parse`](Self::parse) reads it.
    pub fn format(&self, id: OptionId, value: OptionValue) -> String {
        match (value, self.def(id).map(|d| d.kind)) {
            (OptionValue::Bool(b), _) => b.to_string(),
            (OptionValue::Int(n), _) => n.to_string(),
            (OptionValue::Choice(i), Ok(OptionKind::Choice(names))) => names
                .get(usize::from(i))
                .map_or_else(|| i.to_string(), |n| (*n).to_owned()),
            (OptionValue::Choice(i), _) => i.to_string(),
        }
    }
}

fn entry<K: slotmap::Key, V: Default>(map: &mut SecondaryMap<K, V>, key: K) -> Option<&mut V> {
    Some(map.entry(key)?.or_default())
}

/// Whether `value` fits `kind`.
fn check(name: &str, kind: OptionKind, value: OptionValue) -> Result<(), OptionError> {
    let ok = match (kind, value) {
        (OptionKind::Bool, OptionValue::Bool(_)) => true,
        (OptionKind::Int { min, max }, OptionValue::Int(n)) => (min..=max).contains(&n),
        (OptionKind::Choice(names), OptionValue::Choice(i)) => usize::from(i) < names.len(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        let reason = match kind {
            OptionKind::Int { min, max } => format!("expected an integer in {min}..={max}"),
            OptionKind::Bool => "expected true or false".to_owned(),
            OptionKind::Choice(names) => format!("expected one of {names:?}"),
        };
        Err(OptionError::Invalid {
            name: name.to_owned(),
            reason,
        })
    }
}
