//! What a profile looked like at each deploy, so it can go back to any of
//! them: [`Patchwork::revert`] to the last one, [`Patchwork::rollback`] to
//! older ones.
//!
//! Each deploy saves the profile's mods, plugins and lockfile to
//! `deployed.toml` and, unless they're the same as the newest entry, to
//! `history/<n>.toml`. Mods are content-addressed in the store, so going
//! back doesn't download anything that's still there.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::Patchwork;
use crate::error::IoContext;
use crate::fsutil;
use crate::hash::ContentHash;
use crate::lock::{Lockfile, mod_key};
use crate::plugins::PluginEntry;
use crate::profile::{ProfileMod, ProfileRef};
use crate::{Error, Result};

/// How many deploys each profile's history keeps. Older ones are dropped.
pub const HISTORY_LIMIT: usize = 50;

/// A profile's mods and lockfile at a deploy.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Deployed {
    /// Left out by versions from before there was a history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deployed_at: Option<Timestamp>,
    #[serde(default, rename = "mod")]
    mods: Vec<ProfileMod>,
    #[serde(default, rename = "plugin", skip_serializing_if = "Vec::is_empty")]
    plugins: Vec<PluginEntry>,
    lock: Lockfile,
}

impl Deployed {
    /// Whether the two would deploy the same thing; when they happened
    /// doesn't matter.
    fn same_as(&self, other: &Deployed) -> bool {
        self.mods == other.mods && self.plugins == other.plugins && toml_eq(&self.lock, &other.lock)
    }
}

/// One deploy in a profile's history, from [`Patchwork::history`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// What [`Patchwork::rollback`] takes. Numbers only ever go up, so they
    /// stay the same as newer deploys are added.
    pub number: u32,
    pub deployed_at: Option<Timestamp>,
    /// How many of the profile's own mods were enabled, not counting
    /// dependencies.
    pub enabled: usize,
    /// What changed since the entry before it, or nothing for the oldest
    /// one kept.
    pub changes: Vec<Change>,
    /// Whether the profile is exactly like this now.
    pub current: bool,
}

/// A difference between two deploys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Change {
    Added {
        name: String,
        version: Option<String>,
    },
    Removed {
        name: String,
        version: Option<String>,
    },
    Updated {
        name: String,
        from: Option<String>,
        to: Option<String>,
    },
    Enabled {
        name: String,
    },
    Disabled {
        name: String,
    },
    /// The mods' order changed.
    Reordered,
    /// The plugins' load order changed.
    PluginOrder,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = |v: &Option<String>| v.as_deref().unwrap_or("?").to_owned();
        match self {
            Change::Added { name, version } => write!(f, "+ {name} {}", v(version)),
            Change::Removed { name, version } => write!(f, "- {name} {}", v(version)),
            Change::Updated { name, from, to } => write!(f, "~ {name} {} -> {}", v(from), v(to)),
            Change::Enabled { name } => write!(f, "  enabled {name}"),
            Change::Disabled { name } => write!(f, "  disabled {name}"),
            Change::Reordered => write!(f, "  reordered mods"),
            Change::PluginOrder => write!(f, "  changed the plugin load order"),
        }
    }
}

impl Patchwork {
    /// Remembers the profile as it is now, as what was last deployed, and
    /// adds it to the history unless it's the same as the newest entry.
    pub(super) fn save_deployed(&self, profile: &ProfileRef) -> Result<()> {
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(());
        };
        let loaded = self.load_profile(profile)?;
        let deployed = Deployed {
            deployed_at: Some(Timestamp::now()),
            mods: loaded.mods,
            plugins: loaded.plugins,
            lock,
        };
        fsutil::write_toml(&self.deployed_file(profile), &deployed)?;

        let numbers = self.history_numbers(profile)?;
        let newest = match numbers.last() {
            Some(&n) => self.load_history_entry(profile, n)?,
            None => None,
        };
        if newest.is_some_and(|newest| newest.same_as(&deployed)) {
            return Ok(());
        }
        let next = numbers.last().map_or(1, |n| n + 1);
        fsutil::write_toml(&self.history_file(profile, next), &deployed)?;
        // Best effort: whatever's left is dropped next time.
        let excess = (numbers.len() + 1).saturating_sub(HISTORY_LIMIT);
        for &n in &numbers[..excess] {
            let _ = fs::remove_file(self.history_file(profile, n));
        }
        Ok(())
    }

    fn load_deployed(&self, profile: &ProfileRef) -> Result<Option<Deployed>> {
        fsutil::read_toml(&self.deployed_file(profile))
    }

    /// The profile as it is now, to compare against a deploy.
    fn current(&self, profile: &ProfileRef) -> Result<Option<Deployed>> {
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(None);
        };
        let loaded = self.load_profile(profile)?;
        Ok(Some(Deployed {
            deployed_at: None,
            mods: loaded.mods,
            plugins: loaded.plugins,
            lock,
        }))
    }

    /// Whether the profile's mods or their locked versions have changed
    /// since it was last deployed, so [`Patchwork::revert`] would do
    /// anything. Always false for a profile that's never been deployed.
    ///
    /// Hidden tags don't count, since they don't change what's deployed.
    pub fn has_changes(&self, profile: &ProfileRef) -> Result<bool> {
        let Some(deployed) = self.load_deployed(profile)? else {
            return Ok(false);
        };
        Ok(!self
            .current(profile)?
            .is_some_and(|now| now.same_as(&deployed)))
    }

    /// Puts the profile's mods and lockfile back how they were when it was
    /// last deployed, undoing everything done to it since. Nothing in the
    /// game changes; the profile just matches what's deployed again.
    pub fn revert(&self, profile: &ProfileRef) -> Result<()> {
        let deployed = self.load_deployed(profile)?.ok_or_else(|| {
            Error::invalid(format!(
                "{profile} hasn't been deployed, so there's nothing to revert to"
            ))
        })?;
        self.restore(profile, deployed)
    }

    /// The deploys kept in the profile's history, oldest first.
    pub fn history(&self, profile: &ProfileRef) -> Result<Vec<HistoryEntry>> {
        let now = self.current(profile)?;
        let mut entries = Vec::new();
        let mut before: Option<Deployed> = None;
        for n in self.history_numbers(profile)? {
            // A file that went missing in between is just skipped.
            let Some(deployed) = self.load_history_entry(profile, n)? else {
                continue;
            };
            entries.push(HistoryEntry {
                number: n,
                deployed_at: deployed.deployed_at,
                enabled: deployed.mods.iter().filter(|m| m.enabled).count(),
                changes: before
                    .as_ref()
                    .map(|b| changes(b, &deployed))
                    .unwrap_or_default(),
                current: now.as_ref().is_some_and(|now| now.same_as(&deployed)),
            });
            before = Some(deployed);
        }
        Ok(entries)
    }

    /// Puts the profile's mods and lockfile back how they were at deploy
    /// `number` of its [`Patchwork::history`]. Like [`Patchwork::revert`],
    /// nothing in the game changes until it's deployed.
    pub fn rollback(&self, profile: &ProfileRef, number: u32) -> Result<()> {
        let then = self.history_entry(profile, number)?;
        self.restore(profile, then)
    }

    /// What [`Patchwork::rollback`] to deploy `number` would change.
    pub fn rollback_changes(&self, profile: &ProfileRef, number: u32) -> Result<Vec<Change>> {
        let then = self.history_entry(profile, number)?;
        Ok(match self.current(profile)? {
            Some(now) => changes(&now, &then),
            None => Vec::new(),
        })
    }

    fn history_entry(&self, profile: &ProfileRef, number: u32) -> Result<Deployed> {
        match self.load_history_entry(profile, number)? {
            Some(deployed) => Ok(deployed),
            None => Err(Error::invalid(format!(
                "{} has no deploy {number} in its history; `patchwork history` lists them",
                self.load_profile(profile)?.label()
            ))),
        }
    }

    fn restore(&self, profile: &ProfileRef, deployed: Deployed) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        loaded.mods = deployed.mods;
        loaded.plugins = deployed.plugins;
        fsutil::write_toml(&self.lock_file(profile), &deployed.lock)?;
        self.save_profile(profile, &loaded)
    }

    fn history_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("history")
    }

    fn history_file(&self, profile: &ProfileRef, number: u32) -> PathBuf {
        self.history_dir(profile).join(format!("{number}.toml"))
    }

    fn load_history_entry(&self, profile: &ProfileRef, number: u32) -> Result<Option<Deployed>> {
        fsutil::read_toml(&self.history_file(profile, number))
    }

    /// The numbers of the history's entries, in order.
    fn history_numbers(&self, profile: &ProfileRef) -> Result<Vec<u32>> {
        let dir = self.history_dir(profile);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).at(&dir),
        };
        let mut numbers: Vec<u32> = entries
            .filter_map(|e| {
                let name = e.ok()?.file_name();
                name.to_str()?.strip_suffix(".toml")?.parse().ok()
            })
            .collect();
        numbers.sort_unstable();
        Ok(numbers)
    }
}

/// What changed going from `before` to `after`: mods (dependencies too)
/// added, removed or at another version, then the profile's own mods
/// enabled or disabled, then order.
fn changes(before: &Deployed, after: &Deployed) -> Vec<Change> {
    let top_level: HashSet<String> = before
        .mods
        .iter()
        .chain(&after.mods)
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    let old = installed(before, &top_level);
    let new = installed(after, &top_level);
    let mut changes = Vec::new();
    for m in &new {
        match old.iter().find(|o| o.key == m.key) {
            None => changes.push(Change::Added {
                name: m.name.clone(),
                version: m.version.clone(),
            }),
            Some(o) if o.version != m.version || o.hash != m.hash => {
                changes.push(Change::Updated {
                    name: m.name.clone(),
                    from: o.version.clone(),
                    to: m.version.clone(),
                })
            }
            Some(_) => {}
        }
    }
    for m in &old {
        if !new.iter().any(|n| n.key == m.key) {
            changes.push(Change::Removed {
                name: m.name.clone(),
                version: m.version.clone(),
            });
        }
    }

    let was: HashMap<(&str, &str), &ProfileMod> = before
        .mods
        .iter()
        .map(|m| ((m.source.as_str(), m.id.as_str()), m))
        .collect();
    for m in &after.mods {
        match was.get(&(m.source.as_str(), m.id.as_str())) {
            Some(w) if !w.enabled && m.enabled => changes.push(Change::Enabled {
                name: m.name.clone(),
            }),
            Some(w) if w.enabled && !m.enabled => changes.push(Change::Disabled {
                name: m.name.clone(),
            }),
            _ => {}
        }
    }

    // Only the mods in both count, so adding or removing one isn't also a
    // reorder.
    let order = |mods: &[ProfileMod], other: &[ProfileMod]| -> Vec<(String, String)> {
        mods.iter()
            .filter(|m| other.iter().any(|o| o.source == m.source && o.id == m.id))
            .map(|m| (m.source.clone(), m.id.clone()))
            .collect()
    };
    if order(&before.mods, &after.mods) != order(&after.mods, &before.mods) {
        changes.push(Change::Reordered);
    }
    if before.plugins != after.plugins {
        changes.push(Change::PluginOrder);
    }
    changes
}

/// A mod a deploy would install, from [`installed`].
struct Installed {
    key: String,
    name: String,
    version: Option<String>,
    hash: Option<ContentHash>,
}

/// The mods `deployed` installs, dependencies included. Its lockfile may be
/// behind its mods, as it is for the profile as it is now until it's locked
/// again: mods it doesn't have locked yet are in with the version they ask
/// for, and those `top_level` mods (the profile's own, at either end of a
/// comparison) that were taken out of it are left out.
fn installed(deployed: &Deployed, top_level: &HashSet<String>) -> Vec<Installed> {
    let own: HashSet<String> = deployed
        .mods
        .iter()
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    let mut mods: Vec<Installed> = deployed
        .lock
        .mods
        .iter()
        .filter(|m| own.contains(&m.key()) || !top_level.contains(&m.key()))
        .map(|m| Installed {
            key: m.key(),
            name: m.name.clone(),
            version: m.version.clone(),
            hash: Some(m.hash),
        })
        .collect();
    for m in &deployed.mods {
        let key = mod_key(&m.source, &m.id);
        if !mods.iter().any(|i| i.key == key) {
            mods.push(Installed {
                key,
                name: m.name.clone(),
                version: m.version.clone(),
                hash: None,
            });
        }
    }
    mods
}

fn toml_eq<T: Serialize>(a: &T, b: &T) -> bool {
    match (toml::to_string(a), toml::to_string(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
