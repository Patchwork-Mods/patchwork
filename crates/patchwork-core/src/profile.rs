use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Error;
use crate::compat::GameVersions;
use crate::plugins::PluginEntry;

/// Schema 2 added `id` and `name`; profiles used to be identified by their
/// directory, `<game>/<name>`.
pub const PROFILE_SCHEMA: u32 = 2;

/// What the user wants installed for a game. Meant to be readable and
/// hand-editable; the exact versions that were resolved live in the lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub schema: u32,
    /// Stays the same wherever the profile goes, so it can be recognized
    /// when it's shared even if it's been renamed.
    pub id: Uuid,
    pub game: String,
    pub name: String,
    /// Mods with any of these tags (matched ignoring case) are tucked away
    /// at the bottom of a frontend's mod list. It's only about how the list
    /// is shown: hidden mods are still installed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_tags: Vec<String>,
    /// In load order: later mods win when two provide the same file.
    #[serde(default, rename = "mod")]
    pub mods: Vec<ProfileMod>,
    /// The plugins whose place in the load order the user has picked, for
    /// games that have them; see [`crate::plugins`].
    #[serde(default, rename = "plugin", skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginEntry>,
}

impl Profile {
    pub fn new(game: &str, name: &str) -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            id: Uuid::new_v4(),
            game: game.to_owned(),
            name: name.to_owned(),
            hidden_tags: Vec::new(),
            mods: Vec::new(),
            plugins: Vec::new(),
        }
    }

    pub fn reference(&self) -> ProfileRef {
        ProfileRef { uuid: self.id }
    }

    /// `<game>/<name>`, which is how users refer to the profile.
    pub fn label(&self) -> String {
        format!("{}/{}", self.game, self.name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut ProfileMod> {
        self.mods.iter_mut().find(|m| m.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileMod {
    /// Unique within the profile; how the user refers to the mod.
    pub name: String,
    pub source: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Which versions of the game the mod works with, in place of whatever
    /// its source says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<GameVersions>,
}

fn enabled_by_default() -> bool {
    true
}

/// Identifies a profile on this machine. Users refer to profiles as
/// `<game>/<name>`; see `Patchwork::find_profile`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileRef {
    pub uuid: Uuid,
}

impl fmt::Display for ProfileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.uuid)
    }
}

impl FromStr for ProfileRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = Uuid::from_str(s).map(|u| ProfileRef { uuid: u })?;
        Ok(val)
    }
}

/// Checks a game id or profile name, which must not contain `/` so that
/// `<game>/<name>` is unambiguous.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "`{name}` isn't a valid profile name; use letters, digits, `-`, `_` or `.`"
        )))
    }
}

pub(crate) fn no_such_mod(profile: &str, name: &str) -> Error {
    Error::invalid(format!("{profile} has no mod named `{name}`"))
}
