use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    /// Per-source settings, e.g. `[sources.thunderstore] community = "..."`.
    #[serde(default)]
    pub sources: BTreeMap<String, toml::Table>,
    pub deployer: DeployerSpec,
    /// How files get from the store into the game directory.
    #[serde(default)]
    pub link: LinkMethod,
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
