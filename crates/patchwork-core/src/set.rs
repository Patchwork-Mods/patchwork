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

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lock::mod_key;
use crate::profile::{Folder, Linked, Profile, ProfileMod};

/// The schema sets are saved with.
pub const SET_SCHEMA: u32 = 1;

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
}

impl Set {
    pub fn new(game: &str, name: &str) -> Self {
        Self {
            schema: SET_SCHEMA,
            id: Uuid::new_v4(),
            game: game.to_owned(),
            name: name.to_owned(),
            mods: Vec::new(),
        }
    }
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
/// moving one out of it moves it out of the set.
pub fn collapse(profile: &Profile) -> (Profile, Vec<(Uuid, Vec<ProfileMod>)>) {
    let mut stored = profile.clone();
    if profile.includes.is_empty() {
        return (stored, Vec::new());
    }
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
