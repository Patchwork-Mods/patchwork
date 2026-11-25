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
use uuid::Uuid;

use super::Patchwork;
use super::local::{LOCAL, not_here};
use super::personal::keep_added;
use crate::error::{ArtifactChanged, IoContext};
use crate::event::Event;
use crate::fix::Blocker;
use crate::fsutil;
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, mod_key};
use crate::personal;
use crate::profile::{Profile, ProfileMod, ProfileRef};
use crate::set;
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
    /// This machine only follows the profile, so the mods added here go
    /// in its personal layer (see [`Patchwork::is_followed`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    followed: bool,
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

    /// The profile and its lockfile as they are here, as entries. Its
    /// personal layer is left out (see [`crate::personal`]), with the sets
    /// it includes personally and what only those need locked. The sets it
    /// includes for everyone go as ordinary folders (see [`published`]):
    /// whoever it's synced with doesn't have them.
    pub fn sync_entries(&self, profile: &ProfileRef) -> Result<Entries> {
        let loaded = self.load_profile(profile)?;
        let lock = self.load_lock(profile)?;
        let lock = match loaded.has_personal() {
            true => lock.map(|lock| personal::split_lock(&loaded, lock).0),
            false => lock,
        };
        Ok(sync::to_entries(&published(&loaded), lock.as_ref()))
    }

    /// Starts keeping track of what's synced of `profile`, from the profile
    /// as it is now, and returns all of it to publish.
    pub fn start_sync(&self, profile: &ProfileRef) -> Result<Entries> {
        let entries = self.sync_entries(profile)?;
        self.save_sync_base(profile, &entries, false)?;
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

    /// Whether this machine only follows `profile`, rather than sharing
    /// it. Mods added to a followed profile go in its personal layer (see
    /// [`crate::personal`]), since there's no publishing them, and that
    /// way they still go to the person's other computers.
    pub fn is_followed(&self, profile: &ProfileRef) -> Result<bool> {
        let base: Option<SyncBase> = fsutil::read_json(&self.sync_base_file(profile))?;
        Ok(base.is_some_and(|b| b.followed))
    }

    /// What was last synced of `profile`, if this machine only follows it.
    pub(super) fn followed_base(&self, profile: &ProfileRef) -> Result<Option<Entries>> {
        match self.is_followed(profile)? {
            true => self.sync_base(profile),
            false => Ok(None),
        }
    }

    /// Moves the mods added here to a followed profile (`base` being what
    /// was last synced of it) into its personal layer, and saves it if any
    /// were. Profiles followed before additions went there have them in
    /// the profile itself.
    fn keep_additions_personal(&self, profile: &ProfileRef, base: &Entries) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        if keep_added(&mut loaded, base)? {
            self.save_profile(profile, &loaded)?;
            self.split_lock_again(profile)?;
        }
        Ok(())
    }

    /// Splits the lockfile again after mods have moved into the personal
    /// layer, so what only they need stops being synced.
    pub(super) fn split_lock_again(&self, profile: &ProfileRef) -> Result<()> {
        if let Some(lock) = self.load_lock(profile)? {
            self.save_lock(profile, &lock)?;
        }
        Ok(())
    }

    /// Takes a profile as it's been synced, `entries` being the newest
    /// value of each key, saving it and its lockfile as they are. Nothing is
    /// resolved or downloaded; see [`Patchwork::download_locked`].
    ///
    /// Changes made here since the last sync are kept on top and returned.
    /// If `writable`, they're for publishing, and win over what arrived.
    /// A profile that's only followed can't be published to, so they stay
    /// here, and whatever the sharer changed since the last sync wins over
    /// them. Mods added to it go in its personal layer (see
    /// [`Patchwork::is_followed`]), including any added before that was so.
    pub fn apply_synced(&self, entries: &Entries, writable: bool) -> Result<Applied> {
        let applied = self.apply_entries(entries, writable, true)?;
        Ok(applied.expect("a missing profile is created"))
    }

    /// As [`Patchwork::apply_synced`], but only for a profile that's here:
    /// `None` if it isn't, as when it's been removed since it was last
    /// synced, so that the sync doesn't bring it back.
    pub fn update_synced(&self, entries: &Entries, writable: bool) -> Result<Option<Applied>> {
        self.apply_entries(entries, writable, false)
    }

    fn apply_entries(
        &self,
        entries: &Entries,
        writable: bool,
        create: bool,
    ) -> Result<Option<Applied>> {
        let incoming = sync::from_entries(entries)?;
        let profile = incoming.profile.reference();
        let _removing = self.removing.lock().unwrap();
        let exists = self.profile_file(&profile).is_file();
        if !exists && !create {
            return Ok(None);
        }
        if exists
            && !writable
            && let Some(base) = self.sync_base(&profile)?
        {
            self.keep_additions_personal(&profile, &base)?;
        }

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
        let mut left = Vec::new();
        if changed {
            let stored = match exists {
                true => {
                    let (stored, gone) = relink(&self.load_profile(&profile)?, synced.profile);
                    left = gone;
                    stored
                }
                false => synced.profile,
            };
            self.save_synced(&profile, &stored, &lock)?;
        }
        self.save_sync_base(&profile, entries, !writable)?;
        for (set, changed) in left {
            match changed {
                true => self.folder_changed(&profile, set)?,
                false => self.settle_set(set)?,
            }
        }
        Ok(Some(Applied {
            profile,
            created: !exists,
            changed,
            unpublished,
        }))
    }

    /// Saves a profile that came from syncing, as it's stored (see
    /// [`relink`]), and its lockfile.
    fn save_synced(&self, profile: &ProfileRef, stored: &Profile, lock: &Lockfile) -> Result<()> {
        fsutil::write_toml(&self.profile_file(profile), stored)?;
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

    fn save_sync_base(
        &self,
        profile: &ProfileRef,
        entries: &Entries,
        followed: bool,
    ) -> Result<()> {
        let base = SyncBase {
            followed,
            entries: entries
                .iter()
                .map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned()))
                .collect(),
        };
        fsutil::write_json(&self.sync_base_file(profile), &base)
    }

    pub(super) fn sync_base_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("synced.json")
    }
}

/// Whether the mod `key` (`<source>:<id>`) is one of `loaded`'s own, rather
/// than something they depend on.
fn is_in_profile(loaded: &Profile, key: &str) -> bool {
    loaded.mods.iter().any(|m| mod_key(&m.source, &m.id) == key)
}

/// What's synced of `loaded`: without its personal layer (see
/// [`personal::without`]), and with each set it includes as an ordinary
/// folder whose id is the set's, so what comes back maps onto it (see
/// [`Patchwork::apply_synced`]). The sets' config edits are the profile's,
/// where its own don't win.
pub(super) fn published(loaded: &Profile) -> Profile {
    let mut shared = personal::without(loaded);
    for include in std::mem::take(&mut shared.includes) {
        if let Some(linked) = include.linked {
            shared.folders.push(linked.folder);
        }
    }
    shared
}

/// The profile that came back from syncing (`flat`, as [`published`] gives
/// it) as it's stored, for a profile that's `here` as loaded: each set it
/// includes whose folder is still as the set has it is included again,
/// with whatever's off in it off in this profile. Also gives the sets it
/// stops including: those whose folder is gone (`false`), and those
/// someone else changed (`true`), whose folder stays as the profile's own
/// so it's as they have it, without changing the set (see
/// [`Patchwork::folder_changed`]).
///
/// Changes made here, to the profile or to a set from another profile, are
/// in `flat` already (see [`Patchwork::apply_synced`]), so only a change
/// that came from elsewhere makes a folder differ.
fn relink(here: &Profile, mut flat: Profile) -> (Profile, Vec<(Uuid, bool)>) {
    let mut left = Vec::new();
    for include in here.includes.iter().filter(|i| !i.personal) {
        let Some(linked) = &include.linked else {
            // a missing set's include stays until its issue's fix drops it
            flat.includes.push(include.clone());
            continue;
        };
        let Some(folder) = flat.folders.iter().find(|f| f.id == include.set) else {
            left.push((include.set, false));
            continue;
        };
        if folder.name != linked.folder.name
            || folder_mods(&flat, include.set) != folder_mods(here, include.set)
        {
            left.push((include.set, true));
            continue;
        }
        flat.folders.retain(|f| f.id != include.set);
        flat.includes.push(include.clone());
    }
    // nothing's changed in the sets, so there's nothing to save to them
    let (mut stored, _) = set::collapse(&flat);
    stored.schema = stored.saved_schema();
    (stored, left)
}

/// The mods in `profile`'s folder `id`, in order, without whether they're
/// on, which is up to each profile rather than the set.
fn folder_mods(profile: &Profile, id: Uuid) -> Vec<ProfileMod> {
    profile
        .mods
        .iter()
        .filter(|m| m.folder == Some(id))
        .map(|m| ProfileMod {
            enabled: true,
            ..m.clone()
        })
        .collect()
}
