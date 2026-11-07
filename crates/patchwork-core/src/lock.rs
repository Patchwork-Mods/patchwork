use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::compat::{EnvSupport, GameVersions};
use crate::hash::ContentHash;
use crate::source::{ArtifactFormat, ArtifactLocation, SoftDependency, VersionReq};

/// Schema 2 added [`ArtifactLocation::Source`] artifacts. Schema 3 added
/// [`ArtifactLocation::Local`] ones, which replaced paths, and dropped the
/// game definition's hash, which differs between machines with different
/// registries.
pub const LOCK_SCHEMA: u32 = 3;

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
    /// The profile's `env` when it was locked. Mods are picked to suit it,
    /// so when it changes they're all resolved again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
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
    /// `source:id` of each dependency, after replacements: the mod that
    /// stands in for a replaced one is here in its place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    /// The dependencies the profile replaced (see
    /// [`crate::profile::ProfileMod::replaces`]), as the source names them,
    /// each with the `source:id` of the mod standing in for it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub replaced: BTreeMap<String, String>,
    /// What this mod needs of its dependencies' versions, keyed like the
    /// source names them (so not after replacements). Dependencies it
    /// doesn't care about are left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, VersionReq>,
    /// What it lists that it may not need, which isn't locked; see
    /// [`SoftDependency`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soft_dependencies: Vec<SoftDependency>,
    /// Which versions of the game the source says this is for. Left out
    /// for mods that don't care, going by the game's `[versions] files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<GameVersions>,
    /// Which values of the game's other `[env]` axes the source says this
    /// works with.
    #[serde(default, skip_serializing_if = "EnvSupport::is_empty")]
    pub env: EnvSupport,
    /// What sort of thing the source says this is, from
    /// [`crate::source::ResolvedMod::kind`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl LockedMod {
    pub fn key(&self) -> String {
        mod_key(&self.source, &self.id)
    }

    /// Whether its dependencies were replaced just as `replacements` (from
    /// [`crate::profile::Profile::replacements`]) says they should be. If
    /// not, it has to be resolved again to find which they are.
    pub fn fits_replacements(&self, replacements: &BTreeMap<String, String>) -> bool {
        let key = self.key();
        let wanted = |dep: &String| replacements.get(dep).filter(|with| **with != key);
        self.replaced
            .iter()
            .all(|(dep, with)| wanted(dep) == Some(with))
            && self.dependencies.iter().all(|dep| wanted(dep).is_none())
    }
}

/// `dependencies` (as `source:id`, as the source named them) of mod `key`,
/// with the ones in `replacements` swapped for what stands in for them,
/// and which those were, for [`LockedMod::dependencies`] and
/// [`LockedMod::replaced`]. A mod never stands in for its own dependency.
pub fn replace_dependencies(
    key: &str,
    dependencies: Vec<String>,
    replacements: &BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut replaced = BTreeMap::new();
    let mut effective = Vec::new();
    for dep in dependencies {
        let dep = match replacements.get(&dep) {
            Some(with) if with != key => {
                replaced.insert(dep, with.clone());
                with.clone()
            }
            _ => dep,
        };
        // two of them may now be the same mod
        if !effective.contains(&dep) {
            effective.push(dep);
        }
    }
    (effective, replaced)
}

pub fn mod_key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}

/// A mod in a lockfile, or a dependency the profile replaced, and how it's
/// tied to the rest; from [`Lockfile::dependency_info`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyInfo {
    /// `source:id`.
    pub key: String,
    pub source: String,
    pub id: String,
    /// What it's locked as, or its id if it's been replaced, since then it
    /// isn't locked.
    pub name: String,
    /// The locked version; none if it's been replaced.
    pub version: Option<String>,
    /// Whether it's one of the profile's mods, rather than only there
    /// because others need it.
    pub in_profile: bool,
    /// The names of the mods that need it, in lockfile order.
    pub needed_by: Vec<String>,
    /// The name of the profile mod standing in for it, if it's been
    /// replaced.
    pub replaced_by: Option<String>,
}

impl Lockfile {
    /// Every locked mod and every dependency `profile` replaced, with which
    /// mods need each, in lockfile order with the replaced ones last.
    pub fn dependency_info(&self, profile: &crate::profile::Profile) -> Vec<DependencyInfo> {
        let mut needed_by: HashMap<&str, Vec<String>> = HashMap::new();
        for m in &self.mods {
            for dep in m.dependencies.iter().chain(m.replaced.keys()) {
                needed_by.entry(dep).or_default().push(m.name.clone());
            }
        }
        let mut needers = |key: &str| needed_by.remove(key).unwrap_or_default();
        let in_profile = |key: &str| {
            profile
                .mods
                .iter()
                .any(|m| mod_key(&m.source, &m.id) == key)
        };
        let mut info: Vec<DependencyInfo> = self
            .mods
            .iter()
            .map(|m| {
                let key = m.key();
                DependencyInfo {
                    in_profile: in_profile(&key),
                    needed_by: needers(&key),
                    source: m.source.clone(),
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    replaced_by: None,
                    key,
                }
            })
            .collect();
        let replaced: BTreeMap<&String, &String> =
            self.mods.iter().flat_map(|m| &m.replaced).collect();
        for (key, with) in replaced {
            if info.iter().any(|i| i.key == *key) {
                continue;
            }
            let (source, id) = key.split_once(':').unwrap_or(("", key));
            info.push(DependencyInfo {
                key: key.clone(),
                source: source.to_owned(),
                id: id.to_owned(),
                name: id.to_owned(),
                version: None,
                in_profile: false,
                needed_by: needers(key),
                replaced_by: profile
                    .mods
                    .iter()
                    .find(|m| mod_key(&m.source, &m.id) == **with)
                    .map(|m| m.name.clone()),
            });
        }
        info
    }
}

/// A soft dependency of a locked mod, and whether the profile has it; from
/// [`Lockfile::soft_dependencies`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftDependencyInfo {
    #[serde(flatten)]
    pub dependency: SoftDependency,
    /// What [`crate::profile::ProfileMod::ignores`] calls it.
    pub ident: String,
    /// The name of the mod that's it, or that stands in for it, if the
    /// profile or the lockfile has one.
    pub installed: Option<String>,
    /// Whether the mod's entry in the profile ignores it.
    pub ignored: bool,
}

impl SoftDependencyInfo {
    /// Whether it's worth pointing out: it isn't there, and the user
    /// hasn't said to ignore it.
    pub fn missing(&self) -> bool {
        self.installed.is_none() && !self.ignored
    }
}

impl Lockfile {
    /// The soft dependencies of locked mod `key`, with whether `profile`
    /// has each. Empty if it isn't locked.
    pub fn soft_dependencies(
        &self,
        profile: &crate::profile::Profile,
        key: &str,
    ) -> Vec<SoftDependencyInfo> {
        let Some(locked) = self.mods.iter().find(|m| m.key() == key) else {
            return Vec::new();
        };
        let ignores = profile
            .mods
            .iter()
            .find(|m| mod_key(&m.source, &m.id) == key)
            .map_or(&[][..], |m| &m.ignores[..]);
        let replacements = profile.replacements();
        let name_of = |key: &str| {
            profile
                .mods
                .iter()
                .find(|m| mod_key(&m.source, &m.id) == key)
                .map(|m| m.name.clone())
                .or_else(|| {
                    self.mods
                        .iter()
                        .find(|m| m.key() == key)
                        .map(|m| m.name.clone())
                })
        };
        locked
            .soft_dependencies
            .iter()
            .map(|dep| {
                let installed = dep
                    .key
                    .as_deref()
                    .and_then(|key| name_of(key).or_else(|| name_of(replacements.get(key)?)));
                SoftDependencyInfo {
                    ident: dep.ident().to_owned(),
                    ignored: ignores.iter().any(|i| i == dep.ident()),
                    installed,
                    dependency: dep.clone(),
                }
            })
            .collect()
    }
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
                // a replacement's versions aren't the original's
                if m.replaced.contains_key(key) {
                    continue;
                }
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
