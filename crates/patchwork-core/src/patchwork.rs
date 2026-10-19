use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::deploy::{self, DeployContext, DeployPlan, DeployState, Deployer, InstalledMod};
use crate::error::IoContext;
use crate::event::{Event, Reporter, Silent};
use crate::fsutil::{self, StateLock};
use crate::game::GameDef;
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, UnmetRequirement, mod_key};
use crate::paths::Paths;
use crate::profile::{PROFILE_SCHEMA, Profile, ProfileMod, ProfileRef, validate_name};
use crate::registry::Registry;
use crate::search::{Category, SearchPage, SearchQuery, SortBy};
use crate::share::{Bundled, FormatContext, ProfileFormat, SharedProfile, sanitize_name};
use crate::source::{ArtifactFormat, ArtifactLocation, ModSpec, Source, SourceContext};
use crate::store::Store;
use crate::{Error, Result};

/// Where a game is installed on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInstall {
    pub path: PathBuf,
}

/// A mod's versions as of when they were last fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownVersions {
    pub id: String,
    pub fetched: Timestamp,
    /// Newest first.
    pub versions: Vec<String>,
}

/// How to import a profile; see [`Patchwork::import_profile`].
#[derive(Debug, Clone, Default)]
pub struct ImportOptions<'a> {
    /// Worked out from the input if not given.
    pub format: Option<&'a str>,
    /// Needed for formats that don't record the game, unless only one game
    /// supports the format.
    pub game: Option<&'a str>,
    /// Defaults to the name the profile was shared with.
    pub name: Option<&'a str>,
    /// Overwrite the mods of a profile that's already here: the same profile
    /// imported before (matched by id), or else one with the same name. The
    /// existing profile keeps its name.
    pub replace: bool,
}

#[derive(Debug, Clone)]
pub struct Imported {
    pub profile: ProfileRef,
    pub label: String,
    pub format: String,
    pub mods: usize,
    pub replaced: bool,
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
    formats: BTreeMap<String, Arc<dyn ProfileFormat>>,
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

    pub fn format(mut self, format: impl ProfileFormat + 'static) -> Self {
        self.formats
            .insert(format.id().to_owned(), Arc::new(format));
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
        let patchwork = Patchwork {
            store: Store::new(self.paths.store()),
            registry,
            paths: self.paths,
            sources: self.sources,
            deployers: self.deployers,
            formats: self.formats,
            reporter: self.reporter,
            _lock: lock,
        };
        patchwork.migrate_profiles()?;
        Ok(patchwork)
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
    formats: BTreeMap<String, Arc<dyn ProfileFormat>>,
    reporter: Arc<dyn Reporter>,
    _lock: StateLock,
}

impl Patchwork {
    pub fn builder(paths: Paths) -> Builder {
        Builder {
            paths,
            sources: BTreeMap::new(),
            deployers: BTreeMap::new(),
            formats: BTreeMap::new(),
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

    /// Creates `<game>/<name>`, which must not already exist.
    pub fn create_profile(&self, game: &str, name: &str) -> Result<ProfileRef> {
        self.registry.game(game)?;
        validate_name(name)?;
        let profile = Profile::new(game, name);
        if self
            .profiles()?
            .iter()
            .any(|(_, p)| p.label() == profile.label())
        {
            return Err(Error::invalid(format!(
                "profile {} already exists",
                profile.label()
            )));
        }
        let profile_ref = profile.reference();
        fsutil::write_toml(&self.profile_file(&profile_ref), &profile)?;
        Ok(profile_ref)
    }

    /// Every profile on this machine, sorted by game then name.
    pub fn profiles(&self) -> Result<Vec<(ProfileRef, Profile)>> {
        let mut profiles = Vec::new();
        let Ok(entries) = fs::read_dir(self.paths.profiles()) else {
            return Ok(profiles);
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(profile_ref) = entry.file_name().to_string_lossy().parse::<ProfileRef>() else {
                continue;
            };
            if self.profile_file(&profile_ref).is_file() {
                let loaded = self.load_profile(&profile_ref)?;
                profiles.push((profile_ref, loaded));
            }
        }
        profiles.sort_by(|(_, a), (_, b)| (&a.game, &a.name).cmp(&(&b.game, &b.name)));
        Ok(profiles)
    }

    /// Looks up a profile by `<game>/<name>` or by its id.
    pub fn find_profile(&self, query: &str) -> Result<ProfileRef> {
        if let Ok(profile_ref) = query.parse::<ProfileRef>()
            && self.profile_file(&profile_ref).is_file()
        {
            return Ok(profile_ref);
        }
        self.profiles()?
            .into_iter()
            .find(|(_, p)| p.label() == query)
            .map(|(r, _)| r)
            .ok_or_else(|| Error::invalid(format!("no profile named {query}")))
    }

    /// Moves profiles from `profiles/<game>/<name>/` (schema 1) to
    /// `profiles/<id>/`, giving each one an id.
    fn migrate_profiles(&self) -> Result<()> {
        let Ok(games) = fs::read_dir(self.paths.profiles()) else {
            return Ok(());
        };
        let mut state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        let mut state_changed = false;
        for game in games.filter_map(|e| e.ok()) {
            let game_dir = game.path();
            let game_id = game.file_name().to_string_lossy().into_owned();
            if game_id.parse::<ProfileRef>().is_ok() || !game_dir.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&game_dir)
                .at(&game_dir)?
                .filter_map(|e| e.ok())
            {
                let old_dir = entry.path();
                let old_file = old_dir.join("profile.toml");
                let Some(mut table) = fsutil::read_toml::<toml::Table>(&old_file)? else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = Uuid::new_v4();
                table.insert("schema".into(), (PROFILE_SCHEMA as i64).into());
                table.insert("id".into(), id.to_string().into());
                table.insert("name".into(), name.clone().into());
                let profile_ref = ProfileRef { uuid: id };
                let new_dir = self.profile_dir(&profile_ref);
                fs::rename(&old_dir, &new_dir).at(&old_dir)?;
                fsutil::write_toml(&self.profile_file(&profile_ref), &table)?;

                let label = format!("{game_id}/{name}");
                if state.current_profile.as_deref() == Some(label.as_str()) {
                    state.current_profile = Some(profile_ref.to_string());
                    state_changed = true;
                }
                let mut deployed = self.deploy_state(&game_id)?;
                if deployed.profile.as_deref() == Some(label.as_str()) {
                    deployed.profile = Some(profile_ref.to_string());
                    fsutil::write_json(&self.deploy_state_file(&game_id), &deployed)?;
                }
            }
            // Only goes if it's now empty.
            let _ = fs::remove_dir(&game_dir);
        }
        if state_changed {
            fsutil::write_toml(&self.paths.state_file(), &state)?;
        }
        Ok(())
    }

    pub fn load_profile(&self, profile: &ProfileRef) -> Result<Profile> {
        let path = self.profile_file(profile);
        let loaded: Profile = fsutil::read_toml(&path)?
            .ok_or_else(|| Error::invalid(format!("no profile with id {profile}")))?;
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
        self.paths.profiles().join(profile.uuid.to_string())
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
                "{} already has a mod named `{name}`; pick another with --name",
                loaded.label()
            )));
        }
        if loaded
            .mods
            .iter()
            .any(|m| m.source == spec.source && m.id == resolved.id)
        {
            return Err(Error::invalid(format!(
                "{} already has that mod",
                loaded.label()
            )));
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
            return Err(no_such_mod(&loaded.label(), name));
        }
        self.save_profile(profile, &loaded)
    }

    pub fn set_enabled(&self, profile: &ProfileRef, name: &str, enabled: bool) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        loaded
            .find_mut(name)
            .ok_or_else(|| no_such_mod(&label, name))?
            .enabled = enabled;
        self.save_profile(profile, &loaded)
    }

    /// Pins a mod to `version`, or with `None` lets it follow the newest
    /// version again. Takes effect the next time the profile's locked.
    pub fn set_version(
        &self,
        profile: &ProfileRef,
        name: &str,
        version: Option<String>,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        let m = loaded
            .find_mut(name)
            .ok_or_else(|| no_such_mod(&label, name))?;
        if version.is_some() && !self.source(&m.source)?.lists_versions() {
            return Err(Error::invalid(format!(
                "`{name}` comes from {}, which doesn't have versions to pick from",
                m.source
            )));
        }
        m.version = version;
        self.save_profile(profile, &loaded)
    }

    /// Whether `source`'s mods have versions that can be listed and picked.
    pub fn lists_versions(&self, source: &str) -> Result<bool> {
        Ok(self.source(source)?.lists_versions())
    }

    /// Every version of a mod that can be pinned, newest first, fetched from
    /// its source. The list is remembered for [`Patchwork::known_versions`].
    pub async fn versions(&self, game: &str, source: &str, id: &str) -> Result<Vec<String>> {
        let def = &self.registry.game(game)?.value;
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
        };
        let versions = self.source(source)?.versions(&ctx, id).await?;
        let known = KnownVersions {
            id: id.to_owned(),
            fetched: Timestamp::now(),
            versions,
        };
        fsutil::write_json(&self.known_versions_file(game, source, id), &known)?;
        Ok(known.versions)
    }

    /// The versions [`Patchwork::versions`] last fetched for a mod, without
    /// going to the network. `None` if they never have been (or the cache
    /// was cleared).
    pub fn known_versions(&self, game: &str, source: &str, id: &str) -> Option<KnownVersions> {
        // it's only a cache, so a file that can't be read is as good as none
        fsutil::read_json::<KnownVersions>(&self.known_versions_file(game, source, id))
            .ok()
            .flatten()
            .filter(|k| k.id == id)
    }

    /// Ids can be paths or have characters that don't belong in file names,
    /// so the file is named by a hash of the id.
    fn known_versions_file(&self, game: &str, source: &str, id: &str) -> PathBuf {
        self.paths
            .known_versions()
            .join(game)
            .join(source)
            .join(format!("{}.json", ContentHash::of(id.as_bytes())))
    }

    fn source(&self, id: &str) -> Result<&dyn Source> {
        self.sources
            .get(id)
            .map(|s| s.as_ref())
            .ok_or_else(|| Error::UnknownSource(id.to_owned()))
    }

    async fn resolve(&self, game: &str, spec: &ModSpec) -> Result<crate::source::ResolvedMod> {
        let source = self.source(&spec.source)?;
        let game = &self.registry.game(game)?.value;
        let ctx = SourceContext {
            game,
            config: game.sources.get(&spec.source),
        };
        source
            .resolve(&ctx, &spec.id, spec.version.as_deref())
            .await
    }

    // Searching

    /// The sources that can search for `game`'s mods: ones that support
    /// searching and that the game's definition sets up.
    pub fn search_sources(&self, game: &str) -> Result<Vec<&str>> {
        let game = &self.registry.game(game)?.value;
        Ok(self
            .sources
            .iter()
            .filter(|(id, source)| {
                !source.search_sorts().is_empty() && game.sources.contains_key(id.as_str())
            })
            .map(|(id, _)| id.as_str())
            .collect())
    }

    /// The orders `source` can sort search results in.
    pub fn search_sorts(&self, source: &str) -> Result<&[SortBy]> {
        Ok(self.source(source)?.search_sorts())
    }

    pub async fn search_categories(&self, game: &str, source: &str) -> Result<Vec<Category>> {
        let source = self.searchable(source)?;
        let game = &self.registry.game(game)?.value;
        let ctx = SourceContext {
            game,
            config: game.sources.get(source.id()),
        };
        source.search_categories(&ctx).await
    }

    /// Searches `source` for `game`'s mods. Leave `query.text` empty for the
    /// source's home screen. Categories can be given by id or by name.
    pub async fn search(
        &self,
        game: &str,
        source: &str,
        query: &SearchQuery,
    ) -> Result<SearchPage> {
        let source = self.searchable(source)?;
        let sorts = source.search_sorts();
        if let Some(sort) = query.sort
            && !sorts.contains(&sort)
        {
            let sorts: Vec<_> = sorts.iter().map(|s| s.as_str()).collect();
            return Err(Error::invalid(format!(
                "{} can't sort by {sort}; it can sort by {}",
                source.id(),
                sorts.join(", ")
            )));
        }
        let game = &self.registry.game(game)?.value;
        let ctx = SourceContext {
            game,
            config: game.sources.get(source.id()),
        };
        if query.categories.is_empty() {
            return source.search(&ctx, query).await;
        }
        let known = source.search_categories(&ctx).await?;
        let categories = query
            .categories
            .iter()
            .map(|wanted| {
                known
                    .iter()
                    .find(|c| c.id == *wanted)
                    .or_else(|| known.iter().find(|c| c.name.eq_ignore_ascii_case(wanted)))
                    .map(|c| c.id.clone())
                    .ok_or_else(|| {
                        Error::invalid(format!(
                            "{} has no category `{wanted}` for {}",
                            source.id(),
                            game.id
                        ))
                    })
            })
            .collect::<Result<_>>()?;
        let query = SearchQuery {
            categories,
            ..query.clone()
        };
        source.search(&ctx, &query).await
    }

    fn searchable(&self, id: &str) -> Result<&dyn Source> {
        let source = self.source(id)?;
        if source.search_sorts().is_empty() {
            return Err(Error::Source {
                source_id: id.to_owned(),
                message: "can't be searched".to_owned(),
            });
        }
        Ok(source)
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
    /// `update` is set, like `cargo update`, or unless a mod now needs a newer
    /// version of them than is locked. Requirements that still aren't met,
    /// because of a pin or because nothing new enough exists, are warnings.
    pub async fn lock(&self, profile: &ProfileRef, update: bool) -> Result<Lockfile> {
        let loaded = self.load_profile(profile)?;
        let game = self.registry.game(&loaded.game)?;
        let mut previous: HashMap<String, LockedMod> = match (update, self.load_lock(profile)?) {
            (false, Some(lock)) => lock.mods.into_iter().map(|m| (m.key(), m)).collect(),
            _ => HashMap::new(),
        };

        let (lock, unmet) = loop {
            let (mods, resolved) = self.resolve_mods(&loaded, &previous).await?;
            let lock = Lockfile {
                schema: LOCK_SCHEMA,
                game: LockedGame {
                    id: loaded.game.clone(),
                    definition: game.hash,
                },
                mods,
            };
            let unmet = self.unmet_requirements(&lock);
            // Unpinned mods kept from the old lockfile might just be stale,
            // so resolve those again. Each pass drops at least one, so this
            // ends.
            let stale: Vec<String> = lock
                .mods
                .iter()
                .filter(|m| {
                    m.requested.is_none()
                        && !resolved.contains(&m.key())
                        && unmet.iter().any(|u| u.dependency_key == m.key())
                })
                .map(|m| m.key())
                .collect();
            if stale.is_empty() {
                break (lock, unmet);
            }
            for key in stale {
                previous.remove(&key);
            }
        };
        for u in unmet {
            self.reporter.report(Event::Warning(u.to_string()));
        }
        fsutil::write_toml(&self.lock_file(profile), &lock)?;
        Ok(lock)
    }

    /// Requirements in `lock` that the locked versions don't meet.
    pub fn unmet_requirements(&self, lock: &Lockfile) -> Vec<UnmetRequirement> {
        lock.unmet_requirements(|source, a, b| self.source(source).ok()?.compare_versions(a, b))
    }

    /// One pass of [`Patchwork::lock`]: walks the profile's mods and their
    /// dependencies, reusing entries from `previous` where it can. Also
    /// returns the keys of the mods it had to resolve.
    async fn resolve_mods(
        &self,
        loaded: &Profile,
        previous: &HashMap<String, LockedMod>,
    ) -> Result<(Vec<LockedMod>, HashSet<String>)> {
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
        let mut resolved_keys = HashSet::new();
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
                        let hash = self.fetch(&old.artifact, old.format).await?;
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
                    let hash = self.fetch(&resolved.artifact, resolved.format).await?;
                    resolved_keys.insert(key.clone());
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
                        requires: resolved
                            .dependencies
                            .into_iter()
                            .filter(|d| !d.req.is_any())
                            .map(|d| (mod_key(&d.source, &d.id), d.req))
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
        Ok((mods, resolved_keys))
    }

    async fn fetch(
        &self,
        artifact: &ArtifactLocation,
        format: ArtifactFormat,
    ) -> Result<ContentHash> {
        match artifact {
            ArtifactLocation::Path { path } => self.store.ingest_path(path, format),
            ArtifactLocation::Url { url } => {
                if format == ArtifactFormat::Dir {
                    return Err(Error::invalid(format!(
                        "{url} can't be downloaded as a directory"
                    )));
                }
                let tmp = self.paths.downloads().join("download.tmp");
                download(url, &tmp).await?;
                let hash = self.store.ingest_path(&tmp, format);
                fs::remove_file(&tmp).at(&tmp)?;
                hash
            }
        }
    }

    // Sharing

    pub fn formats(&self) -> impl Iterator<Item = &dyn ProfileFormat> {
        self.formats.values().map(|f| f.as_ref())
    }

    pub fn format(&self, id: &str) -> Result<&dyn ProfileFormat> {
        self.formats
            .get(id)
            .map(|f| f.as_ref())
            .ok_or_else(|| Error::UnknownFormat(id.to_owned()))
    }

    fn format_context<'a>(
        &'a self,
        game: Option<&'a GameDef>,
        scratch: &'a Path,
    ) -> FormatContext<'a> {
        FormatContext {
            game,
            sources: &self.sources,
            reporter: self.reporter.as_ref(),
            scratch,
        }
    }

    /// Exports a profile in the given format. Formats that need the exact
    /// versions lock the profile first, which may fetch mods.
    pub async fn export_profile(&self, profile: &ProfileRef, format: &str) -> Result<Vec<u8>> {
        let format = self.format(format)?;
        let loaded = self.load_profile(profile)?;
        let game = &self.registry.game(&loaded.game)?.value;
        if !format.supports(game) {
            return Err(Error::invalid(format!(
                "{} profiles can't be exported as {}",
                game.name,
                format.id()
            )));
        }
        let mut shared = SharedProfile {
            game: Some(loaded.game.clone()),
            name: Some(loaded.name.clone()),
            id: Some(loaded.id),
            mods: loaded.mods,
            ..Default::default()
        };
        if format.needs_lock() {
            let lock = self.lock(profile, false).await?;
            for m in &lock.mods {
                if let ArtifactLocation::Path { .. } = m.artifact {
                    let bundled = Bundled {
                        path: self.store.path(&m.hash, m.format),
                        name: m.artifact.file_name(),
                    };
                    shared.bundled.insert(m.hash, bundled);
                }
            }
            shared.lock = Some(lock);
        }
        let scratch = self.import_scratch();
        format
            .export(&self.format_context(Some(game), &scratch), &shared)
            .await
    }

    /// Creates a profile from an export (or updates one; see
    /// [`ImportOptions::replace`]). Nothing is fetched until it's locked.
    pub async fn import_profile(
        &self,
        input: &[u8],
        options: &ImportOptions<'_>,
    ) -> Result<Imported> {
        let format = match options.format {
            Some(id) => self.format(id)?,
            None => self.formats().find(|f| f.detect(input)).ok_or_else(|| {
                Error::invalid(
                    "couldn't tell what format that is; pass --format (see `patchwork profile formats`)",
                )
            })?,
        };
        let explicit = match options.game {
            Some(id) => {
                let game = &self.registry.game(id)?.value;
                if !format.supports(game) {
                    return Err(Error::invalid(format!(
                        "{} profiles can't be imported from {}",
                        game.name,
                        format.id()
                    )));
                }
                Some(game)
            }
            None => None,
        };
        let game = explicit.or_else(|| {
            let mut supported = self
                .registry
                .games()
                .map(|e| &e.value)
                .filter(|g| format.supports(g));
            match (supported.next(), supported.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        });

        let scratch = self.import_scratch();
        if scratch.exists() {
            fs::remove_dir_all(&scratch).at(&scratch)?;
        }
        fs::create_dir_all(&scratch).at(&scratch)?;
        let ctx = self.format_context(game, &scratch);
        let result = match format.import(&ctx, input).await {
            Ok(shared) => self.save_import(shared, format.id(), game, explicit.is_some(), options),
            Err(e) => Err(e),
        };
        let _ = fs::remove_dir_all(&scratch);
        result
    }

    fn save_import(
        &self,
        mut shared: SharedProfile,
        format: &str,
        game: Option<&GameDef>,
        explicit_game: bool,
        options: &ImportOptions<'_>,
    ) -> Result<Imported> {
        let game = match (shared.game.take(), game) {
            (Some(theirs), Some(ours)) if explicit_game && theirs != ours.id => {
                return Err(Error::invalid(format!(
                    "that's a {theirs} profile, not {}",
                    ours.id
                )));
            }
            (Some(theirs), _) => theirs,
            (None, Some(ours)) => ours.id.clone(),
            (None, None) => {
                return Err(Error::invalid(
                    "couldn't tell which game this profile is for; pass --game",
                ));
            }
        };
        self.registry.game(&game)?;
        if let Some(m) = shared.mods.iter().find(|m| !self.has_source(&m.source)) {
            return Err(Error::UnknownSource(m.source.clone()));
        }
        if let Some(lock) = &shared.lock {
            if lock.schema > LOCK_SCHEMA {
                return Err(Error::invalid(format!(
                    "the imported lockfile uses schema {}, but this build only supports up to {LOCK_SCHEMA}; update patchwork",
                    lock.schema
                )));
            }
            if lock.game.id != game {
                return Err(Error::invalid(format!(
                    "the imported lockfile is for {}, not {game}",
                    lock.game.id
                )));
            }
        }

        let name = match options.name {
            Some(name) => name.to_owned(),
            None => sanitize_name(shared.name.as_deref().unwrap_or("imported")),
        };
        validate_name(&name)?;
        let label = format!("{game}/{name}");
        let profiles = self.profiles()?;
        let same_id = shared
            .id
            .and_then(|id| profiles.iter().find(|(r, _)| r.uuid == id));
        let same_label = profiles.iter().find(|(_, p)| p.label() == label);
        let target = if options.replace {
            same_id.or(same_label)
        } else if let Some((_, existing)) = same_label {
            return Err(Error::invalid(format!(
                "{} already exists; pass --name to import it under another name, or --replace to overwrite it",
                existing.label()
            )));
        } else if let Some((_, existing)) = same_id.filter(|_| options.name.is_none()) {
            return Err(Error::invalid(format!(
                "that profile is already here as {}; pass --replace to update it, or --name to import a separate copy",
                existing.label()
            )));
        } else {
            None
        };

        let (profile_ref, mut profile) = match target {
            Some((r, existing)) => {
                if existing.game != game {
                    return Err(Error::invalid(format!(
                        "{} is a {} profile, not {game}",
                        existing.label(),
                        existing.game
                    )));
                }
                (r.clone(), existing.clone())
            }
            None => {
                let mut profile = Profile::new(&game, &name);
                // A copy of a profile that's already here needs its own id.
                if let Some(id) = shared.id.filter(|_| same_id.is_none()) {
                    profile.id = id;
                }
                (profile.reference(), profile)
            }
        };

        // Local mods come along as files, which need a permanent home.
        let files = self.profile_dir(&profile_ref).join("files");
        if files.exists() {
            fs::remove_dir_all(&files).at(&files)?;
        }
        if let Some(lock) = &mut shared.lock {
            for locked in &mut lock.mods {
                let ArtifactLocation::Path { path: old } = &locked.artifact else {
                    continue;
                };
                let Some(bundled) = shared.bundled.get(&locked.hash) else {
                    continue;
                };
                let dest = files.join(locked.hash.to_string()).join(&bundled.name);
                if !dest.exists() {
                    fs::create_dir_all(dest.parent().expect("has a parent")).at(&files)?;
                    fs::rename(&bundled.path, &dest).at(&bundled.path)?;
                }
                let new_id = dest.to_string_lossy().into_owned();
                // For local mods the id is the path, and it has to follow
                // the file.
                if locked.id == old.to_string_lossy() {
                    for m in &mut shared.mods {
                        if m.source == locked.source && m.id == locked.id {
                            m.id = new_id.clone();
                        }
                    }
                    locked.id = new_id;
                }
                locked.artifact = ArtifactLocation::Path { path: dest };
            }
        }

        profile.mods = dedupe_names(shared.mods);
        self.save_profile(&profile_ref, &profile)?;
        if let Some(lock) = &shared.lock {
            fsutil::write_toml(&self.lock_file(&profile_ref), lock)?;
        }
        Ok(Imported {
            label: profile.label(),
            profile: profile_ref,
            format: format.to_owned(),
            mods: profile.mods.len(),
            replaced: target.is_some(),
        })
    }

    /// Where formats unpack imports. It's under `data` so bundled files can
    /// be renamed into a profile rather than copied.
    fn import_scratch(&self) -> PathBuf {
        self.paths.data.join("import.tmp")
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

/// Streams `url` into the file at `dest`. Holding the state lock means only
/// one download happens at a time, so `dest` can be a fixed path.
async fn download(url: &str, dest: &Path) -> Result<()> {
    let reqwest_error = |result| Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let mut response = reqwest::get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(reqwest_error)?;
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    let mut file = fs::File::create(dest).at(dest)?;
    while let Some(chunk) = response.chunk().await.map_err(reqwest_error)? {
        file.write_all(&chunk).at(dest)?;
    }
    Ok(())
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

/// Mod names have to be unique within a profile, which nothing guarantees for
/// an import.
fn dedupe_names(mods: Vec<ProfileMod>) -> Vec<ProfileMod> {
    let mut taken = HashSet::new();
    mods.into_iter()
        .map(|mut m| {
            let base = m.name.clone();
            let mut n = 2;
            while !taken.insert(m.name.clone()) {
                m.name = format!("{base}-{n}");
                n += 1;
            }
            m
        })
        .collect()
}

fn no_such_mod(profile: &str, name: &str) -> Error {
    Error::invalid(format!("{profile} has no mod named `{name}`"))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// Answers one HTTP request with `status` and `body`, returning its URL.
    fn serve_once(status: &'static str, body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mod.zip", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        url
    }

    #[tokio::test]
    async fn downloads_to_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("downloads/download.tmp");
        download(&serve_once("200 OK", b"zip bytes"), &dest)
            .await
            .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"zip bytes");
    }

    #[tokio::test]
    async fn http_errors_fail_the_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("download.tmp");
        let err = download(&serve_once("404 Not Found", b""), &dest)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }
}
