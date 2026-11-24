//! The personal layer of a synced profile: mods a person adds on top of a
//! profile they share with friends or follow, which stay with them rather
//! than going to everyone (see `docs/design/folders.md`). It follows them
//! to their own linked computers, as a [`Layer`] (patchwork-sync's
//! `personal`), but never goes to anyone else.
//!
//! It's kept like a shared set (see [`crate::set`]) that only its profile
//! includes, whose id is the profile's own, in
//! `<data>/profile-state/<id>/personal.toml`:
//!
//! ```toml
//! schema = 1
//! game = "lethal-company"
//! after = "SomeSharedMod"   # where it goes in the order, like an include
//! disabled = ["Minimap"]    # its mods that are off
//!
//! [[mod]]
//! # ProfileMods
//! ```
//!
//! Loading a profile includes it, so its mods show as a linked folder,
//! "Personal", and everything that reads a profile sees them. Syncing
//! leaves it out ([`without`]), and so does what's synced of the lockfile:
//! what only the personal layer needs is kept in its own lockfile,
//! `personal.lock` beside it ([`split_lock`]), and locked on top of the
//! shared one without changing it.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lock::{LockedMod, Lockfile, mod_key};
use crate::profile::{Include, Profile, ProfileMod};
use crate::set::Set;

/// The newest schema of `personal.toml` this build reads.
pub const PERSONAL_SCHEMA: u32 = 1;

/// What the personal layer's linked folder is called.
pub const PERSONAL_NAME: &str = "Personal";

/// A profile's personal layer, as kept in `personal.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Personal {
    pub schema: u32,
    pub game: String,
    /// The profile's own mod its mods come after, as
    /// [`Include::after`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Its mods that are off, as [`Include::disabled`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
    /// In load order, as a set's are.
    #[serde(default, rename = "mod")]
    pub mods: Vec<ProfileMod>,
}

impl Personal {
    pub fn new(game: &str) -> Self {
        Self {
            schema: PERSONAL_SCHEMA,
            game: game.to_owned(),
            after: None,
            disabled: Vec::new(),
            mods: Vec::new(),
        }
    }

    /// The private set it is for the profile `id`.
    pub fn set(&self, id: Uuid) -> Set {
        Set {
            id,
            mods: self.mods.clone(),
            ..Set::new(&self.game, PERSONAL_NAME)
        }
    }

    /// The profile `id`'s include of it.
    pub fn include(&self, id: Uuid) -> Include {
        Include {
            after: self.after.clone(),
            disabled: self.disabled.clone(),
            ..Include::new(id)
        }
    }
}

/// A profile's personal layer with what only it needs locked, as it's
/// kept in step between the person's own computers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub personal: Personal,
    /// What `personal.lock` has: the locked mods only it needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<LockedMod>,
}

impl Layer {
    /// As it's put in the devices document. The same layer always gives
    /// the same bytes, so they tell whether it's changed.
    pub fn to_bytes(&self) -> Vec<u8> {
        // nothing in it can fail to serialize
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Two computers' layers for one profile that have never been in step
    /// (each had personal mods before they were linked): `theirs`, with
    /// the mods only `ours` has after its own, so neither loses any.
    pub fn merge(theirs: Layer, ours: Layer) -> Layer {
        let key = |m: &ProfileMod| mod_key(&m.source, &m.id);
        let mut personal = theirs.personal;
        let keys: HashSet<String> = personal.mods.iter().map(key).collect();
        for m in ours.personal.mods {
            if keys.contains(&key(&m)) {
                continue;
            }
            if ours.personal.disabled.contains(&m.name) && !personal.disabled.contains(&m.name) {
                personal.disabled.push(m.name.clone());
            }
            personal.mods.push(m);
        }
        let mut locked = theirs.locked;
        let have: HashSet<String> = locked.iter().map(LockedMod::key).collect();
        locked.extend(ours.locked.into_iter().filter(|m| !have.contains(&m.key())));
        let needed = reachable(personal.mods.iter().map(key), &locked);
        locked.retain(|m| needed.contains(&m.key()));
        Layer { personal, locked }
    }
}

/// `loaded` without its personal layer: what's synced of it.
pub fn without(loaded: &Profile) -> Profile {
    let mut shared = loaded.clone();
    shared.mods.retain(|m| !loaded.is_personal(m));
    shared.includes.retain(|i| i.set != loaded.id);
    shared
}

/// The keys (`source:id`) of `roots` and everything they depend on in
/// `mods`.
pub fn reachable(roots: impl IntoIterator<Item = String>, mods: &[LockedMod]) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut todo: Vec<String> = roots.into_iter().collect();
    while let Some(key) = todo.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(m) = mods.iter().find(|m| m.has_key(&key)) {
            todo.extend(m.dependencies.iter().cloned());
        }
    }
    seen
}

/// Splits `loaded`'s lockfile into what's synced and what only its
/// personal layer needs: locked mods that only its personal mods lead to.
/// What neither leads to stays with the synced part, as it would without
/// a personal layer.
pub fn split_lock(loaded: &Profile, lock: Lockfile) -> (Lockfile, Vec<LockedMod>) {
    let key = |m: &ProfileMod| mod_key(&m.source, &m.id);
    let shared = reachable(
        loaded
            .mods
            .iter()
            .filter(|m| !loaded.is_personal(m))
            .map(key),
        &lock.mods,
    );
    let personal = reachable(
        loaded
            .mods
            .iter()
            .filter(|m| loaded.is_personal(m))
            .map(key),
        &lock.mods,
    );
    let (personal, shared): (Vec<LockedMod>, Vec<LockedMod>) =
        lock.mods.into_iter().partition(|m| {
            let key = m.key();
            !shared.contains(&key) && personal.contains(&key)
        });
    (
        Lockfile {
            mods: shared,
            ..lock
        },
        personal,
    )
}

/// The lockfile with the personal layer's locked mods after the synced
/// ones, leaving out any the synced lockfile has.
pub fn merge_lock(shared: Option<Lockfile>, personal: Option<Lockfile>) -> Option<Lockfile> {
    let Some(personal) = personal else {
        return shared;
    };
    let Some(mut lock) = shared else {
        return Some(personal);
    };
    let keys: HashSet<String> = lock.mods.iter().map(LockedMod::key).collect();
    lock.mods.extend(
        personal
            .mods
            .into_iter()
            .filter(|m| !keys.contains(&m.key())),
    );
    Some(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::ContentHash;
    use crate::lock::{LOCK_SCHEMA, LockedGame};
    use crate::source::{ArtifactFormat, ArtifactLocation};

    fn locked(id: &str, deps: &[&str]) -> LockedMod {
        LockedMod {
            name: id.to_owned(),
            source: "s".to_owned(),
            id: id.to_owned(),
            requested: None,
            version: None,
            hash: ContentHash::of(id.as_bytes()),
            format: ArtifactFormat::File,
            artifact: ArtifactLocation::Url {
                url: format!("https://example.com/{id}"),
            },
            dependencies: deps.iter().map(|d| format!("s:{d}")).collect(),
            replaced: Default::default(),
            requires: Default::default(),
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
        }
    }

    fn profile_mod(id: &str) -> ProfileMod {
        toml::from_str(&format!("name = \"{id}\"\nsource = \"s\"\nid = \"{id}\"")).unwrap()
    }

    #[test]
    fn what_only_personal_mods_need_is_split_off() {
        let mut profile = Profile::new("g", "p");
        profile.includes.push(Include::new(profile.id));
        let mut mine = profile_mod("mine");
        mine.folder = Some(profile.id);
        profile.mods = vec![profile_mod("shared"), mine];
        let lock = Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: "g".to_owned(),
                env: Default::default(),
            },
            mods: vec![
                locked("shared", &["lib"]),
                locked("mine", &["lib", "own-lib"]),
                locked("lib", &[]),
                locked("own-lib", &[]),
                locked("stale", &[]),
            ],
        };
        let (shared, personal) = split_lock(&profile, lock);
        let ids = |mods: &[LockedMod]| mods.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&shared.mods), ["shared", "lib", "stale"]);
        assert_eq!(ids(&personal), ["mine", "own-lib"]);

        let merged = merge_lock(
            Some(shared.clone()),
            Some(Lockfile {
                mods: [personal, vec![locked("lib", &[])]].concat(),
                ..shared
            }),
        )
        .unwrap();
        assert_eq!(
            ids(&merged.mods),
            ["shared", "lib", "stale", "mine", "own-lib"]
        );
        assert_eq!(without(&profile).mods.len(), 1);
        assert!(without(&profile).includes.is_empty());
    }

    #[test]
    fn merging_layers_keeps_both_computers_mods() {
        let layer = |ids: &[&str], disabled: &[&str], locked: Vec<LockedMod>| Layer {
            personal: Personal {
                disabled: disabled.iter().map(|d| d.to_string()).collect(),
                mods: ids.iter().map(|id| profile_mod(id)).collect(),
                ..Personal::new("g")
            },
            locked,
        };
        let theirs = layer(
            &["a", "both"],
            &[],
            vec![
                locked("a", &[]),
                locked("both", &["lib"]),
                locked("lib", &[]),
            ],
        );
        let ours = layer(
            &["both", "b"],
            &["b"],
            vec![
                locked("both", &[]),
                locked("b", &["own-lib"]),
                locked("own-lib", &[]),
                locked("gone", &[]),
            ],
        );
        let merged = Layer::merge(theirs, ours);
        let names: Vec<&str> = merged
            .personal
            .mods
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(names, ["a", "both", "b"]);
        assert_eq!(merged.personal.disabled, ["b"]);
        let ids: Vec<&str> = merged.locked.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "both", "lib", "b", "own-lib"]);

        let bytes = merged.to_bytes();
        assert_eq!(Layer::from_bytes(&bytes).unwrap().to_bytes(), bytes);
    }
}
