//! Shared sets kept in step between one person's own computers (see
//! `docs/design/folders.md`, step 6). The networking is `patchwork-sync`'s,
//! which hands over each set as it is on another of their computers; this
//! takes it, and links the profiles here whose folder is that set's.
//!
//! Synced profiles' documents have each set they include as an ordinary
//! folder whose id is the set's (see `sync::published`), so a computer that
//! has the set knows a folder with its id, as the set has it, is the set:
//! the profile includes it there too. That holds whichever arrives first,
//! the set or the profile, since each links what the other has here.

use std::fs;

use uuid::Uuid;

use super::Patchwork;
use super::sets::folder_name;
use super::sync::relink;
use crate::error::IoContext;
use crate::fsutil;
use crate::profile::{Folder, ProfileRef};
use crate::set::{SET_SCHEMA, Set};
use crate::sync;
use crate::{Error, Result};

impl Patchwork {
    /// Takes the shared set `id` as it is on another of the person's
    /// computers (`None` if it's gone there), in place of this one's.
    /// Profiles here with a folder that's the set's, as the set has it,
    /// include the set instead. Gone, the profiles that include it keep its
    /// mods and settings as their own, in a folder with the set's id, so
    /// what they're synced with stays as it is. Returns whether anything
    /// changed.
    pub fn apply_set(&self, id: Uuid, set: Option<&Set>) -> Result<bool> {
        let ours = self.shared_set(id)?;
        let Some(set) = set else {
            if ours.is_none() {
                return Ok(false);
            }
            self.drop_set(id)?;
            return Ok(true);
        };
        if set.schema > SET_SCHEMA {
            return Err(Error::Schema {
                kind: "set",
                path: self.set_file(id),
                found: set.schema,
                supported: SET_SCHEMA,
            });
        }
        if set.id != id {
            return Err(Error::invalid(format!(
                "the set {id} that came from another computer says it's {}",
                set.id
            )));
        }
        // as it'd be saved here
        let set = &Set {
            schema: set.saved_schema(),
            ..set.clone()
        };
        if ours.as_ref() == Some(set) {
            return Ok(false);
        }
        self.save_set(set)?;
        for (profile, loaded) in self.profiles()? {
            if loaded.include(id).is_some() {
                self.profile_changed(&profile, true);
            } else if loaded.game == set.game && loaded.folders.iter().any(|f| f.id == id) {
                self.link_sets(&profile)?;
            }
        }
        Ok(true)
    }

    /// Has the profile include each set here that one of its own folders
    /// is, as the set has it (see [`relink`]), as when the set's just
    /// arrived from another of the person's computers.
    fn link_sets(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        // as it'd come back from syncing, which is what `relink` takes
        let flat = sync::from_entries(&self.sync_entries(profile)?)?.profile;
        let (stored, _) = relink(
            &loaded,
            flat,
            |id| self.shared_set(id).ok().flatten(),
            |_| false,
        );
        let linked = stored
            .includes
            .iter()
            .any(|i| loaded.shared_includes().all(|had| had.set != i.set));
        if linked {
            fsutil::write_toml(&self.profile_file(profile), &stored)?;
            self.profile_changed(profile, true);
        }
        Ok(())
    }

    /// Removes the set `id`, gone on another of the person's computers. The
    /// profiles here that include it for everyone keep its mods (and
    /// settings) as their own, in a folder with its id, so their documents
    /// don't change. Those that include it personally keep the include,
    /// since their personal layer comes from the other computer too.
    fn drop_set(&self, id: Uuid) -> Result<()> {
        let mut including = Vec::new();
        for profile in self.profiles_including(id)? {
            let loaded = self.load_profile(&profile)?;
            if loaded.include(id).is_some_and(|i| !i.personal) {
                including.push((profile, loaded));
            }
        }
        let dir = self.paths.sets().join(id.to_string());
        if dir.exists() {
            fs::remove_dir_all(&dir).at(&dir)?;
        }
        for (profile, mut loaded) in including {
            let Some(at) = loaded.includes.iter().position(|i| i.set == id) else {
                continue;
            };
            let include = loaded.includes.remove(at);
            let name = include
                .linked
                .as_ref()
                .map_or_else(|| id.to_string(), |l| l.folder.name.clone());
            // its mods are in a folder with its id already
            loaded.folders.push(Folder {
                id,
                ..Folder::new(&folder_name(&loaded, &name))
            });
            self.save_profile(&profile, &loaded)?;
        }
        Ok(())
    }
}
