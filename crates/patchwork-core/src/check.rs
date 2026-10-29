//! Checks: problems with a profile that an extension knows to look for,
//! like a Bethesda game having more plugins than it can load. A [`Check`]
//! looks at the mods a deploy would install and says what's wrong as
//! [`Problem`]s; core turns those into [`Issue`](crate::issue::Issue)s, so
//! they're shown with every other issue, and into the deploy's warnings.
//!
//! A problem can say it's about one of the extension's ordered lists, and
//! is then shown with the list too, as the user rearranges it.

use serde::Serialize;

use crate::deploy::{GameFiles, InstalledMod};
use crate::game::GameDef;
use crate::issue::Severity;
use crate::list::List;

/// Looks for problems with a profile, registered with
/// [`Registrar::check`](crate::ext::Registrar::check).
pub trait Check: Send + Sync {
    /// What's wrong with installing `mods` (the profile's enabled mods, in
    /// load order, with their files as transforms leave them). Like an
    /// [`OrderedList`](crate::list::OrderedList), this may read mods' and
    /// the game's files, but not write anything. Called whenever a profile
    /// is shown, so it shouldn't do much more than that.
    fn check(&self, ctx: &CheckContext<'_>, mods: &[InstalledMod]) -> Vec<Problem>;
}

/// What a [`Check`] gets to work with.
pub struct CheckContext<'a> {
    game: &'a GameDef,
    config: &'a toml::Table,
    ext: &'a str,
    files: &'a GameFiles<'a>,
    lists: &'a [List],
}

impl<'a> CheckContext<'a> {
    /// Core makes these; this is for testing checks on their own. `lists`
    /// are the lists that are on for the game, as they are for the profile;
    /// the check sees those of extension `ext`.
    pub fn new(
        game: &'a GameDef,
        config: &'a toml::Table,
        ext: &'a str,
        files: &'a GameFiles<'a>,
        lists: &'a [List],
    ) -> Self {
        Self {
            game,
            config,
            ext,
            files,
            lists,
        }
    }

    pub fn game(&self) -> &GameDef {
        self.game
    }

    /// The check's `[ext.<extension>.<check>]` table in the game
    /// definition.
    pub fn config(&self) -> &toml::Table {
        self.config
    }

    /// The game's own files, as they are without anything patchwork
    /// deployed. Empty if the game isn't set up.
    pub fn files(&self) -> &GameFiles<'a> {
        self.files
    }

    /// The extension's list called `name`, as the user has it, or `None`
    /// if the game turns it off.
    pub fn list(&self, name: &str) -> Option<&List> {
        let id = crate::ext::hook_id(self.ext, name);
        self.lists.iter().find(|l| l.id == id)
    }
}

/// Something wrong that a [`Check`] or an
/// [`OrderedList`](crate::list::OrderedList) found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub severity: Severity,
    /// The profile's mods it's about, by name.
    pub mods: Vec<String>,
    /// The name of the extension's list it's about, if it's about one, so
    /// it's shown with the list.
    pub list: Option<String>,
    /// A whole sentence.
    pub message: String,
}

impl Problem {
    /// Something that might not work, though deploying does.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            mods: Vec::new(),
            list: None,
            message: message.into(),
        }
    }

    /// Something that means the game won't work as deployed.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            ..Self::warning(message)
        }
    }

    /// Says it's about mod `name`, as well as any it was already about.
    pub fn about_mod(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !name.is_empty() && !self.mods.contains(&name) {
            self.mods.push(name);
        }
        self
    }

    /// Says it's about the extension's list `name`.
    pub fn about_list(mut self, name: impl Into<String>) -> Self {
        self.list = Some(name.into());
        self
    }
}
