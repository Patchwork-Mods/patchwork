use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use futures_util::{StreamExt, TryStreamExt, stream};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::deploy::{
    self, DeployContext, DeployPlan, DeployState, DeployTarget, Deployer, InstalledMod,
};
use crate::error::IoContext;
use crate::event::{Event, Reporter, Silent};
use crate::fsutil::{self, StateLock};
use crate::game::{GAME_SCHEMA, GameDef, validate_game_id};
use crate::hash::ContentHash;
use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod, Lockfile, UnmetRequirement, mod_key};
use crate::matching::{self, ModText};
use crate::order::{self, ModFacts, Position, SortKey};
use crate::paths::Paths;
use crate::profile::{Profile, ProfileMod, ProfileRef, no_such_mod, validate_name};
use crate::registry::Registry;
use crate::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use crate::share::{Bundled, FormatContext, ProfileFormat, SharedProfile, sanitize_name};
use crate::source::{
    ArtifactFormat, ArtifactLocation, Download, ModSpec, PendingDownload, Source, SourceContext,
};
use crate::store::Store;
use crate::{Error, Result};

/// A game's settings on this machine, as opposed to its [`GameDef`], which is
/// the same everywhere. Saved to `<data>/games/<id>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInstall {
    /// Where the game is installed.
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
        let patchwork = Patchwork {
            store: Store::new(self.paths.store()),
            registry: RwLock::new(Arc::new(registry)),
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
    /// Swapped out whole when game definitions change, so anything already
    /// working from the old one carries on undisturbed.
    registry: RwLock<Arc<Registry>>,
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
        Ok(())
    }

    pub fn has_source(&self, id: &str) -> bool {
        self.sources.contains_key(id)
    }

    // Games

    pub fn game_install(&self, game: &str) -> Result<Option<GameInstall>> {
        fsutil::read_toml(&self.install_file(game))
    }

    pub fn set_game_path(&self, game: &str, path: &Path) -> Result<GameInstall> {
        self.set_game_install(
            game,
            GameInstall {
                path: path.to_owned(),
            },
        )
    }

    /// Replaces a game's settings, after checking them. Returns them as
    /// saved, with the path made absolute.
    pub fn set_game_install(&self, game: &str, install: GameInstall) -> Result<GameInstall> {
        self.registry().game(game)?;
        let GameInstall { path } = install;
        let path = path.canonicalize().at(&path)?;
        if !path.is_dir() {
            return Err(Error::invalid(format!(
                "{} isn't a directory",
                path.display()
            )));
        }
        let state = self.deploy_state(&DeployTarget::Game(game.to_owned()))?;
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

    fn known_versions_file(&self, game: &str, source: &str, id: &str) -> PathBuf {
        cache_file(&self.paths.known_versions(), game, source, id)
    }

    /// What a search would show for a mod, fetched from its source. It's
    /// remembered for [`Patchwork::known_summary`].
    pub async fn summary(&self, game: &str, source: &str, id: &str) -> Result<Option<ModSummary>> {
        let registry = self.registry();
        let def = &registry.game(game)?.value;
        let ctx = SourceContext {
            game: def,
            config: def.sources.get(source),
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

    fn source(&self, id: &str) -> Result<&dyn Source> {
        self.sources
            .get(id)
            .map(|s| s.as_ref())
            .ok_or_else(|| Error::UnknownSource(id.to_owned()))
    }

    async fn resolve(&self, game: &str, spec: &ModSpec) -> Result<crate::source::ResolvedMod> {
        let source = self.source(&spec.source)?;
        let registry = self.registry();
        let game = &registry.game(game)?.value;
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
        let source = self.searchable(source)?;
        let registry = self.registry();
        let game = &registry.game(game)?.value;
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
        let registry = self.registry();
        let game = &registry.game(game)?.value;
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

        while !level.is_empty() {
            let todo: Vec<_> = level
                .into_iter()
                .filter(|(spec, _)| seen.insert(mod_key(&spec.source, &spec.id)))
                .collect();
            let locked: Vec<(LockedMod, Option<String>)> = stream::iter(todo)
                .map(|(spec, name)| self.lock_mod(loaded, spec, name, previous, known))
                .buffered(LOCK_CONCURRENCY)
                .try_collect()
                .await?;
            level = Vec::new();
            for (locked, resolved) in locked {
                resolved_keys.extend(resolved);
                for dep in &locked.dependencies {
                    if let Some((source, id)) = dep.split_once(':') {
                        let spec = ModSpec {
                            source: source.to_owned(),
                            id: id.to_owned(),
                            version: None,
                        };
                        level.push((spec, None));
                    }
                }
                mods.push(locked);
            }
        }
        Ok((mods, resolved_keys))
    }

    /// Locks one mod for [`Patchwork::resolve_mods`]: keeps its entry from
    /// `previous` if that still fits, or resolves it again. Also returns its
    /// key if it had to be resolved.
    async fn lock_mod(
        &self,
        loaded: &Profile,
        spec: ModSpec,
        name: Option<String>,
        previous: &HashMap<String, LockedMod>,
        known: &HashMap<String, LockedMod>,
    ) -> Result<(LockedMod, Option<String>)> {
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
                let hash = self
                    .fetch(
                        &loaded.game,
                        &spec.source,
                        &old.id,
                        &old.name,
                        &old.artifact,
                        old.format,
                    )
                    .await?;
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
            return Ok((old, None));
        }

        let label = name.clone().unwrap_or_else(|| key.clone());
        self.reporter.report(Event::Resolving { name: label });
        let resolved = self.resolve(&loaded.game, &spec).await?;
        let name = name.unwrap_or_else(|| resolved.name.clone());
        // a URL is taken to always serve the same file, which
        // holds for versioned download links, and so is a source's
        // own file id; paths can change under us, and are cheap to
        // ingest again anyway
        let hash = match known.get(&key) {
            Some(old)
                if !matches!(old.artifact, ArtifactLocation::Path { .. })
                    && old.artifact == resolved.artifact
                    && old.format == resolved.format
                    && self.store.contains(&old.hash, old.format) =>
            {
                old.hash
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
                .await?
            }
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
        };
        Ok((locked, Some(key)))
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
                let registry = self.registry();
                let game = &registry.game(game)?.value;
                let ctx = SourceContext {
                    game,
                    config: game.sources.get(source),
                };
                match self.source(source)?.download(&ctx, id, file).await? {
                    Download::Url(url) => self.fetch_url(&url, format).await,
                    Download::NeedsUser { page } => {
                        Err(Error::NeedsDownload(vec![PendingDownload {
                            name: name.to_owned(),
                            key: mod_key(source, id),
                            page,
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

    /// The directory mods for `target` go in, if it's been set up.
    pub fn target_root(&self, target: &DeployTarget) -> Result<Option<PathBuf>> {
        match target {
            DeployTarget::Game(game) => Ok(self.game_install(game)?.map(|i| i.path)),
        }
    }

    pub fn deploy_state(&self, target: &DeployTarget) -> Result<DeployState> {
        let path = self.deploy_state_file(target);
        if let Some(state) = fsutil::read_json(&path)? {
            return Ok(state);
        }
        let root = self.target_root(target)?.unwrap_or_default();
        Ok(DeployState::empty(target.game(), &root))
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
        let root = self.target_root(&target)?.ok_or_else(|| {
            Error::invalid(format!(
                "patchwork doesn't know where {} is installed; run `patchwork game set-path {} <dir>`",
                game.name, game.id
            ))
        })?;
        let deployer = self
            .deployers
            .get(&game.deployer.kind)
            .ok_or_else(|| Error::UnknownDeployer(game.deployer.kind.clone()))?;

        let order = deploy_order(&loaded, &lock);
        let inputs = deploy_inputs(profile, &registry, entry.hash, &root, &order);
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
            registry: &registry,
        };
        let desired = deployer.plan(&ctx, &mods)?;
        let prev = self.deploy_state(&target)?;
        let prev = if prev.files.is_empty() {
            DeployState::empty(&loaded.game, &root)
        } else if prev.root != root {
            return Err(Error::invalid(format!(
                "mods are deployed to {}, but {} is now set to {}; run `patchwork undeploy {}` first",
                prev.root.display(),
                game.name,
                root.display(),
                game.id
            )));
        } else {
            prev
        };
        let mut plan = deploy::plan(target, &prev, desired, Some(profile.to_string()), game.link);
        plan.inputs = Some(inputs);
        Ok(plan)
    }

    /// Whether deploying `profile` would change anything, worked out from
    /// what was recorded at the last deploy rather than a whole plan, so
    /// it's cheap and never touches the network.
    ///
    /// Anything that might change what's deployed (enabling, reordering or
    /// updating mods, or a changed game definition or ruleset) makes it
    /// [`DeployStatus::Stale`], even when the files would come out the same.
    /// Changes made to the game directory by hand aren't noticed.
    pub fn deploy_status(&self, profile: &ProfileRef) -> Result<DeployStatus> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let state = self.deploy_state(&target)?;
        match state.profile.as_deref() {
            None => return Ok(DeployStatus::NotDeployed),
            Some(p) if p != profile.to_string() => return Ok(DeployStatus::OtherProfile),
            Some(_) => {}
        }
        let (Some(recorded), Some(lock), Some(root)) = (
            state.inputs,
            self.load_lock(profile)?,
            self.target_root(&target)?,
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
        let game = registry.game(&loaded.game)?;
        let order = deploy_order(&loaded, &lock);
        let inputs = deploy_inputs(profile, &registry, game.hash, &root, &order);
        Ok(if inputs == recorded {
            DeployStatus::Deployed
        } else {
            DeployStatus::Stale
        })
    }

    /// Works out how to remove everything patchwork deployed to `target`.
    pub fn plan_undeploy(&self, target: &DeployTarget) -> Result<DeployPlan> {
        let link = self.registry().game(target.game())?.value.link;
        let prev = self.deploy_state(target)?;
        Ok(deploy::plan(
            target.clone(),
            &prev,
            Default::default(),
            None,
            link,
        ))
    }

    pub fn apply(&self, plan: &DeployPlan) -> Result<DeployState> {
        let target = &plan.target;
        let mut state = self.deploy_state(target)?;
        if state.files.is_empty() {
            state = DeployState::empty(target.game(), &plan.root);
        }
        let backups = self.paths.backups().join(target.key());
        let result = deploy::apply(plan, &mut state, &backups, self.reporter.as_ref());
        // Saved even on failure, so whatever did get deployed can be undone.
        fsutil::write_json(&self.deploy_state_file(target), &state)?;
        result?;
        if let Some(profile) = plan.profile.as_deref() {
            self.save_deployed(&profile.parse()?)?;
        }
        Ok(state)
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
    /// This profile is deployed, but deploying again would change something.
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
fn deploy_inputs(
    profile: &ProfileRef,
    registry: &Registry,
    game: ContentHash,
    root: &Path,
    order: &[&LockedMod],
) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    field(env!("CARGO_PKG_VERSION").as_bytes());
    field(profile.to_string().as_bytes());
    field(game.to_string().as_bytes());
    field(root.as_os_str().as_encoded_bytes());
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
