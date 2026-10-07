use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

pub const PROFILE_SCHEMA: u32 = 1;

/// What the user wants installed for a game. Meant to be readable and
/// hand-editable; the exact versions that were resolved live in the lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub schema: u32,
    pub game: String,
    /// In load order: later mods win when two provide the same file.
    #[serde(default, rename = "mod")]
    pub mods: Vec<ProfileMod>,
}

impl Profile {
    pub fn new(game: &str) -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            game: game.to_owned(),
            mods: Vec::new(),
        }
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
}

fn enabled_by_default() -> bool {
    true
}

/// `<game>/<profile>`
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileRef {
    pub game: String,
    pub name: String,
}

impl fmt::Display for ProfileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.game, self.name)
    }
}

impl FromStr for ProfileRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && part != "."
                && part != ".."
        };
        match s.split_once('/') {
            Some((game, name)) if valid(game) && valid(name) => Ok(Self {
                game: game.to_owned(),
                name: name.to_owned(),
            }),
            _ => Err(Error::invalid(format!(
                "`{s}` isn't a valid profile; expected `<game>/<name>` using letters, digits, `-`, `_` or `.`"
            ))),
        }
    }
}
