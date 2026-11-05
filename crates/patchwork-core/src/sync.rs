//! Keeping a profile in step between machines, as key/value entries.
//!
//! A synced profile is a set of entries, each a key and a JSON value, of the
//! kind a replicated key/value store (iroh-docs) keeps the newest of for
//! each key, whoever wrote it. So that two people's changes both survive
//! when they're made at once, each mod is its own entry, as is each mod's
//! locked version:
//!
//! - `format`: [`SYNC_FORMAT`], so peers can tell they can't read each other
//! - `profile`: its id, game, name, hidden tags and `env` picks
//! - `mod/<source>:<id>`: a [`ProfileMod`]
//! - `order`: the mods' load order, as `<source>:<id>` keys. It's one entry,
//!   so of two reorders made at once only one survives. Mods it doesn't
//!   mention (added while someone else reordered) go at the end.
//! - `ext/<id>`: what extension `<id>` keeps for the profile (see
//!   [`Profile::ext`]), so two extensions' changes made at once both
//!   survive. What extensions keep for a mod is part of the mod's entry.
//! - `lock/<source>:<id>`: a [`LockedMod`], for mods and their dependencies.
//!   Whoever changes the profile relocks it and publishes what changed, so
//!   other machines take the lockfile as it is rather than resolving again.
//!   Locked mods nothing depends on any more are left out, and mods that
//!   aren't locked (see [`unlocked`]) are locked wherever the profile is
//!   next locked.
//! - `lock-env`: the `env` the lockfile was locked for (see
//!   [`LockedGame::env`]), there once anything's been locked
//!
//! Keys starting with `file/` or `peer/`, and `moved`, belong to the sync
//! layer, which hands local mods' files to machines that don't have them
//! that way, keeps track of who's syncing, and marks a document everyone's
//! moved on from; they're ignored here.
//!
//! A mod, locked mod or extension's state that's been removed has `null` for its value; nothing
//! is ever deleted outright, so a removal replaces older values like any
//! other change.
//!
//! What only means something on one machine (where local mods were added
//! from, what was deployed, game installs) isn't synced.
//!
//! Format 1 kept the plugin load order in its own `plugins` entry; it's
//! read as the Bethesda extension's state (see
//! [`crate::profile::LEGACY_PLUGINS`]) until that's been written.
//!
//! Entries can be added in a later version without changing [`SYNC_FORMAT`]:
//! unknown keys and fields are ignored. It only goes up for changes older
//! versions would get wrong, and they then refuse the profile rather than
//! misread it.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile};
use crate::profile::{LEGACY_PLUGINS, PROFILE_SCHEMA, Profile, ProfileMod, upgrade_plugins};
use crate::{Error, Result};

/// The version of the entries' layout this build reads and writes. 2 moved
/// the plugin load order into extensions' state, which format 1 builds
/// would lose.
pub const SYNC_FORMAT: u32 = 2;

/// The newest value of each key; see the [module docs](self).
pub type Entries = BTreeMap<String, Vec<u8>>;

pub const FORMAT: &str = "format";
pub const PROFILE: &str = "profile";
pub const ORDER: &str = "order";
/// Format 1's plugin load order.
pub const PLUGINS: &str = "plugins";
pub const MOD: &str = "mod/";
pub const LOCK: &str = "lock/";
pub const LOCK_ENV: &str = "lock-env";
pub const EXT: &str = "ext/";
/// Keys the sync layer keeps for itself.
pub const FILE: &str = "file/";
pub const PEER: &str = "peer/";
pub const MOVED: &str = "moved";
/// The value of a removed mod, locked mod or extension's state.
pub const REMOVED: &[u8] = b"null";

#[derive(Serialize, Deserialize)]
struct FormatEntry {
    format: u32,
}

#[derive(Serialize, Deserialize)]
struct ProfileEntry {
    id: Uuid,
    game: String,
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
}

/// A profile and its lockfile, as they were synced.
#[derive(Debug, Clone)]
pub struct Synced {
    pub profile: Profile,
    /// `None` if nothing's been locked.
    pub lock: Option<Lockfile>,
}

/// `profile` and its lockfile as entries.
pub fn to_entries(profile: &Profile, lock: Option<&Lockfile>) -> Entries {
    let mut entries = Entries::new();
    entries.insert(
        FORMAT.to_owned(),
        json(&FormatEntry {
            format: SYNC_FORMAT,
        }),
    );
    let meta = ProfileEntry {
        id: profile.id,
        game: profile.game.clone(),
        name: profile.name.clone(),
        hidden_tags: profile.hidden_tags.clone(),
        env: profile.env.clone(),
    };
    entries.insert(PROFILE.to_owned(), json(&meta));
    let order: Vec<String> = profile.mods.iter().map(key_of).collect();
    entries.insert(ORDER.to_owned(), json(&order));
    for (id, state) in &profile.ext {
        entries.insert(format!("{EXT}{id}"), json(state));
    }
    for m in &profile.mods {
        entries.insert(format!("{MOD}{}", key_of(m)), json(m));
    }
    if let Some(lock) = lock {
        entries.insert(LOCK_ENV.to_owned(), json(&lock.game.env));
    }
    for m in lock.into_iter().flat_map(|l| &l.mods) {
        entries.insert(format!("{LOCK}{}", m.key()), json(m));
    }
    entries
}

/// The profile and lockfile `entries` describe.
pub fn from_entries(entries: &Entries) -> Result<Synced> {
    check_format(entries)?;
    let meta: ProfileEntry = decode(entries, PROFILE)?
        .ok_or_else(|| Error::invalid("the synced profile hasn't arrived yet"))?;

    let mut mods: BTreeMap<String, ProfileMod> = BTreeMap::new();
    let mut locked: HashMap<String, LockedMod> = HashMap::new();
    let mut ext = BTreeMap::new();
    for key in entries.keys() {
        if let Some(id) = key.strip_prefix(EXT) {
            if let Some(state) = decode(entries, key)? {
                ext.insert(id.to_owned(), state);
            }
        } else if let Some(mod_key) = key.strip_prefix(MOD) {
            if let Some(m) = decode(entries, key)? {
                mods.insert(mod_key.to_owned(), m);
            }
        } else if let Some(mod_key) = key.strip_prefix(LOCK)
            && let Some(m) = decode(entries, key)?
        {
            locked.insert(mod_key.to_owned(), m);
        }
    }

    // in the synced order, then any it doesn't know about
    let order: Vec<String> = decode(entries, ORDER)?.unwrap_or_default();
    let mut ordered = Vec::with_capacity(mods.len());
    for key in order {
        if let Some(m) = mods.remove(&key) {
            ordered.push(m);
        }
    }
    ordered.extend(mods.into_values());
    // Format 1's, unless it's been replaced (or removed) since
    if !entries.contains_key(&format!("{EXT}{}", LEGACY_PLUGINS.0)) {
        upgrade_plugins(&mut ext, decode(entries, PLUGINS)?.unwrap_or_default());
    }

    let profile = Profile {
        schema: PROFILE_SCHEMA,
        id: meta.id,
        game: meta.game.clone(),
        name: meta.name,
        hidden_tags: meta.hidden_tags,
        env: meta.env,
        mods: dedupe_names(ordered),
        ext,
    };
    let env = decode(entries, LOCK_ENV)?.unwrap_or_default();
    let lock = (!locked.is_empty()).then(|| Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame { id: meta.game, env },
        mods: reachable(&profile, locked),
    });
    Ok(Synced { profile, lock })
}

/// What to write so that entries that were `base` become `now`: changed
/// values, and [`REMOVED`] for mods, locked mods and extensions' state that
/// are gone.
pub fn changes(base: &Entries, now: &Entries) -> Vec<(String, Vec<u8>)> {
    let mut changed: Vec<(String, Vec<u8>)> = now
        .iter()
        .filter(|(k, v)| base.get(*k) != Some(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    changed.extend(
        base.iter()
            .filter(|(k, v)| {
                (k.starts_with(MOD) || k.starts_with(LOCK) || k.starts_with(EXT) || *k == PLUGINS)
                    && v.as_slice() != REMOVED
                    && !now.contains_key(*k)
            })
            .map(|(k, _)| (k.clone(), REMOVED.to_vec())),
    );
    changed
}

/// The names of `profile`'s mods that `lock` doesn't have, which happens
/// when one machine removes a mod while another changes it, or when the
/// machine that added one couldn't lock it.
pub fn unlocked<'a>(profile: &'a Profile, lock: Option<&Lockfile>) -> Vec<&'a str> {
    let locked: HashSet<String> = lock
        .into_iter()
        .flat_map(|l| &l.mods)
        .map(|m| m.key())
        .collect();
    profile
        .mods
        .iter()
        .filter(|m| !locked.contains(&key_of(m)))
        .map(|m| m.name.as_str())
        .collect()
}

/// Fails if `entries` were written in a layout this build can't read.
pub fn check_format(entries: &Entries) -> Result<()> {
    let Some(found) = decode::<FormatEntry>(entries, FORMAT)? else {
        return Err(Error::invalid("the synced profile hasn't arrived yet"));
    };
    if found.format > SYNC_FORMAT {
        return Err(Error::invalid(format!(
            "the synced profile was written by a newer patchwork (sync format {}, \
             this build reads up to {SYNC_FORMAT}); update patchwork",
            found.format
        )));
    }
    Ok(())
}

/// The locked mods of `profile`'s mods and of their dependencies, mods
/// first in load order and then their dependencies a level at a time, like
/// a lock leaves them.
fn reachable(profile: &Profile, mut locked: HashMap<String, LockedMod>) -> Vec<LockedMod> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut level: Vec<(String, Option<&str>)> = profile
        .mods
        .iter()
        .map(|m| (key_of(m), Some(m.name.as_str())))
        .collect();
    while !level.is_empty() {
        let mut next = Vec::new();
        for (key, name) in level {
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(mut m) = locked.remove(&key) else {
                continue;
            };
            if let Some(name) = name {
                m.name = name.to_owned();
            }
            next.extend(m.dependencies.iter().map(|d| (d.clone(), None)));
            out.push(m);
        }
        level = next;
    }
    out
}

/// Mod names have to be unique within a profile, which two people adding
/// different mods under one name at once don't keep to.
pub(crate) fn dedupe_names(mods: Vec<ProfileMod>) -> Vec<ProfileMod> {
    let mut taken = HashSet::new();
    mods.into_iter()
        .map(|mut m| {
            let base = m.name.clone();
            let mut n = 2;
            while !taken.insert(m.name.clone()) {
                m.name = format!("{base}-{n}");
                n += 1;
            }
            m
        })
        .collect()
}

fn key_of(m: &ProfileMod) -> String {
    crate::lock::mod_key(&m.source, &m.id)
}

fn json(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("synced values always serialize")
}

/// The value of `key`, or `None` if there isn't one or it was removed.
fn decode<T: for<'de> Deserialize<'de>>(entries: &Entries, key: &str) -> Result<Option<T>> {
    match entries.get(key) {
        None => Ok(None),
        Some(v) if v.as_slice() == REMOVED => Ok(None),
        Some(v) => serde_json::from_slice(v)
            .map(Some)
            .map_err(|e| Error::invalid(format!("synced entry `{key}` can't be read: {e}"))),
    }
}

#[cfg(test)]
mod tests;
