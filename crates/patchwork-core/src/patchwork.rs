use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::deploy::{self, DeployContext, DeployPlan, DeployState, Deployer, InstalledMod};
use crate::error::IoContext;
use crate::event::{Event, Reporter, Silent};
use crate::fsutil::{self, StateLock};
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, mod_key};
use crate::paths::Paths;
use crate::profile::{PROFILE_SCHEMA, Profile, ProfileMod, ProfileRef};
use crate::registry::Registry;
use crate::source::{ArtifactFormat, ArtifactLocation, ModSpec, Source, SourceContext};
use crate::store::Store;
use crate::{Error, Result};

/// Where a game is installed on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInstall {
    pub path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_profile: Option<String>,
}

pub struct Builder {
    paths: Paths,
    sources: BTreeMap<String, Arc<dyn Source>>,
    deployers: BTreeMap<String, Arc<dyn Deployer>>,
    reporter: Arc<dyn Reporter>,
}

impl Builder {
    pub fn source(mut self, source: impl Source + 'static) -> Self {
        self.sources
            .insert(source.id().to_owned(), Arc::new(source));
        self
    }

    pub fn deployer(mut self, deployer: impl Deployer + 'static) -> Self {
        self.deployers
            .insert(deployer.kind().to_owned(), Arc::new(deployer));
        self
    }

    pub fn reporter(mut self, reporter: impl Reporter + 'static) -> Self {
        self.reporter = Arc::new(reporter);
        self
    }

    /// Takes the state lock and loads the registry.
    pub fn open(self) -> Result<Patchwork> {
        let lock = StateLock::acquire(&self.paths.lock_file())?;
        let registry = Registry::load(&self.paths)?;
        Ok(Patchwork {
            store: Store::new(self.paths.store()),
            registry,
            paths: self.paths,
            sources: self.sources,
            deployers: self.deployers,
            reporter: self.reporter,
            _lock: lock,
        })
    }
}

/// Everything a frontend needs. The CLI is a thin layer over this, and a GUI
/// should be able to be one too.
pub struct Patchwork {
    paths: Paths,
    registry: Registry,
    store: Store,
    sources: BTreeMap<String, Arc<dyn Source>>,
    deployers: BTreeMap<String, Arc<dyn Deployer>>,
    reporter: Arc<dyn Reporter>,
    _lock: StateLock,
}

impl Patchwork {
    pub fn builder(paths: Paths) -> Builder {
        Builder {
            paths,
            sources: BTreeMap::new(),
            deployers: BTreeMap::new(),
            reporter: Arc::new(Silent),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn has_source(&self, id: &str) -> bool {
        self.sources.contains_key(id)
    }

    // Games

    pub fn game_install(&self, game: &str) -> Result<Option<GameInstall>> {
        fsutil::read_toml(&self.install_file(game))
    }

    pub fn set_game_path(&self, game: &str, path: &Path) -> Result<GameInstall> {
        self.registry.game(game)?;
        let path = path.canonicalize().at(path)?;
        if !path.is_dir() {
            return Err(Error::invalid(format!(
                "{} isn't a directory",
                path.display()
            )));
        }
        let state = self.deploy_state(game)?;
        if !state.files.is_empty() && state.root != path {
            return Err(Error::invalid(format!(
                "mods are still deployed to {}; run `patchwork undeploy {game}` first",
                state.root.display()
            )));
        }
        let install = GameInstall { path };
        fsutil::write_toml(&self.install_file(game), &install)?;
        Ok(install)
    }

    fn install_file(&self, game: &str) -> PathBuf {
        self.paths.game_installs().join(format!("{game}.toml"))
    }

    // Profiles

    pub fn current_profile(&self) -> Result<Option<ProfileRef>> {
        let state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        state.current_profile.map(|p| p.parse()).transpose()
    }

    pub fn use_profile(&self, profile: &ProfileRef) -> Result<()> {
        self.load_profile(profile)?;
        let state = State {
            current_profile: Some(profile.to_string()),
        };
        fsutil::write_toml(&self.paths.state_file(), &state)
    }

    pub fn create_profile(&self, profile: &ProfileRef) -> Result<()> {
        self.registry.game(&profile.game)?;
        let path = self.profile_file(profile);
        if path.exists() {
            return Err(Error::invalid(format!("profile {profile} already exists")));
        }
        fsutil::write_toml(&path, &Profile::new(&profile.game))
    }

    pub fn profiles(&self) -> Result<Vec<ProfileRef>> {
        let mut profiles = Vec::new();
        let Ok(games) = fs::read_dir(self.paths.profiles()) else {
            return Ok(profiles);
        };
        for game in games.filter_map(|e| e.ok()) {
            let Ok(names) = fs::read_dir(game.path()) else {
                continue;
            };
            for name in names.filter_map(|e| e.ok()) {
                if name.path().join("profile.toml").is_file() {
                    let r = format!(
                        "{}/{}",
                        game.file_name().to_string_lossy(),
                        name.file_name().to_string_lossy()
                    );
                    profiles.extend(r.parse().ok());
                }
            }
        }
        profiles.sort();
        Ok(profiles)
    }

    pub fn load_profile(&self, profile: &ProfileRef) -> Result<Profile> {
        let path = self.profile_file(profile);
        let loaded: Profile = fsutil::read_toml(&path)?
            .ok_or_else(|| Error::invalid(format!("no profile named {profile}")))?;
        if loaded.schema > PROFILE_SCHEMA {
            return Err(Error::Schema {
                kind: "profile",
                path,
                found: loaded.schema,
                supported: PROFILE_SCHEMA,
            });
        }
        Ok(loaded)
    }

    fn save_profile(&self, profile: &ProfileRef, value: &Profile) -> Result<()> {
        fsutil::write_toml(&self.profile_file(profile), value)
    }

    fn profile_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.paths
            .profiles()
            .join(&profile.game)
            .join(&profile.name)
    }

    fn profile_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("profile.toml")
    }

    fn lock_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("profile.lock")
    }

    // Mods

    /// Turns what the user typed into a mod spec: `source:id`, or a bare path
    /// for the `local` source.
    pub fn parse_spec(&self, spec: &str, version: Option<String>) -> Result<ModSpec> {
        if let Some((source, id)) = spec.split_once(':')
            && self.sources.contains_key(source)
        {
            return Ok(ModSpec {
                source: source.to_owned(),
                id: id.to_owned(),
                version,
            });
        }
        if self.sources.contains_key("local") && Path::new(spec).exists() {
            return Ok(ModSpec {
                source: "local".to_owned(),
                id: spec.to_owned(),
                version,
            });
        }
        match spec.split_once(':') {
            Some((source, _)) => Err(Error::UnknownSource(source.to_owned())),
            None => Err(Error::invalid(format!(
                "`{spec}` isn't a path or a `source:id`"
            ))),
        }
    }

    /// Adds a mod to a profile. It's resolved straight away so mistakes show
    /// up now rather than at deploy time.
    pub async fn add_mod(
        &self,
        profile: &ProfileRef,
        spec: ModSpec,
        name: Option<String>,
        enabled: bool,
    ) -> Result<ProfileMod> {
        let mut loaded = self.load_profile(profile)?;
        let resolved = self.resolve(&loaded.game, &spec).await?;
        let name = name.unwrap_or(resolved.name);
        if loaded.mods.iter().any(|m| m.name == name) {
            return Err(Error::invalid(format!(
                "{profile} already has a mod named `{name}`; pick another with --name"
            )));
        }
        if loaded
            .mods
            .iter()
            .any(|m| m.source == spec.source && m.id == resolved.id)
        {
            return Err(Error::invalid(format!("{profile} already has that mod")));
        }
        let added = ProfileMod {
            name,
            source: spec.source,
            id: resolved.id,
            version: spec.version,
            enabled,
        };
        loaded.mods.push(added.clone());
        self.save_profile(profile, &loaded)?;
        Ok(added)
    }

    pub fn remove_mod(&self, profile: &ProfileRef, name: &str) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let before = loaded.mods.len();
        loaded.mods.retain(|m| m.name != name);
        if loaded.mods.len() == before {
            return Err(no_such_mod(profile, name));
        }
        self.save_profile(profile, &loaded)
    }

    pub fn set_enabled(&self, profile: &ProfileRef, name: &str, enabled: bool) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        loaded
            .find_mut(name)
            .ok_or_else(|| no_such_mod(profile, name))?
            .enabled = enabled;
        self.save_profile(profile, &loaded)
    }

    async fn resolve(&self, game: &str, spec: &ModSpec) -> Result<crate::source::ResolvedMod> {
        let source = self
            .sources
            .get(&spec.source)
            .ok_or_else(|| Error::UnknownSource(spec.source.clone()))?;
        let game = &self.registry.game(game)?.value;
        let ctx = SourceContext {
            game,
            config: game.sources.get(&spec.source),
        };
        source
            .resolve(&ctx, &spec.id, spec.version.as_deref())
            .await
    }

    // Locking

    pub fn load_lock(&self, profile: &ProfileRef) -> Result<Option<Lockfile>> {
        let path = self.lock_file(profile);
        let lock: Option<Lockfile> = fsutil::read_toml(&path)?;
        if let Some(lock) = &lock
            && lock.schema > LOCK_SCHEMA
        {
            return Err(Error::Schema {
                kind: "lockfile",
                path,
                found: lock.schema,
                supported: LOCK_SCHEMA,
            });
        }
        Ok(lock)
    }

    /// Resolves every mod in the profile (enabled or not) and its
    /// dependencies, and makes sure they're all in the store.
    ///
    /// Mods already in the lockfile are kept at their locked version unless
    /// `update` is set, like `cargo update`.
    pub async fn lock(&self, profile: &ProfileRef, update: bool) -> Result<Lockfile> {
        let loaded = self.load_profile(profile)?;
        let game = self.registry.game(&loaded.game)?;
        let previous: HashMap<String, LockedMod> = match (update, self.load_lock(profile)?) {
            (false, Some(lock)) => lock.mods.into_iter().map(|m| (m.key(), m)).collect(),
            _ => HashMap::new(),
        };

        let mut queue: VecDeque<(ModSpec, Option<String>)> = loaded
            .mods
            .iter()
            .map(|m| {
                let spec = ModSpec {
                    source: m.source.clone(),
                    id: m.id.clone(),
                    version: m.version.clone(),
                };
                (spec, Some(m.name.clone()))
            })
            .collect();
        let mut seen = HashSet::new();
        let mut mods = Vec::new();

        while let Some((spec, name)) = queue.pop_front() {
            let key = mod_key(&spec.source, &spec.id);
            if !seen.insert(key.clone()) {
                continue;
            }
            let locked = match previous.get(&key) {
                Some(old) if old.requested == spec.version => {
                    let mut old = old.clone();
                    if let Some(name) = name {
                        old.name = name;
                    }
                    if !self.store.contains(&old.hash, old.format) {
                        self.reporter.report(Event::Fetching {
                            name: old.name.clone(),
                        });
                        let hash = self.fetch(&old.artifact, old.format)?;
                        if hash != old.hash {
                            return Err(Error::invalid(format!(
                                "`{}` changed since it was locked (expected {}, got {}); \
                                 run `patchwork lock --update` to accept the new version",
                                old.name,
                                old.hash.short(),
                                hash.short()
                            )));
                        }
                    }
                    old
                }
                _ => {
                    let label = name.clone().unwrap_or_else(|| key.clone());
                    self.reporter.report(Event::Resolving { name: label });
                    let resolved = self.resolve(&loaded.game, &spec).await?;
                    let name = name.unwrap_or_else(|| resolved.name.clone());
                    self.reporter.report(Event::Fetching { name: name.clone() });
                    let hash = self.fetch(&resolved.artifact, resolved.format)?;
                    LockedMod {
                        name,
                        source: spec.source.clone(),
                        id: resolved.id,
                        requested: spec.version.clone(),
                        version: resolved.version,
                        hash,
                        format: resolved.format,
                        artifact: resolved.artifact,
                        dependencies: resolved
                            .dependencies
                            .iter()
                            .map(|d| mod_key(&d.source, &d.id))
                            .collect(),
                    }
                }
            };
            for dep in &locked.dependencies {
                if let Some((source, id)) = dep.split_once(':') {
                    let spec = ModSpec {
                        source: source.to_owned(),
                        id: id.to_owned(),
                        version: None,
                    };
                    queue.push_back((spec, None));
                }
            }
            mods.push(locked);
        }

        let lock = Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: loaded.game.clone(),
                definition: game.hash,
            },
            mods,
        };
        fsutil::write_toml(&self.lock_file(profile), &lock)?;
        Ok(lock)
    }

    fn fetch(&self, artifact: &ArtifactLocation, format: ArtifactFormat) -> Result<ContentHash> {
        match artifact {
            ArtifactLocation::Path { path } => self.store.ingest_path(path, format),
            ArtifactLocation::Url { url } => Err(Error::invalid(format!(
                "can't download {url}: downloading isn't implemented yet"
            ))),
        }
    }

    // Deploying

    pub fn deploy_state(&self, game: &str) -> Result<DeployState> {
        let path = self.deploy_state_file(game);
        if let Some(state) = fsutil::read_json(&path)? {
            return Ok(state);
        }
        let root = self.game_install(game)?.map(|i| i.path).unwrap_or_default();
        Ok(DeployState::empty(game, &root))
    }

    fn deploy_state_file(&self, game: &str) -> PathBuf {
        self.paths.deployments().join(format!("{game}.json"))
    }

    /// Locks the profile if needed and works out what deploying it would
    /// change, without changing anything.
    pub async fn plan_deploy(&self, profile: &ProfileRef) -> Result<DeployPlan> {
        let lock = self.lock(profile, false).await?;
        let loaded = self.load_profile(profile)?;
        let game = &self.registry.game(&loaded.game)?.value;
        let install = self.game_install(&loaded.game)?.ok_or_else(|| {
            Error::invalid(format!(
                "patchwork doesn't know where {} is installed; run `patchwork game set-path {} <dir>`",
                game.name, game.id
            ))
        })?;
        let deployer = self
            .deployers
            .get(&game.deployer.kind)
            .ok_or_else(|| Error::UnknownDeployer(game.deployer.kind.clone()))?;

        let by_key: HashMap<String, &LockedMod> = lock.mods.iter().map(|m| (m.key(), m)).collect();
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        for m in loaded.mods.iter().filter(|m| m.enabled) {
            visit(
                &mod_key(&m.source, &m.id),
                &by_key,
                &mut visited,
                &mut order,
            );
        }
        let mut mods = Vec::new();
        for locked in order {
            let files =
                self.store
                    .files(&locked.hash, locked.format, &locked.artifact.file_name())?;
            mods.push(InstalledMod {
                name: locked.name.clone(),
                source: locked.source.clone(),
                id: locked.id.clone(),
                version: locked.version.clone(),
                hash: locked.hash,
                files,
            });
        }

        let ctx = DeployContext {
            game,
            config: &game.deployer.config,
            registry: &self.registry,
        };
        let desired = deployer.plan(&ctx, &mods)?;
        let prev = self.deploy_state(&loaded.game)?;
        let prev = if prev.files.is_empty() {
            DeployState::empty(&loaded.game, &install.path)
        } else if prev.root != install.path {
            return Err(Error::invalid(format!(
                "mods are deployed to {}, but {} is now set to {}; run `patchwork undeploy {}` first",
                prev.root.display(),
                game.name,
                install.path.display(),
                game.id
            )));
        } else {
            prev
        };
        Ok(deploy::plan(
            &prev,
            desired,
            Some(profile.to_string()),
            game.link,
        ))
    }

    /// Works out how to remove everything patchwork deployed for a game.
    pub fn plan_undeploy(&self, game: &str) -> Result<DeployPlan> {
        let link = self.registry.game(game)?.value.link;
        let prev = self.deploy_state(game)?;
        Ok(deploy::plan(&prev, Default::default(), None, link))
    }

    pub fn apply(&self, plan: &DeployPlan) -> Result<DeployState> {
        let mut state = self.deploy_state(&plan.game)?;
        if state.files.is_empty() {
            state = DeployState::empty(&plan.game, &plan.root);
        }
        let backups = self.paths.backups().join(&plan.game);
        let result = deploy::apply(plan, &mut state, &backups, self.reporter.as_ref());
        // Saved even on failure, so whatever did get deployed can be undone.
        fsutil::write_json(&self.deploy_state_file(&plan.game), &state)?;
        result.map(|()| state)
    }
}

/// Depth-first, so dependencies come before the mods that need them.
fn visit<'a>(
    key: &str,
    by_key: &HashMap<String, &'a LockedMod>,
    visited: &mut HashSet<String>,
    order: &mut Vec<&'a LockedMod>,
) {
    let Some(locked) = by_key.get(key) else {
        return;
    };
    if !visited.insert(key.to_owned()) {
        return;
    }
    for dep in &locked.dependencies {
        visit(dep, by_key, visited, order);
    }
    order.push(locked);
}

fn no_such_mod(profile: &ProfileRef, name: &str) -> Error {
    Error::invalid(format!("{profile} has no mod named `{name}`"))
}
