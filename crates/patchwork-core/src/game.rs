use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::relpath::RelPath;

pub const GAME_SCHEMA: u32 = 1;

/// A game definition: which sources a game's mods come from and how they're
/// deployed. These are plain data and are loaded at runtime from registries,
/// so supporting a new game doesn't need a new patchwork build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameDef {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A URL, or a path to an image file. Relative paths are resolved against
    /// the definition file's directory when the registry is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Left out of game lists, for builtin games a user doesn't play.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Per-source settings, e.g. `[sources.thunderstore] community = "..."`.
    #[serde(default)]
    pub sources: BTreeMap<String, toml::Table>,
    pub deployer: DeployerSpec,
    /// How files get from the store into the game directory.
    #[serde(default)]
    pub link: LinkMethod,
    #[serde(default, skip_serializing_if = "Detect::is_empty")]
    pub detect: Detect,
}

/// Hints for finding where a game is installed. They don't affect deploying.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Steam app ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steam: Vec<u32>,
    /// The install folder's name, e.g. under Steam's `steamapps/common`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Executables, any of which marks a folder as this game's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exe: Vec<String>,
    /// Ways to find out which version of the game is installed, tried in
    /// order until one finds it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version: Vec<VersionProbe>,
}

/// Somewhere a game's version can be read from, relative to its install.
///
/// ```toml
/// [[detect.version]]
/// kind = "exe"               # the file version in a Windows executable
/// file = "SkyrimSE.exe"
/// parts = 3                  # 1.6.1170.0 -> 1.6.1170
///
/// [[detect.version]]
/// kind = "json"              # a string in a JSON file
/// file = "version.json"
/// key = "game.version"       # dots separate nested keys
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum VersionProbe {
    Exe {
        file: RelPath,
        /// How many of the version's four numbers to keep.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parts: Option<usize>,
    },
    Json {
        file: RelPath,
        key: String,
    },
}

impl Detect {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployerSpec {
    pub kind: String,
    /// Everything else in the table, handed to the deployer to interpret.
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMethod {
    /// The safe default: games that rewrite their own files can't corrupt the
    /// store.
    #[default]
    Copy,
    /// Falls back to copying if the store and game are on different
    /// filesystems.
    Hardlink,
    Symlink,
}

/// Game ids name files and appear in profile references (`<game>/<name>`),
/// so they're kept to lowercase letters, digits and `-`.
pub fn validate_game_id(id: &str) -> Result<(), Error> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "`{id}` isn't a valid game id; use lowercase letters, digits and `-`"
        )))
    }
}
