use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::compat::GameVersions;
use crate::hash::ContentHash;
use crate::source::{ArtifactFormat, ArtifactLocation, VersionReq};

/// Schema 2 added [`ArtifactLocation::Source`] artifacts.
pub const LOCK_SCHEMA: u32 = 2;

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
    /// What this mod needs of its dependencies' versions, keyed like
    /// `dependencies`. Dependencies it doesn't care about are left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, VersionReq>,
    /// Which versions of the game the source says this is for. Left out
    /// for mods that don't care, going by the game's `[versions] files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<GameVersions>,
}

impl LockedMod {
    pub fn key(&self) -> String {
        mod_key(&self.source, &self.id)
    }
}

pub fn mod_key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}

/// A dependency whose locked version doesn't meet what a mod needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmetRequirement {
    /// The mod with the requirement.
    pub name: String,
    pub dependency: String,
    /// `source:id` of the dependency.
    pub dependency_key: String,
    pub req: VersionReq,
    /// The version that's locked.
    pub found: String,
    /// Whether the profile pins the dependency to that version. If not,
    /// [`crate::Patchwork::lock`] made sure it's the newest there is.
    pub pinned: bool,
}

impl fmt::Display for UnmetRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let needs = match &self.req {
            VersionReq::Any => "any version".to_owned(),
            VersionReq::AtLeast(v) => format!("{v} or newer"),
            VersionReq::Exact(v) => format!("exactly {v}"),
        };
        write!(
            f,
            "`{}` needs `{}` {needs}, but ",
            self.name, self.dependency
        )?;
        if self.pinned {
            write!(f, "the profile pins it to {}", self.found)
        } else {
            write!(f, "{} is the newest available", self.found)
        }
    }
}

impl Lockfile {
    /// Every requirement that the locked versions don't meet. `compare`
    /// orders two versions from the named source; requirements it can't
    /// decide are assumed to be met.
    pub fn unmet_requirements(
        &self,
        compare: impl Fn(&str, &str, &str) -> Option<Ordering>,
    ) -> Vec<UnmetRequirement> {
        let by_key: HashMap<String, &LockedMod> = self.mods.iter().map(|m| (m.key(), m)).collect();
        let mut unmet = Vec::new();
        for m in &self.mods {
            for (key, req) in &m.requires {
                let Some(dep) = by_key.get(key) else {
                    continue;
                };
                let Some(found) = &dep.version else {
                    continue;
                };
                let met = req.matches(found, |a, b| compare(&dep.source, a, b));
                if met == Some(false) {
                    unmet.push(UnmetRequirement {
                        name: m.name.clone(),
                        dependency: dep.name.clone(),
                        dependency_key: key.clone(),
                        req: req.clone(),
                        found: found.clone(),
                        pinned: dep.requested.is_some(),
                    });
                }
            }
        }
        unmet
    }
}

#[cfg(test)]
mod tests;
