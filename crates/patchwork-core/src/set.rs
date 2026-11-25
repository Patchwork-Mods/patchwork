//! Shared sets: lists of mods that live outside any profile, which several
//! profiles of a game include. In a profile, an included set shows as a
//! linked folder (see `docs/design/folders.md`).
//!
//! A profile is loaded expanded ([`expand`]): each set's mods are among
//! its own, as one block where the include is, in a folder whose id is the
//! set's. Everything that reads a profile sees all it installs that way,
//! and changes to a set's mods are made like any other change to the
//! profile. Saving splits it back up ([`collapse`]): the profile keeps its
//! own mods and each include's place and `disabled`, and the sets whose
//! mods changed are saved with them.
//!
//! A set's config edits are merged in the same way: as loaded, a profile's
//! `config` has its sets' settings (a later set's over an earlier one's)
//! with its own over them, setting by setting, and the personal layer's
//! (with those of the sets it includes) over everything, as the person's
//! own (see [`crate::personal`]). Saving keeps as the profile's own only
//! the settings that aren't what its sets give, and those the personal
//! layer's hide as they were, so saving a profile never changes a set's
//! config; that's done on purpose, with [`crate::Patchwork::set_config`].

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{FileEdits, beyond, covered, overlay};
use crate::lock::mod_key;
use crate::profile::{Folder, Linked, Profile, ProfileMod};

/// The newest schema of sets this build reads. Schema 2 added `config`; a
/// set without config edits is still saved as 1 (see [`Set::saved_schema`]),
/// so builds that would drop them don't open sets that have them.
pub const SET_SCHEMA: u32 = 2;

/// The schema a set with no config edits is saved as.
const SCHEMA_WITHOUT_CONFIG: u32 = 1;

/// A list of mods several profiles of a game can include.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Set {
    pub schema: u32,
    pub id: Uuid,
    pub game: String,
    /// What its linked folder is called. Unique among the game's sets,
    /// ignoring case.
    pub name: String,
    /// In load order. They're always `enabled` here; whether one's on is
    /// up to each profile (see [`crate::profile::Include::disabled`]), and they're in no
    /// folder.
    #[serde(default, rename = "mod")]
    pub mods: Vec<ProfileMod>,
    /// Settings the set changes in config files, as a profile's
    /// [`config`](Profile::config) does. A profile's own win over them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: FileEdits,
}

impl Set {
    pub fn new(game: &str, name: &str) -> Self {
        Self {
            schema: SET_SCHEMA,
            id: Uuid::new_v4(),
            game: game.to_owned(),
            name: name.to_owned(),
            mods: Vec::new(),
            config: BTreeMap::new(),
        }
    }

    /// [`SET_SCHEMA`] if it changes config files, otherwise the schema from
    /// before sets could.
    pub fn saved_schema(&self) -> u32 {
        if self.config.is_empty() {
            SCHEMA_WITHOUT_CONFIG
        } else {
            SET_SCHEMA
        }
    }
}

/// Where a folder another profile has comes from, to use it in one more
/// (see [`Patchwork::use_folder`](crate::Patchwork::use_folder)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FolderSource {
    /// A set, which each profile that has it shows as a linked folder.
    Set { set: Uuid },
    /// One of a profile's own folders, which becomes a set once a second
    /// profile uses it.
    Folder { profile: Uuid, folder: Uuid },
}

/// A folder of the game's other profiles that a profile could use too,
/// from [`Patchwork::other_folders`](crate::Patchwork::other_folders).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherFolder {
    #[serde(flatten)]
    pub source: FolderSource,
    pub name: String,
    /// Its mods' names, in order.
    pub mods: Vec<String>,
    /// The names of the profiles that have it. Empty for a set left over
    /// from before sets went once no profile had them.
    pub profiles: Vec<String>,
    /// Its mods the profile already has of its own, whose own entries win
    /// over the folder's.
    pub already: Vec<String>,
    /// Whether, used as the same folder, it'd be in the profile's personal
    /// layer whatever's asked (see [`crate::personal`]): the profile's
    /// only followed here, so it'd only be on this person's computers.
    /// Copied in, it's the profile's own, which is personal too then.
    pub personal: bool,
}

/// The settings the sets `profile` includes change in config files, as
/// loaded: a later include's over an earlier one's. With `personal`, those
/// of its personal layer and the sets it includes personally, which go
/// over the profile's own; otherwise the rest, which go under them.
pub fn sets_config(profile: &Profile, personal: bool) -> FileEdits {
    let mut config = FileEdits::new();
    for linked in profile
        .includes
        .iter()
        .filter(|i| i.personal == personal)
        .filter_map(|i| i.linked.as_ref())
    {
        overlay(&mut config, &linked.config);
    }
    config
}

/// The settings `profile` (as loaded) changes in config files itself:
/// those that aren't what its sets give them. Where its personal layer
/// changes a setting (or did when it was loaded), it's what the profile
/// had under it, if anything (see [`Profile::personal_config`]).
pub fn own_config(profile: &Profile) -> FileEdits {
    let mut own = beyond(&profile.config, &sets_config(profile, false));
    let mut over = sets_config(profile, true);
    overlay(&mut over, &profile.personal_config.given);
    if !over.is_empty() {
        own = covered(&own, &over, false);
        overlay(&mut own, &profile.personal_config.hidden);
    }
    own
}

/// Sets `profile`'s config to `own` over what its sets change, with what
/// its personal layer changes over that, as [`expand`] does.
pub fn merge_config(profile: &mut Profile, own: &FileEdits) {
    let personal = sets_config(profile, true);
    let mut config = sets_config(profile, false);
    overlay(&mut config, own);
    profile.personal_config.hidden = covered(own, &personal, true);
    overlay(&mut config, &personal);
    profile.personal_config.given = personal;
    profile.config = config;
}

/// Puts the mods of the sets `profile` includes among its own, as it's
/// stored, with `set` giving each set (`None` for one that can't be
/// found). Each set's mods go where its include is, as a linked folder:
/// their `folder` is the set's id and they're `enabled` unless the include
/// has them `disabled`. A set's mod that the profile has too (the same
/// `source:id`) is left out, since the profile's own wins, and one whose
/// name is taken is shown under another.
pub fn expand(profile: &mut Profile, mut set: impl FnMut(Uuid) -> Option<Set>) {
    if profile.includes.is_empty() {
        return;
    }
    let own_keys: HashSet<String> = profile
        .mods
        .iter()
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    let mut taken: HashSet<String> = profile.mods.iter().map(|m| m.name.clone()).collect();
    let mut blocks: Vec<Vec<ProfileMod>> = Vec::new();
    for include in &mut profile.includes {
        let mut block = Vec::new();
        include.linked = set(include.set).map(|set| {
            let mut linked = Linked {
                folder: Folder {
                    id: set.id,
                    name: set.name.clone(),
                },
                mods: set.mods.clone(),
                config: set.config.clone(),
                shadowed: Vec::new(),
                renamed: Vec::new(),
            };
            for (i, mut m) in set.mods.into_iter().enumerate() {
                if own_keys.contains(&mod_key(&m.source, &m.id)) {
                    linked.shadowed.push((i, m));
                    continue;
                }
                m.enabled = !include.disabled.contains(&m.name);
                m.folder = Some(set.id);
                if !taken.insert(m.name.clone()) {
                    let shown = unused_name(&format!("{} ({})", m.name, set.name), &taken);
                    taken.insert(shown.clone());
                    linked.renamed.push((shown.clone(), m.name.clone()));
                    m.name = shown;
                }
                block.push(m);
            }
            linked
        });
        blocks.push(block);
    }

    let own: HashSet<String> = profile.mods.iter().map(|m| m.name.clone()).collect();
    let mut after: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    let mut last = Vec::new();
    for (i, include) in profile.includes.iter().enumerate() {
        match include.after.as_deref() {
            Some(name) if !own.contains(name) => last.push(i),
            at => after.entry(at).or_default().push(i),
        }
    }
    let mut take = |i: &usize| std::mem::take(&mut blocks[*i]);
    let mut mods: Vec<ProfileMod> = after
        .get(&None)
        .into_iter()
        .flatten()
        .flat_map(&mut take)
        .collect();
    for m in std::mem::take(&mut profile.mods) {
        let name = m.name.clone();
        mods.push(m);
        mods.extend(
            after
                .get(&Some(name.as_str()))
                .into_iter()
                .flatten()
                .flat_map(&mut take),
        );
    }
    mods.extend(last.iter().flat_map(take));
    profile.mods = mods;
    let own = std::mem::take(&mut profile.config);
    merge_config(profile, &own);
}

/// A name based on `name` that isn't in `taken`.
pub(crate) fn unused_name(name: &str, taken: &HashSet<String>) -> String {
    (1..)
        .map(|n| match n {
            1 => name.to_owned(),
            n => format!("{name} {n}"),
        })
        .find(|n| !taken.contains(n))
        .expect("there's always one")
}

/// Splits an expanded profile (see [`expand`]) back into the profile as
/// it's stored and, for each set whose mods have changed, its mods now.
/// Mods in a set's linked folder are the set's, anywhere in the profile;
/// moving one out of it moves it out of the set. Config edits that are
/// what the sets give are left out; see [`own_config`].
pub fn collapse(profile: &Profile) -> (Profile, Vec<(Uuid, Vec<ProfileMod>)>) {
    let mut stored = profile.clone();
    if profile.includes.is_empty() {
        return (stored, Vec::new());
    }
    stored.config = own_config(profile);
    let linked: HashSet<Uuid> = profile
        .includes
        .iter()
        .filter(|i| i.linked.is_some())
        .map(|i| i.set)
        .collect();
    let mut own = Vec::new();
    let mut blocks: HashMap<Uuid, Vec<ProfileMod>> = HashMap::new();
    let mut placed: HashMap<Uuid, Option<String>> = HashMap::new();
    let mut order: Vec<Uuid> = Vec::new();
    for m in &profile.mods {
        match m.folder.filter(|id| linked.contains(id)) {
            Some(id) => {
                if let std::collections::hash_map::Entry::Vacant(e) = placed.entry(id) {
                    e.insert(own.last().map(|m: &ProfileMod| m.name.clone()));
                    order.push(id);
                }
                blocks.entry(id).or_default().push(m.clone());
            }
            None => own.push(m.clone()),
        }
    }
    stored.mods = own;

    let mut changed = Vec::new();
    let mut includes = Vec::new();
    for include in &profile.includes {
        let mut include = include.clone();
        let Some(linked) = include.linked.take() else {
            includes.push(include);
            continue;
        };
        if let Some(after) = placed.remove(&include.set) {
            include.after = after;
        }
        let renamed: HashMap<&str, &str> = linked
            .renamed
            .iter()
            .map(|(shown, name)| (shown.as_str(), name.as_str()))
            .collect();
        let mut mods = Vec::new();
        let mut disabled = Vec::new();
        for m in blocks.remove(&include.set).unwrap_or_default() {
            let mut m = m;
            if let Some(name) = renamed.get(m.name.as_str()) {
                m.name = (*name).to_owned();
            }
            if !m.enabled {
                disabled.push(m.name.clone());
            }
            m.enabled = true;
            m.folder = None;
            mods.push(m);
        }
        for (at, m) in linked.shadowed {
            if include.disabled.contains(&m.name) {
                disabled.push(m.name.clone());
            }
            mods.insert(at.min(mods.len()), m);
        }
        include.disabled = disabled;
        if mods != linked.mods {
            changed.push((include.set, mods));
        }
        includes.push(include);
    }
    // in the order their blocks are in, then any that are empty
    includes.sort_by_key(|i| {
        order
            .iter()
            .position(|id| *id == i.set)
            .unwrap_or(usize::MAX)
    });
    stored.includes = includes;
    (stored, changed)
}

#[cfg(test)]
mod tests;
