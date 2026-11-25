use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Error;
use crate::compat::GameVersions;
use crate::config::Edits;
use crate::deploy::Dest;
use crate::edits::ModEdits;
use crate::ext::ExtTables;
use crate::list::ListEntry;

/// Schema 2 added `id` and `name`; profiles used to be identified by their
/// directory, `<game>/<name>`. Schema 3 moved the plugin load order
/// (`[[plugin]]`) and mods' FOMOD picks (`[[mod.fomod]]`) into extensions'
/// state, so builds that would drop extensions' state don't open profiles
/// that have it. Schema 4 added `[[include]]`s of shared sets; a profile
/// that doesn't include one is still saved as 3 (see [`Profile::saved_schema`]).
/// Schema 5 added optional folders ([`Folder::optional`]), whose mods are
/// stored off and turned on by each person's opt-ins, which older builds
/// would publish for everyone; a profile with none is saved as before.
pub const PROFILE_SCHEMA: u32 = 5;

/// The schema a profile that includes sets, but has no optional folders,
/// is saved as.
pub const SCHEMA_WITHOUT_OPTIONAL: u32 = 4;

/// The schema a profile with no includes is saved as, so builds from
/// before sets still open it.
pub const SCHEMA_WITHOUT_INCLUDES: u32 = 3;

/// Where schema 2's plugin load order went: the Bethesda extension's
/// `plugins` list.
pub const LEGACY_PLUGINS: (&str, &str) = ("bethesda", "plugins");

/// Where schema 2's FOMOD picks went: the `fomod` extension's state for
/// the mod, as `picks`.
pub const LEGACY_FOMOD: (&str, &str) = ("fomod", "picks");

/// What the user wants installed for a game. Meant to be readable and
/// hand-editable; the exact versions that were resolved live in the lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ProfileFile")]
pub struct Profile {
    pub schema: u32,
    /// Stays the same wherever the profile goes, so it can be recognized
    /// when it's shared even if it's been renamed.
    pub id: Uuid,
    pub game: String,
    pub name: String,
    /// Mods with any of these tags (matched ignoring case) are tucked away
    /// at the bottom of a frontend's mod list. It's only about how the list
    /// is shown: hidden mods are still installed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_tags: Vec<String>,
    /// What the profile picks for the game's `[env]` axes (see
    /// [`crate::game::EnvDefs`]), like `loader = "fabric"`. Axes left out
    /// get the game's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Groups of mods, for showing them together and turning them on or
    /// off at once; a mod's [`ProfileMod::folder`] says which it's in. Each
    /// folder's mods are kept next to each other in `mods` (see
    /// [`Profile::group_folders`]), so a folder has a place in the load
    /// order like a mod does.
    #[serde(default, rename = "folder", skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<Folder>,
    /// The shared sets (see [`crate::set`]) the profile includes. As
    /// loaded, their mods are in `mods` as linked folders; see
    /// [`crate::set::expand`].
    #[serde(default, rename = "include", skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<Include>,
    /// In load order: later mods win when two provide the same file.
    #[serde(default, rename = "mod")]
    pub mods: Vec<ProfileMod>,
    /// What extensions keep for the profile, by extension id; see
    /// [`crate::ext`]. Kept as it is for extensions this build doesn't
    /// have.
    #[serde(default, skip_serializing_if = "ExtTables::is_empty")]
    pub ext: ExtTables,
    /// The settings changed in config files, by where the file is
    /// deployed; see [`crate::config`]. Only what was changed is kept, so
    /// everything else stays as the file has it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<Dest, Edits>,
    /// As loaded, what of `config` is the personal layer's, and what of
    /// the profile's own that hides (see [`crate::set::merge_config`]).
    #[serde(skip)]
    pub personal_config: PersonalConfig,
}

impl Profile {
    pub fn new(game: &str, name: &str) -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            id: Uuid::new_v4(),
            game: game.to_owned(),
            name: name.to_owned(),
            hidden_tags: Vec::new(),
            env: BTreeMap::new(),
            folders: Vec::new(),
            includes: Vec::new(),
            mods: Vec::new(),
            ext: ExtTables::new(),
            config: BTreeMap::new(),
            personal_config: PersonalConfig::default(),
        }
    }

    pub fn reference(&self) -> ProfileRef {
        ProfileRef { uuid: self.id }
    }

    /// `<game>/<name>`, which is how users refer to the profile.
    pub fn label(&self) -> String {
        format!("{}/{}", self.game, self.name)
    }

    pub fn find(&self, name: &str) -> Option<&ProfileMod> {
        self.mods.iter().find(|m| m.name == name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut ProfileMod> {
        self.mods.iter_mut().find(|m| m.name == name)
    }

    /// The schema the profile is saved as: [`PROFILE_SCHEMA`] if it has
    /// an optional folder, otherwise the one from before there were any if
    /// it includes a set, otherwise the one from before there were sets.
    pub fn saved_schema(&self) -> u32 {
        if self.folders.iter().any(|f| f.optional) {
            PROFILE_SCHEMA
        } else if self.includes.is_empty() {
            SCHEMA_WITHOUT_INCLUDES
        } else {
            SCHEMA_WITHOUT_OPTIONAL
        }
    }

    /// The optional folder (see [`Folder::optional`]) mod `m` is in, if
    /// it's in one. Only the profile's own folders can be.
    pub fn optional_folder(&self, m: &ProfileMod) -> Option<&Folder> {
        let id = m.folder?;
        self.folders.iter().find(|f| f.id == id && f.optional)
    }

    /// The profile's own folders, then the linked folders of the sets it
    /// includes (those that could be found).
    pub fn all_folders(&self) -> impl Iterator<Item = &Folder> {
        self.folders.iter().chain(
            self.includes
                .iter()
                .filter_map(|i| Some(&i.linked.as_ref()?.folder)),
        )
    }

    /// The folders to show: [`Profile::all_folders`], but not the
    /// personal layer while it has no mods (only settings, say).
    pub fn shown_folders(&self) -> impl Iterator<Item = &Folder> {
        self.all_folders()
            .filter(|f| f.id != self.id || self.mods.iter().any(|m| m.folder == Some(self.id)))
    }

    /// The folder `query` names, by name (ignoring case) or by id; a
    /// linked one too.
    pub fn find_folder(&self, query: &str) -> Result<&Folder, Error> {
        self.all_folders()
            .find(|f| f.name.eq_ignore_ascii_case(query) || f.id.to_string() == query)
            .ok_or_else(|| no_such_folder(&self.label(), query))
    }

    /// The folder mod `m` is shown in: `None` if it isn't in one, or if
    /// the profile doesn't have the folder it names (see [`ProfileMod::folder`]).
    pub fn folder_of(&self, m: &ProfileMod) -> Option<&Folder> {
        let id = m.folder?;
        self.all_folders().find(|f| f.id == id)
    }

    /// The include of the set whose linked folder is `id`, if it's one.
    pub fn include(&self, id: Uuid) -> Option<&Include> {
        self.includes.iter().find(|i| i.set == id)
    }

    /// Whether `m` comes from a set the profile includes, rather than
    /// being the profile's own.
    pub fn is_linked(&self, m: &ProfileMod) -> bool {
        m.folder.is_some_and(|id| self.include(id).is_some())
    }

    /// The include of the profile's personal layer (see
    /// [`crate::personal`]), if it has one: the private set whose id is
    /// the profile's own.
    pub fn personal(&self) -> Option<&Include> {
        self.include(self.id)
    }

    /// Whether `m` is in the profile's personal layer, or a set it
    /// includes personally, so it's only on this person's computers.
    pub fn is_personal(&self, m: &ProfileMod) -> bool {
        m.folder
            .is_some_and(|id| self.include(id).is_some_and(|i| i.personal))
    }

    /// Whether anything in the profile is only on this person's computers:
    /// a personal layer, or a set included personally.
    pub fn has_personal(&self) -> bool {
        self.includes.iter().any(|i| i.personal)
    }

    /// The includes of shared sets, leaving out the personal layer. Sets
    /// included personally are among them.
    pub fn shared_includes(&self) -> impl Iterator<Item = &Include> {
        self.includes.iter().filter(|i| i.set != self.id)
    }

    /// Puts each folder's mods next to each other, where the folder's
    /// first mod is, keeping their order otherwise. Hand edits, syncs and
    /// frontends' reorders can leave them apart.
    pub fn group_folders(&mut self) {
        if !self.mods.iter().any(|m| m.folder.is_some()) {
            return;
        }
        let mut groups: Vec<(Option<Uuid>, Vec<ProfileMod>)> = Vec::new();
        for m in self.mods.drain(..) {
            match m.folder {
                Some(id) => match groups.iter_mut().find(|(f, _)| *f == Some(id)) {
                    Some((_, group)) => group.push(m),
                    None => groups.push((Some(id), vec![m])),
                },
                None => groups.push((None, vec![m])),
            }
        }
        self.mods = groups.into_iter().flat_map(|(_, group)| group).collect();
    }

    /// What each replaced dependency (see [`ProfileMod::replaces`]) is
    /// replaced with, both as `source:id`. If two mods replace the same
    /// one, the first in the profile wins.
    pub fn replacements(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for m in &self.mods {
            let key = crate::lock::mod_key(&m.source, &m.id);
            for replaced in &m.replaces {
                if *replaced != key {
                    map.entry(replaced.clone()).or_insert_with(|| key.clone());
                }
            }
        }
        map
    }
}

/// A profile as saved, by any schema: [`Profile`] as it is now, along with
/// what older schemas had that has moved.
#[derive(Deserialize)]
struct ProfileFile {
    schema: u32,
    id: Uuid,
    game: String,
    name: String,
    #[serde(default)]
    hidden_tags: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, rename = "folder")]
    folders: Vec<Folder>,
    #[serde(default, rename = "include")]
    includes: Vec<Include>,
    #[serde(default, rename = "mod")]
    mods: Vec<ProfileMod>,
    #[serde(default)]
    ext: ExtTables,
    #[serde(default)]
    config: BTreeMap<Dest, Edits>,
    /// Schema 2's plugin load order.
    #[serde(default, rename = "plugin")]
    plugins: Vec<ListEntry>,
}

impl From<ProfileFile> for Profile {
    fn from(file: ProfileFile) -> Self {
        let mut ext = file.ext;
        upgrade_plugins(&mut ext, file.plugins);
        let mut profile = Self {
            schema: file.schema,
            id: file.id,
            game: file.game,
            name: file.name,
            hidden_tags: file.hidden_tags,
            env: file.env,
            folders: file.folders,
            includes: file.includes,
            mods: file.mods,
            ext,
            config: file.config,
            personal_config: PersonalConfig::default(),
        };
        profile.group_folders();
        profile
    }
}

/// The settings a profile's personal layer gives as loaded, so saving
/// can tell them from the profile's own; see [`Profile::personal_config`].
#[derive(Debug, Clone, Default)]
pub struct PersonalConfig {
    /// The settings the personal layer (and the sets it includes) give.
    pub given: BTreeMap<Dest, Edits>,
    /// The profile's own settings those hide, which are still its own.
    pub hidden: BTreeMap<Dest, Edits>,
}

/// A group of a profile's mods; see [`Profile::folders`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Stays the same when it's renamed, so syncs agree on which folder a
    /// mod is in.
    pub id: Uuid,
    /// Unique within the profile, ignoring case.
    pub name: String,
    /// Whether it's optional in a synced profile: everyone it's synced
    /// with gets its mods, and each person opts in for themselves (see
    /// [`crate::personal::OptIn`]). Its mods are stored, and published,
    /// as off; as loaded, they're on as the person's opt-in has them.
    /// Only the profile's own folders can be.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl Folder {
    pub fn new(name: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            optional: false,
        }
    }
}

/// A shared set a profile includes; see [`Profile::includes`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Include {
    /// The set's id.
    pub set: Uuid,
    /// The profile's own mod the set's mods come after; `None` for before
    /// all of them. One that names a mod the profile doesn't have puts
    /// them last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The set's mods (by their names in the set) that are off in this
    /// profile. It's the only thing a profile changes about a set for
    /// itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
    /// The set as it was loaded, `None` if it couldn't be found. Never
    /// saved; see [`crate::set::expand`].
    #[serde(skip)]
    pub linked: Option<Linked>,
    /// Whether it's part of the profile's personal layer (see
    /// [`crate::personal`]): the layer's own include, or a set included
    /// only on this person's computers. Those are kept with the personal
    /// layer rather than in the profile's file, so this is never saved
    /// there.
    #[serde(skip)]
    pub personal: bool,
}

impl Include {
    pub fn new(set: Uuid) -> Self {
        Self {
            set,
            after: None,
            disabled: Vec::new(),
            linked: None,
            personal: false,
        }
    }
}

/// What expanding an [`Include`] needs to remember to split the profile
/// back up when it's saved. Deploy history keeps it too, as the set was
/// at the deploy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Linked {
    /// The set's linked folder: its id and name.
    pub folder: Folder,
    /// The set's mods as they were loaded, to tell whether it's changed.
    #[serde(default, rename = "mod")]
    pub(crate) mods: Vec<ProfileMod>,
    /// The set's config edits as they were loaded, which the profile's own
    /// are told apart from. Deploy history doesn't keep them, as it doesn't
    /// keep the profile's.
    #[serde(skip)]
    pub(crate) config: crate::config::FileEdits,
    /// The set's mods left out because the profile has the same mod, by
    /// where they are in the set, so saving keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shadowed: Vec<(usize, ProfileMod)>,
    /// Mods shown under another name because the profile already uses
    /// theirs, as (shown, in the set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) renamed: Vec<(String, String)>,
}

/// Puts schema 2's plugin load order where it's kept now (see
/// [`LEGACY_PLUGINS`]), unless something's already there.
pub fn upgrade_plugins(ext: &mut ExtTables, plugins: Vec<ListEntry>) {
    if plugins.is_empty() {
        return;
    }
    let (id, list) = LEGACY_PLUGINS;
    let table = ext.entry(id.to_owned()).or_insert_with(|| {
        toml::Table::from_iter([(crate::ext::VERSION_KEY.to_owned(), toml::Value::Integer(1))])
    });
    if !table.contains_key(list) {
        let entries = toml::Value::try_from(&plugins).expect("entries serialize");
        table.insert(list.to_owned(), entries);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "ProfileModFile")]
pub struct ProfileMod {
    /// Unique within the profile; how the user refers to the mod.
    pub name: String,
    pub source: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For a mod pinned to a `version`, the newest version the user already
    /// knows is out, from when they pinned it or dismissed the note about
    /// it, so only a version newer than that is pointed out. See
    /// [`crate::Patchwork::known_update`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Mods with a higher priority load later, so they win over ones with a
    /// lower one when two provide the same file, wherever they are in the
    /// profile. Mods with the same priority go by their order in it. See
    /// [`crate::order::load_order`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i32,
    /// Which versions of the game the mod works with, in place of whatever
    /// its source says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<GameVersions>,
    /// What extensions keep for the mod, like [`Profile::ext`].
    #[serde(default, skip_serializing_if = "ExtTables::is_empty")]
    pub ext: ExtTables,
    /// Dependencies (as `source:id`) this mod stands in for, like a fork of
    /// one: mods that need them get this mod instead, and they aren't
    /// downloaded. See [`Profile::replacements`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaces: Vec<String>,
    /// Soft dependencies (see [`crate::source::SoftDependency::ident`])
    /// the user doesn't want to hear about for this mod, because they've
    /// got them some other way or don't want them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignores: Vec<String>,
    /// What the user has changed about how the mod is shown, like its name
    /// or picture. See [`crate::edits`].
    #[serde(default, skip_serializing_if = "ModEdits::is_empty")]
    pub edits: ModEdits,
    /// The [`Folder`] it's in, by id. One the profile doesn't have (removed
    /// on another machine while this one moved the mod into it, say) is
    /// kept, since the folder may still be on its way, but the mod is
    /// shown as not in a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Uuid>,
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// A mod as saved, by any schema, like [`ProfileFile`].
#[derive(Deserialize)]
struct ProfileModFile {
    name: String,
    source: String,
    id: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    seen: Option<String>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    game_versions: Option<GameVersions>,
    #[serde(default)]
    ext: ExtTables,
    #[serde(default)]
    replaces: Vec<String>,
    #[serde(default)]
    ignores: Vec<String>,
    #[serde(default)]
    edits: ModEdits,
    #[serde(default)]
    folder: Option<Uuid>,
    /// Schema 2's FOMOD picks.
    #[serde(default)]
    fomod: Option<toml::Value>,
}

impl From<ProfileModFile> for ProfileMod {
    fn from(file: ProfileModFile) -> Self {
        let mut ext = file.ext;
        if let Some(picks) = file.fomod {
            let (id, key) = LEGACY_FOMOD;
            let table = ext.entry(id.to_owned()).or_insert_with(|| {
                toml::Table::from_iter([(
                    crate::ext::VERSION_KEY.to_owned(),
                    toml::Value::Integer(1),
                )])
            });
            table.entry(key).or_insert(picks);
        }
        Self {
            name: file.name,
            source: file.source,
            id: file.id,
            version: file.version,
            seen: file.seen,
            enabled: file.enabled,
            priority: file.priority,
            game_versions: file.game_versions,
            ext,
            replaces: file.replaces,
            ignores: file.ignores,
            edits: file.edits,
            folder: file.folder,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

/// Identifies a profile on this machine. Users refer to profiles as
/// `<game>/<name>`; see `Patchwork::find_profile`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileRef {
    pub uuid: Uuid,
}

impl fmt::Display for ProfileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.uuid)
    }
}

impl FromStr for ProfileRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = Uuid::from_str(s).map(|u| ProfileRef { uuid: u })?;
        Ok(val)
    }
}

/// Checks a game id or profile name, which must not contain `/` so that
/// `<game>/<name>` is unambiguous.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "`{name}` isn't a valid profile name; use letters, digits, `-`, `_` or `.`"
        )))
    }
}

pub(crate) fn no_such_mod(profile: &str, name: &str) -> Error {
    Error::invalid(format!("{profile} has no mod named `{name}`"))
}

pub(crate) fn no_such_folder(profile: &str, name: &str) -> Error {
    Error::invalid(format!("{profile} has no folder named `{name}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_2s_fomod_picks_move_into_the_fomod_extension() {
        let profile: Profile = toml::from_str(
            r#"
            schema = 2
            id = "6c1d0f4e-6f53-4b8a-9d36-2b7e0c7f6a11"
            game = "skyrim-se"
            name = "default"

            [[mod]]
            name = "SkyUI"
            source = "nexus"
            id = "3863"

            [[mod.fomod]]
            step = "Main"
            group = "Version"
            options = ["SE"]
            "#,
        )
        .unwrap();
        let state = &profile.mods[0].ext["fomod"];
        assert_eq!(state["v"].as_integer(), Some(1));
        assert_eq!(state["picks"][0]["group"].as_str(), Some("Version"));
        assert_eq!(state["picks"][0]["options"][0].as_str(), Some("SE"));
        let saved = toml::to_string(&profile).unwrap();
        assert!(!saved.contains("[[mod.fomod]]"), "{saved}");
        assert!(saved.contains("[[mod.ext.fomod.picks]]"), "{saved}");
    }

    #[test]
    fn a_folders_mods_are_put_together_on_load() {
        let profile: Profile = toml::from_str(
            r#"
            schema = 3
            id = "6c1d0f4e-6f53-4b8a-9d36-2b7e0c7f6a11"
            game = "my-game"
            name = "default"

            [[folder]]
            id = "0b6d3c4e-6f53-4b8a-9d36-2b7e0c7f6a11"
            name = "QOL"

            [[mod]]
            name = "a"
            source = "local"
            id = "a"

            [[mod]]
            name = "b"
            source = "local"
            id = "b"
            folder = "0b6d3c4e-6f53-4b8a-9d36-2b7e0c7f6a11"

            [[mod]]
            name = "c"
            source = "local"
            id = "c"

            [[mod]]
            name = "d"
            source = "local"
            id = "d"
            folder = "0b6d3c4e-6f53-4b8a-9d36-2b7e0c7f6a11"
            "#,
        )
        .unwrap();
        let names: Vec<&str> = profile.mods.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "d", "c"]);
        assert_eq!(profile.folder_of(&profile.mods[1]).unwrap().name, "QOL");
        assert!(profile.folder_of(&profile.mods[0]).is_none());
        assert_eq!(profile.find_folder("qol").unwrap().name, "QOL");
    }

    #[test]
    fn schema_2s_plugins_move_into_the_bethesda_extension() {
        let profile: Profile = toml::from_str(
            r#"
            schema = 2
            id = "6c1d0f4e-6f53-4b8a-9d36-2b7e0c7f6a11"
            game = "skyrim-se"
            name = "default"

            [[plugin]]
            name = "A.esp"
            enabled = false
            "#,
        )
        .unwrap();
        let state = &profile.ext["bethesda"];
        assert_eq!(state["v"].as_integer(), Some(1));
        assert_eq!(state["plugins"][0]["name"].as_str(), Some("A.esp"));
        assert_eq!(state["plugins"][0]["enabled"].as_bool(), Some(false));
        let saved = toml::to_string(&profile).unwrap();
        assert!(!saved.contains("[[plugin]]"), "{saved}");
    }
}
