//! Linked folders of synced profiles that someone the profile's synced
//! with changed (see `docs/design/folders.md`, step 4). Friends have the
//! folder as an ordinary one, so when one of them changes what's in it,
//! syncing doesn't change the set, which would reach into the person's
//! other profiles. The synced profile takes their version as its own
//! folder, keeping its id (they know it), and the set takes a new one,
//! with what happened kept in the profile's state until the person says
//! whether to use the folder in every profile or keep it only in this
//! one.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Patchwork;
use super::sets::{last_own, linked};
use crate::error::IoContext;
use crate::fix::Fix;
use crate::fsutil;
use crate::issue::{Issue, IssueKind, Severity};
use crate::lock::mod_key;
use crate::profile::{Folder, Include, Profile, ProfileMod, ProfileRef};
use crate::{Error, Result};

/// The profile's folders that came from sets when someone else changed
/// them, as kept in its state folder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FromSets {
    folders: Vec<FromSet>,
}

/// One of the profile's own folders that was a set's until someone else
/// changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FromSet {
    /// The folder's id, which was the set's.
    pub folder: Uuid,
    /// The set's id since.
    pub set: Uuid,
}

impl Patchwork {
    /// Someone `profile` is synced with changed the folder of the set `id`
    /// it included, so it's the profile's own folder now (see
    /// [`relink`](super::sync)). If other profiles include the set, it
    /// takes a new id, and the profile keeps a note of it for
    /// [`Patchwork::issues`] to ask about. Otherwise it's settled as usual.
    pub(super) fn folder_changed(&self, profile: &ProfileRef, id: Uuid) -> Result<()> {
        let others = self.profiles_including(id)?;
        if others.iter().all(|p| p == profile) {
            return self.settle_set(id);
        }
        let set = Uuid::new_v4();
        let mut found = self.came_from(profile)?;
        found.folders.retain(|f| f.folder != id);
        found.folders.push(FromSet { folder: id, set });
        // first, so the set isn't settled away while it's being moved
        self.save_came_from(profile, &found)?;
        self.reid_set(id, set)
    }

    /// Saves the profile's folder `folder`, which someone it's synced with
    /// changed, to the set it came from, which takes the folder's id back,
    /// and has the profile include it again in its place: it's the same
    /// folder in every profile that has it once more. Its name goes to the
    /// set too, unless another set has it.
    pub fn use_folder_everywhere(&self, profile: &ProfileRef, folder: &str) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let found = loaded.find_folder(folder)?.clone();
        let from = self.where_from(profile, &loaded, found.id)?;
        if found.optional {
            return Err(optional_folder(&loaded, &found));
        }
        let mut set = self
            .load_set(from.set)?
            .ok_or_else(|| set_gone(&found.name))?;
        set.mods = loaded
            .mods
            .iter()
            .filter(|m| m.folder == Some(found.id))
            .map(|m| ProfileMod {
                enabled: true,
                folder: None,
                ..m.clone()
            })
            .collect();
        if let Ok(name) = self.set_name(&set.game, &found.name, Some(set.id)) {
            set.name = name;
        }
        self.save_set(&set)?;
        self.reid_set(from.set, found.id)?;
        self.forget_came_from(profile, found.id)?;

        let mut loaded = self.load_profile(profile)?;
        set.id = found.id;
        let disabled = loaded
            .mods
            .iter()
            .filter(|m| m.folder == Some(found.id) && !m.enabled)
            .map(|m| m.name.clone())
            .collect();
        loaded.folders.retain(|f| f.id != found.id);
        loaded.includes.push(Include {
            // where an empty folder goes; otherwise where its mods are
            after: last_own(&loaded),
            disabled,
            linked: Some(linked(&set)),
            ..Include::new(found.id)
        });
        self.save_profile(profile, &loaded)
    }

    /// Keeps the profile's folder `folder`, which someone it's synced with
    /// changed, as its own, leaving the set it came from as it is for the
    /// other profiles that include it.
    pub fn keep_folder_here(&self, profile: &ProfileRef, folder: &str) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let found = loaded.find_folder(folder)?.clone();
        let from = self.where_from(profile, &loaded, found.id)?;
        self.forget_came_from(profile, found.id)?;
        self.settle_set(from.set)
    }

    /// Whether some profile's own folder came from the set `id` and hasn't
    /// been asked about yet, so the set stays even if only one other
    /// profile includes it.
    pub(super) fn awaits_answer(&self, id: Uuid) -> Result<bool> {
        let Ok(entries) = fs::read_dir(self.paths.profile_state()) else {
            return Ok(false);
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let file = entry.path().join(FILE);
            let found: Option<FromSets> = fsutil::read_json(&file)?;
            if found.is_some_and(|f| f.folders.iter().any(|f| f.set == id)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Forgets where the folders `loaded` (as just saved) no longer has
    /// came from, and settles their sets.
    pub(super) fn forget_gone_folders(&self, profile: &ProfileRef, loaded: &Profile) -> Result<()> {
        let mut found = self.came_from(profile)?;
        let (kept, gone): (Vec<FromSet>, Vec<FromSet>) = found
            .folders
            .into_iter()
            .partition(|f| loaded.folders.iter().any(|own| own.id == f.folder));
        if gone.is_empty() {
            return Ok(());
        }
        found.folders = kept;
        self.save_came_from(profile, &found)?;
        for f in gone {
            self.settle_set(f.set)?;
        }
        Ok(())
    }

    /// The sets the profile's own folders came from, for settling once
    /// it's removed.
    pub(super) fn sets_folders_came_from(&self, profile: &ProfileRef) -> Result<Vec<Uuid>> {
        Ok(self
            .came_from(profile)?
            .folders
            .into_iter()
            .map(|f| f.set)
            .collect())
    }

    /// An issue for each of `loaded`'s folders that someone it's synced
    /// with changed while it was a set's, while other profiles still
    /// include the set: what's different, and what to do about it.
    pub(super) fn changed_folder_issues(
        &self,
        profile: &ProfileRef,
        loaded: &Profile,
    ) -> Result<Vec<Issue>> {
        let mut issues = Vec::new();
        for from in self.came_from(profile)?.folders {
            let Some(folder) = loaded.folders.iter().find(|f| f.id == from.folder) else {
                continue;
            };
            let Some(set) = self.load_set(from.set)? else {
                continue;
            };
            let mut others = Vec::new();
            for other in self.profiles_including(from.set)? {
                if other != *profile {
                    others.push(self.load_profile(&other)?.name);
                }
            }
            if others.is_empty() {
                continue;
            }
            let ours: Vec<&ProfileMod> = loaded
                .mods
                .iter()
                .filter(|m| m.folder == Some(folder.id))
                .collect();
            let key = |m: &ProfileMod| mod_key(&m.source, &m.id);
            let theirs = |m: &ProfileMod| set.mods.iter().find(|s| key(s) == key(m));
            let added: Vec<String> = ours
                .iter()
                .filter(|m| theirs(m).is_none())
                .map(|m| m.name.clone())
                .collect();
            let removed: Vec<String> = set
                .mods
                .iter()
                .filter(|s| !ours.iter().any(|m| key(m) == key(s)))
                .map(|s| s.name.clone())
                .collect();
            let changed: Vec<String> = ours
                .iter()
                .filter(|m| {
                    theirs(m).is_some_and(|s| {
                        *s != ProfileMod {
                            enabled: true,
                            folder: None,
                            name: s.name.clone(),
                            ..(**m).clone()
                        }
                    })
                })
                .map(|m| m.name.clone())
                .collect();
            let mut what = Vec::new();
            if folder.name != set.name {
                what.push(format!("renamed it from {}", set.name));
            }
            if folder.optional {
                what.push("made it optional".to_owned());
            }
            for (done, names) in [
                ("added", &added),
                ("removed", &removed),
                ("changed", &changed),
            ] {
                if !names.is_empty() {
                    what.push(format!("{done} {}", names.join(", ")));
                }
            }
            if what.is_empty() {
                what.push("moved its mods around".to_owned());
            }
            let use_everywhere = Fix::UseEverywhere {
                folder: folder.id.to_string(),
                name: folder.name.clone(),
            };
            let keep_here = Fix::KeepHere {
                folder: folder.id.to_string(),
                name: folder.name.clone(),
            };
            // a folder in other profiles can't be optional
            let (fix, other_fixes) = match folder.optional {
                true => (keep_here, Vec::new()),
                false => (use_everywhere, vec![keep_here]),
            };
            issues.push(Issue {
                severity: Severity::Warning,
                mods: ours.iter().map(|m| m.name.clone()).collect(),
                message: format!(
                    "Someone you share {} with changed {} ({}), which is also in {}; \
                     it's changed only in this profile for now",
                    loaded.name,
                    folder.name,
                    what.join("; "),
                    others.join(" and ")
                ),
                kind: IssueKind::FolderChanged {
                    folder: folder.id.to_string(),
                    name: folder.name.clone(),
                    profiles: others,
                    added,
                    removed,
                    changed,
                },
                fix: Some(fix),
                other_fixes,
            });
        }
        Ok(issues)
    }

    /// Gives the set `old` the id `new`, for every profile that includes
    /// it. Sets are only on this computer, so nothing else knows their
    /// ids, except through synced profiles that include them for everyone,
    /// which publish the change of folder.
    pub(super) fn reid_set(&self, old: Uuid, new: Uuid) -> Result<()> {
        let mut including = Vec::new();
        for profile in self.profiles_including(old)? {
            let loaded = self.load_profile(&profile)?;
            including.push((profile, loaded));
        }
        let mut set = self
            .load_set(old)?
            .ok_or_else(|| Error::invalid(format!("there's no set with id {old}")))?;
        set.id = new;
        self.save_set(&set)?;
        let dir = self.paths.sets().join(old.to_string());
        fs::remove_dir_all(&dir).at(&dir)?;
        for (profile, mut loaded) in including {
            for include in loaded.includes.iter_mut().filter(|i| i.set == old) {
                include.set = new;
                if let Some(linked) = &mut include.linked {
                    linked.folder.id = new;
                }
            }
            for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(old)) {
                m.folder = Some(new);
            }
            self.save_profile(&profile, &loaded)?;
        }
        Ok(())
    }

    /// Where the profile's folder `folder` came from, failing if it isn't
    /// one someone it's synced with changed while it was a set's.
    fn where_from(&self, profile: &ProfileRef, loaded: &Profile, folder: Uuid) -> Result<FromSet> {
        let found = self.came_from(profile)?;
        found
            .folders
            .into_iter()
            .find(|f| f.folder == folder && loaded.folders.iter().any(|own| own.id == folder))
            .ok_or_else(|| {
                let name = loaded
                    .all_folders()
                    .find(|f| f.id == folder)
                    .map_or_else(|| folder.to_string(), |f| f.name.clone());
                Error::invalid(format!(
                    "{name} in {} isn't a folder someone else changed while other profiles \
                     had it too",
                    loaded.label()
                ))
            })
    }

    fn forget_came_from(&self, profile: &ProfileRef, folder: Uuid) -> Result<()> {
        let mut found = self.came_from(profile)?;
        found.folders.retain(|f| f.folder != folder);
        self.save_came_from(profile, &found)
    }

    fn came_from(&self, profile: &ProfileRef) -> Result<FromSets> {
        Ok(fsutil::read_json(&self.came_from_file(profile))?.unwrap_or_default())
    }

    fn save_came_from(&self, profile: &ProfileRef, found: &FromSets) -> Result<()> {
        let file = self.came_from_file(profile);
        if found.folders.is_empty() {
            if file.exists() {
                fs::remove_file(&file).at(&file)?;
            }
            return Ok(());
        }
        fsutil::write_json(&file, found)
    }

    fn came_from_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join(FILE)
    }
}

/// What a profile's folders that came from sets are kept in, in its state
/// folder.
const FILE: &str = "from-sets.json";

/// A folder that's optional in a synced profile can't be in other
/// profiles too: whether its mods are on is each person's, not each
/// profile's. Making it required again clears that.
pub(super) fn optional_folder(loaded: &Profile, folder: &Folder) -> Error {
    let label = loaded.label();
    let name = &folder.name;
    Error::blocked(
        format!("{name} is optional in {label}, so it can't be in other profiles too"),
        format!("{name} is optional"),
        format!(
            "{name} is optional in {label}: everyone it's synced with picks for themselves \
             whether its mods are on. A folder in more profiles than one has its mods on or \
             off in each profile instead, so it has to be required first. Its mods stay as \
             they are for you, and everyone else gets them as you have them."
        ),
        Fix::MakeRequired {
            profile: loaded.id.to_string(),
            label,
            folder: folder.id.to_string(),
            name: name.clone(),
        },
    )
}

fn set_gone(name: &str) -> Error {
    Error::invalid(format!(
        "the set {name} came from isn't here any more, so it's only this profile's now"
    ))
}
