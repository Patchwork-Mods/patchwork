//! What a profile looked like at each deploy, so it can go back to any of
//! them: [`Patchwork::revert`] to the last one, [`Patchwork::rollback`] to
//! older ones.
//!
//! Each deploy saves the profile's mods, extensions' state and lockfile
//! to
//! `deployed.toml` and, unless they're the same as the newest entry, to
//! `history/<n>.toml`, both kept with what only this machine knows about the
//! profile. Mods are content-addressed in the store, so going
//! back doesn't download anything that's still there.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::personal::personal_include;
use super::{Patchwork, local};
use crate::config::{FileEdits, overlay};
use crate::error::IoContext;
use crate::ext::ExtTables;
use crate::fix::{Blocker, Fix, SetsThen};
use crate::fsutil;
use crate::hash::ContentHash;
use crate::list::ListEntry;
use crate::lock::{Lockfile, mod_key};
use crate::profile::{Folder, Include, Linked, Profile, ProfileMod, ProfileRef, upgrade_plugins};
use crate::set::{Set, merge_config, own_config};
use crate::sync::dedupe_folder_names;
use crate::{Error, Result};

/// How many deploys each profile's history keeps. Older ones are dropped.
pub const HISTORY_LIMIT: usize = 50;

/// A profile's mods and lockfile at a deploy.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Deployed {
    /// Left out by versions from before there was a history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deployed_at: Option<Timestamp>,
    /// The profile's folders, so mods' `folder`s still name them after
    /// going back. Left out by versions from before there were folders.
    #[serde(default, rename = "folder", skip_serializing_if = "Vec::is_empty")]
    folders: Vec<Folder>,
    /// The shared sets the profile included, each as it was then, so going
    /// back can tell whether it's changed since. Its mods are in `mods`,
    /// like the profile's own.
    #[serde(default, rename = "include", skip_serializing_if = "Vec::is_empty")]
    includes: Vec<DeployedInclude>,
    #[serde(default, rename = "mod")]
    mods: Vec<ProfileMod>,
    /// Saved before profile schema 3, which moved it into `ext`.
    #[serde(default, rename = "plugin", skip_serializing)]
    plugins: Vec<ListEntry>,
    /// See [`crate::profile::Profile::ext`].
    #[serde(default, skip_serializing_if = "ExtTables::is_empty")]
    ext: ExtTables,
    lock: Lockfile,
}

/// An [`Include`] in a [`Deployed`], with the set as it was then (`None`
/// if it couldn't be found).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeployedInclude {
    set: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disabled: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked: Option<Linked>,
    /// See [`Include::personal`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    personal: bool,
}

impl DeployedInclude {
    fn of(include: Include) -> Self {
        Self {
            set: include.set,
            after: include.after,
            disabled: include.disabled,
            linked: include.linked,
            personal: include.personal,
        }
    }

    fn include(&self, linked: Option<Linked>) -> Include {
        Include {
            after: self.after.clone(),
            disabled: self.disabled.clone(),
            linked,
            personal: self.personal,
            ..Include::new(self.set)
        }
    }
}

impl Deployed {
    fn of(loaded: Profile, lock: Lockfile, deployed_at: Option<Timestamp>) -> Self {
        Self {
            deployed_at,
            folders: loaded.folders,
            includes: loaded
                .includes
                .into_iter()
                .map(DeployedInclude::of)
                .collect(),
            mods: loaded.mods,
            plugins: Vec::new(),
            ext: loaded.ext,
            lock,
        }
    }

    /// Whether the two would deploy the same thing; when they happened
    /// doesn't matter, nor which folders the mods are in (only how the list
    /// is shown, like hidden tags). Folders' places in the load order do
    /// matter, but that's the mods' order.
    fn same_as(&self, other: &Deployed) -> bool {
        self.mods.len() == other.mods.len()
            && self
                .mods
                .iter()
                .zip(&other.mods)
                .all(|(a, b)| same_but_folder(a, b))
            && self.ext == other.ext
            && toml_eq(&self.lock, &other.lock)
    }
}

/// One deploy in a profile's history, from [`Patchwork::history`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// What [`Patchwork::rollback`] takes. Numbers only ever go up, so they
    /// stay the same as newer deploys are added.
    pub number: u32,
    pub deployed_at: Option<Timestamp>,
    /// How many of the profile's own mods were enabled, not counting
    /// dependencies.
    pub enabled: usize,
    /// What changed since the entry before it, or nothing for the oldest
    /// one kept.
    pub changes: Vec<Change>,
    /// Whether the profile is exactly like this now.
    pub current: bool,
}

/// A difference between two deploys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Change {
    Added {
        name: String,
        version: Option<String>,
    },
    Removed {
        name: String,
        version: Option<String>,
    },
    Updated {
        name: String,
        from: Option<String>,
        to: Option<String>,
    },
    Enabled {
        name: String,
    },
    Disabled {
        name: String,
    },
    /// A mod's priority (see [`ProfileMod::priority`]) changed.
    Priority {
        name: String,
        from: i32,
        to: i32,
    },
    /// The mods' order changed.
    Reordered,
    /// The order of one of the profile's lists changed (see
    /// [`crate::list`]).
    List {
        id: String,
        title: String,
    },
    /// What an extension keeps for the profile changed.
    Ext {
        id: String,
    },
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = |v: &Option<String>| v.as_deref().unwrap_or("?").to_owned();
        match self {
            Change::Added { name, version } => write!(f, "+ {name} {}", v(version)),
            Change::Removed { name, version } => write!(f, "- {name} {}", v(version)),
            Change::Updated { name, from, to } => write!(f, "~ {name} {} -> {}", v(from), v(to)),
            Change::Enabled { name } => write!(f, "  enabled {name}"),
            Change::Disabled { name } => write!(f, "  disabled {name}"),
            Change::Priority { name, from, to } => {
                write!(f, "  {name} priority {from} -> {to}")
            }
            Change::Reordered => write!(f, "  reordered mods"),
            Change::List { title, .. } => write!(f, "  rearranged {}", title.to_lowercase()),
            Change::Ext { id } => write!(f, "  changed what {id} keeps"),
        }
    }
}

impl Patchwork {
    /// Remembers the profile as it is now, as what was last deployed, and
    /// adds it to the history unless it's the same as the newest entry.
    pub(super) fn save_deployed(&self, profile: &ProfileRef) -> Result<()> {
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(());
        };
        let loaded = self.load_profile(profile)?;
        let deployed = Deployed::of(loaded, lock, Some(Timestamp::now()));
        fsutil::write_toml(&self.deployed_file(profile), &deployed)?;

        let numbers = self.history_numbers(profile)?;
        let newest = match numbers.last() {
            Some(&n) => self.load_history_entry(profile, n)?,
            None => None,
        };
        if newest.is_some_and(|newest| newest.same_as(&deployed)) {
            return Ok(());
        }
        let next = numbers.last().map_or(1, |n| n + 1);
        fsutil::write_toml(&self.history_file(profile, next), &deployed)?;
        // Best effort: whatever's left is dropped next time.
        let excess = (numbers.len() + 1).saturating_sub(HISTORY_LIMIT);
        for &n in &numbers[..excess] {
            let _ = fs::remove_file(self.history_file(profile, n));
        }
        Ok(())
    }

    fn load_deployed(&self, profile: &ProfileRef) -> Result<Option<Deployed>> {
        read_deployed(&self.deployed_file(profile))
    }

    /// The profile as it is now, to compare against a deploy.
    fn current(&self, profile: &ProfileRef) -> Result<Option<Deployed>> {
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(None);
        };
        let loaded = self.load_profile(profile)?;
        Ok(Some(Deployed::of(loaded, lock, None)))
    }

    /// Whether the profile's mods or their locked versions have changed
    /// since it was last deployed, so [`Patchwork::revert`] would do
    /// anything. Always false for a profile that's never been deployed.
    ///
    /// Hidden tags don't count, since they don't change what's deployed.
    pub fn has_changes(&self, profile: &ProfileRef) -> Result<bool> {
        let Some(deployed) = self.load_deployed(profile)? else {
            return Ok(false);
        };
        Ok(!self
            .current(profile)?
            .is_some_and(|now| now.same_as(&deployed)))
    }

    /// Puts the profile's mods and lockfile back how they were when it was
    /// last deployed, undoing everything done to it since. Nothing in the
    /// game changes; the profile just matches what's deployed again.
    ///
    /// If a shared set the profile included then has changed since, it's
    /// a blocker whose fixes say what to do with it; see
    /// [`Patchwork::go_back`].
    pub fn revert(&self, profile: &ProfileRef) -> Result<()> {
        self.go_back(profile, None, None)
    }

    /// Goes back to the profile's last deploy (`number` `None`, like
    /// [`Patchwork::revert`]) or deploy `number` of its history (like
    /// [`Patchwork::rollback`]). The shared sets it included then that have
    /// changed since are done with as `sets` says; with `None`, that's a
    /// blocker if there are any.
    pub fn go_back(
        &self,
        profile: &ProfileRef,
        number: Option<u32>,
        sets: Option<SetsThen>,
    ) -> Result<()> {
        let then = match number {
            Some(number) => self.history_entry(profile, number)?,
            None => self.load_deployed(profile)?.ok_or_else(|| {
                Error::invalid(format!(
                    "{profile} hasn't been deployed, so there's nothing to revert to"
                ))
            })?,
        };
        self.restore(profile, then, number, sets)
    }

    /// The deploys kept in the profile's history, oldest first.
    pub fn history(&self, profile: &ProfileRef) -> Result<Vec<HistoryEntry>> {
        let now = self.current(profile)?;
        let lists = self.list_titles();
        let mut entries = Vec::new();
        let mut before: Option<Deployed> = None;
        for n in self.history_numbers(profile)? {
            // A file that went missing in between is just skipped.
            let Some(deployed) = self.load_history_entry(profile, n)? else {
                continue;
            };
            entries.push(HistoryEntry {
                number: n,
                deployed_at: deployed.deployed_at,
                enabled: deployed.mods.iter().filter(|m| m.enabled).count(),
                changes: before
                    .as_ref()
                    .map(|b| changes(b, &deployed, &lists))
                    .unwrap_or_default(),
                current: now.as_ref().is_some_and(|now| now.same_as(&deployed)),
            });
            before = Some(deployed);
        }
        Ok(entries)
    }

    /// Puts the profile's mods and lockfile back how they were at deploy
    /// `number` of its [`Patchwork::history`]. Like [`Patchwork::revert`],
    /// nothing in the game changes until it's deployed.
    pub fn rollback(&self, profile: &ProfileRef, number: u32) -> Result<()> {
        self.go_back(profile, Some(number), None)
    }

    /// What [`Patchwork::rollback`] to deploy `number` would change.
    pub fn rollback_changes(&self, profile: &ProfileRef, number: u32) -> Result<Vec<Change>> {
        let then = self.history_entry(profile, number)?;
        Ok(match self.current(profile)? {
            Some(now) => changes(&now, &then, &self.list_titles()),
            None => Vec::new(),
        })
    }

    fn history_entry(&self, profile: &ProfileRef, number: u32) -> Result<Deployed> {
        match self.load_history_entry(profile, number)? {
            Some(deployed) => Ok(deployed),
            None => Err(Error::invalid(format!(
                "{} has no deploy {number} in its history; `patchwork history` lists them",
                self.load_profile(profile)?.label()
            ))),
        }
    }

    fn restore(
        &self,
        profile: &ProfileRef,
        deployed: Deployed,
        number: Option<u32>,
        sets: Option<SetsThen>,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        // History doesn't keep config edits, so the profile's own stay as
        // they are, and its sets' are as they are now.
        let mut own = own_config(&loaded);
        // each set as it is now, for those included then
        let mut now: HashMap<uuid::Uuid, Option<Set>> = HashMap::new();
        for include in &deployed.includes {
            now.insert(include.set, self.load_set(include.set)?);
        }
        // The personal layer is only this profile's, like its own mods, so
        // it's always put back as it was and isn't asked about.
        let changed: Vec<&DeployedInclude> = deployed
            .includes
            .iter()
            .filter(|i| i.set != loaded.id)
            .filter(|i| {
                i.linked
                    .as_ref()
                    .is_some_and(|then| now[&i.set].as_ref().is_none_or(|s| s.mods != then.mods))
            })
            .collect();
        let sets = match sets {
            Some(sets) => sets,
            None if changed.is_empty() => SetsThen::Keep,
            None => return Err(sets_changed(&loaded, &changed, number)),
        };

        let linked: HashSet<uuid::Uuid> = loaded
            .includes
            .iter()
            .map(|i| i.set)
            .chain(deployed.includes.iter().map(|i| i.set))
            .collect();
        loaded.mods = refile(&loaded.mods, deployed.mods, &linked);
        loaded.folders = restored_folders(
            std::mem::take(&mut loaded.folders),
            deployed.folders,
            &loaded.mods,
        );
        let mut includes = Vec::new();
        let mut copied_personal = false;
        let mut personal_config = FileEdits::new();
        for include in &deployed.includes {
            let Some(then) = &include.linked else {
                // missing then, so as it was
                includes.push(include.include(None));
                continue;
            };
            let set = now.remove(&include.set).flatten();
            let changed = set.as_ref().is_none_or(|s| s.mods != then.mods);
            let sets = if include.set == loaded.id {
                SetsThen::Restore
            } else {
                sets
            };
            match (sets, set) {
                // Its mods are as they were; split back up against the set
                // as it was, nothing's saved to it, so it stays as it is.
                (_, Some(set)) if !changed => includes.push(include.include(Some(Linked {
                    config: set.config,
                    ..then.clone()
                }))),
                (SetsThen::Keep, Some(set)) => includes.push(include.include(Some(Linked {
                    config: set.config,
                    ..then.clone()
                }))),
                // against the set as it is, so it's saved as it was
                (SetsThen::Restore, Some(set)) => includes.push(include.include(Some(Linked {
                    mods: set.mods,
                    config: set.config,
                    ..then.clone()
                }))),
                // gone since, so it's made again
                (SetsThen::Restore, None) => {
                    let mut set = Set::new(&loaded.game, &then.folder.name);
                    set.id = include.set;
                    set.mods = then.mods.clone();
                    self.save_set(&set)?;
                    includes.push(include.include(Some(then.clone())));
                }
                // Included personally, it's copied into the personal layer,
                // so its mods don't go to everyone, and so are its config
                // edits, under the layer's own.
                (SetsThen::Copy | SetsThen::Keep, set) if include.personal => {
                    if let Some(set) = set {
                        let mut copied = set.config;
                        overlay(&mut copied, &personal_config);
                        personal_config = copied;
                    }
                    for m in loaded
                        .mods
                        .iter_mut()
                        .filter(|m| m.folder == Some(include.set))
                    {
                        m.folder = Some(loaded.id);
                    }
                    copied_personal = true;
                }
                (SetsThen::Copy | SetsThen::Keep, set) => {
                    // its config edits are copied in too, under the profile's
                    if let Some(set) = set {
                        let mut copied = set.config;
                        overlay(&mut copied, &own);
                        own = copied;
                    }
                    let folder = Folder::new(&then.folder.name);
                    for m in loaded
                        .mods
                        .iter_mut()
                        .filter(|m| m.folder == Some(include.set))
                    {
                        m.folder = Some(folder.id);
                    }
                    loaded.folders.push(folder);
                    loaded.folders = dedupe_folder_names(std::mem::take(&mut loaded.folders));
                }
            }
        }
        loaded.includes = includes;
        if copied_personal && loaded.personal().is_none() {
            loaded.includes.push(personal_include(&loaded));
        }
        merge_config(&mut loaded, &own);
        loaded.ext = deployed.ext;
        self.save_lock(profile, &deployed.lock)?;
        self.save_profile(profile, &loaded)?;
        self.add_personal_config(profile, &loaded.game, personal_config)
    }

    fn history_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("history")
    }

    /// Gives local mods in what was deployed the ids they have now; see
    /// [`local::portable_ids`].
    pub(super) fn migrate_history_local_mods(&self, profile: &ProfileRef) -> Result<()> {
        let files = self
            .history_numbers(profile)?
            .into_iter()
            .map(|n| self.history_file(profile, n))
            .chain([self.deployed_file(profile)]);
        for file in files {
            let Some(mut deployed) = read_deployed(&file)? else {
                continue;
            };
            if !local::portable_ids(&mut deployed.mods, &mut deployed.lock).is_empty() {
                fsutil::write_toml(&file, &deployed)?;
            }
        }
        Ok(())
    }

    fn history_file(&self, profile: &ProfileRef, number: u32) -> PathBuf {
        self.history_dir(profile).join(format!("{number}.toml"))
    }

    fn load_history_entry(&self, profile: &ProfileRef, number: u32) -> Result<Option<Deployed>> {
        read_deployed(&self.history_file(profile, number))
    }

    /// The numbers of the history's entries, in order.
    fn history_numbers(&self, profile: &ProfileRef) -> Result<Vec<u32>> {
        let dir = self.history_dir(profile);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).at(&dir),
        };
        let mut numbers: Vec<u32> = entries
            .filter_map(|e| {
                let name = e.ok()?.file_name();
                name.to_str()?.strip_suffix(".toml")?.parse().ok()
            })
            .collect();
        numbers.sort_unstable();
        Ok(numbers)
    }
}

/// What changed going from `before` to `after`: mods (dependencies too)
/// added, removed or at another version, then the profile's own mods
/// enabled or disabled, then order.
/// What changed from `before` to `after`. `lists` are the titles of the
/// ordered lists there are, by `(extension, list)`, so changes to their
/// order can be told apart from other changes to extensions' state.
fn changes(
    before: &Deployed,
    after: &Deployed,
    lists: &BTreeMap<(String, String), String>,
) -> Vec<Change> {
    let top_level: HashSet<String> = before
        .mods
        .iter()
        .chain(&after.mods)
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    let old = installed(before, &top_level);
    let new = installed(after, &top_level);
    let mut changes = Vec::new();
    for m in &new {
        match old.iter().find(|o| o.key == m.key) {
            None => changes.push(Change::Added {
                name: m.name.clone(),
                version: m.version.clone(),
            }),
            Some(o) if o.version != m.version || o.hash != m.hash => {
                changes.push(Change::Updated {
                    name: m.name.clone(),
                    from: o.version.clone(),
                    to: m.version.clone(),
                })
            }
            Some(_) => {}
        }
    }
    for m in &old {
        if !new.iter().any(|n| n.key == m.key) {
            changes.push(Change::Removed {
                name: m.name.clone(),
                version: m.version.clone(),
            });
        }
    }

    let was: HashMap<(&str, &str), &ProfileMod> = before
        .mods
        .iter()
        .map(|m| ((m.source.as_str(), m.id.as_str()), m))
        .collect();
    for m in &after.mods {
        match was.get(&(m.source.as_str(), m.id.as_str())) {
            Some(w) if !w.enabled && m.enabled => changes.push(Change::Enabled {
                name: m.name.clone(),
            }),
            Some(w) if w.enabled && !m.enabled => changes.push(Change::Disabled {
                name: m.name.clone(),
            }),
            _ => {}
        }
        if let Some(w) = was.get(&(m.source.as_str(), m.id.as_str()))
            && w.priority != m.priority
        {
            changes.push(Change::Priority {
                name: m.name.clone(),
                from: w.priority,
                to: m.priority,
            });
        }
    }

    // Only the mods in both count, so adding or removing one isn't also a
    // reorder.
    let order = |mods: &[ProfileMod], other: &[ProfileMod]| -> Vec<(String, String)> {
        mods.iter()
            .filter(|m| other.iter().any(|o| o.source == m.source && o.id == m.id))
            .map(|m| (m.source.clone(), m.id.clone()))
            .collect()
    };
    if order(&before.mods, &after.mods) != order(&after.mods, &before.mods) {
        changes.push(Change::Reordered);
    }
    let ids: BTreeSet<&String> = before.ext.keys().chain(after.ext.keys()).collect();
    for id in ids {
        let (mut was, mut now) = (
            before.ext.get(id).cloned().unwrap_or_default(),
            after.ext.get(id).cloned().unwrap_or_default(),
        );
        for ((ext, list), title) in lists.iter().filter(|((ext, _), _)| ext == id) {
            if was.remove(list) != now.remove(list) {
                changes.push(Change::List {
                    id: crate::ext::hook_id(ext, list),
                    title: title.clone(),
                });
            }
        }
        // The version only matters if something else changed too.
        was.remove(crate::ext::VERSION_KEY);
        now.remove(crate::ext::VERSION_KEY);
        if was != now {
            changes.push(Change::Ext { id: id.clone() });
        }
    }
    changes
}

/// Whether `a` and `b` are the same apart from which folder they're in.
fn same_but_folder(a: &ProfileMod, b: &ProfileMod) -> bool {
    if a.folder == b.folder {
        return a == b;
    }
    ProfileMod {
        folder: b.folder,
        ..a.clone()
    } == *b
}

/// The mods `then` (going back to a deploy), each in the folder it's in
/// `now` if it's still in the profile, so going back keeps how they've
/// been filed since. A mod is filed as it was at the deploy instead when
/// keeping its folder would change the order, as keeping each folder's
/// mods together can: the deploy's filing was in that order. Mods that
/// have kept their place since are seen to first, so a mod that moved into
/// a folder is the one taken back out of it.
///
/// Mods in a set's linked folder (one of `linked`), then or now, are left
/// as they were then: which folder they're in is which set they're in.
fn refile(
    now: &[ProfileMod],
    then: Vec<ProfileMod>,
    linked: &HashSet<uuid::Uuid>,
) -> Vec<ProfileMod> {
    let is_linked = |m: &ProfileMod| m.folder.is_some_and(|f| linked.contains(&f));
    let filed: HashMap<(&str, &str), Option<uuid::Uuid>> = now
        .iter()
        .filter(|m| !is_linked(m))
        .map(|m| ((m.source.as_str(), m.id.as_str()), m.folder))
        .collect();
    let in_order = |mods: &[ProfileMod]| {
        let mut grouped = Profile::new("", "");
        grouped.mods = mods.to_vec();
        grouped.group_folders();
        grouped
            .mods
            .iter()
            .zip(mods)
            .all(|(a, b)| a.source == b.source && a.id == b.id)
    };
    let mut mods = then;
    let unmoved = kept_places(now, &mods);
    let (first, moved): (Vec<usize>, Vec<usize>) = (0..mods.len()).partition(|&i| unmoved[i]);
    for i in first.into_iter().chain(moved) {
        if is_linked(&mods[i]) {
            continue;
        }
        let key = (mods[i].source.as_str(), mods[i].id.as_str());
        let Some(&folder) = filed.get(&key) else {
            continue;
        };
        if folder == mods[i].folder {
            continue;
        }
        let was = std::mem::replace(&mut mods[i].folder, folder);
        if !in_order(&mods) {
            mods[i].folder = was;
        }
    }
    mods
}

/// Which of `then`'s mods are where they were in `now`, among the mods in
/// both: those in a longest run the two orders have in common.
fn kept_places(now: &[ProfileMod], then: &[ProfileMod]) -> Vec<bool> {
    let same = |a: &ProfileMod, b: &ProfileMod| a.source == b.source && a.id == b.id;
    let (n, t) = (now.len(), then.len());
    // longest[i][j]: the longest common run of now[i..] and then[j..]
    let mut longest = vec![vec![0u32; t + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..t).rev() {
            longest[i][j] = if same(&now[i], &then[j]) {
                longest[i + 1][j + 1] + 1
            } else {
                longest[i + 1][j].max(longest[i][j + 1])
            };
        }
    }
    let mut kept = vec![false; t];
    let (mut i, mut j) = (0, 0);
    while i < n && j < t {
        if same(&now[i], &then[j]) {
            kept[j] = true;
            i += 1;
            j += 1;
        } else if longest[i + 1][j] >= longest[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    kept
}

/// The profile's folders after going back to a deploy: the ones it has
/// now, so renames and new folders stay, and any of the deploy's that a
/// mod is in again, renamed if another has taken its name.
fn restored_folders(now: Vec<Folder>, then: Vec<Folder>, mods: &[ProfileMod]) -> Vec<Folder> {
    let back: Vec<Folder> = then
        .into_iter()
        .filter(|f| {
            !now.iter().any(|n| n.id == f.id) && mods.iter().any(|m| m.folder == Some(f.id))
        })
        .collect();
    dedupe_folder_names(now.into_iter().chain(back).collect())
}

/// A deploy saved at `path`, with what older versions saved where it is
/// now.
fn read_deployed(path: &std::path::Path) -> Result<Option<Deployed>> {
    let Some(mut deployed) = fsutil::read_toml::<Deployed>(path)? else {
        return Ok(None);
    };
    upgrade_plugins(&mut deployed.ext, std::mem::take(&mut deployed.plugins));
    Ok(Some(deployed))
}

/// A mod a deploy would install, from [`installed`].
struct Installed {
    key: String,
    name: String,
    version: Option<String>,
    hash: Option<ContentHash>,
}

/// The mods `deployed` installs, dependencies included. Its lockfile may be
/// behind its mods, as it is for the profile as it is now until it's locked
/// again: mods it doesn't have locked yet are in with the version they ask
/// for, and those `top_level` mods (the profile's own, at either end of a
/// comparison) that were taken out of it are left out.
fn installed(deployed: &Deployed, top_level: &HashSet<String>) -> Vec<Installed> {
    let own: HashSet<String> = deployed
        .mods
        .iter()
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    let mut mods: Vec<Installed> = deployed
        .lock
        .mods
        .iter()
        .filter(|m| own.contains(&m.key()) || !top_level.contains(&m.key()))
        .map(|m| Installed {
            key: m.key(),
            name: m.name.clone(),
            version: m.version.clone(),
            hash: Some(m.hash),
        })
        .collect();
    for m in &deployed.mods {
        let key = mod_key(&m.source, &m.id);
        if !mods.iter().any(|i| i.key == key) {
            mods.push(Installed {
                key,
                name: m.name.clone(),
                version: m.version.clone(),
                hash: None,
            });
        }
    }
    mods
}

fn toml_eq<T: Serialize>(a: &T, b: &T) -> bool {
    match (toml::to_string(a), toml::to_string(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The blocker for going back to a deploy (`number`, or the last one) of
/// `profile` when the shared sets it included then, `changed`, have
/// changed since.
fn sets_changed(profile: &Profile, changed: &[&DeployedInclude], number: Option<u32>) -> Error {
    let names: Vec<&str> = changed
        .iter()
        .filter_map(|i| i.linked.as_ref())
        .map(|l| l.folder.name.as_str())
        .collect();
    let names = names.join(", ");
    let deploy = match number {
        Some(n) => format!("deploy #{n}"),
        None => "the last deploy".to_owned(),
    };
    let one = changed.len() == 1;
    let (sets, it, was, is) = if one {
        ("set", "it", "it was", "it is")
    } else {
        ("sets", "them", "they were", "they are")
    };
    let label = profile.label();
    let personal = changed.iter().filter(|i| i.personal).count();
    let copied = if personal == changed.len() {
        format!(
            "puts {} mods in your Personal folder",
            if one { "its" } else { "their" }
        )
    } else if one {
        format!("makes an ordinary folder of {label}'s own")
    } else if personal == 0 {
        format!("makes ordinary folders of {label}'s own")
    } else {
        format!(
            "makes ordinary folders of {label}'s own (or, for those included just for you, \
             puts their mods in your Personal folder)"
        )
    };
    let fixes = [SetsThen::Restore, SetsThen::Copy, SetsThen::Keep]
        .into_iter()
        .map(|sets| Fix::GoBack { number, sets })
        .collect();
    Error::blocked_by(
        format!("the shared {sets} {names} that {label} included at {deploy} changed since"),
        Blocker::with_fixes(
            format!("Shared {sets} changed since {deploy}"),
            format!(
                "{names} changed since {deploy}. Putting {it} back as {was} changes {it} for \
                 every profile that includes {it}. Copying {it} in as {was} {copied} instead, \
                 and keeping {it} as {is} goes back for everything else."
            ),
            fixes,
        ),
    )
}
