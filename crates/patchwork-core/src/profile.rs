use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Error;
use crate::compat::GameVersions;
use crate::config::Edits;
use crate::deploy::Dest;
use crate::ext::ExtTables;
use crate::list::ListEntry;

/// Schema 2 added `id` and `name`; profiles used to be identified by their
/// directory, `<game>/<name>`. Schema 3 moved the plugin load order
/// (`[[plugin]]`) and mods' FOMOD picks (`[[mod.fomod]]`) into extensions'
/// state, so builds that would drop extensions' state don't open profiles
/// that have it.
pub const PROFILE_SCHEMA: u32 = 3;

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
            mods: Vec::new(),
            ext: ExtTables::new(),
            config: BTreeMap::new(),
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
        Self {
            schema: file.schema,
            id: file.id,
            game: file.game,
            name: file.name,
            hidden_tags: file.hidden_tags,
            env: file.env,
            mods: file.mods,
            ext,
            config: file.config,
        }
    }
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
