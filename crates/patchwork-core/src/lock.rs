use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;
use crate::source::{ArtifactFormat, ArtifactLocation, VersionReq};

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
    /// What this mod needs of its dependencies' versions, keyed like
    /// `dependencies`. Dependencies it doesn't care about are left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, VersionReq>,
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
mod tests {
    use super::*;
    use crate::source::compare_dotted;

    fn locked(id: &str, version: &str, requested: bool, requires: &[(&str, &str)]) -> LockedMod {
        LockedMod {
            name: id.to_owned(),
            source: "thunderstore".to_owned(),
            id: id.to_owned(),
            requested: requested.then(|| version.to_owned()),
            version: Some(version.to_owned()),
            hash: ContentHash::of(id.as_bytes()),
            format: ArtifactFormat::Zip,
            artifact: ArtifactLocation::Url {
                url: format!("https://example.com/{id}.zip"),
            },
            dependencies: requires.iter().map(|(k, _)| k.to_string()).collect(),
            requires: requires
                .iter()
                .map(|(k, r)| (k.to_string(), r.parse().unwrap()))
                .collect(),
        }
    }

    fn lockfile(mods: Vec<LockedMod>) -> Lockfile {
        Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: "lethal-company".to_owned(),
                definition: ContentHash::of(b"game"),
            },
            mods,
        }
    }

    #[test]
    fn finds_unmet_requirements() {
        let lock = lockfile(vec![
            locked("Old", "1.0.0", true, &[("thunderstore:Lib", ">=1.0")]),
            locked("New", "3.0.0", false, &[("thunderstore:Lib", ">=2.0")]),
            locked("Picky", "1.0.0", false, &[("thunderstore:Lib", "=1.0")]),
            locked("Lib", "1.5", true, &[]),
        ]);
        let unmet = lock.unmet_requirements(|_, a, b| compare_dotted(a, b));
        let names: Vec<_> = unmet.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["New", "Picky"]);
        assert_eq!(unmet[0].dependency_key, "thunderstore:Lib");
        assert_eq!(
            unmet[0].to_string(),
            "`New` needs `Lib` 2.0 or newer, but the profile pins it to 1.5"
        );
    }

    #[test]
    fn undecidable_requirements_are_assumed_met() {
        let lock = lockfile(vec![
            locked("Mod", "1.0", false, &[("thunderstore:Lib", ">=2.0")]),
            locked("Lib", "1.0beta", false, &[]),
        ]);
        assert!(
            lock.unmet_requirements(|_, a, b| compare_dotted(a, b))
                .is_empty()
        );
    }

    #[test]
    fn requirements_round_trip_through_toml() {
        let lock = lockfile(vec![locked(
            "Mod",
            "1.0",
            false,
            &[("thunderstore:BepInEx-BepInExPack", ">=5.4.2100")],
        )]);
        let text = toml::to_string(&lock).unwrap();
        assert!(
            text.contains(r#""thunderstore:BepInEx-BepInExPack" = ">=5.4.2100""#),
            "{text}"
        );
        let back: Lockfile = toml::from_str(&text).unwrap();
        assert_eq!(back.mods[0].requires, lock.mods[0].requires);
    }
}
