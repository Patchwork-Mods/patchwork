//! Finding where games are installed, so the user doesn't have to say.
//!
//! Every Steam install's libraries are listed in its `libraryfolders.vdf`,
//! and each library has a manifest for every app in it saying which folder
//! it's in. Games from GOG and the Epic Games Store are found from what
//! their launchers keep (see [`stores`]). A game is found by the store ids in
//! its definition's [`Detect`](crate::game::Detect), or, for GOG and Epic,
//! by one of its executables being in a folder named like it.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::game::GameDef;
use crate::vdf;

mod stores;

use stores::StoreInstall;
pub use stores::{Launcher, Store};

/// Somewhere a game looks to be installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    pub path: PathBuf,
    pub origin: Origin,
}

/// What found an install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Origin {
    Steam {
        app: u32,
        /// The library folder it's in, which also has the app's Proton
        /// prefix (`steamapps/compatdata/<app>`) on Linux.
        library: PathBuf,
    },
    /// Installed from GOG.
    Gog { id: String, launcher: Launcher },
    /// Installed from the Epic Games Store; `app` is its app name.
    Epic { app: String, launcher: Launcher },
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Steam { library, .. } => write!(f, "Steam library {}", library.display()),
            Origin::Gog { launcher, .. } => match launcher {
                Launcher::Galaxy => f.write_str("GOG Galaxy"),
                other => write!(f, "{other} (GOG)"),
            },
            Origin::Epic { launcher, .. } => match launcher {
                Launcher::Epic => f.write_str("Epic Games Launcher"),
                other => write!(f, "{other} (Epic)"),
            },
        }
    }
}

/// What's installed on this machine, looked up once so that finding any
/// number of games after that is quick.
#[derive(Debug, Default)]
pub struct Locator {
    /// In the order their libraries were found.
    steam: Vec<SteamApp>,
    /// Games from GOG and Epic, in the order their launchers were looked in.
    stores: Vec<StoreInstall>,
}

#[derive(Debug)]
struct SteamApp {
    app: u32,
    library: PathBuf,
    path: PathBuf,
}

impl Locator {
    /// Looks in every Steam install and every GOG and Epic launcher on
    /// this machine. `$PATCHWORK_STEAM_PATH`, `$PATCHWORK_HEROIC_PATH`,
    /// `$PATCHWORK_LEGENDARY_PATH` and `$PATCHWORK_EPIC_PATH` (each
    /// separated like `$PATH`, and empty for none) say where to look
    /// instead.
    pub fn scan() -> Self {
        Self {
            stores: stores::scan(),
            ..Self::steam(steam_search_roots())
        }
    }

    /// Looks in the Steam installs in `roots` (the folders with `steamapps`
    /// in them) and every library they know about.
    pub fn steam(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut libraries = Vec::new();
        for root in roots {
            // the same install is often reachable more than one way, e.g.
            // through the ~/.steam/steam symlink
            let Ok(root) = root.canonicalize() else {
                continue;
            };
            for library in std::iter::once(root.clone()).chain(library_folders(&root)) {
                if let Ok(library) = library.canonicalize()
                    && !libraries.contains(&library)
                {
                    libraries.push(library);
                }
            }
        }
        Self {
            steam: libraries.iter().flat_map(|l| installed_apps(l)).collect(),
            stores: Vec::new(),
        }
    }

    /// Where `game` looks to be installed. Usually there's one place at
    /// most, but nothing stops a game being in two Steam installs, or
    /// bought from more than one store.
    pub fn find(&self, game: &GameDef) -> Vec<Found> {
        let steam = self
            .steam
            .iter()
            .filter(|a| game.detect.steam.contains(&a.app))
            .map(|a| Found {
                path: a.path.clone(),
                origin: Origin::Steam {
                    app: a.app,
                    library: a.library.clone(),
                },
            });
        let stores = self
            .stores
            .iter()
            .filter(|s| is_store_install(game, s))
            .map(|s| Found {
                path: s.path.clone(),
                origin: match s.store {
                    Store::Gog => Origin::Gog {
                        id: s.id.clone(),
                        launcher: s.launcher,
                    },
                    Store::Epic => Origin::Epic {
                        app: s.id.clone(),
                        launcher: s.launcher,
                    },
                },
            });
        let mut found: Vec<Found> = Vec::new();
        // Heroic and Legendary can both know about the same install
        for f in steam.chain(stores) {
            if !found.iter().any(|seen| seen.path == f.path) {
                found.push(f);
            }
        }
        found
    }
}

/// Whether a game a launcher installed is `game`: it has one of the ids
/// `game` lists for its store, or one of `game`'s executables is in its
/// folder and its name or folder is the game's. Most definitions don't
/// list GOG or Epic ids, and checking the name too keeps a game that ships
/// another's executable (Enderal has a SkyrimSE.exe) from passing as it.
fn is_store_install(game: &GameDef, install: &StoreInstall) -> bool {
    let detect = &game.detect;
    let listed = match install.store {
        Store::Gog => detect.gog.iter().any(|id| id.to_string() == install.id),
        Store::Epic => detect.epic.contains(&install.id),
    };
    if listed {
        return true;
    }
    if !detect.exe.iter().any(|exe| install.path.join(exe).exists()) {
        return false;
    }
    let name = squash(&game.name);
    let folder = detect.folder.as_deref().map(squash);
    let named = |text: &str| {
        let text = squash(text);
        // "Fallout 3: Game of the Year Edition" is Fallout 3
        (!name.is_empty() && text.starts_with(&name))
            || folder.as_ref().is_some_and(|f| !f.is_empty() && text == *f)
    };
    install.title.as_deref().is_some_and(named)
        || install
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(named)
}

/// `name` in lower case without anything but letters and digits, so that
/// names spelled a little differently compare equal.
fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The Wine prefix that something other than patchwork made for `game`
/// installed at `install`: the Proton prefix Steam made (see
/// [`proton_prefix`]), or the prefix Heroic runs it in.
pub fn wine_prefix(game: &GameDef, install: &Path) -> Option<PathBuf> {
    proton_prefix(game, install).or_else(|| stores::heroic_prefix(install))
}

/// The Proton prefix Steam made for `game`, if it's installed at `install`
/// in a Steam library and has been run through Proton there. It's the
/// folder with `drive_c` in it.
///
/// Apps installed to the same folder count too, since a game can have more
/// than one (HITMAN 3 and World of Assassination share theirs), and the
/// prefix is the one that was run.
pub fn proton_prefix(game: &GameDef, install: &Path) -> Option<PathBuf> {
    let steamapps = steamapps_of(install)?;
    let sharing = installed_apps(steamapps.parent()?)
        .into_iter()
        .filter(|a| a.path == install)
        .map(|a| a.app);
    game.detect
        .steam
        .iter()
        .copied()
        .chain(sharing)
        .map(|app| steamapps.join(format!("compatdata/{app}/pfx")))
        .find(|pfx| pfx.join("drive_c").is_dir())
}

/// A game Steam installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamInstall {
    pub app: u32,
    /// The library it's in.
    pub library: PathBuf,
}

/// Which Steam app `install` is, if Steam installed `game` there: one of
/// the game's own app ids if Steam has a manifest for it there, or else
/// any app installed to the same folder.
pub fn steam_install(game: &GameDef, install: &Path) -> Option<SteamInstall> {
    let library = steamapps_of(install)?.parent()?.to_owned();
    let apps: Vec<u32> = installed_apps(&library)
        .into_iter()
        .filter(|a| a.path == install)
        .map(|a| a.app)
        .collect();
    let app = game
        .detect
        .steam
        .iter()
        .find(|app| apps.contains(app))
        .or(apps.first())
        .copied()?;
    Some(SteamInstall { app, library })
}

/// The Steam install `library` belongs to: the folder Steam itself is in,
/// which Proton needs to be told about. Looked for the same way as
/// [`Locator::scan`] does.
pub fn steam_client(library: &Path) -> Option<PathBuf> {
    let library = library.canonicalize().ok()?;
    steam_search_roots().into_iter().find_map(|root| {
        let root = root.canonicalize().ok()?;
        let owns = root == library
            || library_folders(&root)
                .iter()
                .any(|l| l.canonicalize().is_ok_and(|l| l == library));
        owns.then_some(root)
    })
}

/// The Proton that last ran the prefix in `compat_data` (the folder with
/// `pfx` in it): the folder with its `proton` script. Proton lists files
/// from its own install in the prefix's `config_info` every time it runs.
pub fn proton_install(compat_data: &Path) -> Option<PathBuf> {
    let info = std::fs::read_to_string(compat_data.join("config_info")).ok()?;
    info.lines().skip(1).find_map(|line| {
        // older Protons keep their files in `dist` rather than `files`
        let (proton, _) = line
            .split_once("/files/")
            .or_else(|| line.split_once("/dist/"))?;
        let proton = PathBuf::from(proton);
        proton.join("proton").is_file().then_some(proton)
    })
}

/// The app id of the Steam Linux Runtime that the Proton in `proton` has to
/// run inside, from its `toolmanifest.vdf`. `None` for older Protons, which
/// run on their own.
pub fn proton_runtime_app(proton: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(proton.join("toolmanifest.vdf")).ok()?;
    vdf::parse(&text)?
        .get("manifest")?
        .get_str("require_tool_appid")?
        .parse()
        .ok()
}

/// Where Steam app `app` is installed, in any library of the Steam install
/// at `client`.
pub fn steam_app_path(client: &Path, app: u32) -> Option<PathBuf> {
    std::iter::once(client.to_owned())
        .chain(library_folders(client))
        .flat_map(|library| installed_apps(&library))
        .find(|a| a.app == app)
        .map(|a| a.path)
}

/// The `steamapps` folder `install` is in, if it's laid out like a game
/// folder in a Steam library: `steamapps/common/<game>`.
fn steamapps_of(install: &Path) -> Option<&Path> {
    let common = install.parent()?;
    let steamapps = common.parent()?;
    (common.file_name()? == "common" && steamapps.file_name()? == "steamapps").then_some(steamapps)
}

/// The Steam installs to look in: the ones in `$PATCHWORK_STEAM_PATH`
/// (separated like `$PATH`) if it's set, or wherever Steam usually is.
fn steam_search_roots() -> Vec<PathBuf> {
    match std::env::var_os("PATCHWORK_STEAM_PATH") {
        Some(list) => std::env::split_paths(&list)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => steam_roots(),
    }
}

/// Where Steam is usually installed on this platform.
fn steam_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
        let registry = [
            (CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
            (
                LOCAL_MACHINE,
                r"SOFTWARE\WOW6432Node\Valve\Steam",
                "InstallPath",
            ),
        ];
        let program_files = std::env::var_os("ProgramFiles(x86)")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        registry
            .into_iter()
            .filter_map(|(hive, key, name)| hive.open(key).ok()?.get_string(name).ok())
            .map(PathBuf::from)
            .chain([program_files.join("Steam")])
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        directories::BaseDirs::new()
            .map(|dirs| vec![dirs.data_dir().join("Steam")])
            .unwrap_or_default()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Some(dirs) = directories::BaseDirs::new() else {
            return Vec::new();
        };
        let home = dirs.home_dir();
        vec![
            dirs.data_dir().join("Steam"),
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".steam/root"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            home.join("snap/steam/common/.local/share/Steam"),
        ]
    }
}

/// The libraries listed in a Steam install's `libraryfolders.vdf`. Newer
/// Steams give each one a block with its `path` in it; older ones just the
/// path.
fn library_folders(root: &Path) -> Vec<PathBuf> {
    let Some(file) = ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"]
        .iter()
        .find_map(|f| std::fs::read_to_string(root.join(f)).ok())
    else {
        return Vec::new();
    };
    let Some(folders) = vdf::parse(&file) else {
        return Vec::new();
    };
    let Some(folders) = folders.get("libraryfolders") else {
        return Vec::new();
    };
    folders
        .entries()
        // other entries, like `contentstatsid`, aren't libraries
        .filter(|(key, _)| key.parse::<u32>().is_ok())
        .filter_map(|(_, value)| value.as_str().or_else(|| value.get_str("path")))
        .map(PathBuf::from)
        .collect()
}

/// The apps with a manifest in `library` whose folders are there, by app
/// id. Steam deletes an app's manifest when it's uninstalled, so these are
/// what's installed.
fn installed_apps(library: &Path) -> Vec<SteamApp> {
    let steamapps = library.join("steamapps");
    let Ok(entries) = std::fs::read_dir(&steamapps) else {
        return Vec::new();
    };
    let mut apps: Vec<SteamApp> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("appmanifest_") && name.ends_with(".acf")
        })
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            let manifest = vdf::parse(&text)?;
            let state = manifest.get("AppState")?;
            let app = state.get_str("appid")?.parse().ok()?;
            let dir = state.get_str("installdir").filter(|d| !d.is_empty())?;
            let path = steamapps.join("common").join(dir);
            path.is_dir().then(|| SteamApp {
                app,
                library: library.to_owned(),
                path,
            })
        })
        .collect();
    apps.sort_by_key(|a| a.app);
    apps
}

#[cfg(test)]
mod tests;
