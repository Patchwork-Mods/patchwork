//! Finding where games are installed, so the user doesn't have to say.
//!
//! For now that's Steam: every Steam install's libraries are listed in its
//! `libraryfolders.vdf`, and each library has a manifest for every app in it
//! saying which folder it's in. A game is found by the Steam app ids in its
//! definition's [`Detect`](crate::game::Detect).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::game::GameDef;
use crate::vdf;

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
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Steam { library, .. } => write!(f, "Steam library {}", library.display()),
        }
    }
}

/// What's installed on this machine, looked up once so that finding any
/// number of games after that is quick.
#[derive(Debug, Default)]
pub struct Locator {
    /// In the order their libraries were found.
    steam: Vec<SteamApp>,
}

#[derive(Debug)]
struct SteamApp {
    app: u32,
    library: PathBuf,
    path: PathBuf,
}

impl Locator {
    /// Looks in every Steam install on this machine, or only the ones in
    /// `$PATCHWORK_STEAM_PATH` (separated like `$PATH`) if it's set.
    pub fn scan() -> Self {
        let roots = match std::env::var_os("PATCHWORK_STEAM_PATH") {
            Some(list) => std::env::split_paths(&list)
                .filter(|p| !p.as_os_str().is_empty())
                .collect(),
            None => steam_roots(),
        };
        Self::steam(roots)
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
        }
    }

    /// Where `game` looks to be installed. Usually there's one place at
    /// most, but nothing stops a game being in two Steam installs.
    pub fn find(&self, game: &GameDef) -> Vec<Found> {
        self.steam
            .iter()
            .filter(|a| game.detect.steam.contains(&a.app))
            .map(|a| Found {
                path: a.path.clone(),
                origin: Origin::Steam {
                    app: a.app,
                    library: a.library.clone(),
                },
            })
            .collect()
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
