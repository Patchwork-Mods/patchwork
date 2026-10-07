use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;
use crate::source::{ArtifactFormat, ArtifactLocation};

pub const LOCK_SCHEMA: u32 = 1;

/// Exactly what a profile resolved to. Together with the profile, this is
/// enough to reproduce a setup on another machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub schema: u32,
    pub game: LockedGame,
    #[serde(default, rename = "mod")]
    pub mods: Vec<LockedMod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedGame {
    pub id: String,
    /// Hash of the game definition this was locked against.
    pub definition: ContentHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedMod {
    pub name: String,
    pub source: String,
    pub id: String,
    /// The version requested in the profile when this was resolved, so a
    /// changed request triggers a fresh resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub hash: ContentHash,
    pub format: ArtifactFormat,
    pub artifact: ArtifactLocation,
    /// `source:id` of each dependency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

impl LockedMod {
    pub fn key(&self) -> String {
        mod_key(&self.source, &self.id)
    }
}

pub fn mod_key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}
