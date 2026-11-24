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
//! The person can include shared sets in it too: those includes are kept
//! here rather than in the profile's file, so the sets' mods are only on
//! their computers as well. With any, it's schema 2, and the layer's own
//! place is an include among them (see [`Personal::includes`]):
//!
//! ```toml
//! schema = 2
//! game = "lethal-company"
//!
//! [[include]]
//! set = "<the profile's id>"   # its own mods
//! after = "SomeSharedMod"
//!
//! [[include]]
//! set = "<a shared set's id>"
//! after = "SomeSharedMod"
//! disabled = ["Shaders"]
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
use crate::set::{Set, merge_config, own_config};

/// The newest schema of `personal.toml` this build reads. Schema 2 added
/// `include`; a layer that includes no sets is still saved as 1 (see
/// [`Personal::saved_schema`]), so builds that would drop them don't open
/// ones that do.
pub const PERSONAL_SCHEMA: u32 = 2;

/// The schema a personal layer that includes no sets is saved as.
const SCHEMA_WITHOUT_INCLUDES: u32 = 1;

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
    /// The shared sets the person includes in the profile, in order. Once
    /// there are any, the layer's own mods' place is among them, as an
    /// include of the profile's own id, rather than `after` and
    /// `disabled`. See [`Personal::includes`].
    #[serde(default, rename = "include", skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<Include>,
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
            includes: Vec::new(),
            mods: Vec::new(),
        }
    }

    /// Whether there's nothing in the profile `id`'s layer: no mods of its
    /// own, and no sets.
    pub fn is_empty(&self, id: Uuid) -> bool {
        self.mods.is_empty() && self.includes.iter().all(|i| i.set == id)
    }

    /// [`PERSONAL_SCHEMA`] if it includes sets, otherwise the schema from
    /// before it could.
    pub fn saved_schema(&self) -> u32 {
        if self.includes.is_empty() {
            SCHEMA_WITHOUT_INCLUDES
        } else {
            PERSONAL_SCHEMA
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

    /// The profile `id`'s personal includes, in order: the sets included
    /// personally, and the layer's own (the private set whose id is the
    /// profile's) if it has mods, first unless it's been put among them.
    pub fn includes(&self, id: Uuid) -> Vec<Include> {
        let mut includes = self.includes.clone();
        if !includes.iter().any(|i| i.set == id) {
            includes.insert(
                0,
                Include {
                    after: self.after.clone(),
                    disabled: self.disabled.clone(),
                    ..Include::new(id)
                },
            );
        }
        if self.mods.is_empty() {
            includes.retain(|i| i.set != id);
        }
        for include in &mut includes {
            include.personal = true;
            include.linked = None;
        }
        includes
    }

    /// Keeps `includes` (as [`Personal::includes`] gives them) as the
    /// profile `id`'s personal includes: the layer's own place in `after`
    /// and `disabled` if there are no others, so it's saved as it was
    /// before sets could be included, or among them if there are.
    pub fn set_includes(&mut self, id: Uuid, includes: Vec<Include>) {
        let includes: Vec<Include> = includes
            .into_iter()
            .map(|i| Include {
                linked: None,
                personal: false,
                ..i
            })
            .collect();
        if includes.iter().all(|i| i.set == id) {
            let own = includes.into_iter().next();
            self.after = own.as_ref().and_then(|i| i.after.clone());
            self.disabled = own.map(|i| i.disabled).unwrap_or_default();
            self.includes = Vec::new();
        } else {
            self.after = None;
            self.disabled = Vec::new();
            self.includes = includes;
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
        for include in ours.personal.includes {
            if !personal.includes.iter().any(|i| i.set == include.set) {
                personal.includes.push(include);
            }
        }
        let mut locked = theirs.locked;
        let have: HashSet<String> = locked.iter().map(LockedMod::key).collect();
        locked.extend(ours.locked.into_iter().filter(|m| !have.contains(&m.key())));
        // What the sets included need can't be told without them, so then
        // it's left to the next lock to tidy.
        if personal.includes.is_empty() {
            let needed = reachable(personal.mods.iter().map(key), &locked);
            locked.retain(|m| needed.contains(&m.key()));
        }
        Layer { personal, locked }
    }
}

/// `loaded` without its personal layer: what's synced of it. The config
/// edits of sets included personally go with them.
pub fn without(loaded: &Profile) -> Profile {
    let mut shared = loaded.clone();
    shared.mods.retain(|m| !loaded.is_personal(m));
    shared.includes.retain(|i| !i.personal);
    merge_config(&mut shared, &own_config(loaded));
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
        profile.includes.push(Include {
            personal: true,
            ..Include::new(profile.id)
        });
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
    fn its_own_place_is_kept_among_the_sets_it_includes() {
        let id = Uuid::new_v4();
        let set = Uuid::new_v4();
        let mut personal = Personal {
            mods: vec![profile_mod("mine")],
            ..Personal::new("g")
        };
        let at = |after: &str, set| Include {
            after: Some(after.to_owned()),
            ..Include::new(set)
        };
        // with no sets, it's kept as before there were any
        personal.set_includes(id, vec![at("a", id)]);
        assert_eq!(personal.after.as_deref(), Some("a"));
        assert_eq!(personal.saved_schema(), 1);
        let sets = |p: &Personal| p.includes(id).iter().map(|i| i.set).collect::<Vec<_>>();
        assert_eq!(sets(&personal), [id]);
        assert!(personal.includes(id).iter().all(|i| i.personal));

        // with one, after its own mods
        personal.set_includes(id, vec![at("a", set), at("a", id)]);
        assert_eq!(personal.after, None);
        assert_eq!(personal.saved_schema(), 2);
        assert_eq!(sets(&personal), [set, id]);
        assert!(!personal.is_empty(id));
        // which aren't there once there are none
        personal.mods.clear();
        assert_eq!(sets(&personal), [set]);
        assert!(!personal.is_empty(id));
        personal.set_includes(id, Vec::new());
        assert!(personal.is_empty(id));
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
