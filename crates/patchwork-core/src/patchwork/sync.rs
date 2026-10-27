//! Patchwork's side of keeping profiles in step between machines. The
//! networking is someone else's (`patchwork-sync`); this turns profiles into
//! the entries of [`crate::sync`] and back, keeps track of what's been
//! synced, and says when a profile changes.
//!
//! What was last synced is kept for each profile, so that what's changed
//! here since (including edits made by hand) can be told apart from what's
//! changed elsewhere: only the former is published, and the latter is taken
//! as it is, lockfile included, without resolving anything again.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::Patchwork;
use super::local::{LOCAL, not_here};
use crate::error::IoContext;
use crate::event::Event;
use crate::fsutil;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile};
use crate::profile::{Profile, ProfileRef};
use crate::sync::{self, Entries};
use crate::{Error, Result};

/// A profile or its lockfile was saved, or the profile was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChange {
    pub profile: ProfileRef,
    /// It was [`Patchwork::apply_synced`] that saved it, so there's nothing
    /// new to send anywhere.
    pub synced: bool,
}

/// What [`Patchwork::apply_synced`] did.
#[derive(Debug)]
pub struct Applied {
    pub profile: ProfileRef,
    /// The profile wasn't on this machine before.
    pub created: bool,
    /// The profile or its lockfile had to change.
    pub changed: bool,
    /// Changes made here that the entries don't have, which were kept:
    /// still to publish, or, for a followed profile, to stay here.
    pub unpublished: Vec<(String, Vec<u8>)>,
}

/// What was last synced, as saved in the profile's state folder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncBase {
    /// Values are JSON, so they're kept as text.
    entries: BTreeMap<String, String>,
}

impl Patchwork {
    /// Every change to a profile or its lockfile from now on, and every
    /// profile removed, through this `Patchwork`. Edits to their files by hand aren't seen; watch the
    /// files for those.
    pub fn watch_profiles(&self) -> broadcast::Receiver<ProfileChange> {
        self.changed.subscribe()
    }

    pub(super) fn profile_changed(&self, profile: &ProfileRef, synced: bool) {
        // there's no one listening if this fails
        let _ = self.changed.send(ProfileChange {
            profile: profile.clone(),
            synced,
        });
    }

    /// The profile and its lockfile as they are here, as entries.
    pub fn sync_entries(&self, profile: &ProfileRef) -> Result<Entries> {
        let loaded = self.load_profile(profile)?;
        let lock = self.load_lock(profile)?;
        Ok(sync::to_entries(&loaded, lock.as_ref()))
    }

    /// Starts keeping track of what's synced of `profile`, from the profile
    /// as it is now, and returns all of it to publish.
    pub fn start_sync(&self, profile: &ProfileRef) -> Result<Entries> {
        let entries = self.sync_entries(profile)?;
        self.save_sync_base(profile, &entries)?;
        Ok(entries)
    }

    /// Stops keeping track of what's synced of `profile`.
    pub fn stop_sync(&self, profile: &ProfileRef) -> Result<()> {
        let path = self.sync_base_file(profile);
        if path.exists() {
            fs::remove_file(&path).at(&path)?;
        }
        Ok(())
    }

    /// What's changed here since `profile` was last synced, to publish.
    /// Nothing if it isn't being synced.
    pub fn unsynced_changes(&self, profile: &ProfileRef) -> Result<Vec<(String, Vec<u8>)>> {
        let Some(base) = self.sync_base(profile)? else {
            return Ok(Vec::new());
        };
        Ok(sync::changes(&base, &self.sync_entries(profile)?))
    }

    /// The names of the mods added here to a followed profile, which
    /// whoever shares it doesn't have. Empty if it isn't synced.
    pub fn unshared_mods(&self, profile: &ProfileRef) -> Result<Vec<String>> {
        let Some(base) = self.sync_base(profile)? else {
            return Ok(Vec::new());
        };
        let loaded = self.load_profile(profile)?;
        Ok(loaded
            .mods
            .into_iter()
            .filter(|m| {
                let key = format!("{}{}", sync::MOD, crate::lock::mod_key(&m.source, &m.id));
                base.get(&key).is_none_or(|v| v.as_slice() == sync::REMOVED)
            })
            .map(|m| m.name)
            .collect())
    }

    /// Takes a profile as it's been synced, `entries` being the newest
    /// value of each key, saving it and its lockfile as they are. Nothing is
    /// resolved or downloaded; see [`Patchwork::download_locked`].
    ///
    /// Changes made here since the last sync are kept on top and returned.
    /// If `writable`, they're for publishing, and win over what arrived.
    /// A profile that's only followed can't be published to, so they stay
    /// here (see [`Patchwork::unshared_mods`]), and whatever the sharer
    /// changed since the last sync wins over them.
    pub fn apply_synced(&self, entries: &Entries, writable: bool) -> Result<Applied> {
        let incoming = sync::from_entries(entries)?;
        let profile = incoming.profile.reference();
        let exists = self.profile_file(&profile).is_file();

        let mut unpublished = match self.sync_base(&profile)? {
            Some(base) if exists => {
                let mut local = sync::changes(&base, &self.sync_entries(&profile)?);
                if !writable {
                    local.retain(|(k, _)| entries.get(k) == base.get(k));
                }
                local
            }
            _ => Vec::new(),
        };
        // ones that have been published by now
        unpublished.retain(|(k, v)| entries.get(k) != Some(v));
        let synced = if unpublished.is_empty() {
            incoming
        } else {
            let mut merged = entries.clone();
            merged.extend(unpublished.iter().cloned());
            sync::from_entries(&merged)?
        };

        if !exists {
            // fails for a game that isn't defined here
            self.registry().game(&synced.profile.game)?;
        }
        self.check_new_name(&synced.profile)?;
        let lock = synced.lock.unwrap_or_else(|| Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: synced.profile.game.clone(),
            },
            mods: Vec::new(),
        });
        let changed = !exists
            || self.sync_entries(&profile)? != sync::to_entries(&synced.profile, Some(&lock));
        if changed {
            self.save_synced(&profile, &synced.profile, &lock)?;
        }
        self.save_sync_base(&profile, entries)?;
        Ok(Applied {
            profile,
            created: !exists,
            changed,
            unpublished,
        })
    }

    fn save_synced(&self, profile: &ProfileRef, loaded: &Profile, lock: &Lockfile) -> Result<()> {
        fsutil::write_toml(&self.profile_file(profile), loaded)?;
        fsutil::write_toml(&self.lock_file(profile), lock)?;
        self.profile_changed(profile, true);
        Ok(())
    }

    /// Local mods in `profile`'s lockfile that were added on another
    /// machine and whose files aren't in the store here, so have to be got
    /// from a machine that has them.
    pub fn missing_local_files(&self, profile: &ProfileRef) -> Result<Vec<LockedMod>> {
        let mut missing = Vec::new();
        for m in self.load_lock(profile)?.map(|l| l.mods).unwrap_or_default() {
            if m.source == LOCAL
                && !self.store.contains(&m.hash, m.format)
                && self.local_origin(&m.id)?.is_none()
            {
                missing.push(m);
            }
        }
        Ok(missing)
    }

    /// Where the store has `m`'s file (or folder), to hand to another
    /// machine, if it does.
    pub fn stored_file(&self, m: &LockedMod) -> Option<PathBuf> {
        self.store
            .contains(&m.hash, m.format)
            .then(|| self.store.path(&m.hash, m.format))
    }

    /// Puts `m`'s file (or folder), got from another machine, into the
    /// store. Fails if it isn't what was locked.
    pub fn add_synced_file(&self, m: &LockedMod, path: &std::path::Path) -> Result<()> {
        let hash = self.store.ingest_path(path, m.format)?;
        if hash != m.hash {
            return Err(Error::invalid(format!(
                "the file received for `{}` isn't the one that was locked (expected {}, got {})",
                m.name,
                m.hash.short(),
                hash.short()
            )));
        }
        Ok(())
    }

    /// Downloads whatever `profile`'s lockfile has that the store doesn't,
    /// exactly as locked, without resolving anything or changing the
    /// lockfile. Mods that can't be downloaded are warned about, except
    /// for files the user has to download themselves, which are all
    /// returned at once as [`Error::NeedsDownload`].
    pub async fn download_locked(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(());
        };
        let mut pending = Vec::new();
        for m in &lock.mods {
            if self.store.contains(&m.hash, m.format) {
                continue;
            }
            if self.local_elsewhere(&m.source, &m.id)? {
                self.reporter
                    .report(Event::Warning(not_here(&m.name).to_string()));
                continue;
            }
            self.reporter.report(Event::Fetching {
                name: m.name.clone(),
            });
            let fetched = self
                .fetch(
                    &loaded.game,
                    &m.source,
                    &m.id,
                    &m.name,
                    &m.artifact,
                    m.format,
                )
                .await;
            match fetched {
                Ok(hash) if hash == m.hash => {}
                Ok(hash) => self.reporter.report(Event::Warning(format!(
                    "`{}` isn't what was locked (expected {}, got {}); \
                     run `patchwork lock --update` to accept the new version",
                    m.name,
                    m.hash.short(),
                    hash.short()
                ))),
                Err(Error::NeedsDownload(waiting)) => pending.extend(waiting),
                Err(e) => self
                    .reporter
                    .report(Event::Warning(format!("`{}`: {e}", m.name))),
            }
        }
        if !pending.is_empty() {
            return Err(Error::NeedsDownload(pending));
        }
        Ok(())
    }

    fn sync_base(&self, profile: &ProfileRef) -> Result<Option<Entries>> {
        let base: Option<SyncBase> = fsutil::read_json(&self.sync_base_file(profile))?;
        Ok(base.map(|b| {
            b.entries
                .into_iter()
                .map(|(k, v)| (k, v.into_bytes()))
                .collect()
        }))
    }

    fn save_sync_base(&self, profile: &ProfileRef, entries: &Entries) -> Result<()> {
        let base = SyncBase {
            entries: entries
                .iter()
                .map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned()))
                .collect(),
        };
        fsutil::write_json(&self.sync_base_file(profile), &base)
    }

    fn sync_base_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("synced.json")
    }
}
