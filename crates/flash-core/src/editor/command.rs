//! The command registry (PLAN Stage 7).
//!
//! Every user-visible action is a named command, so anything the editor
//! can do can be bound to a key, run from the `:` command line, or called
//! by a plugin. A command takes a [`Context`] (view, count, argument),
//! never a key. Registrations have an owner and can be removed; IDs are
//! never reused, so a stale ID fails cleanly.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;

use super::owner::Owner;
use super::{Editor, Effects, ViewId};

#[cfg(test)]
mod tests;

/// A registered command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(pub(crate) u32);

/// What a command runs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context<'a> {
    /// The view to act on; the focused view if `None`.
    pub view: Option<ViewId>,
    /// A repeat count (motions, edits), if one was given.
    pub count: Option<NonZeroUsize>,
    /// The argument: from the binding (`"count 3"`), the command line, or
    /// the caller.
    pub arg: Option<&'a str>,
}

impl Context<'_> {
    /// The count, or 1.
    pub fn count(&self) -> usize {
        self.count.map_or(1, NonZeroUsize::get)
    }
}

/// A command's body.
pub type CommandFn = dyn Fn(&mut Editor, &Context<'_>) -> Effects;

/// Error from registering a command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    /// A live command already has this name.
    Duplicate(String),
    /// Names are non-empty and contain no whitespace.
    BadName(String),
    /// No live command has this ID.
    Unknown(CommandId),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Duplicate(n) => write!(f, "command {n:?} already exists"),
            CommandError::BadName(n) => write!(f, "bad command name {n:?}"),
            CommandError::Unknown(id) => write!(f, "no command #{}", id.0),
        }
    }
}

impl std::error::Error for CommandError {}

#[derive(Clone)]
struct Entry {
    name: String,
    owner: Owner,
    doc: String,
    run: Arc<CommandFn>,
}

/// One command as listed by [`Commands::iter`].
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo<'a> {
    pub id: CommandId,
    pub name: &'a str,
    pub owner: Owner,
    pub doc: &'a str,
}

/// The registry.
#[derive(Clone, Default)]
pub struct Commands {
    /// Indexed by `CommandId`; `None` once removed.
    entries: Vec<Option<Entry>>,
    by_name: HashMap<String, CommandId>,
}

impl fmt::Debug for Commands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Commands")
            .field("live", &self.by_name.len())
            .finish()
    }
}

impl Commands {
    pub(crate) fn register(
        &mut self,
        name: &str,
        owner: Owner,
        doc: &str,
        run: Arc<CommandFn>,
    ) -> Result<CommandId, CommandError> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(CommandError::BadName(name.to_owned()));
        }
        if self.by_name.contains_key(name) {
            return Err(CommandError::Duplicate(name.to_owned()));
        }
        let id = CommandId(
            u32::try_from(self.entries.len()).map_err(|_| CommandError::BadName(name.into()))?,
        );
        self.entries.push(Some(Entry {
            name: name.to_owned(),
            owner,
            doc: doc.to_owned(),
            run,
        }));
        self.by_name.insert(name.to_owned(), id);
        Ok(id)
    }

    pub(crate) fn remove(&mut self, id: CommandId) -> Result<(), CommandError> {
        let entry = self
            .entries
            .get_mut(id.0 as usize)
            .and_then(Option::take)
            .ok_or(CommandError::Unknown(id))?;
        self.by_name.remove(&entry.name);
        Ok(())
    }

    /// Removes every command of `owner`; returns their IDs.
    pub(crate) fn remove_owner(&mut self, owner: Owner) -> Vec<CommandId> {
        let ids: Vec<CommandId> = self
            .iter()
            .filter(|c| c.owner == owner)
            .map(|c| c.id)
            .collect();
        for &id in &ids {
            let _ = self.remove(id);
        }
        ids
    }

    pub fn id(&self, name: &str) -> Option<CommandId> {
        self.by_name.get(name).copied()
    }

    pub fn info(&self, id: CommandId) -> Option<CommandInfo<'_>> {
        let e = self.entries.get(id.0 as usize)?.as_ref()?;
        Some(CommandInfo {
            id,
            name: &e.name,
            owner: e.owner,
            doc: &e.doc,
        })
    }

    /// Every live command, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = CommandInfo<'_>> {
        (0..self.entries.len()).filter_map(|i| self.info(CommandId(i as u32)))
    }

    /// The body of `id` (a reference count, no allocation).
    pub(crate) fn body(&self, id: CommandId) -> Option<Arc<CommandFn>> {
        Some(Arc::clone(&self.entries.get(id.0 as usize)?.as_ref()?.run))
    }
}

impl Editor {
    /// The command registry.
    pub fn commands(&self) -> &Commands {
        &self.commands
    }

    /// Registers a command. Names are unique among live commands and
    /// contain no whitespace.
    pub fn register_command(
        &mut self,
        name: &str,
        owner: Owner,
        doc: &str,
        run: impl Fn(&mut Editor, &Context<'_>) -> Effects + 'static,
    ) -> Result<CommandId, CommandError> {
        self.commands.register(name, owner, doc, Arc::new(run))
    }

    /// Removes a command. Key bindings to it then do nothing (and say so).
    pub fn remove_command(&mut self, id: CommandId) -> Result<(), CommandError> {
        self.commands.remove(id)
    }

    /// Runs command `id`. An unknown ID changes nothing and sets the
    /// message.
    pub fn run(&mut self, id: CommandId, cx: &Context<'_>) -> Effects {
        self.delivering(|ed| match ed.commands.body(id) {
            Some(body) => body(ed, cx),
            None => ed.notify(CommandError::Unknown(id)),
        })
    }

    /// Runs the command named `name`.
    pub fn run_named(&mut self, name: &str, cx: &Context<'_>) -> Effects {
        match self.commands.id(name) {
            Some(id) => self.run(id, cx),
            None => self.notify(format_args!("unknown command {name:?}")),
        }
    }

    /// Removes everything `owner` registered: modes (their views return to
    /// normal mode), key layers, commands, options, and hooks. Built-in
    /// modes stay even if their owner is removed.
    pub fn remove_owner(&mut self, owner: Owner) {
        self.remove_modes_of(owner);
        self.keymap.remove_owner(owner);
        self.commands.remove_owner(owner);
        self.options.remove_owner(owner);
        self.hooks.remove_owner(owner);
        self.refresh_options();
    }
}
