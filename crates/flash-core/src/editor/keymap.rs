//! Keymaps (PLAN Stage 7, 2.11): layered bindings per mode, compiled into
//! a trie, resolved without allocating.
//!
//! **Notation** (Helix-style): `x`, `C-s`, `A-z`, `S-left`, `C-A-x`; named
//! keys `ret esc tab backspace del left right up down home end pageup
//! pagedown space minus F1`–`F12`; a sequence is space-separated: `g g`.
//! A character key is written as the character it types (`U`, not
//! `S-u`): Shift is never part of a character key.
//!
//! **Layers:** each mode has layers from the default config, plugins, and
//! the user's config; higher layers win, and [`Bind::Unbound`] hides a
//! lower layer's binding. Any change recompiles that mode's trie.
//!
//! **Resolution:** a key that continues the pending sequence descends; a
//! key that does not resets it and is resolved from the root. A node with
//! both an action and children (an ambiguous prefix) runs its action when
//! the next key does not continue it, or when that key comes after the
//! sequence timeout (input time, so replay is exact).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use smallvec::SmallVec;

use super::command::CommandId;
use super::mode::ModeId;
use super::owner::Owner;
use super::{KeyCode, KeyEvent, Modifiers};
use crate::document::Timestamp;

#[cfg(test)]
mod tests;

/// A key sequence (inline up to four keys).
pub type KeySeq = SmallVec<[KeyEvent; 4]>;

/// What a sequence is bound to in one layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    /// Run a command, with an optional argument.
    Command {
        id: CommandId,
        arg: Option<Arc<str>>,
    },
    /// Explicitly unbound: hides lower layers' binding of this sequence.
    Unbound,
}

/// A resolved binding: what to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub command: CommandId,
    pub arg: Option<Arc<str>>,
}

/// Error parsing key notation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyError(pub String);

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyError {}

const NAMED: &[(&str, KeyCode)] = &[
    ("ret", KeyCode::Enter),
    ("esc", KeyCode::Esc),
    ("tab", KeyCode::Tab),
    ("backspace", KeyCode::Backspace),
    ("del", KeyCode::Delete),
    ("left", KeyCode::Left),
    ("right", KeyCode::Right),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pageup", KeyCode::PageUp),
    ("pagedown", KeyCode::PageDown),
    ("space", KeyCode::Char(' ')),
    ("minus", KeyCode::Char('-')),
];

/// `key` as bindings see it: Shift dropped from character keys.
pub fn normalize(key: KeyEvent) -> KeyEvent {
    match key.code {
        KeyCode::Char(_) => KeyEvent {
            code: key.code,
            modifiers: Modifiers {
                shift: false,
                ..key.modifiers
            },
        },
        _ => key,
    }
}

/// Parses one key in the notation above.
pub fn parse_key(text: &str) -> Result<KeyEvent, KeyError> {
    let bad = |why: &str| KeyError(format!("bad key {text:?}: {why}"));
    let mut rest = text;
    let mut modifiers = Modifiers::NONE;
    loop {
        let mut chars = rest.chars();
        match (chars.next(), chars.next()) {
            (Some(m @ ('C' | 'A' | 'S')), Some('-')) if rest.len() > 2 => {
                let flag = match m {
                    'C' => &mut modifiers.ctrl,
                    'A' => &mut modifiers.alt,
                    _ => &mut modifiers.shift,
                };
                if *flag {
                    return Err(bad("modifier repeated"));
                }
                *flag = true;
                rest = &rest[2..];
            }
            _ => break,
        }
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (None, _) => return Err(bad("empty")),
        (Some(c), None) if !c.is_whitespace() && !c.is_control() => KeyCode::Char(c),
        _ => match NAMED.iter().find(|(name, _)| *name == rest) {
            Some(&(_, code)) => code,
            None => match rest.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=12) => KeyCode::F(n),
                _ => return Err(bad("unknown key name")),
            },
        },
    };
    if modifiers.shift && matches!(code, KeyCode::Char(_)) {
        return Err(bad("write the shifted character instead of S-"));
    }
    Ok(KeyEvent { code, modifiers })
}

/// Parses a space-separated key sequence.
pub fn parse_sequence(text: &str) -> Result<KeySeq, KeyError> {
    let keys = text
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(parse_key)
        .collect::<Result<KeySeq, _>>()?;
    if keys.is_empty() {
        return Err(KeyError(format!("bad key sequence {text:?}: empty")));
    }
    Ok(keys)
}

/// Formats `key` in the notation [`parse_key`] reads.
pub fn key_name(key: KeyEvent) -> String {
    let key = normalize(key);
    let mut s = String::new();
    for (on, prefix) in [
        (key.modifiers.ctrl, "C-"),
        (key.modifiers.alt, "A-"),
        (key.modifiers.shift, "S-"),
    ] {
        if on {
            s.push_str(prefix);
        }
    }
    match NAMED.iter().find(|&&(_, code)| code == key.code) {
        Some((name, _)) => s.push_str(name),
        None => match key.code {
            KeyCode::Char(c) => s.push(c),
            KeyCode::F(n) => s.push_str(&format!("F{n}")),
            other => s.push_str(&format!("{other:?}")),
        },
    }
    s
}

/// A total order on keys for the trie's sorted children.
fn sort_key(key: KeyEvent) -> u64 {
    let code: u64 = match key.code {
        KeyCode::Char(c) => u64::from(u32::from(c)),
        KeyCode::F(n) => 0x20_0000 + u64::from(n),
        other => {
            0x10_0000
                + match other {
                    KeyCode::Enter => 0,
                    KeyCode::Tab => 1,
                    KeyCode::Backspace => 2,
                    KeyCode::Delete => 3,
                    KeyCode::Left => 4,
                    KeyCode::Right => 5,
                    KeyCode::Up => 6,
                    KeyCode::Down => 7,
                    KeyCode::Home => 8,
                    KeyCode::End => 9,
                    KeyCode::PageUp => 10,
                    KeyCode::PageDown => 11,
                    _ => 12,
                }
        }
    };
    let m = key.modifiers;
    (code << 3) | u64::from(m.shift) | (u64::from(m.ctrl) << 1) | (u64::from(m.alt) << 2)
}

#[derive(Debug, Clone, Default)]
struct Node {
    /// Sorted by key.
    children: Vec<(u64, u32)>,
    action: Option<Action>,
}

/// One mode's compiled bindings.
#[derive(Debug, Clone)]
struct Trie {
    /// Node 0 is the root.
    nodes: Vec<Node>,
}

impl Trie {
    fn build(bindings: &BTreeMap<Vec<u64>, Action>) -> Trie {
        let mut nodes = vec![Node::default()];
        for (keys, action) in bindings {
            let mut at = 0usize;
            for &k in keys {
                at = match nodes[at].children.binary_search_by_key(&k, |&(c, _)| c) {
                    Ok(i) => nodes[at].children[i].1 as usize,
                    Err(i) => {
                        let next = nodes.len();
                        nodes.push(Node::default());
                        nodes[at].children.insert(i, (k, next as u32));
                        next
                    }
                };
            }
            nodes[at].action = Some(action.clone());
        }
        Trie { nodes }
    }

    fn child(&self, node: u32, key: KeyEvent) -> Option<u32> {
        let children = &self.nodes.get(node as usize)?.children;
        let k = sort_key(normalize(key));
        children
            .binary_search_by_key(&k, |&(c, _)| c)
            .ok()
            .map(|i| children[i].1)
    }

    fn node(&self, node: u32) -> &Node {
        &self.nodes[node as usize]
    }
}

#[derive(Debug, Clone)]
struct Layer {
    owner: Owner,
    mode: ModeId,
    binds: Vec<(KeySeq, Bind)>,
}

#[derive(Debug, Clone)]
struct Pending {
    mode: ModeId,
    node: u32,
    /// Time of the last key of the sequence.
    at: Timestamp,
    keys: KeySeq,
}

/// What one key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Run this action.
    Run(Action),
    /// The key extended a pending sequence.
    Pending,
    /// No binding (the pending sequence, if any, was dropped).
    Unbound,
}

/// The result of feeding one key: an ambiguous prefix's action to run
/// first (the key did not continue it, or came too late), then the key's
/// own step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub first: Option<Action>,
    pub step: Step,
}

/// Every layer, the compiled tries, and the pending sequence.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    layers: Vec<Layer>,
    /// Indexed by mode ID; `None` for a mode without bindings.
    tries: Vec<Option<Trie>>,
    pending: Option<Pending>,
}

/// Higher wins: the user's config, then plugins (later owners higher),
/// then the default config.
fn rank(owner: Owner) -> u64 {
    match owner {
        Owner::CONFIG => u64::MAX,
        Owner::DEFAULTS => 0,
        Owner(n) => u64::from(n) + 1,
    }
}

impl Keymap {
    /// Replaces `owner`'s layer for `mode` with `binds` and recompiles the
    /// mode.
    pub fn set_layer(&mut self, owner: Owner, mode: ModeId, binds: Vec<(KeySeq, Bind)>) {
        self.layers
            .retain(|l| !(l.owner == owner && l.mode == mode));
        if !binds.is_empty() {
            self.layers.push(Layer { owner, mode, binds });
        }
        self.compile(mode);
    }

    /// Removes every layer of `owner` and recompiles the modes it bound.
    pub fn remove_owner(&mut self, owner: Owner) {
        let modes: Vec<ModeId> = self
            .layers
            .iter()
            .filter(|l| l.owner == owner)
            .map(|l| l.mode)
            .collect();
        self.layers.retain(|l| l.owner != owner);
        for mode in modes {
            self.compile(mode);
        }
    }

    /// Removes every layer of `mode` (the mode was removed).
    pub fn remove_mode(&mut self, mode: ModeId) {
        self.layers.retain(|l| l.mode != mode);
        if let Some(slot) = self.tries.get_mut(mode.0 as usize) {
            *slot = None;
        }
        if self.pending.as_ref().is_some_and(|p| p.mode == mode) {
            self.pending = None;
        }
    }

    /// The effective bindings of `mode` (each sequence's highest layer),
    /// in key order.
    pub fn bindings(&self, mode: ModeId) -> Vec<(KeySeq, Action)> {
        let mut layers: Vec<&Layer> = self.layers.iter().filter(|l| l.mode == mode).collect();
        layers.sort_by_key(|l| rank(l.owner));
        let mut map: BTreeMap<Vec<u64>, (KeySeq, Action)> = BTreeMap::new();
        for layer in layers {
            for (keys, bind) in &layer.binds {
                let k: Vec<u64> = keys.iter().map(|&k| sort_key(normalize(k))).collect();
                match bind {
                    Bind::Command { id, arg } => {
                        let normal: KeySeq = keys.iter().map(|&k| normalize(k)).collect();
                        map.insert(
                            k,
                            (
                                normal,
                                Action {
                                    command: *id,
                                    arg: arg.clone(),
                                },
                            ),
                        );
                    }
                    Bind::Unbound => {
                        map.remove(&k);
                    }
                }
            }
        }
        map.into_values().collect()
    }

    /// The binding of exactly `keys` in `mode`, if any.
    pub fn lookup(&self, mode: ModeId, keys: &[KeyEvent]) -> Option<&Action> {
        let trie = self.tries.get(mode.0 as usize)?.as_ref()?;
        let mut node = 0;
        for &k in keys {
            node = trie.child(node, k)?;
        }
        trie.node(node).action.as_ref()
    }

    fn compile(&mut self, mode: ModeId) {
        let i = mode.0 as usize;
        if self.tries.len() <= i {
            self.tries.resize(i + 1, None);
        }
        let bindings: BTreeMap<Vec<u64>, Action> = self
            .bindings(mode)
            .into_iter()
            .map(|(keys, a)| (keys.iter().map(|&k| sort_key(k)).collect(), a))
            .collect();
        self.tries[i] = if bindings.is_empty() {
            None
        } else {
            Some(Trie::build(&bindings))
        };
        // The trie changed under a pending sequence: drop it.
        if self.pending.as_ref().is_some_and(|p| p.mode == mode) {
            self.pending = None;
        }
    }

    /// The keys of the pending sequence, if any.
    pub fn pending(&self) -> &[KeyEvent] {
        self.pending.as_ref().map_or(&[], |p| &p.keys)
    }

    /// Drops the pending sequence (a mode change, a prompt).
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// Resolves `key` in `mode` at time `now`; a pending sequence older
    /// than `timeout_ns` (since its last key) expires. Allocates nothing
    /// (sequences up to four keys are stored inline).
    pub fn feed(
        &mut self,
        mode: ModeId,
        key: KeyEvent,
        now: Timestamp,
        timeout_ns: u64,
    ) -> Resolved {
        let Some(trie) = self.tries.get(mode.0 as usize).and_then(Option::as_ref) else {
            self.pending = None;
            return Resolved {
                first: None,
                step: Step::Unbound,
            };
        };
        let mut first = None;
        if let Some(p) = self.pending.take() {
            let expired = now.as_nanos().saturating_sub(p.at.as_nanos()) > timeout_ns;
            if p.mode == mode && !expired {
                if let Some(next) = trie.child(p.node, key) {
                    let mut keys = p.keys;
                    keys.push(normalize(key));
                    return self.descend(mode, next, now, keys);
                }
            }
            // Not continued (or too late, or the mode changed): an
            // ambiguous prefix's own action runs, and the key starts over.
            if p.mode == mode {
                first = trie.node(p.node).action.clone();
            }
        }
        let Some(trie) = self.tries.get(mode.0 as usize).and_then(Option::as_ref) else {
            return Resolved {
                first,
                step: Step::Unbound,
            };
        };
        match trie.child(0, key) {
            Some(next) => {
                let mut keys = KeySeq::new();
                keys.push(normalize(key));
                let mut r = self.descend(mode, next, now, keys);
                r.first = first;
                r
            }
            None => Resolved {
                first,
                step: Step::Unbound,
            },
        }
    }

    fn descend(&mut self, mode: ModeId, node: u32, now: Timestamp, keys: KeySeq) -> Resolved {
        let Some(trie) = self.tries.get(mode.0 as usize).and_then(Option::as_ref) else {
            return Resolved {
                first: None,
                step: Step::Unbound,
            };
        };
        let n = trie.node(node);
        if n.children.is_empty() {
            let step = n.action.clone().map_or(Step::Unbound, Step::Run);
            Resolved { first: None, step }
        } else {
            self.pending = Some(Pending {
                mode,
                node,
                at: now,
                keys,
            });
            Resolved {
                first: None,
                step: Step::Pending,
            }
        }
    }
}
