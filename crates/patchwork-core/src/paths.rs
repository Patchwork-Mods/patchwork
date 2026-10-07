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
}

impl Paths {
    /// Uses `$PATCHWORK_HOME` if set (handy for tests and portable installs),
    /// otherwise the platform's standard directories.
    pub fn from_env() -> Result<Self> {
        if let Some(home) = std::env::var_os("PATCHWORK_HOME") {
            return Ok(Self::rooted(Path::new(&home)));
        }
        let dirs = directories::ProjectDirs::from("", "", "patchwork")
            .ok_or_else(|| Error::invalid("could not determine the home directory"))?;
        Ok(Self {
            config: dirs.config_dir().to_owned(),
            data: dirs.data_dir().to_owned(),
            cache: dirs.cache_dir().to_owned(),
        })
    }

    pub fn rooted(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
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
