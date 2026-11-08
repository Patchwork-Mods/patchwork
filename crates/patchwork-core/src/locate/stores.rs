//! Games installed from GOG and the Epic Games Store, by whichever launcher
//! installed them: GOG Galaxy and the Epic Games Launcher themselves, or
//! Heroic and Legendary, which is how they're usually installed on Linux.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// A store a game can be bought from, besides Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Gog,
    Epic,
}

/// What installed a game from a [`Store`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    /// GOG Galaxy, on Windows.
    Galaxy,
    /// The Epic Games Launcher.
    Epic,
    Heroic,
    Legendary,
}

impl std::fmt::Display for Launcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Launcher::Galaxy => "GOG Galaxy",
            Launcher::Epic => "Epic Games Launcher",
            Launcher::Heroic => "Heroic",
            Launcher::Legendary => "Legendary",
        })
    }
}

/// A game some launcher installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoreInstall {
    pub store: Store,
    /// Its GOG product id or Epic app name.
    pub id: String,
    /// Its name in the store, if the launcher keeps it.
    pub title: Option<String>,
    pub path: PathBuf,
    pub launcher: Launcher,
}

/// Every game installed by a launcher patchwork knows about, looking only
/// where `$PATCHWORK_HEROIC_PATH`, `$PATCHWORK_LEGENDARY_PATH` and
/// `$PATCHWORK_EPIC_PATH` say for any that are set.
pub(super) fn scan() -> Vec<StoreInstall> {
    let mut installs = Vec::new();
    for root in search_roots("PATCHWORK_HEROIC_PATH", heroic_roots) {
        installs.extend(heroic(&root));
    }
    for root in search_roots("PATCHWORK_LEGENDARY_PATH", legendary_roots) {
        installs.extend(legendary(&root, Launcher::Legendary));
    }
    for dir in search_roots("PATCHWORK_EPIC_PATH", epic_manifest_dirs) {
        installs.extend(epic_manifests(&dir));
    }
    installs.extend(galaxy());
    installs
}

/// The folders in `$var` (separated like `$PATH`) if it's set, or
/// `default()`.
fn search_roots(var: &str, default: fn() -> Vec<PathBuf>) -> Vec<PathBuf> {
    match std::env::var_os(var) {
        Some(list) => std::env::split_paths(&list)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => default(),
    }
}

/// The games Heroic installed with the config folder `root`: GOG ones, and
/// Epic ones through the copy of Legendary it comes with.
pub(super) fn heroic(root: &Path) -> Vec<StoreInstall> {
    let mut installs = heroic_gog(root);
    installs.extend(legendary(
        &root.join("legendaryConfig/legendary"),
        Launcher::Heroic,
    ));
    installs
}

/// Heroic's `gog_store/installed.json`: `{"installed": [{"appName": "<id>",
/// "install_path": "...", "is_dlc": false}, ...]}`.
fn heroic_gog(root: &Path) -> Vec<StoreInstall> {
    let Some(json) = read_json(&root.join("gog_store/installed.json")) else {
        return Vec::new();
    };
    let Some(games) = json.get("installed").and_then(Value::as_array) else {
        return Vec::new();
    };
    games
        .iter()
        .filter(|g| !g.get("is_dlc").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|g| {
            let id = g.get("appName")?.as_str()?;
            let path = installed_dir(g.get("install_path")?.as_str()?)?;
            Some(StoreInstall {
                store: Store::Gog,
                id: id.to_owned(),
                title: None,
                path,
                launcher: Launcher::Heroic,
            })
        })
        .collect()
}

/// Legendary's `installed.json` in its config folder `root`: `{"<app
/// name>": {"app_name": ..., "title": ..., "install_path": ..., "is_dlc":
/// false}, ...}`.
pub(super) fn legendary(root: &Path, launcher: Launcher) -> Vec<StoreInstall> {
    let Some(json) = read_json(&root.join("installed.json")) else {
        return Vec::new();
    };
    let Some(games) = json.as_object() else {
        return Vec::new();
    };
    games
        .iter()
        .filter(|(_, g)| !g.get("is_dlc").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|(name, g)| {
            let id = g.get("app_name").and_then(Value::as_str).unwrap_or(name);
            let path = installed_dir(g.get("install_path")?.as_str()?)?;
            Some(StoreInstall {
                store: Store::Epic,
                id: id.to_owned(),
                title: g.get("title").and_then(Value::as_str).map(str::to_owned),
                path,
                launcher,
            })
        })
        .collect()
}

/// The Epic Games Launcher's manifests in `dir`, one `.item` file of JSON
/// for each game (and each DLC) it installed.
pub(super) fn epic_manifests(dir: &Path) -> Vec<StoreInstall> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut installs: Vec<StoreInstall> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("item"))
        })
        .filter_map(|p| {
            let item = read_json(&p)?;
            let text = |key: &str| item.get(key).and_then(Value::as_str);
            let id = text("AppName")?;
            // a DLC's manifest points at its game's folder
            if text("MainGameAppName").is_some_and(|main| main != id)
                || item.get("bIsIncompleteInstall").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            Some(StoreInstall {
                store: Store::Epic,
                id: id.to_owned(),
                title: text("DisplayName").map(str::to_owned),
                path: installed_dir(text("InstallLocation")?)?,
                launcher: Launcher::Epic,
            })
        })
        .collect();
    installs.sort_by(|a, b| a.id.cmp(&b.id));
    installs
}

/// The games GOG Galaxy installed, which it lists in the registry under
/// `GOG.com\Games\<id>`.
fn galaxy() -> Vec<StoreInstall> {
    #[cfg(windows)]
    {
        use windows_registry::LOCAL_MACHINE;
        let Ok(games) = LOCAL_MACHINE.open(r"SOFTWARE\WOW6432Node\GOG.com\Games") else {
            return Vec::new();
        };
        let Ok(ids) = games.keys() else {
            return Vec::new();
        };
        ids.filter_map(|id| {
            let game = games.open(&id).ok()?;
            let path = installed_dir(&game.get_string("path").ok()?)?;
            Some(StoreInstall {
                store: Store::Gog,
                title: game.get_string("gameName").ok(),
                id,
                path,
                launcher: Launcher::Galaxy,
            })
        })
        .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The Wine prefix Heroic runs the game installed at `install` in, from
/// its settings for the game. For a Proton prefix, that's the `pfx` folder
/// in the one Heroic names, as with Steam's.
pub(super) fn heroic_prefix(install: &Path) -> Option<PathBuf> {
    heroic_prefix_in(search_roots("PATCHWORK_HEROIC_PATH", heroic_roots), install)
}

/// [`heroic_prefix`], for the Heroic config folders `roots`.
pub(super) fn heroic_prefix_in(
    roots: impl IntoIterator<Item = PathBuf>,
    install: &Path,
) -> Option<PathBuf> {
    let install = install.canonicalize().ok()?;
    roots.into_iter().find_map(|root| {
        let game = heroic(&root).into_iter().find(|g| g.path == install)?;
        let config = read_json(&root.join(format!("GamesConfig/{}.json", game.id)))?;
        let prefix = config.get(&game.id)?.get("winePrefix")?.as_str()?;
        let prefix = expand_home(prefix);
        [prefix.join("pfx"), prefix]
            .into_iter()
            .find(|p| p.join("drive_c").is_dir())
    })
}

/// Where Heroic keeps its config on this platform: in the usual config
/// folder, or the Flatpak's.
fn heroic_roots() -> Vec<PathBuf> {
    let Some(dirs) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let mut roots = vec![dirs.config_dir().join("heroic")];
    if cfg!(all(unix, not(target_os = "macos"))) {
        roots.push(
            dirs.home_dir()
                .join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
        );
    }
    roots
}

/// Where Legendary keeps its config: `$LEGENDARY_CONFIG_PATH`, or
/// `~/.config/legendary` (on every platform).
fn legendary_roots() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("LEGENDARY_CONFIG_PATH") {
        return vec![PathBuf::from(path)];
    }
    let Some(dirs) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let mut roots = vec![dirs.home_dir().join(".config/legendary")];
    let usual = dirs.config_dir().join("legendary");
    if !roots.contains(&usual) {
        roots.push(usual);
    }
    roots
}

/// Where the Epic Games Launcher keeps its manifests on this platform.
fn epic_manifest_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let data = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        vec![data.join(r"Epic\EpicGamesLauncher\Data\Manifests")]
    }
    #[cfg(target_os = "macos")]
    {
        directories::BaseDirs::new()
            .map(|dirs| {
                vec![
                    dirs.data_dir()
                        .join("Epic/EpicGamesLauncher/Data/Manifests"),
                ]
            })
            .unwrap_or_default()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Vec::new()
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `path` made absolute, if there's a folder there.
fn installed_dir(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let path = expand_home(path).canonicalize().ok()?;
    path.is_dir().then_some(path)
}

/// `path` with a leading `~` for the home folder spelled out, as Heroic
/// sometimes writes it.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}
