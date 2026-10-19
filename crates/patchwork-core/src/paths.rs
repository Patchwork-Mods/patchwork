use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Where patchwork keeps its state on disk.
///
/// Everything under `config` and `data` is the source of truth; `cache` can be
/// deleted at any time.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    /// Extra registry directories from `$PATCHWORK_REGISTRY_PATH`, loaded on
    /// top of installed registries. Meant for working on definitions without
    /// rebuilding, e.g. pointing at this repo's `registry/`.
    pub extra_registries: Vec<PathBuf>,
}

impl Paths {
    /// Uses `$PATCHWORK_HOME` if set (handy for tests and portable installs),
    /// otherwise the platform's standard directories. `$PATCHWORK_REGISTRY_PATH`
    /// is a list of extra registry directories, separated like `$PATH`.
    pub fn from_env() -> Result<Self> {
        let mut paths = match std::env::var_os("PATCHWORK_HOME") {
            Some(home) => Self::rooted(Path::new(&home)),
            None => {
                let dirs = directories::ProjectDirs::from("", "", "patchwork")
                    .ok_or_else(|| Error::invalid("could not determine the home directory"))?;
                Self {
                    config: dirs.config_dir().to_owned(),
                    data: dirs.data_dir().to_owned(),
                    cache: dirs.cache_dir().to_owned(),
                    extra_registries: Vec::new(),
                }
            }
        };
        if let Some(list) = std::env::var_os("PATCHWORK_REGISTRY_PATH") {
            paths.extra_registries = std::env::split_paths(&list)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
        }
        Ok(paths)
    }

    pub fn rooted(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            extra_registries: Vec::new(),
        }
    }

    /// The user's own registry layer, which overrides every other one.
    pub fn user_registry(&self) -> &Path {
        &self.config
    }

    pub fn registries(&self) -> PathBuf {
        self.data.join("registries")
    }

    pub fn store(&self) -> PathBuf {
        self.data.join("store")
    }

    /// Where artifacts are downloaded to before they go into the store.
    pub fn downloads(&self) -> PathBuf {
        self.cache.join("downloads")
    }

    /// The last version lists fetched for mods, so UIs can show them
    /// without going to the network.
    pub fn known_versions(&self) -> PathBuf {
        self.cache.join("versions")
    }

    pub fn profiles(&self) -> PathBuf {
        self.data.join("profiles")
    }

    pub fn game_installs(&self) -> PathBuf {
        self.data.join("games")
    }

    pub fn deployments(&self) -> PathBuf {
        self.data.join("deployments")
    }

    pub fn backups(&self) -> PathBuf {
        self.data.join("backups")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data.join("state.toml")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data.join(".lock")
    }
}
