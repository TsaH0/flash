//! Who registered something: every command, hook, mode, option, and key
//! layer has an owner, and removing an owner removes all of them
//! (PLAN Stage 7: owned, removable registrations).

/// An owner of registrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner(pub(crate) u32);

impl Owner {
    /// The editor's built-in commands, modes, and options.
    pub const BUILTIN: Owner = Owner(0);
    /// The default config (the default keymap and option defaults).
    pub const DEFAULTS: Owner = Owner(1);
    /// The user's config file.
    pub const CONFIG: Owner = Owner(2);

    pub fn get(self) -> u32 {
        self.0
    }
}
