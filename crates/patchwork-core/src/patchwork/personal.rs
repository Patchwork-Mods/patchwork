//! Synced profiles' personal layers (see [`crate::personal`]): where
//! they're kept, and moving mods into and out of them.

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use super::Patchwork;
use super::folders::move_to_folder;
use super::sets::{folder_name, last_own, linked};
use crate::error::IoContext;
use crate::fsutil;
use crate::lock::{LOCK_SCHEMA, Lockfile};
use crate::personal::{PERSONAL_NAME, PERSONAL_SCHEMA, Personal};
use crate::profile::{Folder, Include, Profile, ProfileRef, no_such_mod};
use crate::set::Set;
use crate::{Error, Result};

impl Patchwork {
    /// Moves the mods `names` into the profile's personal layer, so they
    /// stay here rather than going to everyone the profile's synced with, or with `personal` false back out of it, to
    /// go to everyone. Only synced profiles have one, since everything in
    /// the others is only here anyway.
    ///
    /// The mods aren't locked again; the next lock puts what only they
    /// need in the personal layer's lockfile.
    pub fn set_personal(
        &self,
        profile: &ProfileRef,
        names: &[String],
        personal: bool,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        if personal && loaded.personal().is_none() {
            if !self.is_synced(profile) {
                return Err(Error::invalid(format!(
                    "{} isn't synced, so its mods are only on this computer already",
                    loaded.label()
                )));
            }
            loaded.includes.push(Include {
                after: last_own(&loaded),
                linked: Some(linked(&Personal::new(&loaded.game).set(loaded.id))),
                ..Include::new(loaded.id)
            });
        }
        let to = personal.then_some(loaded.id);
        let mut changed = false;
        for name in names {
            let m = loaded
                .find(name)
                .ok_or_else(|| no_such_mod(&loaded.label(), name))?;
            if loaded.is_personal(m) == personal {
                continue;
            }
            if personal && loaded.is_linked(m) {
                let set = loaded
                    .folder_of(m)
                    .map_or("a shared set", |f| f.name.as_str());
                return Err(Error::invalid(format!(
                    "`{name}` is in {set}, which other profiles include; take it out of \
                     the set first"
                )));
            }
            changed |= move_to_folder(&mut loaded, name, to)?;
        }
        if changed {
            self.save_profile(profile, &loaded)?;
        }
        Ok(())
    }

    /// The profile's personal layer, if it has one.
    pub(super) fn load_personal(&self, profile: &ProfileRef) -> Result<Option<Personal>> {
        let path = self.personal_file(profile);
        let Some(personal) = fsutil::read_toml::<Personal>(&path)? else {
            return Ok(None);
        };
        if personal.schema > PERSONAL_SCHEMA {
            return Err(Error::Schema {
                kind: "personal layer",
                path,
                found: personal.schema,
                supported: PERSONAL_SCHEMA,
            });
        }
        Ok(Some(personal))
    }

    /// Saves the profile's personal layer, or removes it once it has no
    /// mods.
    pub(super) fn save_personal(&self, profile: &ProfileRef, personal: &Personal) -> Result<()> {
        if personal.mods.is_empty() {
            return self.remove_personal(profile);
        }
        let mut personal = personal.clone();
        personal.schema = PERSONAL_SCHEMA;
        fsutil::write_toml(&self.personal_file(profile), &personal)
    }

    pub(super) fn remove_personal(&self, profile: &ProfileRef) -> Result<()> {
        let path = self.personal_file(profile);
        if path.exists() {
            fs::remove_file(&path).at(&path)?;
        }
        Ok(())
    }

    /// The personal layer of the profile whose id is `id`, as the private
    /// set it's loaded as, if `id` is a profile's and it has one.
    pub(super) fn load_personal_set(&self, id: Uuid) -> Result<Option<Set>> {
        let profile = ProfileRef { uuid: id };
        Ok(self.load_personal(&profile)?.map(|p| p.set(id)))
    }

    /// Saves `set`'s mods as the personal layer of the profile whose id
    /// it has, if it's one of those; returns whether it was.
    pub(super) fn save_personal_set(&self, set: &Set) -> Result<bool> {
        let profile = ProfileRef { uuid: set.id };
        if !self.profile_file(&profile).is_file() {
            return Ok(false);
        }
        if !set.config.is_empty() {
            return Err(Error::invalid(
                "settings can't be saved in a profile's personal mods yet",
            ));
        }
        let mut personal = self
            .load_personal(&profile)?
            .unwrap_or_else(|| Personal::new(&set.game));
        personal.mods = set.mods.clone();
        self.save_personal(&profile, &personal)?;
        Ok(true)
    }

    /// What only the profile's personal layer needs locked, if anything.
    pub(super) fn load_personal_lock(&self, profile: &ProfileRef) -> Result<Option<Lockfile>> {
        let path = self.personal_lock_file(profile);
        let lock: Option<Lockfile> = fsutil::read_toml(&path)?;
        if let Some(lock) = &lock
            && lock.schema > LOCK_SCHEMA
        {
            return Err(Error::Schema {
                kind: "lockfile",
                path,
                found: lock.schema,
                supported: LOCK_SCHEMA,
            });
        }
        Ok(lock)
    }

    /// Saves what only the profile's personal layer needs locked, or
    /// removes its lockfile if that's nothing.
    pub(super) fn save_personal_lock(&self, profile: &ProfileRef, lock: &Lockfile) -> Result<()> {
        let path = self.personal_lock_file(profile);
        if !lock.mods.is_empty() {
            return fsutil::write_toml(&path, lock);
        }
        if path.exists() {
            fs::remove_file(&path).at(&path)?;
        }
        Ok(())
    }

    fn personal_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("personal.toml")
    }

    fn personal_lock_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("personal.lock")
    }
}

/// Makes the personal layer of `profile` (whose id was `id`) an ordinary
/// folder of its own, for a copy of it that isn't synced.
pub(super) fn fold_personal(profile: &mut Profile, id: Uuid) {
    let Some(at) = profile.includes.iter().position(|i| i.set == id) else {
        return;
    };
    let include = profile.includes.remove(at);
    if include.linked.is_none() {
        return;
    }
    let folder = Folder::new(&folder_name(profile, PERSONAL_NAME));
    for m in profile.mods.iter_mut().filter(|m| m.folder == Some(id)) {
        m.folder = Some(folder.id);
    }
    profile.folders.push(folder);
}
