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
use crate::error::{ArtifactChanged, IoContext};
use crate::event::Event;
use crate::fix::{Blocker, Fix};
use crate::fsutil;
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, mod_key};
use crate::profile::{Profile, ProfileRef};
use crate::sync::{self, Entries};
use crate::{Error, Result};

/// A profile or its lockfile was saved, or the profile was removed, or its
/// game was set up here, so its mods can be downloaded.
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

/// What [`Patchwork::download_locked`] found had changed since it was
/// locked.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ChangedFiles {
    mods: Vec<ChangedFile>,
}

/// The mods [`Patchwork::download_locked`] couldn't download because
/// something was in the way, like a source's missing API key.
#[derive(Debug, Default, Serialize, Deserialize)]
struct BlockedDownloads {
    mods: Vec<BlockedDownload>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlockedDownload {
    /// The mod's key, `<source>:<id>`.
    key: String,
    blocker: Blocker,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChangedFile {
    /// The mod's key, `<source>:<id>`.
    key: String,
    expected: ContentHash,
    got: ContentHash,
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
        if !loaded.includes.is_empty() {
            return Err(sets_cant_sync(&loaded));
        }
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
                env: BTreeMap::new(),
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
    ///
    /// Mods whose files changed since they were locked are warned about
    /// too, and kept for [`Patchwork::issues`] to report until the new
    /// file's taken or the mod's locked again.
    ///
    /// Nothing's downloaded for a game that isn't set up here, since its
    /// mods couldn't be used; setting it up has it synced again (see
    /// [`Patchwork::set_game_install`]), which downloads them then.
    pub async fn download_locked(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        if self.game_install(&loaded.game)?.is_none() {
            // what an earlier download found no longer needs dealing with
            for file in [
                self.blocked_downloads_file(profile),
                self.changed_files_file(profile),
            ] {
                if file.exists() {
                    fs::remove_file(&file).at(&file)?;
                }
            }
            return Ok(());
        }
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(());
        };
        let mut pending = Vec::new();
        let mut changed = ChangedFiles::default();
        let mut blocked = BlockedDownloads::default();
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
                Ok(hash) => {
                    let in_profile = is_in_profile(&loaded, &m.key());
                    let one = ArtifactChanged::new(m, hash, in_profile);
                    self.reporter.report(Event::Warning(one.to_string()));
                    changed.mods.push(ChangedFile {
                        key: m.key(),
                        expected: m.hash,
                        got: hash,
                    });
                }
                Err(Error::NeedsDownload(waiting)) => pending.extend(waiting),
                Err(e) => match e.blocker() {
                    // an issue (see `blocked_downloads`) says what to do
                    Some(blocker) => blocked.mods.push(BlockedDownload {
                        key: m.key(),
                        blocker,
                    }),
                    None => self
                        .reporter
                        .report(Event::Warning(format!("`{}`: {e}", m.name))),
                },
            }
        }
        let file = self.blocked_downloads_file(profile);
        if blocked.mods.is_empty() {
            if file.exists() {
                fs::remove_file(&file).at(&file)?;
            }
        } else {
            fsutil::write_json(&file, &blocked)?;
        }
        // Only mods that aren't in the store were fetched, so this is all of
        // them that are still changed.
        let file = self.changed_files_file(profile);
        if changed.mods.is_empty() {
            if file.exists() {
                fs::remove_file(&file).at(&file)?;
            }
        } else {
            fsutil::write_json(&file, &changed)?;
        }
        if !pending.is_empty() {
            return Err(Error::NeedsDownload(pending));
        }
        Ok(())
    }

    /// Where [`Patchwork::download_locked`] keeps the mods it found had
    /// changed.
    fn changed_files_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("changed-files.json")
    }

    /// The mods [`Patchwork::download_locked`] last found had changed since
    /// they were locked, and still are: locked to the file they were, which
    /// isn't here.
    pub(super) fn changed_files(
        &self,
        profile: &ProfileRef,
        loaded: &Profile,
        lock: &Lockfile,
    ) -> Result<Vec<ArtifactChanged>> {
        let found: ChangedFiles =
            fsutil::read_json(&self.changed_files_file(profile))?.unwrap_or_default();
        Ok(found
            .mods
            .into_iter()
            .filter_map(|c| {
                let m = lock.mods.iter().find(|m| m.key() == c.key)?;
                (m.hash == c.expected && !self.store.contains(&m.hash, m.format))
                    .then(|| ArtifactChanged::new(m, c.got, is_in_profile(loaded, &c.key)))
            })
            .collect())
    }

    /// Where [`Patchwork::download_locked`] keeps the mods it couldn't
    /// download because something was in the way.
    fn blocked_downloads_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile)
            .join("blocked-downloads.json")
    }

    /// The mods [`Patchwork::download_locked`] last couldn't download
    /// because something was in the way, and still aren't here, by what was
    /// in the way: mods blocked by the same thing (all needing a Nexus API
    /// key, say) together, by name.
    pub(super) fn blocked_downloads(
        &self,
        profile: &ProfileRef,
        lock: &Lockfile,
    ) -> Result<Vec<(Blocker, Vec<String>)>> {
        let found: BlockedDownloads =
            fsutil::read_json(&self.blocked_downloads_file(profile))?.unwrap_or_default();
        let mut by_blocker: Vec<(Blocker, Vec<String>)> = Vec::new();
        for b in found.mods {
            let Some(m) = lock.mods.iter().find(|m| m.key() == b.key) else {
                continue;
            };
            if self.store.contains(&m.hash, m.format) {
                continue;
            }
            match by_blocker
                .iter_mut()
                .find(|(blocker, _)| *blocker == b.blocker)
            {
                Some((_, names)) => names.push(m.name.clone()),
                None => by_blocker.push((b.blocker, vec![m.name.clone()])),
            }
        }
        Ok(by_blocker)
    }

    /// Whether `profile` is synced (shared or followed).
    pub(super) fn is_synced(&self, profile: &ProfileRef) -> bool {
        self.sync_base_file(profile).is_file()
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

/// Whether the mod `key` (`<source>:<id>`) is one of `loaded`'s own, rather
/// than something they depend on.
fn is_in_profile(loaded: &Profile, key: &str) -> bool {
    loaded.mods.iter().any(|m| mod_key(&m.source, &m.id) == key)
}

/// Synced profiles can't include shared sets yet: their mods would go to
/// everyone as the profile's own.
fn sets_cant_sync(loaded: &Profile) -> Error {
    let (sets, names): (Vec<String>, Vec<String>) = loaded
        .includes
        .iter()
        .map(|i| {
            let name = i
                .linked
                .as_ref()
                .map_or_else(|| i.set.to_string(), |l| l.folder.name.clone());
            (i.set.to_string(), name)
        })
        .unzip();
    Error::blocked(
        format!(
            "{} includes shared sets ({}), which can't be synced yet",
            loaded.label(),
            names.join(", ")
        ),
        "Shared sets can't be synced yet",
        format!(
            "{} includes {}, and syncing shared sets isn't supported yet. \
             Copying them in makes each linked folder an ordinary folder of this \
             profile's own, which syncs like the rest. The sets stay as they are \
             for other profiles.",
            loaded.label(),
            names.join(", ")
        ),
        Fix::UnlinkSets { sets, names },
    )
}
