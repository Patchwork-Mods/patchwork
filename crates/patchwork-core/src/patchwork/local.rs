//! Local mods: files and folders added from this machine rather than
//! fetched from a source.
//!
//! A local mod's id is the hash its file had when it was added, which means
//! the same thing on every machine, where its path wouldn't. Where it was
//! added from is only true here, so that's kept apart from the profile, in
//! [`Paths::local_mods`](crate::paths::Paths::local_mods); relocking reads
//! the file from there again, so changes to it are picked up. Machines that
//! didn't add it get its file into their store some other way, e.g. with a
//! profile export.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Patchwork;
use crate::error::IoContext;
use crate::fsutil;
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, Lockfile, mod_key};
use crate::profile::{ProfileMod, ProfileRef};
use crate::source::{ArtifactFormat, ArtifactLocation, ResolvedMod};
use crate::{Error, Result};

/// The id of the source for local mods.
pub(super) const LOCAL: &str = "local";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Origins {
    /// Where each local mod was added from, by id.
    #[serde(default)]
    origins: BTreeMap<String, PathBuf>,
}

impl Patchwork {
    /// Where local mod `id` was added from on this machine, if it was.
    pub(super) fn local_origin(&self, id: &str) -> Result<Option<PathBuf>> {
        let origins: Origins = fsutil::read_toml(&self.paths.local_mods())?.unwrap_or_default();
        Ok(origins.origins.get(id).cloned())
    }

    fn remember_local_origins(&self, added: Vec<(String, PathBuf)>) -> Result<()> {
        if added.is_empty() {
            return Ok(());
        }
        let path = self.paths.local_mods();
        let mut origins: Origins = fsutil::read_toml(&path)?.unwrap_or_default();
        origins.origins.extend(added);
        fsutil::write_toml(&path, &origins)
    }

    /// Puts a newly added local mod's file in the store and remembers where
    /// it came from, returning its id.
    pub(super) fn add_local(&self, resolved: &ResolvedMod) -> Result<String> {
        let ArtifactLocation::Path { path } = &resolved.artifact else {
            return Ok(resolved.id.clone());
        };
        let id = self.store.ingest_path(path, resolved.format)?.to_string();
        self.remember_local_origins(vec![(id.clone(), path.clone())])?;
        Ok(id)
    }

    /// What to show people for a mod's id: its id, except for local mods,
    /// whose ids are hashes that mean nothing to them. Those show their
    /// file's name, as it was locked in `lock` or as it was added here.
    pub fn display_id(&self, lock: Option<&Lockfile>, m: &ProfileMod) -> String {
        if m.source != LOCAL || !is_local_id(&m.id) {
            return m.id.clone();
        }
        let key = mod_key(&m.source, &m.id);
        let locked = lock
            .into_iter()
            .flat_map(|l| &l.mods)
            .find(|l| l.key() == key)
            .and_then(|l| match &l.artifact {
                ArtifactLocation::Local { name } => Some(name.clone()),
                _ => None,
            });
        let added = || {
            let path = self.local_origin(&m.id).ok().flatten()?;
            Some(path.file_name()?.to_string_lossy().into_owned())
        };
        locked
            .or_else(added)
            .unwrap_or_else(|| m.id[..12].to_owned())
    }

    /// Whether `id` is a local mod added on another machine, so its file
    /// can't be read again here, only taken from the store as it was locked.
    pub(super) fn local_elsewhere(&self, source: &str, id: &str) -> Result<bool> {
        Ok(source == LOCAL && is_local_id(id) && self.local_origin(id)?.is_none())
    }

    /// Gives a profile's local mods ids that work on any machine, from
    /// before they had them, and moves out of its folder what only this
    /// machine needs to know about. See [`portable_ids`].
    pub(super) fn migrate_local_mods(&self, profile: &ProfileRef) -> Result<()> {
        let imported = self.profile_dir(profile).join("files");
        // Ones that can't be read are left for whatever uses them to report.
        let Ok(mut loaded) = self.load_profile(profile) else {
            return Ok(());
        };
        let Ok(mut lock) = self.load_lock(profile) else {
            return Ok(());
        };
        let mut added = Vec::new();
        let mut changed = false;

        if let Some(lock) = &mut lock {
            let moved = portable_ids(&mut loaded.mods, lock);
            changed |= !moved.is_empty();
            for (id, path) in moved {
                let hash: ContentHash = id.parse().expect("ids are hashes");
                let format = lock
                    .mods
                    .iter()
                    .find(|m| m.hash == hash)
                    .expect("came from the lockfile")
                    .format;
                if !self.store.contains(&hash, format) && path.exists() {
                    self.store.ingest_path(&path, format)?;
                }
                // Imported files are in the store now, and their folder
                // goes.
                if !path.starts_with(&imported) {
                    added.push((id, path));
                }
            }
        }
        // Ones that were never locked are read now, if they're still there.
        for m in &mut loaded.mods {
            if m.source != LOCAL || is_local_id(&m.id) {
                continue;
            }
            let path = PathBuf::from(&m.id);
            if !path.exists() || path.starts_with(&imported) {
                continue;
            }
            let format = ArtifactFormat::of_path(&path);
            m.id = self.store.ingest_path(&path, format)?.to_string();
            added.push((m.id.clone(), path));
            changed = true;
        }

        if changed {
            self.remember_local_origins(added)?;
            if let Some(lock) = &lock {
                self.save_lock(profile, lock)?;
            }
            self.save_profile(profile, &loaded)?;
        }
        if imported.exists() {
            fs::remove_dir_all(&imported).at(&imported)?;
        }
        Ok(())
    }
}

/// Whether `id` is a local mod's id rather than a path, which is what they
/// were before.
pub(super) fn is_local_id(id: &str) -> bool {
    id.parse::<ContentHash>().is_ok()
}

/// Gives local mods locked at a path, as they were up to lockfile schema 2,
/// their locked hash as their id, in both `mods` and `lock`, and records
/// them as [`ArtifactLocation::Local`]. Returns the paths they were at, by
/// new id.
pub(super) fn portable_ids(mods: &mut [ProfileMod], lock: &mut Lockfile) -> Vec<(String, PathBuf)> {
    let mut moved = Vec::new();
    for locked in &mut lock.mods {
        if locked.source != LOCAL {
            continue;
        }
        let ArtifactLocation::Path { path } = &locked.artifact else {
            continue;
        };
        let path = path.clone();
        let id = locked.hash.to_string();
        for m in mods.iter_mut() {
            if m.source == LOCAL && m.id == locked.id {
                m.id = id.clone();
            }
        }
        locked.artifact = ArtifactLocation::Local {
            name: locked.artifact.file_name(),
        };
        locked.id = id.clone();
        moved.push((id, path));
    }
    if !moved.is_empty() {
        lock.schema = LOCK_SCHEMA;
    }
    moved
}

pub(super) fn not_here(name: &str) -> Error {
    Error::invalid(format!(
        "`{name}` is a local mod from another machine, and its file isn't on this one"
    ))
}
