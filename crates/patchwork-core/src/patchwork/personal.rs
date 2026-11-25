//! Synced profiles' personal layers (see [`crate::personal`]): where
//! they're kept, and moving mods into and out of them.

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use super::Patchwork;
use super::folders::move_to_folder;
use super::sets::{folder_name, last_own, linked};
use crate::config::{FileEdits, overlay};
use crate::error::IoContext;
use crate::fsutil;
use crate::lock::mod_key;
use crate::lock::{LOCK_SCHEMA, LockedGame, Lockfile};
use crate::personal::{Layer, PERSONAL_NAME, PERSONAL_SCHEMA, Personal};
use crate::profile::{Folder, Include, Profile, ProfileRef, no_such_mod};
use crate::set::Set;
use crate::sync::{self, Entries};
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
        if self.is_followed(profile)? {
            return Err(Error::invalid(format!(
                "you follow {}, so the mods you add to it are only on your computers \
                 already, and only whoever shares it can change the rest",
                loaded.label()
            )));
        }
        if personal && loaded.personal().is_none() {
            if !self.is_synced(profile) {
                return Err(Error::invalid(format!(
                    "{} isn't synced, so its mods are only on this computer already",
                    loaded.label()
                )));
            }
            loaded.includes.push(personal_include(&loaded));
        }
        let to = personal.then_some(loaded.id);
        let mut changed = false;
        for name in names {
            let m = loaded
                .find(name)
                .ok_or_else(|| no_such_mod(&loaded.label(), name))?;
            let in_layer = m.folder == Some(loaded.id) && loaded.personal().is_some();
            if loaded.is_linked(m) && !in_layer {
                let set = loaded
                    .folder_of(m)
                    .map_or("a shared set", |f| f.name.as_str());
                return Err(Error::invalid(format!(
                    "`{name}` is in {set}, which other profiles include; take it out of \
                     the set first"
                )));
            }
            if in_layer == personal {
                continue;
            }
            changed |= move_to_folder(&mut loaded, name, to)?;
        }
        if changed {
            self.save_profile(profile, &loaded)?;
        }
        Ok(())
    }

    /// The profile's personal layer with what only it needs locked, if it
    /// has one, to keep in step between the person's own computers.
    pub fn personal_layer(&self, profile: &ProfileRef) -> Result<Option<Layer>> {
        let Some(personal) = self.load_personal(profile)? else {
            return Ok(None);
        };
        let locked = self
            .load_personal_lock(profile)?
            .map(|l| l.mods)
            .unwrap_or_default();
        Ok(Some(Layer { personal, locked }))
    }

    /// Takes the profile's personal layer as it is on another of the
    /// person's computers (`None` if it has none there), in place of this
    /// one's, lockfile and all. Nothing is resolved or downloaded; see
    /// [`Patchwork::download_locked`]. Returns whether anything changed.
    pub fn apply_personal(&self, profile: &ProfileRef, layer: Option<&Layer>) -> Result<bool> {
        let ours = self.personal_layer(profile)?;
        if ours.as_ref().map(Layer::to_bytes) == layer.map(Layer::to_bytes) {
            return Ok(false);
        }
        let Some(layer) = layer else {
            self.remove_personal(profile)?;
            let lock = self.personal_lock_file(profile);
            if lock.exists() {
                fs::remove_file(&lock).at(&lock)?;
            }
            self.profile_changed(profile, true);
            return Ok(true);
        };
        if layer.personal.schema > PERSONAL_SCHEMA {
            return Err(Error::Schema {
                kind: "personal layer",
                path: self.personal_file(profile),
                found: layer.personal.schema,
                supported: PERSONAL_SCHEMA,
            });
        }
        let loaded = self.load_profile(profile)?;
        if layer.personal.game != loaded.game {
            return Err(Error::invalid(format!(
                "the personal mods for {} that came from another computer are for {}, not {}",
                loaded.label(),
                layer.personal.game,
                loaded.game
            )));
        }
        let game = match self.load_shared_lock(profile)? {
            Some(lock) => lock.game,
            None => LockedGame {
                id: loaded.game.clone(),
                env: Default::default(),
            },
        };
        self.save_personal(profile, &layer.personal)?;
        self.save_personal_lock(
            profile,
            &Lockfile {
                schema: LOCK_SCHEMA,
                game,
                mods: layer.locked.clone(),
            },
        )?;
        self.profile_changed(profile, true);
        Ok(true)
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
    /// mods and includes no sets.
    pub(super) fn save_personal(&self, profile: &ProfileRef, personal: &Personal) -> Result<()> {
        if personal.is_empty(profile.uuid) {
            return self.remove_personal(profile);
        }
        let mut personal = personal.clone();
        personal.schema = personal.saved_schema();
        fsutil::write_toml(&self.personal_file(profile), &personal)
    }

    pub(super) fn remove_personal(&self, profile: &ProfileRef) -> Result<()> {
        let path = self.personal_file(profile);
        if path.exists() {
            fs::remove_file(&path).at(&path)?;
        }
        Ok(())
    }

    /// The shared sets the profile includes personally, as stored.
    pub(super) fn personal_sets(&self, profile: &ProfileRef) -> Result<Vec<Uuid>> {
        Ok(self
            .load_personal(profile)?
            .map(|p| {
                p.includes
                    .into_iter()
                    .map(|i| i.set)
                    .filter(|set| *set != profile.uuid)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The personal layer of the profile whose id is `id`, as the private
    /// set it's loaded as, if `id` is a profile's and it has one.
    pub(super) fn load_personal_set(&self, id: Uuid) -> Result<Option<Set>> {
        let profile = ProfileRef { uuid: id };
        Ok(self.load_personal(&profile)?.map(|p| p.set(id)))
    }

    /// Saves `set`'s mods and settings as the personal layer of the
    /// profile whose id it has, if it's one of those; returns whether it
    /// was.
    pub(super) fn save_personal_set(&self, set: &Set) -> Result<bool> {
        let profile = ProfileRef { uuid: set.id };
        if !self.profile_file(&profile).is_file() {
            return Ok(false);
        }
        let mut personal = self
            .load_personal(&profile)?
            .unwrap_or_else(|| Personal::new(&set.game));
        personal.mods = set.mods.clone();
        personal.config = set.config.clone();
        self.save_personal(&profile, &personal)?;
        Ok(true)
    }

    /// Adds `config` to the settings the profile's personal layer changes,
    /// under those it has already, as when a set included personally is
    /// copied into it.
    pub(super) fn add_personal_config(
        &self,
        profile: &ProfileRef,
        game: &str,
        config: FileEdits,
    ) -> Result<()> {
        if config.is_empty() {
            return Ok(());
        }
        let mut personal = self
            .load_personal(profile)?
            .unwrap_or_else(|| Personal::new(game));
        let mut added = config;
        overlay(&mut added, &personal.config);
        personal.config = added;
        self.save_personal(profile, &personal)?;
        self.profile_changed(profile, false);
        Ok(())
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

/// An include of the profile's personal layer while it has no mods, to
/// move some into, after the profile's own mods.
pub(super) fn personal_include(loaded: &Profile) -> Include {
    Include {
        after: last_own(loaded),
        linked: Some(linked(&Personal::new(&loaded.game).set(loaded.id))),
        personal: true,
        ..Include::new(loaded.id)
    }
}

/// Moves the mods added to a followed profile into its personal layer
/// (see [`Patchwork::is_followed`]), `base` being what was last synced of
/// it: the profile's own mods that whoever shares it doesn't have. Returns
/// whether any were.
pub(super) fn keep_added(loaded: &mut Profile, base: &Entries) -> Result<bool> {
    let added: Vec<String> = loaded
        .mods
        .iter()
        .filter(|m| m.folder.is_none_or(|id| loaded.include(id).is_none()))
        .filter(|m| {
            let key = format!("{}{}", sync::MOD, mod_key(&m.source, &m.id));
            base.get(&key).is_none_or(|v| v.as_slice() == sync::REMOVED)
        })
        .map(|m| m.name.clone())
        .collect();
    if added.is_empty() {
        return Ok(false);
    }
    if loaded.personal().is_none() {
        let include = personal_include(loaded);
        loaded.includes.push(include);
    }
    for name in &added {
        move_to_folder(loaded, name, Some(loaded.id))?;
    }
    Ok(true)
}

/// Makes the personal layer of `profile` (whose id was `id`) an ordinary
/// folder of its own, for a copy of it that isn't synced. The sets it
/// included personally are the copy's own includes, and its optional
/// folders ordinary ones, with their mods on as they were for the person.
pub(super) fn fold_personal(profile: &mut Profile, id: Uuid) {
    for include in &mut profile.includes {
        include.personal = false;
    }
    // and optional folders are ordinary ones, as the person has them
    for folder in &mut profile.folders {
        folder.optional = false;
    }
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
