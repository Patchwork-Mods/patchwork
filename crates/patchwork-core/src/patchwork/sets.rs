//! Shared sets (see [`crate::set`]): where they're kept, and which profiles
//! include them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use uuid::Uuid;

use super::Patchwork;
use crate::error::IoContext;
use crate::fix::Fix;
use crate::fsutil;
use crate::lock::mod_key;
use crate::profile::{Folder, Include, Linked, Profile, ProfileRef};
use crate::set::{SET_SCHEMA, Set, unused_name};
use crate::{Error, Result};

impl Patchwork {
    /// Makes an empty set called `name` for `game`.
    pub fn create_set(&self, game: &str, name: &str) -> Result<Set> {
        self.registry().game(game)?;
        let name = self.set_name(game, name, None)?;
        let set = Set::new(game, &name);
        self.save_set(&set)?;
        Ok(set)
    }

    /// The set of `game` that `query` names, by name (ignoring case) or by
    /// id.
    pub fn find_set(&self, game: &str, query: &str) -> Result<Set> {
        self.sets(game)?
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(query) || s.id.to_string() == query)
            .ok_or_else(|| Error::invalid(format!("{game} has no set named `{query}`")))
    }

    /// Renames a set, for every profile that includes it.
    pub fn rename_set(&self, id: Uuid, name: &str) -> Result<()> {
        let mut set = self.load_set(id)?.ok_or_else(|| no_such_set(id))?;
        set.name = self.set_name(&set.game, name, Some(id))?;
        self.save_set(&set)?;
        for profile in self.profiles_including(id)? {
            self.profile_changed(&profile, false);
        }
        Ok(())
    }

    /// Removes a set no profile includes.
    pub fn remove_set(&self, id: Uuid) -> Result<()> {
        let set = self.load_set(id)?.ok_or_else(|| no_such_set(id))?;
        if let Some(including) = self.includers()?.remove(&id) {
            let labels: Vec<&str> = including.iter().map(|(_, l)| l.as_str()).collect();
            return Err(Error::invalid(format!(
                "{} is still included by {}; unlink it from them first",
                set.name,
                labels.join(", ")
            )));
        }
        let dir = self.paths.sets().join(id.to_string());
        fs::remove_dir_all(&dir).at(&dir)
    }

    /// Turns one of the profile's folders into a set, which the profile
    /// then includes in its place, so other profiles can include it too.
    /// Mods that are off stay off in this profile.
    pub fn share_folder(&self, profile: &ProfileRef, folder: &str) -> Result<Set> {
        let mut loaded = self.load_profile(profile)?;
        let found = loaded.find_folder(folder)?.clone();
        if loaded.include(found.id).is_some() {
            return Err(Error::invalid(format!("{} is already a set", found.name)));
        }
        if self.is_synced(profile) {
            return Err(Error::invalid(format!(
                "{} is synced, and synced profiles can't share folders as sets yet",
                loaded.label()
            )));
        }
        let mut set = Set::new(
            &loaded.game,
            &self.set_name(&loaded.game, &found.name, None)?,
        );
        for m in loaded
            .mods
            .iter_mut()
            .filter(|m| m.folder == Some(found.id))
        {
            m.folder = Some(set.id);
            let mut copy = m.clone();
            copy.enabled = true;
            copy.folder = None;
            set.mods.push(copy);
        }
        loaded.folders.retain(|f| f.id != found.id);
        loaded.includes.push(Include {
            // where an empty folder goes; otherwise where its mods are
            after: last_own(&loaded),
            linked: Some(linked(&set)),
            ..Include::new(set.id)
        });
        self.save_set(&set)?;
        self.save_profile(profile, &loaded)?;
        Ok(set)
    }

    /// Has the profile include the set `id`, after its own mod `after`
    /// (`None` for after all of them).
    pub fn include_set(&self, profile: &ProfileRef, id: Uuid, after: Option<&str>) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let set = self.load_set(id)?.ok_or_else(|| no_such_set(id))?;
        if set.game != loaded.game {
            return Err(Error::invalid(format!(
                "{} is a set for {}, not {}",
                set.name, set.game, loaded.game
            )));
        }
        if loaded.include(id).is_some() {
            return Err(Error::invalid(format!(
                "{} already includes {}",
                loaded.label(),
                set.name
            )));
        }
        if self.is_synced(profile) {
            return Err(Error::blocked(
                format!(
                    "{} is synced, and synced profiles can't include shared sets yet",
                    loaded.label()
                ),
                "Synced profiles can't include sets yet",
                format!(
                    "{} is synced with other computers or people, and including shared \
                     sets in synced profiles isn't supported yet. {} can be copied in as an \
                     ordinary folder of the profile's own instead, which syncs like the \
                     rest; changing it won't change the set.",
                    loaded.label(),
                    set.name
                ),
                Fix::CopySet {
                    set: id.to_string(),
                    name: set.name.clone(),
                },
            ));
        }
        if let Some(name) = after
            && loaded.find(name).is_none_or(|m| loaded.is_linked(m))
        {
            return Err(Error::invalid(format!(
                "{} has no mod of its own named `{name}`",
                loaded.label()
            )));
        }
        let after = match after {
            Some(name) => Some(name.to_owned()),
            None => last_own(&loaded),
        };
        // saved as it is, then loaded again to bring its mods in
        loaded.includes.push(Include {
            after,
            ..Include::new(id)
        });
        self.save_profile(profile, &loaded)
    }

    /// Copies the set `id` into the profile as an ordinary folder of its
    /// own, after its own mods, rather than including it.
    pub fn copy_set(&self, profile: &ProfileRef, id: Uuid) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let set = self.load_set(id)?.ok_or_else(|| no_such_set(id))?;
        if set.game != loaded.game {
            return Err(Error::invalid(format!(
                "{} is a set for {}, not {}",
                set.name, set.game, loaded.game
            )));
        }
        let folder = Folder::new(&folder_name(&loaded, &set.name));
        let mut taken: HashSet<String> = loaded.mods.iter().map(|m| m.name.clone()).collect();
        let keys: HashSet<String> = loaded
            .mods
            .iter()
            .map(|m| mod_key(&m.source, &m.id))
            .collect();
        for mut m in set.mods {
            if keys.contains(&mod_key(&m.source, &m.id)) {
                continue;
            }
            m.folder = Some(folder.id);
            if taken.contains(&m.name) {
                m.name = unused_name(&format!("{} ({})", m.name, set.name), &taken);
            }
            taken.insert(m.name.clone());
            loaded.mods.push(m);
        }
        loaded.folders.push(folder);
        self.save_profile(profile, &loaded)
    }

    /// Stops the profile including the set `id`. With `keep`, its mods stay
    /// in the profile as its own, in an ordinary folder named like the
    /// set; otherwise they go.
    pub fn unlink_set(&self, profile: &ProfileRef, id: Uuid, keep: bool) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let Some(at) = loaded.includes.iter().position(|i| i.set == id) else {
            return Err(Error::invalid(format!(
                "{} doesn't include the set {id}",
                loaded.label()
            )));
        };
        let include = loaded.includes.remove(at);
        if keep && let Some(linked) = &include.linked {
            let folder = Folder::new(&folder_name(&loaded, &linked.folder.name));
            for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(id)) {
                m.folder = Some(folder.id);
            }
            loaded.folders.push(folder);
        } else {
            loaded.mods.retain(|m| m.folder != Some(id));
        }
        self.save_profile(profile, &loaded)
    }

    /// `name` trimmed, if it isn't blank and no set of `game` but `except`
    /// has it (ignoring case).
    fn set_name(&self, game: &str, name: &str, except: Option<Uuid>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("a set needs a name"));
        }
        let taken = self
            .sets(game)?
            .iter()
            .any(|s| Some(s.id) != except && s.name.eq_ignore_ascii_case(name));
        if taken {
            return Err(Error::invalid(format!(
                "{game} already has a set named `{name}`"
            )));
        }
        Ok(name.to_owned())
    }

    /// The set with id `id`, if there is one.
    pub fn load_set(&self, id: Uuid) -> Result<Option<Set>> {
        let path = self.set_file(id);
        let Some(set) = fsutil::read_toml::<Set>(&path)? else {
            return Ok(None);
        };
        if set.schema > SET_SCHEMA {
            return Err(Error::Schema {
                kind: "set",
                path,
                found: set.schema,
                supported: SET_SCHEMA,
            });
        }
        Ok(Some(set))
    }

    pub(super) fn save_set(&self, set: &Set) -> Result<()> {
        let mut set = set.clone();
        set.schema = SET_SCHEMA;
        fsutil::write_toml(&self.set_file(set.id), &set)
    }

    /// The sets for `game`, by name.
    pub fn sets(&self, game: &str) -> Result<Vec<Set>> {
        let mut sets = Vec::new();
        let Ok(entries) = fs::read_dir(self.paths.sets()) else {
            return Ok(sets);
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(id) = entry.file_name().to_string_lossy().parse::<Uuid>() else {
                continue;
            };
            if let Some(set) = self.load_set(id)?
                && set.game == game
            {
                sets.push(set);
            }
        }
        sets.sort_by_key(|s| s.name.to_lowercase());
        Ok(sets)
    }

    /// The profiles that include the set `id`, read as they're stored
    /// (without loading their sets).
    pub fn profiles_including(&self, id: Uuid) -> Result<Vec<ProfileRef>> {
        Ok(self
            .includers()?
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .map(|(profile, _)| profile)
            .collect())
    }

    /// Every set some profile includes, with those profiles and their
    /// labels, read as they're stored (without loading their sets).
    pub fn includers(&self) -> Result<HashMap<Uuid, Vec<(ProfileRef, String)>>> {
        /// Just what's needed of a stored profile.
        #[derive(Deserialize)]
        struct Includes {
            game: String,
            name: String,
            #[serde(default, rename = "include")]
            includes: Vec<Include>,
        }
        let mut found: HashMap<Uuid, Vec<(ProfileRef, String)>> = HashMap::new();
        let Ok(entries) = fs::read_dir(self.paths.profiles()) else {
            return Ok(found);
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(profile) = entry.file_name().to_string_lossy().parse::<ProfileRef>() else {
                continue;
            };
            let Some(stored) = fsutil::read_toml::<Includes>(&self.profile_file(&profile))? else {
                continue;
            };
            let label = format!("{}/{}", stored.game, stored.name);
            for include in stored.includes {
                found
                    .entry(include.set)
                    .or_default()
                    .push((profile.clone(), label.clone()));
            }
        }
        for profiles in found.values_mut() {
            profiles.sort();
        }
        Ok(found)
    }

    fn set_file(&self, id: Uuid) -> PathBuf {
        self.paths.sets().join(id.to_string()).join("set.toml")
    }
}

fn no_such_set(id: Uuid) -> Error {
    Error::invalid(format!("there's no set with id {id}"))
}

/// What an include of `set` remembers of it when it's just been loaded.
fn linked(set: &Set) -> Linked {
    Linked {
        folder: Folder {
            id: set.id,
            name: set.name.clone(),
        },
        mods: set.mods.clone(),
        shadowed: Vec::new(),
        renamed: Vec::new(),
    }
}

/// The last of the profile's own mods (not a set's).
fn last_own(profile: &Profile) -> Option<String> {
    profile
        .mods
        .iter()
        .rev()
        .find(|m| !profile.is_linked(m))
        .map(|m| m.name.clone())
}

/// `name`, or a numbered one if a folder of `profile` already has it.
fn folder_name(profile: &Profile, name: &str) -> String {
    (1..)
        .map(|n| match n {
            1 => name.to_owned(),
            n => format!("{name} {n}"),
        })
        .find(|n| {
            !profile
                .all_folders()
                .any(|f| f.name.eq_ignore_ascii_case(n))
        })
        .expect("there's always one")
}
