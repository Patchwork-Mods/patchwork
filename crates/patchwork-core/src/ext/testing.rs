//! A harness for testing extensions end to end: a game with mods, set up in
//! a temporary directory and run through the same [`Patchwork`] the
//! frontends use. Behind the `testing` feature, so extension crates add
//! `patchwork-core = { workspace = true, features = ["testing"] }` to their
//! dev-dependencies.
//!
//! ```ignore
//! let t = TestGame::builtin("skyrim-se")
//!     .extension(Bethesda)
//!     .extension(Fomod)
//!     .mod_from_dir("SkyUI", "tests/fixtures/skyui")
//!     .game_file("Data/Skyrim.esm", b"...");
//!
//! let order = t.order("bethesda.plugins")?;
//! t.answer_options("SkyUI", &answers)?;
//! let plan = t.deploy()?;
//! assert!(plan.places("Data/SkyUI_SE.esp"));
//! assert!(t.read("appdata:plugins.txt")?.contains("*SkyUI_SE.esp"));
//! ```
//!
//! Nothing is set up until the game is first used, so the builder methods
//! ([`TestGame::extension`], [`TestGame::mod_files`] and so on) have to come
//! first. Setting up panics if it fails, since that's a broken test rather
//! than something to assert on; everything after returns [`Result`]s.
//!
//! Everything lives under one temporary directory, removed when the
//! [`TestGame`] is dropped: patchwork's own state, the game's folder and its
//! other roots, each of which is set to a folder of its own (see
//! [`TestGame::root`]) so no test can touch the real ones. Paths are written
//! the way deploy plans show them: `Data/Skyrim.esm` in the game's folder,
//! `appdata:plugins.txt` in its `appdata` root.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Extension, StateOf};
use crate::deploy::{DeployOp, DeployPlan, RootId};
use crate::error::IoContext;
use crate::game::{GAME_SCHEMA, GameDef};
use crate::issue::Issue;
use crate::list::{List, ListEntry};
use crate::options::Answer;
use crate::paths::Paths;
use crate::profile::ProfileRef;
use crate::source::{
    ArtifactFormat, ArtifactLocation, ModSpec, ResolvedMod, Source, SourceContext,
};
use crate::{Builder, Error, GameInstall, ModOptionsView, Patchwork, Result};

/// The id of the source the harness adds mods with. A mod's id is its name.
pub const SOURCE: &str = "test";

/// A game with mods and extensions, for testing extensions against. See
/// the [module docs](self).
pub struct TestGame {
    pending: Mutex<Option<Pending>>,
    ready: OnceLock<Ready>,
}

enum GameSpec {
    Builtin(String),
    Def(Box<GameDef>),
    Toml(String),
}

enum ModFrom {
    Path(PathBuf),
    Files(Vec<(String, Vec<u8>)>),
}

type AddExtension = Box<dyn FnOnce(Builder) -> Builder + Send>;

struct Pending {
    game: GameSpec,
    extensions: Vec<AddExtension>,
    mods: Vec<(String, ModFrom)>,
    files: Vec<(String, Vec<u8>)>,
}

struct Ready {
    runtime: tokio::runtime::Runtime,
    pw: Patchwork,
    profile: ProfileRef,
    game: PathBuf,
    /// Every root but `game`, by id.
    roots: BTreeMap<String, PathBuf>,
    /// Last, so it's removed once everything using it has gone.
    _tmp: tempfile::TempDir,
}

impl TestGame {
    /// A game from the registry built into patchwork, by id.
    pub fn builtin(id: &str) -> Self {
        Self::with(GameSpec::Builtin(id.to_owned()))
    }

    /// A game defined in TOML, as in `registry/games/`. Its `schema` can be
    /// left out. Its id mustn't be a builtin game's.
    pub fn from_toml(toml: &str) -> Self {
        Self::with(GameSpec::Toml(toml.to_owned()))
    }

    /// A game from its definition. Its id mustn't be a builtin game's.
    pub fn from_def(def: GameDef) -> Self {
        Self::with(GameSpec::Def(Box::new(def)))
    }

    fn with(game: GameSpec) -> Self {
        Self {
            pending: Mutex::new(Some(Pending {
                game,
                extensions: Vec::new(),
                mods: Vec::new(),
                files: Vec::new(),
            })),
            ready: OnceLock::new(),
        }
    }

    fn pending(&mut self) -> &mut Pending {
        self.pending
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .expect("a TestGame's extensions, mods and files are added before it's used")
    }

    /// Registers `extension`, as [`Builder::extension`] does.
    pub fn extension(mut self, extension: impl Extension + 'static) -> Self {
        self.pending()
            .extensions
            .push(Box::new(move |b: Builder| b.extension(extension)));
        self
    }

    /// Adds a source, deployer or the like straight to the builder, for
    /// things that aren't in an extension.
    pub fn builder(mut self, add: impl FnOnce(Builder) -> Builder + Send + 'static) -> Self {
        self.pending().extensions.push(Box::new(add));
        self
    }

    /// Adds an enabled mod called `name` from a folder, archive or single
    /// file, as a local mod would be. Relative paths are from the current
    /// directory, which for `cargo test` is the crate's.
    pub fn mod_from_dir(mut self, name: &str, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        self.pending()
            .mods
            .push((name.to_owned(), ModFrom::Path(path)));
        self
    }

    /// Adds an enabled mod called `name` that's a folder of `files`, as
    /// paths in it and their contents.
    pub fn mod_files<P: AsRef<str>, B: AsRef<[u8]>>(
        mut self,
        name: &str,
        files: impl IntoIterator<Item = (P, B)>,
    ) -> Self {
        let files = files
            .into_iter()
            .map(|(p, b)| (p.as_ref().to_owned(), b.as_ref().to_owned()))
            .collect();
        self.pending()
            .mods
            .push((name.to_owned(), ModFrom::Files(files)));
        self
    }

    /// Puts a file in the game's folder before anything's deployed, as if it
    /// came with the game: `Data/Skyrim.esm`, or `<root>:<path>` for one of
    /// the game's other roots.
    pub fn game_file(mut self, path: &str, contents: impl AsRef<[u8]>) -> Self {
        let contents = contents.as_ref().to_owned();
        self.pending().files.push((path.to_owned(), contents));
        self
    }

    fn ready(&self) -> &Ready {
        self.ready.get_or_init(|| {
            let pending = self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .expect("a TestGame is only set up once");
            pending
                .start()
                .unwrap_or_else(|e| panic!("couldn't set up the test game: {e}"))
        })
    }

    /// The patchwork the game is set up in, for anything the harness
    /// doesn't wrap.
    pub fn patchwork(&self) -> &Patchwork {
        &self.ready().pw
    }

    /// The profile the mods are in.
    pub fn profile(&self) -> &ProfileRef {
        &self.ready().profile
    }

    /// Waits for `future`, such as one of [`Patchwork`]'s async methods.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.ready().runtime.block_on(future)
    }

    /// The game's folder.
    pub fn game_dir(&self) -> &Path {
        &self.ready().game
    }

    /// Where root `id` is: the game's folder for `game`, otherwise a folder
    /// of its own in the temporary directory. Panics if the game has no such
    /// root.
    pub fn root(&self, id: &str) -> &Path {
        let ready = self.ready();
        if id == RootId::GAME {
            return &ready.game;
        }
        ready
            .roots
            .get(id)
            .unwrap_or_else(|| panic!("the test game has no `{id}` root"))
    }

    /// Where `path` is on disk: `Data/Skyrim.esm` in the game's folder, or
    /// `<root>:<path>` in another root.
    pub fn path(&self, path: &str) -> PathBuf {
        let (root, rel) = split_root(path);
        self.root(root).join(rel)
    }

    /// What's in the file at `path` (see [`TestGame::path`]).
    pub fn read(&self, path: &str) -> Result<String> {
        let path = self.path(path);
        fs::read_to_string(&path).at(&path)
    }

    /// Whether there's a file at `path` (see [`TestGame::path`]).
    pub fn exists(&self, path: &str) -> bool {
        self.path(path).exists()
    }

    /// Ordered list `id` as the frontends show it.
    pub fn list(&self, id: &str) -> Result<List> {
        self.patchwork().list(self.profile(), id)
    }

    /// The names of what's enabled in ordered list `id`, in order.
    pub fn order(&self, id: &str) -> Result<Vec<String>> {
        Ok(self
            .list(id)?
            .items
            .into_iter()
            .filter(|i| i.enabled)
            .map(|i| i.item.name)
            .collect())
    }

    /// Saves `names` as ordered list `id`'s order, all enabled.
    pub fn set_order(&self, id: &str, names: &[&str]) -> Result<()> {
        let entries = names
            .iter()
            .map(|name| ListEntry {
                name: (*name).to_owned(),
                enabled: true,
            })
            .collect();
        self.patchwork().set_list(self.profile(), id, entries)
    }

    /// Turns `name` in ordered list `id` on or off.
    pub fn set_list_enabled(&self, id: &str, name: &str, enabled: bool) -> Result<()> {
        self.patchwork()
            .set_list_enabled(self.profile(), id, name, enabled)
    }

    /// Mod `name`'s choices (its only ones), worked out for what's saved.
    pub fn options(&self, name: &str) -> Result<ModOptionsView> {
        self.patchwork()
            .mod_options(self.profile(), name, None, None)
    }

    /// Mod `name`'s choices (its only ones), worked out for `answers`
    /// without saving them.
    pub fn options_for(&self, name: &str, answers: &[Answer]) -> Result<ModOptionsView> {
        self.patchwork()
            .mod_options(self.profile(), name, None, Some(answers))
    }

    /// Saves `answers` as what's picked in mod `name`'s choices (its only
    /// ones).
    pub fn answer_options(&self, name: &str, answers: &[Answer]) -> Result<()> {
        self.patchwork()
            .set_mod_options(self.profile(), name, None, Some(answers))
    }

    /// Turns mod `name` on or off.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.patchwork().set_enabled(self.profile(), name, enabled)
    }

    /// Extension `ext`'s state for the profile or one of its mods.
    pub fn ext_state<T: DeserializeOwned + Default>(
        &self,
        ext: &str,
        of: StateOf<'_>,
    ) -> Result<T> {
        self.patchwork().ext_state(self.profile(), ext, of)
    }

    /// Saves extension `ext`'s state for the profile or one of its mods.
    pub fn set_ext_state<T: Serialize>(&self, ext: &str, of: StateOf<'_>, value: &T) -> Result<()> {
        self.patchwork()
            .set_ext_state(self.profile(), ext, of, value)
    }

    /// Everything wrong with the profile, as the frontends show it.
    pub fn issues(&self) -> Result<Vec<Issue>> {
        self.patchwork().issues(self.profile())
    }

    /// What deploying would change, without changing it.
    pub fn plan(&self) -> Result<TestPlan> {
        let plan = self.block_on(self.patchwork().plan_deploy(self.profile()))?;
        Ok(TestPlan(plan))
    }

    /// Deploys the profile, returning the plan that was applied.
    pub fn deploy(&self) -> Result<TestPlan> {
        let plan = self.plan()?;
        self.patchwork().apply(&plan.0)?;
        Ok(plan)
    }

    /// Every file that's deployed now, sorted, written as in
    /// [`TestGame::path`].
    pub fn deployed(&self) -> Result<Vec<String>> {
        let pw = self.patchwork();
        let loaded = pw.load_profile(self.profile())?;
        let state = pw.deploy_state(&pw.deploy_target(&loaded)?)?;
        let mut files: Vec<String> = state.files.keys().map(|d| d.to_string()).collect();
        files.sort();
        Ok(files)
    }
}

/// `path`'s root and its path in it.
fn split_root(path: &str) -> (&str, &str) {
    path.split_once(':').unwrap_or((RootId::GAME, path))
}

impl Pending {
    fn start(self) -> Result<Ready> {
        let tmp = tempfile::tempdir().at(&std::env::temp_dir())?;
        let base = tmp.path().to_owned();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .at(&base)?;

        let mods = Arc::new(Mutex::new(BTreeMap::new()));
        let mut builder =
            Patchwork::builder(Paths::rooted(&base.join("home"))).source(TestSource(mods.clone()));
        for add in self.extensions {
            builder = add(builder);
        }
        let pw = builder.open()?;

        let id = match self.game {
            GameSpec::Builtin(id) => id,
            GameSpec::Def(def) => {
                let id = def.id.clone();
                pw.add_game(*def)?;
                id
            }
            GameSpec::Toml(toml) => {
                let mut table: toml::Table = toml::from_str(&toml)
                    .map_err(|e| Error::invalid(format!("the test game's TOML: {e}")))?;
                table
                    .entry("schema")
                    .or_insert(toml::Value::Integer(GAME_SCHEMA.into()));
                let def: GameDef = table
                    .try_into()
                    .map_err(|e| Error::invalid(format!("the test game's TOML: {e}")))?;
                let id = def.id.clone();
                pw.add_game(def)?;
                id
            }
        };
        let def = pw.registry().game(&id)?.value.clone();

        // Every root is a folder in `base`, so none are looked for (in a
        // Wine prefix, say) and nothing real is deployed to.
        let game = base.join("game");
        mkdir(&game)?;
        let mut install = GameInstall::new(game);
        let prefix = base.join("prefix");
        mkdir(&prefix)?;
        install.prefix = Some(prefix);
        let mut roots = BTreeMap::new();
        for (root, _) in def.roots.iter() {
            let dir = base.join("roots").join(root.as_str());
            mkdir(&dir)?;
            install.roots.insert(root.clone(), dir.clone());
            roots.insert(root.as_str().to_owned(), dir);
        }
        let game = pw.set_game_install(&id, install)?.path;

        for (path, contents) in &self.files {
            let (root, rel) = split_root(path);
            let dir = match root {
                RootId::GAME => &game,
                root => roots.get(root).ok_or_else(|| {
                    Error::invalid(format!("{} has no `{root}` root, for {path}", def.name))
                })?,
            };
            write(&dir.join(rel), contents)?;
        }

        let profile = pw.create_profile(&id, "test")?;
        for (name, from) in self.mods {
            let path = match from {
                ModFrom::Path(path) => path.canonicalize().at(&path)?,
                ModFrom::Files(files) => {
                    let dir = base.join("mods").join(&name);
                    mkdir(&dir)?;
                    for (rel, contents) in files {
                        write(&dir.join(rel), &contents)?;
                    }
                    dir
                }
            };
            mods.lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.clone(), path);
            let spec = ModSpec {
                source: SOURCE.to_owned(),
                id: name.clone(),
                version: None,
            };
            runtime.block_on(pw.add_mod(&profile, spec, Some(name), true))?;
        }
        // Mods' options come from their files, so they have to be locked.
        runtime.block_on(pw.lock(&profile, false))?;

        Ok(Ready {
            runtime,
            pw,
            profile,
            game,
            roots,
            _tmp: tmp,
        })
    }
}

fn mkdir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).at(dir)
}

fn write(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        mkdir(parent)?;
    }
    fs::write(path, contents).at(path)
}

/// The mods a [`TestGame`] was given: each mod's id is its name, and its
/// files are at the path it was given (or written to).
struct TestSource(Arc<Mutex<BTreeMap<String, PathBuf>>>);

#[async_trait]
impl Source for TestSource {
    fn id(&self) -> &str {
        SOURCE
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<ResolvedMod> {
        let path = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Source {
                source_id: SOURCE.to_owned(),
                message: format!("the test game has no mod called `{id}`"),
            })?;
        Ok(ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: None,
            format: ArtifactFormat::of_path(&path),
            artifact: ArtifactLocation::Path { path },
            dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
        })
    }
}

/// A deploy plan, with ways to ask what it does.
#[derive(Debug)]
pub struct TestPlan(pub DeployPlan);

impl TestPlan {
    /// Whether it places a file at `path`, written as in
    /// [`TestGame::path`]. Only files that change count, so files a
    /// deploy before this one placed, and that are still the same, don't.
    pub fn places(&self, path: &str) -> bool {
        self.placed_by(path).is_some()
    }

    /// Which mod the file it places at `path` comes from, if it places one
    /// there. Files deployers build say `built`.
    pub fn placed_by(&self, path: &str) -> Option<&str> {
        self.0.ops.iter().find_map(|op| match op {
            DeployOp::Place { dst, mod_name, .. } if dst.to_string() == path => {
                Some(mod_name.as_str())
            }
            _ => None,
        })
    }

    /// Every file it places, sorted, written as in [`TestGame::path`].
    pub fn placed(&self) -> Vec<String> {
        self.dests(|op| matches!(op, DeployOp::Place { .. }))
    }

    /// Every file it removes, sorted.
    pub fn removed(&self) -> Vec<String> {
        self.dests(|op| matches!(op, DeployOp::Remove { .. }))
    }

    fn dests(&self, keep: impl Fn(&DeployOp) -> bool) -> Vec<String> {
        let mut dests: Vec<String> = self
            .0
            .ops
            .iter()
            .filter(|op| keep(op))
            .map(|op| match op {
                DeployOp::Place { dst, .. }
                | DeployOp::Remove { dst }
                | DeployOp::Backup { dst } => dst.to_string(),
            })
            .collect();
        dests.sort();
        dests
    }

    pub fn warnings(&self) -> &[String] {
        &self.0.warnings
    }
}

impl std::ops::Deref for TestPlan {
    type Target = DeployPlan;

    fn deref(&self) -> &DeployPlan {
        &self.0
    }
}

#[cfg(test)]
mod tests;
