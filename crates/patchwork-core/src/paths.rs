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
    /// Whether secrets can go in the system keychain. Not for installs under
    /// `$PATCHWORK_HOME`, which keep everything in one place (and so that
    /// tests never touch the real keychain).
    pub keychain: bool,
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
                    keychain: true,
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
            keychain: false,
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

    /// The last summaries fetched for mods (name, picture, ...), so UIs can
    /// show added mods without going to the network.
    pub fn known_summaries(&self) -> PathBuf {
        self.cache.join("summaries")
    }

    /// Which store entry each [`crate::source::SourceFile`] that's been
    /// downloaded is. Files the user downloaded themselves can't be fetched
    /// again without them, so this isn't a cache.
    pub fn source_files(&self) -> PathBuf {
        self.data.join("source-files")
    }

    /// Each profile's own files, `profiles/<id>/`: only what describes the
    /// profile and is the same on every machine that has it.
    pub fn profiles(&self) -> PathBuf {
        self.data.join("profiles")
    }

    /// What only this machine knows about each profile, `profile-state/<id>/`:
    /// what was deployed from it here, and when.
    pub fn profile_state(&self) -> PathBuf {
        self.data.join("profile-state")
    }

    /// Where on this machine each local mod was added from, so it can be
    /// read again when it's relocked.
    pub fn local_mods(&self) -> PathBuf {
        self.data.join("local-mods.toml")
    }

    /// What keeping profiles in step with other machines needs: this
    /// machine's key, and what's been synced.
    pub fn sync(&self) -> PathBuf {
        self.data.join("sync")
    }

    pub fn game_installs(&self) -> PathBuf {
        self.data.join("games")
    }

    /// Profiles' own folders to deploy to, `instances/<profile id>/`; see
    /// [`crate::game::InstancesDef`].
    pub fn instances(&self) -> PathBuf {
        self.data.join("instances")
    }

    pub fn deployments(&self) -> PathBuf {
        self.data.join("deployments")
    }

    /// What tools wrote into games' folders, captured as local mods, in
    /// `captured/<profile id>/<mod name>/`. Each is where its mod was added
    /// from, so capturing again updates it.
    pub fn captured(&self) -> PathBuf {
        self.data.join("captured")
    }

    pub fn backups(&self) -> PathBuf {
        self.data.join("backups")
    }

    /// Files deployers built, which deployed files can be links to.
    pub fn generated(&self) -> PathBuf {
        self.data.join("generated")
    }

    /// Where a kind of deployer keeps intermediate results between builds.
    pub fn deployer_cache(&self, kind: &str) -> PathBuf {
        self.cache.join("deployers").join(kind)
    }

    /// What a game printed the last time patchwork started it.
    pub fn launch_log(&self, game: &str) -> PathBuf {
        self.data.join("logs").join(format!("launch-{game}.log"))
    }

    /// What a tool printed the last time patchwork ran one for `game`.
    pub fn tool_log(&self, game: &str) -> PathBuf {
        self.data.join("logs").join(format!("tool-{game}.log"))
    }

    pub fn state_file(&self) -> PathBuf {
        self.data.join("state.toml")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data.join(".lock")
    }
}
