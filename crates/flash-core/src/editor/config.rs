//! Configuration from TOML (PLAN 2.11): options and key bindings.
//!
//! ```toml
//! [options]
//! tab-width = 8
//! cursor-shape-insert = "underline"
//!
//! [keys.normal]
//! "g g" = "move-document-start"
//! "C-s" = false            # unbind
//! "Q" = "insert-text TODO" # a command with an argument
//! ```
//!
//! The default keymap is the embedded `default-config.toml`, applied by
//! [`Editor::new`] through the same code as a user's file, into the
//! default layer: its `[options]` set option defaults. A user's file goes
//! into the user layer: its `[options]` set global values. Every entry is
//! checked on its own; a bad one is reported and skipped, and everything
//! else applies (a config never prevents startup). Applying replaces the
//! owner's previous layers and values as a whole, after checking.

use std::fmt;
use std::sync::Arc;

use super::keymap::{Bind, KeySeq, parse_sequence};
use super::mode::ModeId;
use super::owner::Owner;
use super::{Editor, OptionId, OptionValue};

#[cfg(test)]
mod tests;

/// The default configuration: the default keymap.
pub const DEFAULT_CONFIG: &str = include_str!("default-config.toml");

/// A problem with one config entry (or the whole file, if it does not
/// parse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// Where the config came from (a path, or "default config").
    pub source: String,
    /// The entry, e.g. `keys.normal."g g"` or `options.tab-width`.
    pub entry: String,
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.entry.is_empty() {
            write!(f, "{}: {}", self.source, self.reason)
        } else {
            write!(f, "{}: {}: {}", self.source, self.entry, self.reason)
        }
    }
}

impl std::error::Error for ConfigError {}

/// A checked config, ready to apply.
struct Checked {
    options: Vec<(OptionId, OptionValue)>,
    keys: Vec<(ModeId, Vec<(KeySeq, Bind)>)>,
}

impl Editor {
    /// Checks and applies config `text` from `source` as `owner`'s layer
    /// ([`Owner::DEFAULTS`] or [`Owner::CONFIG`]), replacing that owner's
    /// previous bindings and values. Returns the entries it skipped.
    pub fn apply_config(&mut self, owner: Owner, source: &str, text: &str) -> Vec<ConfigError> {
        let mut errors = Vec::new();
        let checked = self.check_config(source, text, &mut errors);
        // Replace the owner's previous values and layers as a whole.
        if owner == Owner::CONFIG {
            self.options.unset_globals();
        }
        for (id, value) in checked.options {
            let set = if owner == Owner::DEFAULTS {
                self.options.set_default(id, value)
            } else {
                self.options.set(super::Scope::Global, id, value)
            };
            if let Err(e) = set {
                errors.push(ConfigError {
                    source: source.to_owned(),
                    entry: String::new(),
                    reason: e.to_string(),
                });
            }
        }
        self.keymap.remove_owner(owner);
        for (mode, binds) in checked.keys {
            self.keymap.set_layer(owner, mode, binds);
        }
        self.refresh_options();
        errors
    }

    fn check_config(&self, source: &str, text: &str, errors: &mut Vec<ConfigError>) -> Checked {
        let mut checked = Checked {
            options: Vec::new(),
            keys: Vec::new(),
        };
        let error = |entry: String, reason: String| ConfigError {
            source: source.to_owned(),
            entry,
            reason,
        };
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                errors.push(error(String::new(), format!("not valid TOML: {e}")));
                return checked;
            }
        };
        for (section, value) in &table {
            match (section.as_str(), value) {
                ("options", toml::Value::Table(options)) => {
                    for (name, value) in options {
                        let entry = format!("options.{name}");
                        let Some(id) = self.options.id(name) else {
                            errors.push(error(entry, "unknown option".into()));
                            continue;
                        };
                        let text = match value {
                            toml::Value::String(s) => s.clone(),
                            toml::Value::Integer(n) => n.to_string(),
                            toml::Value::Boolean(b) => b.to_string(),
                            other => {
                                errors
                                    .push(error(entry, format!("unexpected {}", other.type_str())));
                                continue;
                            }
                        };
                        match self.options.parse(id, &text) {
                            Ok(v) => checked.options.push((id, v)),
                            Err(e) => errors.push(error(entry, e.to_string())),
                        }
                    }
                }
                ("keys", toml::Value::Table(modes)) => {
                    for (mode_name, binds) in modes {
                        let Some(mode) = self.modes.id(mode_name) else {
                            errors.push(error(format!("keys.{mode_name}"), "unknown mode".into()));
                            continue;
                        };
                        let toml::Value::Table(binds) = binds else {
                            errors.push(error(
                                format!("keys.{mode_name}"),
                                "expected a table".into(),
                            ));
                            continue;
                        };
                        let mut layer = Vec::new();
                        for (keys, bind) in binds {
                            let entry = format!("keys.{mode_name}.{keys:?}");
                            let seq = match parse_sequence(keys) {
                                Ok(s) => s,
                                Err(e) => {
                                    errors.push(error(entry, e.to_string()));
                                    continue;
                                }
                            };
                            match self.check_bind(bind) {
                                Ok(b) => layer.push((seq, b)),
                                Err(reason) => errors.push(error(entry, reason)),
                            }
                        }
                        checked.keys.push((mode, layer));
                    }
                }
                (name, _) => errors.push(error(
                    name.to_owned(),
                    "unknown section (expected [options] or [keys.<mode>])".into(),
                )),
            }
        }
        checked
    }

    /// `"command [argument]"` or `false`.
    fn check_bind(&self, value: &toml::Value) -> Result<Bind, String> {
        match value {
            toml::Value::Boolean(false) => Ok(Bind::Unbound),
            toml::Value::String(s) => {
                let (name, arg) = match s.trim().split_once(' ') {
                    Some((name, arg)) => (name, Some(arg.trim())),
                    None => (s.trim(), None),
                };
                let id = self
                    .commands
                    .id(name)
                    .ok_or_else(|| format!("unknown command {name:?}"))?;
                Ok(Bind::Command {
                    id,
                    arg: arg.filter(|a| !a.is_empty()).map(Arc::from),
                })
            }
            other => Err(format!(
                "expected a command name or false, got {}",
                other.type_str()
            )),
        }
    }
}
