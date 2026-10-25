use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use futures_util::{StreamExt, TryStreamExt, stream};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::changelog::{self, ChangelogEntry};
use crate::compat::{self, GameVersions};
use crate::deploy::{
    self, BuildContext, DeployContext, DeployPlan, DeployState, DeployTarget, Deployer,
    DesiredState, GameFiles, InstalledMod, Resolved, RootId, Roots,
};
use crate::environment::GameEnv;
use crate::error::IoContext;
use crate::event::{Event, Reporter, Silent};
use crate::fsutil::{self, StateLock};
use crate::game::{GAME_SCHEMA, GameDef, LinkMethod, validate_game_id};
use crate::hash::ContentHash;
use crate::issue::Issue;
use crate::locate::{Found, Locator};
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, UnmetRequirement, mod_key};
use crate::matching::{self, ModText};
use crate::order::{self, ModFacts, Position, SortKey};
use crate::paths::Paths;
use crate::profile::{Profile, ProfileMod, ProfileRef, no_such_mod, validate_name};
use crate::registry::Registry;
use crate::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use crate::settings::{Origin, SettingSpec, SettingState, Settings};
use crate::share::{Bundled, FormatContext, ProfileFormat, SharedProfile, sanitize_name};
use crate::source::{
    ArtifactFormat, ArtifactLocation, Download, ModSpec, PendingDownload, Source, SourceContext,
    SourceFile,
};
use crate::store::Store;
use crate::{Error, Result};

/// A game's settings on this machine, as opposed to its [`GameDef`], which is
/// the same everywhere. Saved to `<data>/games/<id>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInstall {
    /// Where the game is installed.
    pub path: PathBuf,
    /// Overrides the definition's [`GameDef::link`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkMethod>,
    /// The user's [`Deployer::settings`] for this game, on top of the
    /// definition's `[deployer]`.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub deployer: toml::Table,
}

/// A mod's versions as of when they were last fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownVersions {
    pub id: String,
    pub fetched: Timestamp,
    /// Newest first.
    pub versions: Vec<String>,
}

impl KnownVersions {
    /// How long fetched versions are trusted before they're worth fetching
    /// again.
    pub const TTL: SignedDuration = SignedDuration::from_hours(1);

    pub fn is_stale(&self) -> bool {
        Timestamp::now().duration_since(self.fetched) > Self::TTL
    }
}

/// How many mods locking resolves or downloads at once.
const LOCK_CONCURRENCY: usize = 6;

/// Where a cache under `dir` keeps what it knows about a mod. Ids can be
/// paths or have characters that don't belong in file names, so the file is
/// named by a hash of the id.
fn cache_file(dir: &Path, game: &str, source: &str, id: &str) -> PathBuf {
    dir.join(game)
        .join(source)
        .join(format!("{}.json", ContentHash::of(id.as_bytes())))
}

/// What [`Patchwork::lock_mod`] made of a mod.
enum Locking {
    /// Locked, along with its key if it had to be resolved.
    Locked(Box<LockedMod>, Option<String>),
    /// Waiting on the user to download it. Its dependencies (as
    /// `source:id`) still get locked in the meantime.
    Pending(Vec<PendingDownload>, Vec<String>),
}

/// Which store entry a [`SourceFile`] is, saved under
/// [`Paths::source_files`].
#[derive(Debug, Serialize, Deserialize)]
struct KnownSourceFile {
    file: SourceFile,
    hash: ContentHash,
}

/// A mod's summary as of when it was last fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownSummary {
    pub id: String,
    pub fetched: Timestamp,
    /// `None` if the source has nothing to show for the mod.
    pub summary: Option<ModSummary>,
}

impl KnownSummary {
    /// The same as [`KnownVersions::TTL`], for summaries, which matter less
    /// and change less.
    pub const TTL: SignedDuration = SignedDuration::from_hours(24);

    /// Whether it's old enough to fetch again, or is behind `versions` (see
    /// [`KnownSummary::is_behind`]).
    pub fn is_stale(&self, versions: Option<&KnownVersions>) -> bool {
        Timestamp::now().duration_since(self.fetched) > Self::TTL
            || versions.is_some_and(|v| self.is_behind(v))
    }

    /// Whether versions fetched after it have a newer version than the one
    /// it knew about, so its details (when the mod was updated, say) are out
    /// of date. Sources whose summaries name versions differently from
    /// their version lists just get their summary fetched again each time
    /// the versions are.
    pub fn is_behind(&self, versions: &KnownVersions) -> bool {
        let latest = self
            .summary
            .as_ref()
            .and_then(|s| s.latest_version.as_ref());
        match (latest, versions.versions.first()) {
            (Some(latest), Some(newest)) => versions.fetched > self.fetched && latest != newest,
            _ => false,
        }
    }
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
        let settings = Settings::load(&self.paths, &source_specs(&self.sources))?;
        for problem in &settings.problems {
            self.reporter.report(Event::Warning(problem.clone()));
        }
        let patchwork = Patchwork {
            store: Store::new(self.paths.store()),
            registry: RwLock::new(Arc::new(registry)),
            settings: RwLock::new(Arc::new(settings)),
            paths: self.paths,
            sources: self.sources,
            deployers: self.deployers,
            formats: self.formats,
            reporter: self.reporter,
            categories: Mutex::default(),
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
    /// Swapped out whole when game definitions change, so anything already
    /// working from the old one carries on undisturbed.
    registry: RwLock<Arc<Registry>>,
    /// Swapped out whole when they change, like the registry.
    settings: RwLock<Arc<Settings>>,
    store: Store,
    sources: BTreeMap<String, Arc<dyn Source>>,
    deployers: BTreeMap<String, Arc<dyn Deployer>>,
    formats: BTreeMap<String, Arc<dyn ProfileFormat>>,
    reporter: Arc<dyn Reporter>,
    /// Each source's search categories for each game, by `(source, game)`.
    /// They hardly ever change, and a search by category needs them, so
    /// they're only fetched once.
    categories: Mutex<HashMap<(String, String), Vec<Category>>>,
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

    /// The registry as it is now. Hold on to it for as long as you need a
    /// consistent view; [`Patchwork::reload_registry`] won't change it.
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().unwrap().clone()
    }

    /// Loads the registry again, picking up definitions added or changed on
    /// disk since patchwork was opened.
    pub fn reload_registry(&self) -> Result<()> {
        let registry = Registry::load(&self.paths)?;
        *self.registry.write().unwrap() = Arc::new(registry);
        // a game's sources may be set up differently now
        self.categories.lock().unwrap().clear();
        Ok(())
    }

    pub fn has_source(&self, id: &str) -> bool {
        self.sources.contains_key(id)
    }

    // Settings

    /// The user's settings for sources as they are now.
    fn settings(&self) -> Arc<Settings> {
        self.settings.read().unwrap().clone()
    }

    /// Sources that take settings, as `(id, name)`.
    pub fn configurable_sources(&self) -> Vec<(&str, &str)> {
        self.sources
            .iter()
            .filter(|(_, s)| !s.settings().is_empty())
            .map(|(id, s)| (id.as_str(), s.name()))
            .collect()
    }

    pub fn source_settings(&self, source: &str) -> Result<Vec<SettingState>> {
        let specs = self.source(source)?.settings();
        Ok(self.settings().states(source, specs))
    }

    /// Sets one of a source's settings, or clears it if `value` is `None`.
    /// Returns where it was saved: secrets go in the system keychain if
    /// there is one. An environment variable for it still takes its place.
    pub fn set_source_setting(
        &self,
        source: &str,
        key: &str,
        value: Option<toml::Value>,
    ) -> Result<Option<Origin>> {
        let found = self.source(source)?;
        let spec = SettingSpec::find(found.settings(), found.name(), key)?;
        let origin = Settings::save(&self.paths, source, spec, value)?;
        let settings = Settings::load(&self.paths, &source_specs(&self.sources))?;
        *self.settings.write().unwrap() = Arc::new(settings);
        // what a source can see may depend on who it's signed in as
        self.categories.lock().unwrap().clear();
        Ok(origin)
    }

    /// Asks a source whether its settings work, and what they amount to
    /// (the account an API key is for, say).
    pub async fn check_source_settings(&self, source: &str) -> Result<Option<String>> {
        let settings = self.settings();
        self.source(source)?
            .check_settings(settings.source(source))
            .await
    }

    // Games

    pub fn game_install(&self, game: &str) -> Result<Option<GameInstall>> {
        fsutil::read_toml(&self.install_file(game))
    }

    /// Sets where a game is installed, keeping the rest of its settings.
    pub fn set_game_path(&self, game: &str, path: &Path) -> Result<GameInstall> {
        let install = match self.game_install(game)? {
            Some(install) => GameInstall {
                path: path.to_owned(),
                ..install
            },
            None => GameInstall {
                path: path.to_owned(),
                link: None,
                deployer: toml::Table::new(),
            },
        };
        self.set_game_install(game, install)
    }

    /// Replaces a game's settings, after checking them. Returns them as
    /// saved, with the path made absolute.
    pub fn set_game_install(&self, game: &str, install: GameInstall) -> Result<GameInstall> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let GameInstall {
            path,
            link,
            deployer,
        } = install;
        let path = path.canonicalize().at(&path)?;
        if !path.is_dir() {
            return Err(Error::invalid(format!(
                "{} isn't a directory",
                path.display()
            )));
        }
        let specs = self.deployer(&def.deployer.kind)?.settings();
        for (key, value) in &deployer {
            SettingSpec::find(specs, &format!("{}'s deployer", def.name), key)?.check(value)?;
        }
        let state = self.deploy_state(&DeployTarget::Game(game.to_owned()))?;
        let moved = state.roots().with(RootId::game(), &path);
        if let Some((_, was)) = state.moved_root(&moved) {
            return Err(Error::invalid(format!(
                "mods are still deployed to {}; run `patchwork undeploy {game}` first",
                was.display()
            )));
        }
        // files already placed stay the way they were placed, so the method
        // can only change while there aren't any
        let was = self.game_install(game)?.and_then(|i| i.link);
        if was.unwrap_or(def.link) != link.unwrap_or(def.link) && !state.files.is_empty() {
            return Err(Error::invalid(format!(
                "mods are still deployed to {}; run `patchwork undeploy {game}` before changing how they're linked",
                path.display()
            )));
        }
        let install = GameInstall {
            path,
            link,
            deployer,
        };
        fsutil::write_toml(&self.install_file(game), &install)?;
        Ok(install)
    }

    /// The settings `game`'s deployer takes, with what's set for this game
    /// and what the game's definition says otherwise.
    pub fn deployer_settings(&self, game: &str) -> Result<Vec<SettingState>> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let set = self
            .game_install(game)?
            .map(|i| i.deployer)
            .unwrap_or_default();
        Ok(self
            .deployer(&def.deployer.kind)?
            .settings()
            .iter()
            .map(|spec| SettingState {
                spec: *spec,
                value: set.get(spec.key).cloned(),
                default: def.deployer.config.get(spec.key).cloned(),
                origin: set.get(spec.key).map(|_| Origin::File),
            })
            .collect())
    }

    /// The deployer config and link method to deploy `game` with: the
    /// definition's, with the user's settings on top. Also returns the
    /// user's deployer settings on their own.
    fn deploy_config(&self, game: &GameDef) -> Result<(toml::Table, LinkMethod, toml::Table)> {
        let install = self.game_install(&game.id)?;
        let mut config = game.deployer.config.clone();
        let (link, overrides) = match install {
            Some(i) => (i.link.unwrap_or(game.link), i.deployer),
            None => (game.link, toml::Table::new()),
        };
        // only settings the deployer still takes, in case it's changed since
        let specs = self.deployer(&game.deployer.kind)?.settings();
        let overrides: toml::Table = overrides
            .into_iter()
            .filter(|(k, v)| specs.iter().any(|s| s.key == k && s.check(v).is_ok()))
            .collect();
        config.extend(overrides.clone());
        Ok((config, link, overrides))
    }

    /// Where `game` looks to be installed on this machine, so it can be set
    /// up without the user having to find it.
    pub fn find_installs(&self, game: &str) -> Result<Vec<Found>> {
        let registry = self.registry();
        Ok(Locator::scan().find(&registry.game(game)?.value))
    }

    fn install_file(&self, game: &str) -> PathBuf {
        self.paths.game_installs().join(format!("{game}.toml"))
    }

    /// What can be found out about the game `target` deploys to, such as
    /// its version. `None` if it hasn't been set up.
    pub fn game_env(&self, target: &DeployTarget) -> Result<Option<GameEnv>> {
        let Some(root) = self.target_root(target)? else {
            return Ok(None);
        };
        let registry = self.registry();
        let game = &registry.game(target.game())?.value;
        Ok(Some(GameEnv::detect(game, &root)))
    }

    /// Saves a new game definition to the user's registry (`<config>/games/`)
    /// and reloads the registry so it can be used straight away. Refuses ids
    /// that are already defined anywhere, so this never shadows a builtin or
    /// registry game by accident.
    pub fn add_game(&self, mut def: GameDef) -> Result<()> {
        validate_game_id(&def.id)?;
        if def.name.trim().is_empty() {
            return Err(Error::invalid("the game needs a name"));
        }
        if self.registry().game(&def.id).is_ok() {
            return Err(Error::invalid(format!(
                "there's already a game with the id `{}`",
                def.id
            )));
        }
        if !self.deployers.contains_key(&def.deployer.kind) {
            return Err(Error::UnknownDeployer(def.deployer.kind.clone()));
        }
        if let Some(source) = def.sources.keys().find(|s| !self.has_source(s)) {
            return Err(Error::UnknownSource(source.clone()));
        }
        def.schema = GAME_SCHEMA;
        let path = self
            .paths
            .user_registry()
            .join("games")
            .join(format!("{}.toml", def.id));
        fsutil::write_toml(&path, &def)?;
        self.reload_registry()
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
            game_versions: None,
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

    /// Sets which tags hide a mod from the main list; see
    /// [`Profile::hidden_tags`]. Blank and repeated tags are dropped.
    pub fn set_hidden_tags(&self, profile: &ProfileRef, tags: &[String]) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let mut kept: Vec<String> = Vec::new();
        for tag in tags.iter().map(|t| t.trim()) {
            if !tag.is_empty() && !kept.iter().any(|k| k.to_lowercase() == tag.to_lowercase()) {
                kept.push(tag.to_owned());
            }
        }
        loaded.hidden_tags = kept;
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

    /// Says which versions of the game a mod works with, overriding what its
    /// source says, or with `None` goes back to what the source says.
    pub fn set_game_versions(
        &self,
        profile: &ProfileRef,
        name: &str,
        versions: Option<GameVersions>,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        let m = loaded
            .find_mut(name)
            .ok_or_else(|| no_such_mod(&label, name))?;
        m.game_versions = versions;
        self.save_profile(profile, &loaded)
    }

    /// Moves a mod within the profile's load order, returning its new
    /// (zero-based) position.
    pub fn move_mod(&self, profile: &ProfileRef, name: &str, to: &Position) -> Result<usize> {
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        let index = order::move_mod(&mut loaded.mods, &label, name, to)?;
        self.save_profile(profile, &loaded)?;
        Ok(index)
    }

    /// Puts the profile's mods in the order of `names`, which has to name
    /// each of them exactly once. This is how a frontend saves a list the
    /// user has rearranged by hand.
    pub fn reorder_mods(&self, profile: &ProfileRef, names: &[String]) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let mut by_name: HashMap<String, ProfileMod> =
            loaded.mods.drain(..).map(|m| (m.name.clone(), m)).collect();
        for name in names {
            let m = by_name.remove(name).ok_or_else(|| {
                Error::invalid(format!(
                    "`{name}` is listed twice or isn't in {}",
                    loaded.label()
                ))
            })?;
            loaded.mods.push(m);
        }
        if let Some(name) = by_name.keys().next() {
            return Err(Error::invalid(format!(
                "the new order for {} leaves out `{name}`",
                loaded.label()
            )));
        }
        self.save_profile(profile, &loaded)
    }

    /// The profile's mods sorted by `keys` (see [`order::sort_mods`]),
    /// without saving anything; pass the result's names to
    /// [`Patchwork::reorder_mods`] to keep it.
    ///
    /// Sorting by tags uses each mod's [`Patchwork::known_summary`], fetching
    /// the ones that haven't been; a mod whose summary can't be fetched is
    /// warned about and treated as untagged. Sorting by dependencies locks
    /// the profile first.
    pub async fn sorted_mods(
        &self,
        profile: &ProfileRef,
        keys: &[SortKey],
        tag_order: &[String],
    ) -> Result<Vec<ProfileMod>> {
        let mut loaded = self.load_profile(profile)?;
        let mut facts: HashMap<String, ModFacts> = if SortKey::needs_dependencies(keys) {
            order::dependency_facts(&self.lock(profile, false).await?)
        } else {
            HashMap::new()
        };
        if SortKey::needs_summaries(keys) {
            for m in &loaded.mods {
                let summary = match self.known_summary(&loaded.game, &m.source, &m.id) {
                    Some(known) => known.summary,
                    None => match self.summary(&loaded.game, &m.source, &m.id).await {
                        Ok(summary) => summary,
                        Err(e) => {
                            self.reporter.report(Event::Warning(format!(
                                "couldn't get `{}`'s details: {e}",
                                m.name
                            )));
                            None
                        }
                    },
                };
                let facts = facts.entry(mod_key(&m.source, &m.id)).or_default();
                if let Some(summary) = summary {
                    facts.tags = summary.categories.into_iter().map(|c| c.name).collect();
                    facts.updated = summary.updated;
                }
            }
        }
        order::sort_mods(&mut loaded.mods, keys, tag_order, &facts);
        Ok(loaded.mods)
    }

    /// The profile's mods matching `query` (see [`matching::rank`]), best
    /// first, each with its place in the load order. Nothing is fetched:
    /// mods are found by their [`Patchwork::known_summary`] if they have
    /// one, and otherwise just by name.
    pub fn find_mods(&self, profile: &ProfileRef, query: &str) -> Result<Vec<(usize, ProfileMod)>> {
        let loaded = self.load_profile(profile)?;
        let summaries: Vec<Option<ModSummary>> = loaded
            .mods
            .iter()
            .map(|m| {
                self.known_summary(&loaded.game, &m.source, &m.id)
                    .and_then(|k| k.summary)
            })
            .collect();
        let texts: Vec<ModText> = loaded
            .mods
            .iter()
            .zip(&summaries)
            .map(|(m, s)| ModText::of(m, s.as_ref()))
            .collect();
        let found = matching::rank(&texts, query);
        let mut mods: Vec<Option<ProfileMod>> = loaded.mods.into_iter().map(Some).collect();
        Ok(found
            .into_iter()
            .map(|i| (i, mods[i].take().expect("rank gives each mod once")))
            .collect())
    }

    /// Whether `source`'s mods have versions that can be listed and picked.
    pub fn lists_versions(&self, source: &str) -> Result<bool> {
        Ok(self.source(source)?.lists_versions())
    }

    /// Every version of a mod that can be pinned, newest first, fetched from
    /// its source. The list is remembered for [`Patchwork::known_versions`].
    pub async fn versions(&self, game: &str, source: &str, id: &str) -> Result<Vec<String>> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
            settings: settings.source(source),
            env: None,
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

    fn known_versions_file(&self, game: &str, source: &str, id: &str) -> PathBuf {
        cache_file(&self.paths.known_versions(), game, source, id)
    }

    /// What a search would show for a mod, fetched from its source. It's
    /// remembered for [`Patchwork::known_summary`].
    pub async fn summary(&self, game: &str, source: &str, id: &str) -> Result<Option<ModSummary>> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
            settings: settings.source(source),
            env: None,
        };
        let summary = self.source(source)?.summary(&ctx, id).await?;
        let known = KnownSummary {
            id: id.to_owned(),
            fetched: Timestamp::now(),
            summary,
        };
        fsutil::write_json(&self.known_summary_file(game, source, id), &known)?;
        Ok(known.summary)
    }

    /// The summary [`Patchwork::summary`] last fetched for a mod, without
    /// going to the network. `None` if it never has been (or the cache was
    /// cleared).
    pub fn known_summary(&self, game: &str, source: &str, id: &str) -> Option<KnownSummary> {
        fsutil::read_json::<KnownSummary>(&self.known_summary_file(game, source, id))
            .ok()
            .flatten()
            .filter(|k| k.id == id)
    }

    fn known_summary_file(&self, game: &str, source: &str, id: &str) -> PathBuf {
        cache_file(&self.paths.known_summaries(), game, source, id)
    }

    /// A mod's changelog as of `version` (the newest if `None`), newest entry
    /// first, fetched from its source. Empty if the source has none.
    pub async fn changelog(
        &self,
        game: &str,
        source: &str,
        id: &str,
        version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
            settings: settings.source(source),
            env: None,
        };
        self.source(source)?.changelog(&ctx, id, version).await
    }

    /// What moving a mod from `from` to `to` brings in: its changelog as of
    /// `to`, cut down to the versions in between (see
    /// [`changelog::between`]).
    pub async fn changes(
        &self,
        game: &str,
        source: &str,
        id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let entries = self.changelog(game, source, id, to).await?;
        let compare = |a: &str, b: &str| self.source(source).ok()?.compare_versions(a, b);
        Ok(changelog::between(&entries, from, to, compare))
    }

    /// A link to a mod's page on its source, if it has one.
    pub fn mod_url(&self, game: &str, source: &str, id: &str) -> Option<String> {
        let registry = self.registry();
        let def = &registry.game(game).ok()?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
            settings: settings.source(source),
            env: None,
        };
        self.source(source).ok()?.mod_url(&ctx, id, None)
    }

    fn deployer(&self, kind: &str) -> Result<&dyn Deployer> {
        self.deployers
            .get(kind)
            .map(|d| d.as_ref())
            .ok_or_else(|| Error::UnknownDeployer(kind.to_owned()))
    }

    fn source(&self, id: &str) -> Result<&dyn Source> {
        self.sources
            .get(id)
            .map(|s| s.as_ref())
            .ok_or_else(|| Error::UnknownSource(id.to_owned()))
    }

    async fn resolve(&self, game: &str, spec: &ModSpec) -> Result<crate::source::ResolvedMod> {
        let source = self.source(&spec.source)?;
        let env = self.game_env(&DeployTarget::Game(game.to_owned()))?;
        let registry = self.registry();
        let game = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game,
            config: game.sources.get(&spec.source),
            settings: settings.source(&spec.source),
            env: env.as_ref(),
        };
        source
            .resolve(&ctx, &spec.id, spec.version.as_deref())
            .await
    }

    // Searching

    /// The sources that can search for `game`'s mods: ones that support
    /// searching and that the game's definition sets up.
    pub fn search_sources(&self, game: &str) -> Result<Vec<&str>> {
        let registry = self.registry();
        let game = &registry.game(game)?.value;
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
        let key = (source.to_owned(), game.to_owned());
        if let Some(known) = self.categories.lock().unwrap().get(&key) {
            return Ok(known.clone());
        }
        let source = self.searchable(source)?;
        let registry = self.registry();
        let game = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game,
            config: game.sources.get(source.id()),
            settings: settings.source(source.id()),
            env: None,
        };
        let known = source.search_categories(&ctx).await?;
        self.categories.lock().unwrap().insert(key, known.clone());
        Ok(known)
    }

    /// Searches `source` for `game`'s mods. Leave `query.text` empty for the
    /// source's home screen. Categories, included or excluded, can be given by
    /// id or by name.
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
        let registry = self.registry();
        let game = &registry.game(game)?.value;
        let settings = self.settings();
        let ctx = SourceContext {
            game,
            config: game.sources.get(source.id()),
            settings: settings.source(source.id()),
            env: None,
        };
        if query.categories.is_empty() && query.excluded_categories.is_empty() {
            return source.search(&ctx, query).await;
        }
        let known = self.search_categories(&game.id, source.id()).await?;
        let resolve = |wanted: &Vec<String>| {
            wanted
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
                .collect::<Result<Vec<_>>>()
        };
        let query = SearchQuery {
            categories: resolve(&query.categories)?,
            excluded_categories: resolve(&query.excluded_categories)?,
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
        self.relock(profile, |_| update).await
    }

    /// Like [`Patchwork::lock`] with `update`, but only the mods in `keys`
    /// (as `<source>:<id>`) move to their newest versions; the rest stay
    /// where they're locked unless one of those now needs them newer.
    pub async fn lock_updating(&self, profile: &ProfileRef, keys: &[String]) -> Result<Lockfile> {
        self.relock(profile, |key| keys.iter().any(|k| k == key))
            .await
    }

    /// [`Patchwork::lock`], resolving again the locked mods `update` says
    /// to.
    async fn relock(
        &self,
        profile: &ProfileRef,
        update: impl Fn(&str) -> bool,
    ) -> Result<Lockfile> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = registry.game(&loaded.game)?;
        let known: HashMap<String, LockedMod> = self
            .load_lock(profile)?
            .map(|lock| lock.mods.into_iter().map(|m| (m.key(), m)).collect())
            .unwrap_or_default();
        let mut previous: HashMap<String, LockedMod> = known
            .iter()
            .filter(|(key, _)| !update(key))
            .map(|(key, m)| (key.clone(), m.clone()))
            .collect();

        let (lock, unmet) = loop {
            let (mods, resolved) = self.resolve_mods(&loaded, &previous, &known).await?;
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

    /// Everything worth warning the user about in `profile`, from what's
    /// already known: it doesn't lock or go online.
    pub fn issues(&self, profile: &ProfileRef) -> Result<Vec<Issue>> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let env = self.game_env(&target)?;
        let mut issues: Vec<Issue> = match self.load_lock(profile)? {
            Some(lock) => {
                let mut issues: Vec<Issue> = self
                    .unmet_requirements(&lock)
                    .into_iter()
                    .map(Issue::from)
                    .collect();
                if let Some(env) = &env {
                    issues.extend(self.incompatible(&loaded, &lock, env)?);
                }
                issues
            }
            None => Vec::new(),
        };
        let state = self.deploy_state(&target)?;
        if state.profile.as_deref() == Some(profile.to_string().as_str())
            && let Some(deployed) = state.env
            && let Some(now) = env
            && deployed != now
        {
            let name = self.registry().game(&loaded.game)?.value.name.clone();
            issues.push(Issue::game_changed(&name, deployed, now));
        }
        Ok(issues)
    }

    /// The mods deploying `loaded` would place that say they don't work
    /// with the installed version of the game.
    fn incompatible(&self, loaded: &Profile, lock: &Lockfile, env: &GameEnv) -> Result<Vec<Issue>> {
        let Some(version) = &env.version else {
            return Ok(Vec::new());
        };
        let overrides: HashMap<String, &GameVersions> = loaded
            .mods
            .iter()
            .filter_map(|m| Some((mod_key(&m.source, &m.id), m.game_versions.as_ref()?)))
            .collect();
        let registry = self.registry();
        let name = &registry.game(&loaded.game)?.value.name;
        Ok(deploy_order(loaded, lock)
            .into_iter()
            .filter_map(|m| {
                let supports = overrides
                    .get(&m.key())
                    .copied()
                    .or(m.game_versions.as_ref())?;
                (supports.matches(version) == Some(false))
                    .then(|| Issue::incompatible_game(&m.name, name, version, supports.clone()))
            })
            .collect())
    }

    /// One pass of [`Patchwork::lock`]: walks the profile's mods and their
    /// dependencies, reusing entries from `previous` where it can. Also
    /// returns the keys of the mods it had to resolve.
    ///
    /// `known` is the whole old lockfile, even when updating: a mod that
    /// resolves to the same download it was locked to isn't downloaded again.
    ///
    /// The walk goes a level at a time, breadth first, locking each level's
    /// mods [`LOCK_CONCURRENCY`] at a time. Mods come out in the same order
    /// as if they'd been locked one by one.
    async fn resolve_mods(
        &self,
        loaded: &Profile,
        previous: &HashMap<String, LockedMod>,
        known: &HashMap<String, LockedMod>,
    ) -> Result<(Vec<LockedMod>, HashSet<String>)> {
        let mut level: Vec<(ModSpec, Option<String>)> = loaded
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
        let mut pending = Vec::new();

        while !level.is_empty() {
            let todo: Vec<_> = level
                .into_iter()
                .filter(|(spec, _)| seen.insert(mod_key(&spec.source, &spec.id)))
                .collect();
            let locked: Vec<Locking> = stream::iter(todo)
                .map(|(spec, name)| self.lock_mod(loaded, spec, name, previous, known))
                .buffered(LOCK_CONCURRENCY)
                .try_collect()
                .await?;
            level = Vec::new();
            for locking in locked {
                let dependencies = match locking {
                    Locking::Locked(locked, resolved) => {
                        resolved_keys.extend(resolved);
                        let dependencies = locked.dependencies.clone();
                        mods.push(*locked);
                        dependencies
                    }
                    Locking::Pending(waiting, dependencies) => {
                        pending.extend(waiting);
                        dependencies
                    }
                };
                for dep in dependencies {
                    if let Some((source, id)) = dep.split_once(':') {
                        let spec = ModSpec {
                            source: source.to_owned(),
                            id: id.to_owned(),
                            version: None,
                        };
                        level.push((spec, None));
                    }
                }
            }
        }
        // everything else is fetched by now, so the user can see to all of
        // these at once
        if !pending.is_empty() {
            return Err(Error::NeedsDownload(pending));
        }
        Ok((mods, resolved_keys))
    }

    /// Locks one mod for [`Patchwork::resolve_mods`]: keeps its entry from
    /// `previous` if that still fits, or resolves it again.
    async fn lock_mod(
        &self,
        loaded: &Profile,
        spec: ModSpec,
        name: Option<String>,
        previous: &HashMap<String, LockedMod>,
        known: &HashMap<String, LockedMod>,
    ) -> Result<Locking> {
        let key = mod_key(&spec.source, &spec.id);
        if let Some(old) = previous.get(&key)
            && old.requested == spec.version
        {
            let mut old = old.clone();
            if let Some(name) = name {
                old.name = name;
            }
            if !self.store.contains(&old.hash, old.format) {
                self.reporter.report(Event::Fetching {
                    name: old.name.clone(),
                });
                let fetched = self
                    .fetch(
                        &loaded.game,
                        &spec.source,
                        &old.id,
                        &old.name,
                        &old.artifact,
                        old.format,
                    )
                    .await;
                let hash = match fetched {
                    Err(Error::NeedsDownload(waiting)) => {
                        return Ok(Locking::Pending(waiting, old.dependencies));
                    }
                    fetched => fetched?,
                };
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
            return Ok(Locking::Locked(Box::new(old), None));
        }

        let label = name.clone().unwrap_or_else(|| key.clone());
        self.reporter.report(Event::Resolving { name: label });
        let resolved = self.resolve(&loaded.game, &spec).await?;
        let name = name.unwrap_or_else(|| resolved.name.clone());
        let dependencies: Vec<String> = resolved
            .dependencies
            .iter()
            .map(|d| mod_key(&d.source, &d.id))
            .collect();
        // a URL is taken to always serve the same file, which
        // holds for versioned download links, and so is a source's
        // own file id; paths can change under us, and are cheap to
        // ingest again anyway
        let fetched = match known.get(&key) {
            Some(old)
                if !matches!(old.artifact, ArtifactLocation::Path { .. })
                    && old.artifact == resolved.artifact
                    && old.format == resolved.format
                    && self.store.contains(&old.hash, old.format) =>
            {
                Ok(old.hash)
            }
            _ => {
                self.reporter.report(Event::Fetching { name: name.clone() });
                self.fetch(
                    &loaded.game,
                    &spec.source,
                    &resolved.id,
                    &name,
                    &resolved.artifact,
                    resolved.format,
                )
                .await
            }
        };
        let hash = match fetched {
            Err(Error::NeedsDownload(waiting)) => {
                return Ok(Locking::Pending(waiting, dependencies));
            }
            fetched => fetched?,
        };
        let game_versions = match resolved.game_versions {
            Some(versions) => {
                let registry = self.registry();
                let game = &registry.game(&loaded.game)?.value;
                let specific = game.versions.files.is_empty() || {
                    let files =
                        self.store
                            .files(&hash, resolved.format, &resolved.artifact.file_name())?;
                    compat::is_version_specific(game, files.iter().map(|f| &f.rel))
                };
                specific.then_some(versions)
            }
            None => None,
        };
        let locked = LockedMod {
            name,
            source: spec.source,
            id: resolved.id,
            requested: spec.version,
            version: resolved.version,
            hash,
            format: resolved.format,
            artifact: resolved.artifact,
            dependencies,
            requires: resolved
                .dependencies
                .into_iter()
                .filter(|d| !d.req.is_any())
                .map(|d| (mod_key(&d.source, &d.id), d.req))
                .collect(),
            game_versions,
        };
        Ok(Locking::Locked(Box::new(locked), Some(key)))
    }

    /// Puts the artifact of mod `id` (named `name` in the profile) from
    /// `source` into the store.
    async fn fetch(
        &self,
        game: &str,
        source: &str,
        id: &str,
        name: &str,
        artifact: &ArtifactLocation,
        format: ArtifactFormat,
    ) -> Result<ContentHash> {
        match artifact {
            ArtifactLocation::Path { path } => self.store.ingest_path(path, format),
            ArtifactLocation::Url { url } => self.fetch_url(url, format).await,
            ArtifactLocation::Source { file, .. } => {
                let file = SourceFile {
                    game: game.to_owned(),
                    source: source.to_owned(),
                    id: id.to_owned(),
                    file: file.clone(),
                };
                if let Some(hash) = self.known_source_file(&file, format) {
                    return Ok(hash);
                }
                let registry = self.registry();
                let game = &registry.game(game)?.value;
                let settings = self.settings();
                let ctx = SourceContext {
                    game,
                    config: game.sources.get(source),
                    settings: settings.source(source),
                    env: None,
                };
                match self.source(source)?.download(&ctx, id, &file.file).await? {
                    Download::Url(url) => {
                        let hash = self.fetch_url(&url, format).await?;
                        self.remember_source_file(&file, hash)?;
                        Ok(hash)
                    }
                    Download::NeedsUser { page } => {
                        Err(Error::NeedsDownload(vec![PendingDownload {
                            name: name.to_owned(),
                            page,
                            file,
                        }]))
                    }
                }
            }
        }
    }

    async fn fetch_url(&self, url: &str, format: ArtifactFormat) -> Result<ContentHash> {
        if format == ArtifactFormat::Dir {
            return Err(Error::invalid(format!(
                "{url} can't be downloaded as a directory"
            )));
        }
        // downloads run side by side while locking, so each gets a
        // file of its own, removed when it's dropped
        let dir = self.paths.downloads();
        fs::create_dir_all(&dir).at(&dir)?;
        let tmp = tempfile::Builder::new()
            .suffix(".tmp")
            .tempfile_in(&dir)
            .at(&dir)?
            .into_temp_path();
        download(url, &tmp).await?;
        self.store.ingest_path(&tmp, format)
    }

    // Downloads the user finishes

    /// Takes a file the user downloaded themselves for a
    /// [`PendingDownload`]. The next lock uses it. There's no telling
    /// whether it's really that file, so it's up to the user.
    pub fn provide(&self, file: &SourceFile, path: &Path) -> Result<ContentHash> {
        self.source(&file.source)?;
        self.registry().game(&file.game)?;
        // source files are single files, which are stored the same way
        // whether they're archives or not
        let hash = self.store.ingest_path(path, ArtifactFormat::File)?;
        self.remember_source_file(file, hash)?;
        Ok(hash)
    }

    /// Downloads the file that a link from the user's browser is for, like a
    /// Nexus `nxm://` link, so the next lock uses it. `Ok(None)` means no
    /// source recognizes the link.
    pub async fn accept_link(&self, link: &str) -> Result<Option<SourceFile>> {
        let registry = self.registry();
        for (source_id, source) in &self.sources {
            for entry in registry.games() {
                let game = &entry.value;
                let Some(config) = game.sources.get(source_id) else {
                    continue;
                };
                let settings = self.settings();
                let ctx = SourceContext {
                    game,
                    config: Some(config),
                    settings: settings.source(source_id),
                    env: None,
                };
                let Some(handoff) = source.accept_link(&ctx, link).await? else {
                    continue;
                };
                let file = SourceFile {
                    game: game.id.clone(),
                    source: source_id.clone(),
                    id: handoff.id,
                    file: handoff.file,
                };
                let hash = self.fetch_url(&handoff.url, ArtifactFormat::File).await?;
                self.remember_source_file(&file, hash)?;
                return Ok(Some(file));
            }
        }
        Ok(None)
    }

    /// The store entry for `file`, if it's been downloaded and is still
    /// there.
    fn known_source_file(&self, file: &SourceFile, format: ArtifactFormat) -> Option<ContentHash> {
        let known: KnownSourceFile = fsutil::read_json(&self.source_file_entry(file))
            .ok()
            .flatten()?;
        (known.file == *file && self.store.contains(&known.hash, format)).then_some(known.hash)
    }

    fn remember_source_file(&self, file: &SourceFile, hash: ContentHash) -> Result<()> {
        let known = KnownSourceFile {
            file: file.clone(),
            hash,
        };
        fsutil::write_json(&self.source_file_entry(file), &known)
    }

    fn source_file_entry(&self, file: &SourceFile) -> PathBuf {
        let id = format!("{}\0{}", file.id, file.file);
        cache_file(&self.paths.source_files(), &file.game, &file.source, &id)
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
            settings: self.settings(),
            reporter: self.reporter.as_ref(),
            scratch,
        }
    }

    /// Exports a profile in the given format. Formats that need the exact
    /// versions lock the profile first, which may fetch mods.
    pub async fn export_profile(&self, profile: &ProfileRef, format: &str) -> Result<Vec<u8>> {
        let format = self.format(format)?;
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
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
            hidden_tags: Some(loaded.hidden_tags),
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
        let registry = self.registry();
        let explicit = match options.game {
            Some(id) => {
                let game = &registry.game(id)?.value;
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
            let mut supported = registry
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
        self.registry().game(&game)?;
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
        if let Some(tags) = shared.hidden_tags {
            profile.hidden_tags = tags;
        }
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

    /// Where `profile`'s mods are deployed to.
    pub fn deploy_target(&self, profile: &Profile) -> Result<DeployTarget> {
        Ok(DeployTarget::Game(profile.game.clone()))
    }

    /// The directory mods for `target` go in, if it's been set up: its
    /// `game` root.
    pub fn target_root(&self, target: &DeployTarget) -> Result<Option<PathBuf>> {
        match target {
            DeployTarget::Game(game) => Ok(self.game_install(game)?.map(|i| i.path)),
        }
    }

    /// Every folder mods for `target` can go in, if it's been set up.
    pub fn target_roots(&self, target: &DeployTarget) -> Result<Option<Roots>> {
        Ok(self.target_root(target)?.map(|root| Roots::game(&root)))
    }

    pub fn deploy_state(&self, target: &DeployTarget) -> Result<DeployState> {
        let path = self.deploy_state_file(target);
        if let Some(state) = fsutil::read_json(&path)? {
            return Ok(state);
        }
        let roots = self.target_roots(target)?;
        let roots = roots.unwrap_or_else(|| Roots::game(Path::new("")));
        Ok(DeployState::empty(target.game(), &roots))
    }

    fn deploy_state_file(&self, target: &DeployTarget) -> PathBuf {
        self.paths
            .deployments()
            .join(format!("{}.json", target.key()))
    }

    /// Locks the profile if needed and works out what deploying it would
    /// change, without changing anything.
    pub async fn plan_deploy(&self, profile: &ProfileRef) -> Result<DeployPlan> {
        let lock = self.lock(profile, false).await?;
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let entry = registry.game(&loaded.game)?;
        let game = &entry.value;
        let target = self.deploy_target(&loaded)?;
        let Some(roots) = self.target_roots(&target)? else {
            let fix = match Locator::scan().find(game).as_slice() {
                [found] => format!(
                    "it looks to be in {}, so run `patchwork game detect {} --set` to use that, or `patchwork game set-path {} <dir>`",
                    found.path.display(),
                    game.id,
                    game.id
                ),
                _ => format!("run `patchwork game set-path {} <dir>`", game.id),
            };
            return Err(Error::invalid(format!(
                "patchwork doesn't know where {} is installed; {fix}",
                game.name
            )));
        };
        let root = roots.game_path().to_owned();
        let deployer = self.deployer(&game.deployer.kind)?;
        let (config, link, overrides) = self.deploy_config(game)?;

        let ctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let mut prev = self.deploy_state(&target)?;
        if prev.files.is_empty() {
            prev = DeployState::empty(&loaded.game, &roots);
        } else if let Some((moved, was)) = prev.moved_root(&roots) {
            let now = roots
                .get(&moved)
                .map_or_else(|| "nowhere".to_owned(), |p| p.display().to_string());
            return Err(Error::invalid(format!(
                "mods are deployed to {}, but {} is now set to {now}; run `patchwork undeploy {}` first",
                was.display(),
                game.name,
                game.id
            )));
        } else {
            prev.set_roots(&roots);
        }

        let order = deploy_order(&loaded, &lock);
        let fingerprint = deployer.fingerprint(&ctx, &GameFiles::new(&root, &prev))?;
        let inputs = deploy_inputs(
            profile,
            &registry,
            entry.hash,
            &overrides,
            &roots,
            &order,
            fingerprint,
        );
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

        let env = GameEnv::detect(game, &root);
        let desired = deployer.plan(&ctx, &mods)?;
        let mut placements =
            deploy::resolve(deployer.kind(), desired.placements, &mods, None, &roots)?;
        let mut warnings = desired.warnings;
        for issue in self.incompatible(&loaded, &lock, &env)? {
            warnings.push(issue.message);
        }
        if deployer.builds(&ctx) {
            let (built, built_warnings) =
                self.build(deployer, &ctx, &target, inputs, &prev, &mods)?;
            placements.extend(built);
            warnings.extend(built_warnings);
        }
        let mut plan = deploy::plan(
            target,
            &prev,
            placements,
            warnings,
            Some(profile.to_string()),
            link,
        )?;
        plan.inputs = Some(inputs);
        plan.env = Some(env);
        Ok(plan)
    }

    /// Whether deploying `profile` would change anything, worked out from
    /// what was recorded at the last deploy rather than a whole plan, so
    /// it's cheap and never touches the network.
    ///
    /// Anything that might change what's deployed (enabling, reordering or
    /// updating mods, or a changed game definition or ruleset) makes it
    /// [`DeployStatus::Stale`], even when the files would come out the same.
    /// So does the game changing, as far as [`GameEnv`] can tell. Other
    /// changes made to the game directory aren't noticed.
    pub fn deploy_status(&self, profile: &ProfileRef) -> Result<DeployStatus> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let state = self.deploy_state(&target)?;
        match state.profile.as_deref() {
            None => return Ok(DeployStatus::NotDeployed),
            Some(p) if p != profile.to_string() => return Ok(DeployStatus::OtherProfile),
            Some(_) => {}
        }
        let (Some(recorded), Some(lock), Some(roots)) = (
            state.inputs,
            self.load_lock(profile)?,
            self.target_roots(&target)?,
        ) else {
            return Ok(DeployStatus::Stale);
        };
        // Deploying would lock again first, so a mod that isn't locked the
        // way the profile asks could deploy as something else.
        let locked = loaded.mods.iter().filter(|m| m.enabled).all(|m| {
            let key = mod_key(&m.source, &m.id);
            lock.mods
                .iter()
                .any(|l| l.key() == key && l.requested == m.version)
        });
        if !locked {
            return Ok(DeployStatus::Stale);
        }
        let registry = self.registry();
        let entry = registry.game(&loaded.game)?;
        let game = &entry.value;
        let deployer = self.deployer(&game.deployer.kind)?;
        let (config, _, overrides) = self.deploy_config(game)?;
        let ctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let root = roots.game_path();
        let fingerprint = deployer.fingerprint(&ctx, &GameFiles::new(root, &state))?;
        let order = deploy_order(&loaded, &lock);
        let inputs = deploy_inputs(
            profile,
            &registry,
            entry.hash,
            &overrides,
            &roots,
            &order,
            fingerprint,
        );
        // Not part of the inputs, so that deploys from before it was
        // recorded don't all look stale.
        let same_game = state
            .env
            .as_ref()
            .is_none_or(|env| *env == GameEnv::detect(game, root));
        Ok(if inputs == recorded && same_game {
            DeployStatus::Deployed
        } else {
            DeployStatus::Stale
        })
    }

    /// Works out how to remove everything patchwork deployed to `target`.
    pub fn plan_undeploy(&self, target: &DeployTarget) -> Result<DeployPlan> {
        let registry = self.registry();
        let (_, link, _) = self.deploy_config(&registry.game(target.game())?.value)?;
        let prev = self.deploy_state(target)?;
        deploy::plan(target.clone(), &prev, Vec::new(), Vec::new(), None, link)
    }

    pub fn apply(&self, plan: &DeployPlan) -> Result<DeployState> {
        let target = &plan.target;
        let mut state = self.deploy_state(target)?;
        if state.files.is_empty() {
            state = DeployState::empty(target.game(), &plan.roots);
        }
        let backups = self.paths.backups().join(target.key());
        let result = deploy::apply(plan, &mut state, &backups, self.reporter.as_ref());
        // Saved even on failure, so whatever did get deployed can be undone.
        fsutil::write_json(&self.deploy_state_file(target), &state)?;
        self.prune_generated(target, &state, plan.inputs);
        result?;
        if let Some(profile) = plan.profile.as_deref() {
            self.save_deployed(&profile.parse()?)?;
        }
        Ok(state)
    }

    /// Runs `deployer`'s build for a deploy with these `inputs`, or reuses
    /// what an earlier build with the same inputs made. Returns the build's
    /// placements, resolved, and its warnings.
    fn build(
        &self,
        deployer: &dyn Deployer,
        ctx: &DeployContext<'_>,
        target: &DeployTarget,
        inputs: ContentHash,
        prev: &DeployState,
        mods: &[InstalledMod],
    ) -> Result<(Vec<Resolved>, Vec<String>)> {
        let dir = self.paths.generated().join(target.key());
        let out = dir.join(inputs.to_string());
        let record = dir.join(format!("{inputs}.json"));
        // Anything wrong with an earlier build (a record from an older
        // version, or files that have since been deleted) just means
        // building again.
        if let Ok(Some(built)) = fsutil::read_json::<DesiredState>(&record)
            && let Ok(placements) = deploy::resolve(
                deployer.kind(),
                built.placements,
                mods,
                Some(&out),
                &prev.roots(),
            )
        {
            return Ok((placements, built.warnings));
        }
        // Left over from a build that didn't finish.
        if out.exists() {
            fs::remove_dir_all(&out).at(&out)?;
        }
        fs::create_dir_all(&out).at(&out)?;
        let cache = self.paths.deployer_cache(deployer.kind());
        fs::create_dir_all(&cache).at(&cache)?;
        let build = BuildContext {
            game: GameFiles::new(&prev.root, prev),
            out: &out,
            cache: &cache,
            reporter: self.reporter.as_ref(),
        };
        let built = match deployer.build(ctx, &build, mods) {
            Ok(built) => built,
            Err(e) => {
                let _ = fs::remove_dir_all(&out);
                return Err(e);
            }
        };
        let placements = match deploy::resolve(
            deployer.kind(),
            built.placements.clone(),
            mods,
            Some(&out),
            &prev.roots(),
        ) {
            Ok(placements) => placements,
            Err(e) => {
                let _ = fs::remove_dir_all(&out);
                return Err(e);
            }
        };
        fsutil::write_json(&record, &built)?;
        Ok((placements, built.warnings))
    }

    /// Deletes built files for `target` that nothing deployed comes from,
    /// keeping those for `keep` (the deploy just applied) for next time.
    fn prune_generated(
        &self,
        target: &DeployTarget,
        state: &DeployState,
        keep: Option<ContentHash>,
    ) {
        let dir = self.paths.generated().join(target.key());
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let keep = keep.map(|k| k.to_string());
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let build = name.strip_suffix(".json").unwrap_or(name);
            let out = dir.join(build);
            let used = keep.as_deref() == Some(build)
                || state.files.values().any(|f| f.src.starts_with(&out));
            if used {
                continue;
            }
            // Best effort: whatever's left is tried again next time.
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }

    /// Remembers the profile as it is now, as what was last deployed.
    fn save_deployed(&self, profile: &ProfileRef) -> Result<()> {
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(());
        };
        let deployed = Deployed {
            mods: self.load_profile(profile)?.mods,
            lock,
        };
        fsutil::write_toml(&self.deployed_file(profile), &deployed)
    }

    fn load_deployed(&self, profile: &ProfileRef) -> Result<Option<Deployed>> {
        fsutil::read_toml(&self.deployed_file(profile))
    }

    /// Whether the profile's mods or their locked versions have changed
    /// since it was last deployed, so [`Patchwork::revert`] would do
    /// anything. Always false for a profile that's never been deployed.
    ///
    /// Hidden tags don't count, since they don't change what's deployed.
    pub fn has_changes(&self, profile: &ProfileRef) -> Result<bool> {
        let Some(deployed) = self.load_deployed(profile)? else {
            return Ok(false);
        };
        if self.load_profile(profile)?.mods != deployed.mods {
            return Ok(true);
        }
        let lock = self.load_lock(profile)?;
        Ok(lock.map(|l| toml_eq(&l, &deployed.lock)) != Some(true))
    }

    /// Puts the profile's mods and lockfile back how they were when it was
    /// last deployed, undoing everything done to it since. Nothing in the
    /// game changes; the profile just matches what's deployed again.
    pub fn revert(&self, profile: &ProfileRef) -> Result<()> {
        let deployed = self.load_deployed(profile)?.ok_or_else(|| {
            Error::invalid(format!(
                "{profile} hasn't been deployed, so there's nothing to revert to"
            ))
        })?;
        let mut loaded = self.load_profile(profile)?;
        loaded.mods = deployed.mods;
        fsutil::write_toml(&self.lock_file(profile), &deployed.lock)?;
        self.save_profile(profile, &loaded)
    }
}

/// Streams `url` into the file at `dest`.
async fn download(url: &str, dest: &Path) -> Result<()> {
    let reqwest_error = |result| Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let mut response = crate::http_get(url)
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

/// A profile's mods and lockfile at its last deploy.
#[derive(Debug, Serialize, Deserialize)]
struct Deployed {
    #[serde(default, rename = "mod")]
    mods: Vec<ProfileMod>,
    lock: Lockfile,
}

/// Whether two values serialize the same, for types that can't be compared
/// directly.
fn toml_eq<T: Serialize>(a: &T, b: &T) -> bool {
    match (toml::to_string(a), toml::to_string(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Where a profile stands against what's deployed to its
/// [`DeployTarget`], from [`Patchwork::deploy_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeployStatus {
    /// Nothing's deployed to the target.
    NotDeployed,
    /// A different profile is deployed to the target.
    OtherProfile,
    /// This profile is deployed, but deploying again would change something,
    /// or the game has changed since (say, by updating).
    Stale,
    /// This profile is deployed as it is now.
    Deployed,
}

/// The locked mods deploying `loaded` places, in load order.
fn deploy_order<'a>(loaded: &Profile, lock: &'a Lockfile) -> Vec<&'a LockedMod> {
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
    order
}

/// Hashes everything a deploy depends on, for [`DeployPlan::inputs`].
///
/// Deployers can read any registry data, so all of it is included rather
/// than only what this game uses. Patchwork's version stands in for changes
/// to the deployers themselves.
/// The settings each source takes, by id.
fn source_specs(sources: &BTreeMap<String, Arc<dyn Source>>) -> BTreeMap<&str, &[SettingSpec]> {
    sources
        .iter()
        .map(|(id, s)| (id.as_str(), s.settings()))
        .filter(|(_, specs)| !specs.is_empty())
        .collect()
}

fn deploy_inputs(
    profile: &ProfileRef,
    registry: &Registry,
    game: ContentHash,
    deployer_settings: &toml::Table,
    roots: &Roots,
    order: &[&LockedMod],
    fingerprint: Option<ContentHash>,
) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    field(env!("CARGO_PKG_VERSION").as_bytes());
    field(profile.to_string().as_bytes());
    field(game.to_string().as_bytes());
    // Only when there are any, so deploys from before they existed don't
    // all look stale.
    if !deployer_settings.is_empty() {
        field(deployer_settings.to_string().as_bytes());
    }
    // The game root on its own, as it was before there were others, so
    // existing deploys don't all look stale.
    for (id, path) in roots.iter() {
        if !id.is_game() {
            field(id.as_str().as_bytes());
        }
        field(path.as_os_str().as_encoded_bytes());
    }
    // Only when there is one, so deploys from before fingerprints existed
    // don't all look stale.
    if let Some(fingerprint) = fingerprint {
        field(fingerprint.to_string().as_bytes());
    }
    for (kind, id, hash) in registry.data_hashes() {
        field(kind.as_bytes());
        field(id.as_bytes());
        field(hash.to_string().as_bytes());
    }
    for m in order {
        field(m.key().as_bytes());
        field(m.name.as_bytes());
        field(m.hash.to_string().as_bytes());
        field(format!("{:?}", m.format).as_bytes());
        field(m.artifact.file_name().as_bytes());
    }
    hasher.finalize().into()
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

mod profiles;

#[cfg(test)]
mod tests;
