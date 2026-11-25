//! Shared sets (see [`crate::set`]): where they're kept, and which profiles
//! include them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use uuid::Uuid;

use super::Patchwork;
use super::changed_folders::optional_folder;
use super::personal::personal_include;
use crate::ConfigFileInfo;
use crate::config::{FileEdits, overlay, same_dest};
use crate::error::IoContext;
use crate::fsutil;
use crate::lock::mod_key;
use crate::profile::{Folder, Include, Linked, Profile, ProfileMod, ProfileRef};
use crate::set::{
    FolderSource, OtherFolder, SET_SCHEMA, Set, merge_config, own_config, unused_name,
};
use crate::{Error, Result};

impl Patchwork {
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

    /// Removes a set no profile includes. Sets go by themselves once none
    /// does (see [`Patchwork::settle_set`]); this is for ones left over
    /// from before they did.
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
    /// Mods that are off stay off in this profile. The settings the profile
    /// changes in its mods' config files go with them, if the game's set up
    /// to find which those are. It's how [`Patchwork::use_folder`] starts
    /// using one of a profile's own folders in another.
    pub fn share_folder(&self, profile: &ProfileRef, folder: &str) -> Result<Set> {
        // First, since it can read settings changed in game back in.
        let files = self.config_files(profile).unwrap_or_default();
        let mut loaded = self.load_profile(profile)?;
        let found = loaded.find_folder(folder)?.clone();
        if loaded.include(found.id).is_some() {
            return Err(Error::invalid(format!("{} is already a set", found.name)));
        }
        if found.optional {
            return Err(optional_folder(&loaded, &found));
        }
        let name = self.set_name(&loaded.game, &found.name, None)?;
        let (mut set, own) = folder_as_set(&loaded, &found, &files);
        set.name = name;
        // The same id, so a synced profile's document has the same folder
        // as before (see `sync::published`), unless a set has it already
        // (the one a folder someone else changed came from, say).
        if self.load_set(found.id)?.is_none() {
            set.id = found.id;
        }
        for m in loaded
            .mods
            .iter_mut()
            .filter(|m| m.folder == Some(found.id))
        {
            m.folder = Some(set.id);
        }
        loaded.folders.retain(|f| f.id != found.id);
        loaded.includes.push(Include {
            // where an empty folder goes; otherwise where its mods are
            after: last_own(&loaded),
            linked: Some(linked(&set)),
            ..Include::new(set.id)
        });
        merge_config(&mut loaded, &own);
        self.save_set(&set)?;
        self.save_profile(profile, &loaded)?;
        Ok(set)
    }

    /// The folders of the game's other profiles that `profile` could use
    /// too (see [`Patchwork::use_folder`]), by name: their own folders, and
    /// the sets they include that it doesn't, each once. Sets no profile
    /// includes, left over from before they went once none did, are there
    /// too.
    pub fn other_folders(&self, profile: &ProfileRef) -> Result<Vec<OtherFolder>> {
        let loaded = self.load_profile(profile)?;
        let followed = self.is_followed(profile)?;
        let keys: HashSet<String> = loaded
            .mods
            .iter()
            .filter(|m| !loaded.is_linked(m))
            .map(|m| mod_key(&m.source, &m.id))
            .collect();
        let already = |mods: &[ProfileMod]| -> Vec<String> {
            mods.iter()
                .filter(|m| keys.contains(&mod_key(&m.source, &m.id)))
                .map(|m| m.name.clone())
                .collect()
        };
        let mut found = Vec::new();
        let mut sets: HashMap<Uuid, usize> = HashMap::new();
        for (other_ref, other) in self.profiles()? {
            if other.game != loaded.game || other_ref == *profile {
                continue;
            }
            for folder in &other.folders {
                let mods: Vec<ProfileMod> = other
                    .mods
                    .iter()
                    .filter(|m| m.folder == Some(folder.id))
                    .cloned()
                    .collect();
                found.push(OtherFolder {
                    source: FolderSource::Folder {
                        profile: other_ref.uuid,
                        folder: folder.id,
                    },
                    name: folder.name.clone(),
                    mods: mods.iter().map(|m| m.name.clone()).collect(),
                    profiles: vec![other.name.clone()],
                    already: already(&mods),
                    personal: followed,
                });
            }
            for include in other.shared_includes() {
                let Some(set) = &include.linked else { continue };
                if loaded.include(include.set).is_some() {
                    continue;
                }
                match sets.get(&include.set) {
                    Some(&at) => found[at].profiles.push(other.name.clone()),
                    None => {
                        sets.insert(include.set, found.len());
                        found.push(set_folder(&set.folder, &set.mods, already, followed));
                        found.last_mut().expect("just pushed").profiles = vec![other.name.clone()];
                    }
                }
            }
        }
        for set in self.sets(&loaded.game)? {
            if !sets.contains_key(&set.id) && loaded.include(set.id).is_none() {
                let folder = Folder {
                    id: set.id,
                    ..Folder::new(&set.name)
                };
                found.push(set_folder(&folder, &set.mods, already, followed));
            }
        }
        for f in &mut found {
            f.profiles.sort_by_key(|p| p.to_lowercase());
        }
        found.sort_by(|a, b| {
            (a.name.to_lowercase(), &a.profiles).cmp(&(b.name.to_lowercase(), &b.profiles))
        });
        Ok(found)
    }

    /// Has the profile use a folder another profile of its game has (see
    /// [`Patchwork::other_folders`]), after its own mods. With `link`, it's
    /// the same folder in both: changing what's in it changes it in every
    /// profile that has it, which makes one of a profile's own folders a
    /// set. Otherwise it's copied in as an ordinary folder of the profile's
    /// own. Its config edits come with it either way. With `personal`, a
    /// synced profile includes it just for this person (see
    /// [`Patchwork::include_set`]).
    pub fn use_folder(
        &self,
        profile: &ProfileRef,
        source: &FolderSource,
        link: bool,
        personal: bool,
    ) -> Result<()> {
        match *source {
            FolderSource::Set { set } if link => {
                self.include_set(profile, set, None, personal)?;
                // a set left over from before is this profile's alone now
                self.settle_set(set)
            }
            FolderSource::Set { set } => self.copy_set(profile, set),
            FolderSource::Folder {
                profile: from,
                folder,
            } => {
                let from = ProfileRef { uuid: from };
                if from == *profile {
                    return Err(Error::invalid("that's this profile's own folder"));
                }
                let source = self.load_profile(&from)?;
                let loaded = self.load_profile(profile)?;
                if source.game != loaded.game {
                    return Err(Error::invalid(format!(
                        "{} is a profile for {}, not {}",
                        source.label(),
                        source.game,
                        loaded.game
                    )));
                }
                let Some(found) = source.folders.iter().find(|f| f.id == folder) else {
                    return Err(Error::invalid(format!(
                        "{} has no folder of its own with id {folder}",
                        source.label()
                    )));
                };
                if !link {
                    let files = self.config_files(&from).unwrap_or_default();
                    let (set, _) = folder_as_set(&source, found, &files);
                    return self.copy_set_in(profile, set);
                }
                let set = self.share_folder(&from, &folder.to_string())?;
                self.include_set(profile, set.id, None, personal)
            }
        }
    }

    /// A set is for sharing a folder, so once only one profile includes
    /// the set `id` it's an ordinary folder of that profile's own again,
    /// and once none does it goes. Runs whenever a profile stops including
    /// one.
    pub(super) fn settle_set(&self, id: Uuid) -> Result<()> {
        let dir = self.paths.sets().join(id.to_string());
        // a personal layer, or one that's gone already
        if !self.set_file(id).is_file() {
            return Ok(());
        }
        match self.includers()?.remove(&id).unwrap_or_default().as_slice() {
            [] => fs::remove_dir_all(&dir).at(&dir),
            // A synced profile's own folders go to everyone, so one that
            // includes it personally keeps it as a set.
            [(only, _)] if self.includes_personally(only, id)? => Ok(()),
            // it's still to be asked whether a folder that came from it
            // goes back in it (see `folder_changed`)
            [_] if self.awaits_answer(id)? => Ok(()),
            // saving it settles the set again, with none left
            [(only, _)] => self.unlink_set(only, id, true),
            _ => Ok(()),
        }
    }

    /// Has the profile include the set `id`, after its own mod `after`
    /// (`None` for after all of them). In a synced profile, everyone it's
    /// synced with gets its mods as an ordinary folder (see
    /// [`sync::published`](super::sync)); with `personal`, it's in the
    /// profile's personal layer instead (see [`crate::personal`]), so it's
    /// only on this person's computers. A profile this computer only
    /// follows always includes it personally, since whoever shares it
    /// decides what everyone has. `personal` means nothing for a profile
    /// that isn't synced: everything in it is only here anyway.
    pub fn include_set(
        &self,
        profile: &ProfileRef,
        id: Uuid,
        after: Option<&str>,
        personal: bool,
    ) -> Result<()> {
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
            personal: (personal && self.is_synced(profile)) || self.is_followed(profile)?,
            ..Include::new(id)
        });
        self.save_profile(profile, &loaded)
    }

    /// Makes the profile's include of the set `id` part of its personal
    /// layer (see [`crate::personal`]), so the profile can be synced with
    /// it: its mods stay on this person's computers rather than going to
    /// everyone who has the profile. What only they need locked moves to
    /// the personal lockfile.
    pub fn include_personally(&self, profile: &ProfileRef, id: Uuid) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        let include = loaded
            .includes
            .iter_mut()
            .find(|i| i.set == id)
            .ok_or_else(|| Error::invalid(format!("{label} doesn't include the set {id}")))?;
        if include.personal {
            return Ok(());
        }
        include.personal = true;
        self.save_profile(profile, &loaded)?;
        self.split_lock_again(profile)
    }

    /// Copies the set `id` into the profile as an ordinary folder of its
    /// own, after its own mods, rather than including it. Its config edits
    /// are copied in too, under the profile's own.
    pub fn copy_set(&self, profile: &ProfileRef, id: Uuid) -> Result<()> {
        let set = self.load_set(id)?.ok_or_else(|| no_such_set(id))?;
        self.copy_set_in(profile, set)
    }

    /// Copies `set`'s mods and config edits into the profile, as
    /// [`Patchwork::copy_set`] does. The profile's own mods and settings
    /// win over the set's.
    fn copy_set_in(&self, profile: &ProfileRef, set: Set) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
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
        let mut own = set.config;
        overlay(&mut own, &own_config(&loaded));
        merge_config(&mut loaded, &own);
        self.save_profile(profile, &loaded)
    }

    /// Stops the profile including the set `id`. With `keep`, its mods (and
    /// config edits) stay in the profile as its own, in an ordinary folder
    /// named like the set; otherwise they go. A set included personally
    /// keeps its mods and config edits in the personal layer instead (under
    /// its own settings).
    pub fn unlink_set(&self, profile: &ProfileRef, id: Uuid, keep: bool) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let Some(at) = loaded.includes.iter().position(|i| i.set == id) else {
            return Err(Error::invalid(format!(
                "{} doesn't include the set {id}",
                loaded.label()
            )));
        };
        let own = own_config(&loaded);
        let include = loaded.includes.remove(at);
        // a personal include's go to the personal layer, once it's saved
        let mut personal_config = None;
        if !keep || include.personal {
            merge_config(&mut loaded, &own);
        }
        // Kept, they're the profile's own once it doesn't include the set,
        // and personal ones stay personal.
        if keep && include.personal && include.linked.is_some() && include.set != loaded.id {
            personal_config = include.linked.as_ref().map(|l| l.config.clone());
            if loaded.personal().is_none() {
                loaded.includes.push(personal_include(&loaded));
            }
            for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(id)) {
                m.folder = Some(loaded.id);
            }
        } else if keep && let Some(linked) = &include.linked {
            let mut folder = Folder::new(&folder_name(&loaded, &linked.folder.name));
            // The set's id if it's going, so a synced profile's document has
            // the same folder as before (see `sync::published`).
            if self.profiles_including(id)?.iter().all(|p| p == profile) {
                folder.id = id;
            }
            for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(id)) {
                m.folder = Some(folder.id);
            }
            loaded.folders.push(folder);
        } else {
            loaded.mods.retain(|m| m.folder != Some(id));
        }
        self.save_profile(profile, &loaded)?;
        if let Some(config) = personal_config {
            self.add_personal_config(profile, &loaded.game, config)?;
        }
        Ok(())
    }

    /// `name` trimmed, if it isn't blank and no set of `game` but `except`
    /// has it (ignoring case).
    pub(super) fn set_name(&self, game: &str, name: &str, except: Option<Uuid>) -> Result<String> {
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

    /// The set with id `id`, if there is one. A profile's id gives its
    /// personal layer (see [`crate::personal`]), the private set only it
    /// includes.
    pub fn load_set(&self, id: Uuid) -> Result<Option<Set>> {
        let path = self.set_file(id);
        let Some(set) = fsutil::read_toml::<Set>(&path)? else {
            return self.load_personal_set(id);
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

    /// Saves a set whose config edits have changed, for every profile that
    /// includes it.
    pub(super) fn save_set_config(&self, set: &Set) -> Result<()> {
        self.save_set(set)?;
        for profile in self.profiles_including(set.id)? {
            self.profile_changed(&profile, false);
        }
        Ok(())
    }

    pub(super) fn save_set(&self, set: &Set) -> Result<()> {
        if self.save_personal_set(set)? {
            return Ok(());
        }
        let mut set = set.clone();
        set.schema = set.saved_schema();
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

    /// The synced profiles that include the set `id` for everyone (not
    /// personally), by name: changing the set changes what everyone
    /// they're synced with has.
    pub fn synced_includers(&self, id: Uuid) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for (profile, label) in self.includers()?.remove(&id).unwrap_or_default() {
            if self.is_synced(&profile) && !self.includes_personally(&profile, id)? {
                names.push(
                    label
                        .split_once('/')
                        .map_or(label.clone(), |(_, n)| n.to_owned()),
                );
            }
        }
        Ok(names)
    }

    /// Every set some profile includes, with those profiles and their
    /// labels, read as they're stored (without loading their sets).
    pub fn includers(&self) -> Result<HashMap<Uuid, Vec<(ProfileRef, String)>>> {
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
            let personal = self.personal_sets(&profile)?;
            for set in stored.includes.into_iter().map(|i| i.set).chain(personal) {
                found
                    .entry(set)
                    .or_default()
                    .push((profile.clone(), label.clone()));
            }
        }
        for profiles in found.values_mut() {
            profiles.sort();
        }
        Ok(found)
    }

    /// The sets the profile includes, as it's stored, those it includes
    /// personally too (but not its personal layer); none if it hasn't been
    /// saved.
    pub(super) fn stored_includes(&self, profile: &ProfileRef) -> Result<Vec<Uuid>> {
        let stored = fsutil::read_toml::<Includes>(&self.profile_file(profile))?;
        let mut sets: Vec<Uuid> = stored
            .map(|s| s.includes.into_iter().map(|i| i.set).collect())
            .unwrap_or_default();
        sets.extend(self.personal_sets(profile)?);
        Ok(sets)
    }

    /// Whether the profile includes the set `id` personally, as stored.
    fn includes_personally(&self, profile: &ProfileRef, id: Uuid) -> Result<bool> {
        Ok(self.personal_sets(profile)?.contains(&id))
    }

    fn set_file(&self, id: Uuid) -> PathBuf {
        self.paths.sets().join(id.to_string()).join("set.toml")
    }
}

/// Just what's needed of a stored profile to know which sets it includes.
#[derive(Deserialize)]
struct Includes {
    game: String,
    name: String,
    #[serde(default, rename = "include")]
    includes: Vec<Include>,
}

fn no_such_set(id: Uuid) -> Error {
    Error::invalid(format!("there's no set with id {id}"))
}

/// `folder` of `profile` (as loaded) as a set of the same name: its mods,
/// on and in no folder, and the settings the profile changes in their
/// config files (those of `files` they own). Also gives the profile's own
/// settings left once those are taken out.
fn folder_as_set(profile: &Profile, folder: &Folder, files: &[ConfigFileInfo]) -> (Set, FileEdits) {
    let mut set = Set::new(&profile.game, &folder.name);
    for m in profile.mods.iter().filter(|m| m.folder == Some(folder.id)) {
        let mut copy = m.clone();
        copy.enabled = true;
        copy.folder = None;
        set.mods.push(copy);
    }
    let mut own = own_config(profile);
    for f in files {
        if f.owner
            .as_deref()
            .is_some_and(|o| set.mods.iter().any(|m| m.name == o))
            && let Some(path) = own.keys().find(|p| same_dest(p, &f.path)).cloned()
            && let Some(edits) = own.remove(&path)
        {
            set.config.insert(path, edits);
        }
    }
    (set, own)
}

/// A set, named and with the mods `folder` and `mods` give, for
/// [`Patchwork::other_folders`]; with no profiles yet. `personal` if the
/// profile it's for is followed, so would include it personally.
fn set_folder(
    folder: &Folder,
    mods: &[ProfileMod],
    already: impl Fn(&[ProfileMod]) -> Vec<String>,
    personal: bool,
) -> OtherFolder {
    OtherFolder {
        source: FolderSource::Set { set: folder.id },
        name: folder.name.clone(),
        mods: mods.iter().map(|m| m.name.clone()).collect(),
        profiles: Vec::new(),
        already: already(mods),
        personal,
    }
}

/// What an include of `set` remembers of it when it's just been loaded.
pub(super) fn linked(set: &Set) -> Linked {
    Linked {
        folder: Folder {
            id: set.id,
            ..Folder::new(&set.name)
        },
        mods: set.mods.clone(),
        config: set.config.clone(),
        shadowed: Vec::new(),
        renamed: Vec::new(),
    }
}

/// The last of the profile's own mods (not a set's).
pub(super) fn last_own(profile: &Profile) -> Option<String> {
    profile
        .mods
        .iter()
        .rev()
        .find(|m| !profile.is_linked(m))
        .map(|m| m.name.clone())
}

/// `name`, or a numbered one if a folder of `profile` already has it.
pub(super) fn folder_name(profile: &Profile, name: &str) -> String {
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
