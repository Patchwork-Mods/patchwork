use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::compat::GameVersions;
use crate::deploy::RootId;
use crate::relpath::RelPath;

pub const GAME_SCHEMA: u32 = 1;

/// A game definition: which sources a game's mods come from and how they're
/// deployed. These are plain data and are loaded at runtime from registries,
/// so supporting a new game doesn't need a new patchwork build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameDef {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A URL, or a path to an image file. Relative paths are resolved against
    /// the definition file's directory when the registry is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Left out of game lists, for builtin games a user doesn't play.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Per-source settings, e.g. `[sources.thunderstore] community = "..."`.
    #[serde(default)]
    pub sources: BTreeMap<String, toml::Table>,
    pub deployer: DeployerSpec,
    /// How files get from the store into the game directory.
    #[serde(default)]
    pub link: LinkMethod,
    #[serde(default, skip_serializing_if = "Detect::is_empty")]
    pub detect: Detect,
    #[serde(default, skip_serializing_if = "Versions::is_empty")]
    pub versions: Versions,
    /// What each profile picks about the game it runs, such as which mod
    /// loader, rather than it being read from the install.
    #[serde(default, skip_serializing_if = "EnvDefs::is_empty")]
    pub env: EnvDefs,
    /// Folders besides its own that the game reads mods' files from.
    #[serde(default, skip_serializing_if = "RootDefs::is_empty")]
    pub roots: RootDefs,
    /// Ways to start the game besides the usual one.
    #[serde(default, skip_serializing_if = "LaunchDefs::is_empty")]
    pub launch: LaunchDefs,
    /// Whether mods' FOMOD installers are used, and where what they install
    /// goes. Without it, mods with one are deployed as they're packed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fomod: Option<FomodDef>,
    /// Where tools the user runs on their own (ones that generate LOD,
    /// animations or patches, say) write files that should be kept as a
    /// mod. Without it, nothing is captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureDef>,
    /// Whether each profile can have a folder of its own to be deployed
    /// to (an instance) rather than sharing the game's. Without it, every
    /// profile is deployed to the game's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<InstancesDef>,
    /// What sets up the game itself in a profile's instance, so patchwork
    /// can start it there. Without it, an instance only has mods in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision: Option<ProvisionSpec>,
}

/// What a profile picks about the game it runs, by axis id. Sources get
/// the picks (see [`crate::environment::GameEnv`]) so they can find files
/// that work with them, and mods that say they don't are warned about.
///
/// ```toml
/// [env.loader]
/// name = "Mod loader"
/// # Leave it out to allow any value.
/// choices = ["fabric", "quilt", "forge", "neoforge"]
/// # What a profile that hasn't picked gets.
/// default = "fabric"
///
/// # `version` is the game's own version, for games where the profile
/// # picks it. It takes the place of one read from the install.
/// [env.version]
/// name = "Minecraft version"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, EnvAxis>",
    into = "BTreeMap<String, EnvAxis>"
)]
pub struct EnvDefs(BTreeMap<String, EnvAxis>);

impl EnvDefs {
    /// The axis that's the game's own version.
    pub const VERSION: &str = "version";

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&EnvAxis> {
        self.0.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &EnvAxis)> {
        self.0.iter().map(|(id, axis)| (id.as_str(), axis))
    }

    /// What a profile whose own picks are `picked` runs on: each axis's
    /// pick, or its default. Picks for axes the game doesn't have are left
    /// out.
    pub fn picks(&self, picked: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        self.iter()
            .filter_map(|(axis, def)| {
                let value = picked.get(axis).or(def.default.as_ref())?;
                Some((axis.to_owned(), value.clone()))
            })
            .collect()
    }

    /// Checks that `value` can be picked for `axis`, and gives it as the
    /// axis spells it.
    pub fn check(&self, axis: &str, value: &str) -> Result<String, Error> {
        let def = self.get(axis).ok_or_else(|| {
            let ids: Vec<&str> = self.0.keys().map(String::as_str).collect();
            Error::invalid(match ids.is_empty() {
                true => format!("there's nothing to pick about this game, so not `{axis}`"),
                false => format!("`{axis}` isn't one of {}", ids.join(", ")),
            })
        })?;
        let value = value.trim();
        if value.is_empty() {
            return Err(Error::invalid(format!("{} can't be empty", def.name)));
        }
        if def.choices.is_empty() {
            return Ok(value.to_owned());
        }
        def.choice(value).map(str::to_owned).ok_or_else(|| {
            Error::invalid(format!(
                "{} can't be `{value}`; pick one of {}",
                def.name,
                def.choices.join(", ")
            ))
        })
    }
}

impl TryFrom<BTreeMap<String, EnvAxis>> for EnvDefs {
    type Error = String;

    fn try_from(axes: BTreeMap<String, EnvAxis>) -> Result<Self, String> {
        for (id, axis) in &axes {
            let valid = !id.is_empty()
                && id.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')
                });
            if !valid {
                return Err(format!(
                    "`{id}` isn't a valid env id; use lowercase letters, digits, `-` and `_`"
                ));
            }
            if axis.choices.iter().any(|c| c.trim().is_empty()) {
                return Err(format!("env `{id}` has an empty choice"));
            }
            if let Some(default) = &axis.default
                && !axis.choices.is_empty()
                && axis.choice(default).is_none()
            {
                return Err(format!(
                    "env `{id}`: the default `{default}` isn't one of its choices"
                ));
            }
        }
        Ok(Self(axes))
    }
}

impl From<EnvDefs> for BTreeMap<String, EnvAxis> {
    fn from(axes: EnvDefs) -> Self {
        axes.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvAxis {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl EnvAxis {
    /// The choice that's `value`, ignoring case.
    fn choice(&self, value: &str) -> Option<&str> {
        self.choices
            .iter()
            .find(|c| c.eq_ignore_ascii_case(value))
            .map(String::as_str)
    }
}

/// Where to look for files that tools wrote into the game's folder, so
/// they can be captured as a mod (see [`crate::capture`]).
///
/// ```toml
/// [capture]
/// # Folders in the game's that tools write to. What's in them when a
/// # profile is deployed is taken to be the game's own.
/// folders = ["Data"]
/// # Files in them never to capture, like logs and caches.
/// ignore = ["**/*.log"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDef {
    pub folders: Vec<RelPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<FilePattern>,
}

/// How profiles get instances: folders of their own, under patchwork's
/// data, that they're deployed to instead of the game's folder. Each
/// profile picks on each machine whether it uses one.
///
/// ```toml
/// [instances]
/// # Whether profiles that haven't picked use one.
/// default = true
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstancesDef {
    #[serde(default)]
    pub default: bool,
}

/// How a game's mods' FOMOD installers (see [`crate::fomod`]) are used.
///
/// ```toml
/// [fomod]
/// # The folder in the game's that installers' destinations are relative
/// # to, and the files their conditions ask about.
/// folder = "Data"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FomodDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<RelPath>,
}

/// A game's roots other than its own folder, by id.
///
/// ```toml
/// [roots.appdata]
/// # Starts with one of the folders in `Folder`. On Linux, a Windows game's
/// # folders are inside the Wine prefix it runs in.
/// path = "{localappdata}/Skyrim Special Edition"
/// # Used instead of `path` for a native build on these systems.
/// linux = "{data}/skyrim"
/// # The only files that may go here, which every root but the game's own
/// # has to say.
/// allow = ["plugins.txt", "loadorder.txt"]
/// # Files among those that the game writes to itself.
/// rewritten = ["plugins.txt"]
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<RootId, RootDef>",
    into = "BTreeMap<RootId, RootDef>"
)]
pub struct RootDefs(BTreeMap<RootId, RootDef>);

impl RootDefs {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &RootId) -> Option<&RootDef> {
        self.0.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RootId, &RootDef)> {
        self.0.iter()
    }
}

impl TryFrom<BTreeMap<RootId, RootDef>> for RootDefs {
    type Error = String;

    fn try_from(roots: BTreeMap<RootId, RootDef>) -> Result<Self, String> {
        for (id, root) in &roots {
            if id.is_game() {
                return Err("the `game` root is the game's own folder and can't be moved".into());
            }
            if root.allow.is_empty() {
                return Err(format!(
                    "root `{id}` has to say which files may go in it, with `allow`"
                ));
            }
            for native in root.linux.iter().chain(&root.macos) {
                if native.folder.is_windows_only() {
                    return Err(format!(
                        "root `{id}`: `{{{}}}` is only for Windows games, so it can't be used for a native build",
                        native.folder
                    ));
                }
            }
        }
        Ok(Self(roots))
    }
}

impl From<RootDefs> for BTreeMap<RootId, RootDef> {
    fn from(roots: RootDefs) -> Self {
        roots.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootDef {
    pub path: RootPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<RootPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<RootPath>,
    pub allow: Vec<FilePattern>,
    /// Files the game writes to itself, like the load order Skyrim keeps.
    /// What it changes them to doesn't count as the user's: deploying again
    /// replaces it, and undeploying puts back the file from before the
    /// first deploy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewritten: Vec<FilePattern>,
}

/// Where a root is: a [`Folder`] and, usually, a path inside it, written
/// like `{documents}/My Games/Skyrim Special Edition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RootPath {
    pub folder: Folder,
    pub path: Option<RelPath>,
}

impl TryFrom<String> for RootPath {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        let bad = || {
            format!(
                "`{s}` has to start with one of {}",
                Folder::ALL.map(|f| format!("{{{f}}}")).join(", ")
            )
        };
        let (folder, rest) = s
            .strip_prefix('{')
            .and_then(|s| s.split_once('}'))
            .ok_or_else(bad)?;
        let folder = Folder::ALL
            .into_iter()
            .find(|f| f.name() == folder)
            .ok_or_else(bad)?;
        let path = match rest {
            "" => None,
            rest => {
                let rest = rest
                    .strip_prefix(['/', '\\'])
                    .ok_or_else(|| format!("`{s}` needs a `/` after `{{{folder}}}`"))?;
                Some(RelPath::new(rest)?)
            }
        };
        Ok(Self { folder, path })
    }
}

impl From<RootPath> for String {
    fn from(p: RootPath) -> String {
        match p.path {
            Some(path) => format!("{{{}}}/{path}", p.folder),
            None => format!("{{{}}}", p.folder),
        }
    }
}

/// The folders a [`RootPath`] can start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    /// The user's home folder.
    Home,
    /// Their Documents folder, where many games keep saves and settings.
    Documents,
    /// `%APPDATA%`: `AppData/Roaming`.
    AppData,
    /// `%LOCALAPPDATA%`: `AppData/Local`.
    LocalAppData,
    /// Where programs keep their data: `~/.local/share` on Linux,
    /// `~/Library/Application Support` on macOS, and `%APPDATA%` on Windows.
    Data,
    /// Where programs keep their settings: `~/.config` on Linux, and the
    /// same as `Data` elsewhere.
    Config,
}

impl Folder {
    pub const ALL: [Folder; 6] = [
        Folder::Home,
        Folder::Documents,
        Folder::AppData,
        Folder::LocalAppData,
        Folder::Data,
        Folder::Config,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Folder::Home => "home",
            Folder::Documents => "documents",
            Folder::AppData => "appdata",
            Folder::LocalAppData => "localappdata",
            Folder::Data => "data",
            Folder::Config => "config",
        }
    }

    /// Whether only Windows has it, so it means nothing to a native build
    /// on other systems.
    pub fn is_windows_only(self) -> bool {
        matches!(self, Folder::AppData | Folder::LocalAppData)
    }
}

impl std::fmt::Display for Folder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The ways a game can be started, by id. There's always `game`, which
/// starts it the way it usually starts: through Steam if it's installed
/// from there, and otherwise with the first of its `detect.exe` that's
/// there. A `[launch.game]` entry changes how that one starts.
///
/// ```toml
/// [launch.skse]
/// name = "Skyrim with SKSE"
/// # Relative to the game's folder. Only offered when it's there, which for
/// # a loader a mod installs means once that mod is deployed.
/// exe = "skse64_loader.exe"
/// args = ["-forcesteamloader"]
/// env = { DXVK_HUD = "fps" }
/// # DLLs the game's folder has its own copies of, for Wine to load instead
/// # of its own. Patchwork adds the usual ones mods replace (`winhttp`,
/// # `dxgi`, ...) by itself when it deploys them next to the exe.
/// dll_overrides = ["d3dcompiler_47"]
/// # Used when nothing else is asked for, if it's available.
/// default = true
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, LaunchDef>",
    into = "BTreeMap<String, LaunchDef>"
)]
pub struct LaunchDefs(BTreeMap<String, LaunchDef>);

impl LaunchDefs {
    /// The id of the usual way to start a game.
    pub const GAME: &str = "game";

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&LaunchDef> {
        self.0.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &LaunchDef)> {
        self.0.iter().map(|(id, def)| (id.as_str(), def))
    }
}

impl TryFrom<BTreeMap<String, LaunchDef>> for LaunchDefs {
    type Error = String;

    fn try_from(launch: BTreeMap<String, LaunchDef>) -> Result<Self, String> {
        for (id, def) in &launch {
            let valid = !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid {
                return Err(format!(
                    "`{id}` isn't a valid launch id; use lowercase letters, digits and `-`"
                ));
            }
            if let Some(dll) = def
                .dll_overrides
                .iter()
                .find(|d| d.is_empty() || d.contains(['=', ';', ',', '/', '\\']))
            {
                return Err(format!(
                    "launch `{id}`: `{dll}` isn't a DLL name; leave out `.dll` and `=n,b`"
                ));
            }
        }
        let defaults: Vec<&str> = launch
            .iter()
            .filter(|(_, d)| d.default)
            .map(|(id, _)| id.as_str())
            .collect();
        if defaults.len() > 1 {
            return Err(format!(
                "only one way to launch can be the default, not {}",
                defaults.join(" and ")
            ));
        }
        Ok(Self(launch))
    }
}

impl From<LaunchDefs> for BTreeMap<String, LaunchDef> {
    fn from(launch: LaunchDefs) -> Self {
        launch.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What to run instead of the game's usual exe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<RelPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Wine DLL overrides, by name without `.dll`, set to native first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dll_overrides: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

/// How a game's versions matter to its mods.
///
/// ```toml
/// [versions]
/// # Only mods with files like these care which version they're on. Leave
/// # it out if they all do.
/// files = ["**/SKSE/Plugins/*.dll"]
/// # Words in what a source says about a file that mark which versions
/// # it's for, for sources that don't say in so many words.
/// labels = { AE = ">=1.6", "1.5.97" = "1.5.97" }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FilePattern>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, GameVersions>,
}

impl Versions {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A glob matched against paths inside a mod, ignoring case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FilePattern(String);

impl FilePattern {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FilePattern {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        globset::Glob::new(&s).map_err(|e| format!("`{s}` isn't a valid pattern: {e}"))?;
        Ok(FilePattern(s))
    }
}

impl From<FilePattern> for String {
    fn from(p: FilePattern) -> String {
        p.0
    }
}

/// Hints for finding where a game is installed. They don't affect deploying.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Steam app ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steam: Vec<u32>,
    /// The install folder's name, e.g. under Steam's `steamapps/common`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Executables, any of which marks a folder as this game's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exe: Vec<String>,
    /// Ways to find out which version of the game is installed, tried in
    /// order until one finds it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version: Vec<VersionProbe>,
}

/// Somewhere a game's version can be read from, relative to its install.
///
/// ```toml
/// [[detect.version]]
/// kind = "exe"               # the file version in a Windows executable
/// file = "SkyrimSE.exe"
/// parts = 3                  # 1.6.1170.0 -> 1.6.1170
///
/// [[detect.version]]
/// kind = "json"              # a string in a JSON file
/// file = "version.json"
/// key = "game.version"       # dots separate nested keys
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum VersionProbe {
    Exe {
        file: RelPath,
        /// How many of the version's four numbers to keep.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parts: Option<usize>,
    },
    Json {
        file: RelPath,
        key: String,
    },
}

impl Detect {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployerSpec {
    pub kind: String,
    /// Everything else in the table, handed to the deployer to interpret.
    #[serde(flatten)]
    pub config: toml::Table,
}

/// Which [`Provisioner`](crate::provision::Provisioner) sets up the game.
///
/// ```toml
/// [provision]
/// kind = "minecraft"
/// # Anything else is for the provisioner.
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvisionSpec {
    pub kind: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMethod {
    /// The safe default: games that rewrite their own files can't corrupt the
    /// store.
    #[default]
    Copy,
    /// Falls back to copying if the store and game are on different
    /// filesystems.
    Hardlink,
    Symlink,
}

/// Game ids name files and appear in profile references (`<game>/<name>`),
/// so they're kept to lowercase letters, digits and `-`.
pub fn validate_game_id(id: &str) -> Result<(), Error> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "`{id}` isn't a valid game id; use lowercase letters, digits and `-`"
        )))
    }
}
