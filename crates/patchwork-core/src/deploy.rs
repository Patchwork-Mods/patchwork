/// This file is all about turning a set of mods into files in a game directory and back again
///
/// A [`Deployer`] only says where each file should go. The host then diffs
/// that against what's already deployed to get a [`DeployPlan`], which can be
/// shown to the user before [`apply`] carries it out. Everything applied is
/// recorded in a [`DeployState`], so it can be removed exactly and any game
/// files that were overwritten can be restored.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capture::Baseline;
use crate::environment::GameEnv;
use crate::error::IoContext;
use crate::event::{Event, Reporter};
use crate::game::{GameDef, LinkMethod};
use crate::hash::ContentHash;
use crate::plugins::{LoadOrder, Plugins};
use crate::registry::Registry;
use crate::relpath::RelPath;
use crate::settings::SettingSpec;
use crate::{Error, Result};

mod roots;

pub use roots::{
    Dest, Folders, Os, ResolvedRoot, ResolvedRoots, RootId, RootOrigin, RootSetup, Roots,
};

#[derive(Debug, Clone)]
pub struct ModFile {
    /// The path inside the mod.
    pub rel: RelPath,
    /// Where the file lives in the store.
    pub src: PathBuf,
}

/// A mod that's ready to deploy, as handed to a [`Deployer`].
#[derive(Debug, Clone)]
pub struct InstalledMod {
    pub name: String,
    pub source: String,
    pub id: String,
    pub version: Option<String>,
    pub hash: ContentHash,
    pub files: Vec<ModFile>,
    /// What sort of thing its source says it is, like `shader`, if it
    /// says.
    pub kind: Option<String>,
}

pub struct DeployContext<'a> {
    pub game: &'a GameDef,
    /// The deployer's settings from the game definition, with the user's
    /// own for this game (see [`Deployer::settings`]) on top.
    pub config: &'a toml::Table,
    /// For shared data such as rulesets.
    pub registry: &'a Registry,
}

impl InstalledMod {
    /// Where to place `file`, one of this mod's files, from.
    pub fn src(&self, file: &ModFile) -> Src {
        Src::Mod {
            hash: self.hash,
            rel: file.rel.clone(),
        }
    }
}

/// A file that should exist in one of the target's roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub dst: Dest,
    pub src: Src,
    pub mod_name: String,
}

/// Where a placed file comes from. Deployers can only name files they were
/// given or made, never a path of their own choosing, so the only files that
/// can end up in a game directory are mods' and what a deployer built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Src {
    /// A file in one of the mods being deployed, by the mod's hash.
    Mod { hash: ContentHash, rel: RelPath },
    /// A file [`Deployer::build`] wrote, relative to [`BuildContext::out`].
    Built(RelPath),
}

/// A [`Placement`] whose source has been checked and found on disk.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub dst: Dest,
    pub src: PathBuf,
    pub mod_name: String,
    /// Whether the game rewrites the file itself; see [`RootDef::rewritten`](crate::game::RootDef::rewritten).
    pub rewritten: bool,
}

/// Finds where each placement's file is, checking that it's one of `mods`'
/// files or, when `out` is given (for [`Deployer::build`]'s placements), a
/// file the build made there, and that it goes somewhere `roots` allows.
/// `kind` is the deployer's, for errors.
pub fn resolve(
    kind: &str,
    placements: Vec<Placement>,
    mods: &[InstalledMod],
    out: Option<&Path>,
    roots: &ResolvedRoots,
) -> Result<Vec<Resolved>> {
    let error = |message: String| Error::Deployer {
        kind: kind.to_owned(),
        message,
    };
    let files: HashMap<(ContentHash, &RelPath), &Path> = mods
        .iter()
        .flat_map(|m| m.files.iter().map(|f| ((m.hash, &f.rel), f.src.as_path())))
        .collect();
    // Canonicalized so a symlink the build made can't point outside it.
    let out = out
        .map(|out| out.canonicalize().at(out).map(|canon| (out, canon)))
        .transpose()?;
    placements
        .into_iter()
        .map(|p| {
            let rewritten = match roots.check(&p.dst) {
                Ok(rewritten) => rewritten,
                Err((true, message)) => return Err(error(message)),
                Err((false, message)) => return Err(Error::invalid(message)),
            };
            let src = match &p.src {
                Src::Mod { hash, rel } => files
                    .get(&(*hash, rel))
                    .map(|src| src.to_path_buf())
                    .ok_or_else(|| {
                        error(format!("{} isn't a file of any mod being deployed", rel))
                    })?,
                Src::Built(rel) => {
                    let Some((out, canon)) = &out else {
                        return Err(error(format!("{rel} can only be placed by a build")));
                    };
                    let src = rel.under(out);
                    let inside = src
                        .canonicalize()
                        .is_ok_and(|real| real.starts_with(canon) && real.is_file());
                    if !inside {
                        return Err(error(format!("{rel} isn't a file the build made")));
                    }
                    src
                }
            };
            Ok(Resolved {
                dst: p.dst,
                src,
                mod_name: p.mod_name,
                rewritten,
            })
        })
        .collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DesiredState {
    pub placements: Vec<Placement>,
    pub warnings: Vec<String>,
}

/// Knows how a particular game or mod loader expects mods to be laid out.
///
/// Deployers must not touch the filesystem when planning: everything they
/// need is in `mods`, and everything they decide goes in the returned
/// [`DesiredState`]. The exception is [`Deployer::build`], for the few
/// deployers that have to generate files.
pub trait Deployer: Send + Sync {
    fn kind(&self) -> &str;

    /// The keys of its config that users can set for themselves, for each
    /// game. What they set replaces what the game definition says in
    /// [`DeployContext::config`].
    fn settings(&self) -> &[SettingSpec] {
        &[]
    }

    /// `mods` are in load order, with dependencies before the mods that need
    /// them.
    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState>;

    /// The plugins in `mods` that the game loads in an order the user picks
    /// (see [`crate::plugins`]), or `None` for games without them, which is
    /// most. Like [`Deployer::build`], this may read the game's files and
    /// the mods' own, but not write anything.
    fn plugins(
        &self,
        _ctx: &DeployContext<'_>,
        _game: &GameFiles<'_>,
        _mods: &[InstalledMod],
    ) -> Result<Option<Plugins>> {
        Ok(None)
    }

    /// Whether [`Deployer::build`] has anything to do for this game. Most
    /// deployers only ever place mod files and leave this `false`.
    fn builds(&self, _ctx: &DeployContext<'_>) -> bool {
        false
    }

    /// Makes files that can't simply be copied out of mods, such as archives
    /// repacked from the game's own files with mods' changes applied.
    ///
    /// Unlike [`Deployer::plan`], this may read the game's files (through
    /// [`BuildContext::game`]) and write files of its own, but only into
    /// [`BuildContext::out`]. It returns placements of those files
    /// ([`Src::Built`]) or mods' files; they're added after the ones from
    /// `plan`, so they win over them.
    ///
    /// The result is kept and reused for as long as the deploy's inputs stay
    /// the same, so it has to depend only on `mods`, the game definition and
    /// whatever [`Deployer::fingerprint`] covers. (The game definition here
    /// includes the user's settings, and the profile's plugin order is
    /// covered too.)
    fn build(
        &self,
        _ctx: &DeployContext<'_>,
        _build: &BuildContext<'_>,
        _mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        Ok(DesiredState::default())
    }

    /// A hash of the game files [`Deployer::build`] reads, so a game update
    /// makes the deploy stale and gets it built again. Called whenever
    /// patchwork checks whether a deploy is current, so it should be cheap:
    /// sizes and modification times rather than contents for big files.
    fn fingerprint(
        &self,
        _ctx: &DeployContext<'_>,
        _game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        Ok(None)
    }
}

/// What [`Deployer::build`] gets to work with.
pub struct BuildContext<'a> {
    pub game: GameFiles<'a>,
    /// An empty directory for the build's output. It's kept for as long as
    /// anything deployed comes from it.
    pub out: &'a Path,
    /// Somewhere to keep intermediate results between builds. Like the rest
    /// of patchwork's cache it can be deleted at any time.
    pub cache: &'a Path,
    pub reporter: &'a dyn Reporter,
    /// The order to load the mods' plugins in, if
    /// [`Deployer::plugins`] found that the game has them.
    pub load_order: Option<&'a LoadOrder>,
}

/// The game's own files, as they are without anything patchwork deployed:
/// files a mod replaced are found in their backups, and files patchwork
/// added are left out.
pub struct GameFiles<'a> {
    root: &'a Path,
    state: &'a DeployState,
}

impl<'a> GameFiles<'a> {
    pub fn new(root: &'a Path, state: &'a DeployState) -> Self {
        Self { root, state }
    }

    pub fn root(&self) -> &Path {
        self.root
    }

    /// Where the game's own copy of `rel` is, if it has one.
    ///
    /// A deployed file that's been changed since (say by a game update) is
    /// the game's again, and is what deploying next will back up.
    pub fn original(&self, rel: &RelPath) -> Option<PathBuf> {
        let path = rel.under(self.root);
        match self.state.files.get(&Dest::game(rel.clone())) {
            Some(deployed) if !changed_since_deployed(&path, deployed) => deployed.backup.clone(),
            _ => path.is_file().then_some(path),
        }
    }

    /// The game's own files directly inside `dir` (the game directory itself
    /// if `None`), sorted. Unlike [`GameFiles::original`], files patchwork
    /// added are left out without checking whether they've changed, since
    /// that would mean reading every one.
    pub fn list(&self, dir: Option<&RelPath>) -> Result<Vec<RelPath>> {
        let path = dir.map_or_else(|| self.root.to_owned(), |d| d.under(self.root));
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).at(&path),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.at(&path)?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let rel = match dir {
                Some(d) => format!("{d}/{name}"),
                None => name,
            };
            let Ok(rel) = RelPath::new(&rel) else {
                continue;
            };
            let is_file = entry.path().is_file();
            let ours = self
                .state
                .files
                .get(&Dest::game(rel.clone()))
                .is_some_and(|d| d.backup.is_none());
            if is_file && !ours {
                files.push(rel);
            }
        }
        files.sort();
        Ok(files)
    }
}

/// Somewhere mods are deployed to. Each target has its own [`DeployState`],
/// so it can have a different profile deployed from any other target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployTarget {
    /// A game's install, shared by all of the game's profiles: deploying one
    /// takes the place of whichever was deployed before.
    Game(String),
}

impl DeployTarget {
    pub fn game(&self) -> &str {
        match self {
            DeployTarget::Game(game) => game,
        }
    }

    /// Unique among targets, and safe to name files with (the target's
    /// deploy state and backups).
    pub fn key(&self) -> &str {
        match self {
            DeployTarget::Game(game) => game,
        }
    }
}

pub const DEPLOY_STATE_SCHEMA: u32 = 1;

/// What patchwork has put into a target's roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployState {
    pub schema: u32,
    pub game: String,
    pub profile: Option<String>,
    /// The `game` root. It's kept apart from the others so that states
    /// from before there were other roots still read the same.
    pub root: PathBuf,
    /// The other roots; see [`DeployState::roots`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roots: BTreeMap<RootId, PathBuf>,
    #[serde(default)]
    pub files: BTreeMap<Dest, DeployedFile>,
    /// Directories patchwork created, removed again once they're empty.
    #[serde(default)]
    pub created_dirs: BTreeSet<Dest>,
    /// [`DeployPlan::inputs`] of the last deploy that finished, so it's
    /// possible to tell whether deploying again would change anything
    /// without working out a whole plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<ContentHash>,
    /// [`DeployPlan::env`] of the last deploy that finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<GameEnv>,
    /// What was in the game's capture folders after the last deploy that
    /// finished, besides what patchwork put there; see [`crate::capture`].
    /// `None` for games without any, and until a profile is deployed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
}

impl DeployState {
    pub fn empty(game: &str, roots: &Roots) -> Self {
        let mut state = Self {
            schema: DEPLOY_STATE_SCHEMA,
            game: game.to_owned(),
            profile: None,
            root: PathBuf::new(),
            roots: BTreeMap::new(),
            files: BTreeMap::new(),
            created_dirs: BTreeSet::new(),
            inputs: None,
            env: None,
            baseline: None,
        };
        state.set_roots(roots);
        state
    }

    /// Every root, `game` included.
    pub fn roots(&self) -> Roots {
        self.roots
            .iter()
            .fold(Roots::game(&self.root), |roots, (id, path)| {
                roots.with(id.clone(), path)
            })
    }

    pub fn set_roots(&mut self, roots: &Roots) {
        self.root = roots.game_path().to_owned();
        self.roots = roots
            .iter()
            .filter(|(id, _)| !id.is_game())
            .map(|(id, path)| (id.clone(), path.to_owned()))
            .collect();
    }

    /// A root that something's deployed to which isn't where `roots` says
    /// it is now, and where it was. Deploying there would lose track of
    /// what's in the old place, so it has to be undeployed first.
    pub fn moved_root(&self, roots: &Roots) -> Option<(RootId, PathBuf)> {
        let current = self.roots();
        let used = self
            .files
            .keys()
            .chain(&self.created_dirs)
            .map(|dest| &dest.root);
        for root in used {
            let was = current.get(root)?;
            if roots.get(root) != Some(was) {
                return Some((root.clone(), was.to_owned()));
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedFile {
    pub mod_name: String,
    pub src: PathBuf,
    /// Hash of the file as deployed, to notice if something changed it since.
    pub hash: ContentHash,
    /// Where the game's original file was moved to, if this replaced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// The game rewrites this file itself, so it's replaced and removed
    /// whatever it's been changed to since.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rewritten: bool,
}

#[derive(Debug, Clone)]
pub enum DeployOp {
    /// Remove a deployed file, restoring the original if one was backed up.
    Remove { dst: Dest },
    /// Move a file that patchwork didn't create out of the way.
    Backup { dst: Dest },
    Place {
        dst: Dest,
        src: PathBuf,
        mod_name: String,
        rewritten: bool,
    },
}

#[derive(Debug)]
pub struct DeployPlan {
    pub target: DeployTarget,
    pub profile: Option<String>,
    pub roots: Roots,
    pub link: LinkMethod,
    pub ops: Vec<DeployOp>,
    /// Files that are already in place.
    pub unchanged: usize,
    pub warnings: Vec<String>,
    /// A hash of everything that went into the plan, recorded in the
    /// [`DeployState`] once it's applied. `None` for undeploying.
    pub inputs: Option<ContentHash>,
    /// The game install the plan was made for, recorded like `inputs` so
    /// it's possible to tell when the game has changed since.
    pub env: Option<GameEnv>,
}

impl DeployPlan {
    pub fn is_noop(&self) -> bool {
        self.ops.is_empty()
    }
}

/// Works out how to get from `prev` to `desired`, deploying to `prev`'s
/// roots. Reads them to find files that would be overwritten, but doesn't
/// change anything.
pub fn plan(
    target: DeployTarget,
    prev: &DeployState,
    placements: Vec<Resolved>,
    mut warnings: Vec<String>,
    profile: Option<String>,
    link: LinkMethod,
    check: Check,
) -> Result<DeployPlan> {
    let roots = prev.roots();
    let placements = match_case(&roots, placements)?;
    // Later placements win, which is what makes load order meaningful.
    let mut wanted: BTreeMap<Dest, Resolved> = BTreeMap::new();
    // One warning for them all, since big profiles have hundreds; see
    // `crate::conflicts` for which mod wins each.
    let mut contested = BTreeSet::new();
    for placement in placements {
        if let Some(old) = wanted.get(&placement.dst)
            && old.mod_name != placement.mod_name
        {
            contested.insert(placement.dst.clone());
        }
        wanted.insert(placement.dst.clone(), placement);
    }
    match contested.len() {
        0 => {}
        1 => warnings.push(format!(
            "{} is provided by more than one mod; using the copy from the one latest in the load order (`patchwork conflicts` lists them)",
            contested.first().expect("one file")
        )),
        n => warnings.push(format!(
            "{n} files are provided by more than one mod; using the copies from the ones latest in the load order (`patchwork conflicts` lists them)"
        )),
    }

    let mut ops = Vec::new();
    for dst in prev.files.keys() {
        if !wanted.contains_key(dst) {
            ops.push(DeployOp::Remove { dst: dst.clone() });
        }
    }

    let mut unchanged = 0;
    let mut drifted = 0;
    for (dst, placement) in wanted {
        let current = match prev.files.get(&dst) {
            // A file the game rewrites is put back if it has been.
            Some(existing) if placement.rewritten => {
                existing.src == placement.src
                    && !changed_since_deployed(&roots.path(&dst)?, existing)
            }
            Some(existing) if existing.src == placement.src => {
                let intact = drift(&roots.path(&dst)?, existing, check).is_none();
                if !intact {
                    drifted += 1;
                }
                intact
            }
            Some(_) => false,
            None => {
                if roots.path(&dst)?.symlink_metadata().is_ok() {
                    ops.push(DeployOp::Backup { dst: dst.clone() });
                }
                false
            }
        };
        if current {
            unchanged += 1;
        } else {
            ops.push(DeployOp::Place {
                dst,
                src: placement.src,
                mod_name: placement.mod_name,
                rewritten: placement.rewritten,
            });
        }
    }

    match drifted {
        0 => {}
        1 => warnings.push(
            "1 deployed file was changed or deleted since it was deployed, \
             most likely by a game update or a check of the game's files; it'll be put back"
                .to_owned(),
        ),
        n => warnings.push(format!(
            "{n} deployed files were changed or deleted since they were deployed, \
             most likely by a game update or a check of the game's files; they'll be put back"
        )),
    }

    Ok(DeployPlan {
        target,
        profile,
        roots,
        link,
        ops,
        unchanged,
        warnings,
        inputs: None,
        env: None,
    })
}

/// Spells each placement's path the way it's already spelled on disk or,
/// for what isn't there yet, the way the first placement to name it did.
/// Mods are made on Windows, which ignores case, and rarely agree on it:
/// without this, on other systems they'd end up in folders next to the
/// game's own (`data/` beside `Data/`), and two spellings of one file would
/// both be deployed.
fn match_case(roots: &Roots, placements: Vec<Resolved>) -> Result<Vec<Resolved>> {
    // What's in each folder, by lowercased name, keyed by where it is.
    let mut folders: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    placements
        .into_iter()
        .map(|mut placement| {
            let mut dir = match roots.get(&placement.dst.root) {
                Some(root) => root.to_owned(),
                // For its error about a root the target doesn't have.
                None => return Err(roots.path(&placement.dst).unwrap_err()),
            };
            let mut parts = Vec::new();
            for part in placement.dst.path.components() {
                let names = folders.entry(dir.clone()).or_insert_with(|| list_dir(&dir));
                let name = names
                    .entry(part.to_lowercase())
                    .or_insert_with(|| part.to_owned())
                    .clone();
                dir.push(&name);
                parts.push(name);
            }
            placement.dst.path = RelPath::new(&parts.join("/")).map_err(Error::invalid)?;
            Ok(placement)
        })
        .collect()
}

/// The names in `dir` by lowercased name, or nothing if it isn't there.
/// Where several differ only in case, the first in sort order is used.
fn list_dir(dir: &Path) -> HashMap<String, String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut by_lower = HashMap::new();
    for name in names {
        by_lower.entry(name.to_lowercase()).or_insert(name);
    }
    by_lower
}

/// Carries out a plan, updating `state` as it goes. Files it moves out of
/// the way go in `backups`, under their root's name.
///
/// If an operation fails, `state` still reflects everything done up to that
/// point, so the caller should save it either way.
pub fn apply(
    plan: &DeployPlan,
    state: &mut DeployState,
    backups: &Path,
    reporter: &dyn Reporter,
) -> Result<()> {
    if state.moved_root(&plan.roots).is_some() {
        return Err(Error::invalid(
            "deploy plan doesn't match the current state",
        ));
    }
    let roots = &plan.roots;
    state.set_roots(roots);
    state.profile = plan.profile.clone();
    // Only set again once everything's done, so a deploy that failed
    // partway is never mistaken for a current one.
    state.inputs = None;
    state.env = None;

    for op in &plan.ops {
        reporter.report(Event::Applying(op.clone()));
        match op {
            DeployOp::Remove { dst } => {
                if let Some(entry) = state.files.remove(dst)
                    && let Some(warning) = remove(&roots.path(dst)?, dst, &entry)?
                {
                    reporter.report(Event::Warning(warning));
                }
            }
            DeployOp::Backup { dst } => {
                let from = roots.path(dst)?;
                let to = backup_path(backups, dst);
                if to.symlink_metadata().is_ok() {
                    return Err(Error::invalid(format!(
                        "a backup of {dst} already exists at {}; move it somewhere safe and try again",
                        to.display()
                    )));
                }
                create_parent(&to)?;
                move_file(&from, &to)?;
                // The file that replaces it will claim this backup.
                state.files.insert(
                    dst.clone(),
                    DeployedFile {
                        mod_name: String::new(),
                        src: PathBuf::new(),
                        hash: ContentHash::of(b""),
                        backup: Some(to),
                        rewritten: false,
                    },
                );
            }
            DeployOp::Place {
                dst,
                src,
                mod_name,
                rewritten,
            } => {
                let target = roots.path(dst)?;
                create_dirs(roots, dst, &mut state.created_dirs)?;
                let mut backup = state.files.get(dst).and_then(|e| e.backup.clone());
                // What the game made of a file it rewrites is never its
                // original; that was backed up before the first deploy.
                if !rewritten
                    && let Some(entry) = state.files.get(dst)
                    && changed_since_deployed(&target, entry)
                {
                    // Most likely a game update put its own file back, which
                    // is now the original to restore.
                    let to = backup_path(backups, dst);
                    create_parent(&to)?;
                    if to.symlink_metadata().is_ok() {
                        fs::remove_file(&to).at(&to)?;
                    }
                    move_file(&target, &to)?;
                    reporter.report(Event::Warning(format!(
                        "{dst} was changed after it was deployed; keeping the new version as the original"
                    )));
                    backup = Some(to);
                } else if target.symlink_metadata().is_ok() {
                    fs::remove_file(&target).at(&target)?;
                }
                // Other roots are the user's own folders, where a link
                // would let the game write through it into the store.
                let method = if dst.root.is_game() {
                    plan.link
                } else {
                    LinkMethod::Copy
                };
                link(src, &target, method)?;
                state.files.insert(
                    dst.clone(),
                    DeployedFile {
                        mod_name: mod_name.clone(),
                        src: src.clone(),
                        hash: ContentHash::of_file(src)?,
                        backup,
                        rewritten: *rewritten,
                    },
                );
            }
        }
    }

    remove_empty_dirs(roots, &mut state.created_dirs);
    state.inputs = plan.inputs;
    state.env = plan.env.clone();
    Ok(())
}

/// How closely [`drift`] looks at deployed files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Only what a file's metadata says: whether it's there, whether a link
    /// still is one to the file it was made from, and whether a copy is
    /// the same size as that file. Cheap enough to do every time.
    Quick,
    /// Hashes every file as well, which catches a copy that was changed
    /// without changing its size, but reads everything.
    Full,
}

/// What happened to a deployed file since it was deployed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Drift {
    Missing,
    /// Something else is there now, most likely the game's own file put
    /// back by an update.
    Changed,
}

/// What's happened to the file deployed as `entry`, which belongs at
/// `path`, since it was deployed, as far as `check` can tell.
pub fn drift(path: &Path, entry: &DeployedFile, check: Check) -> Option<Drift> {
    let Ok(meta) = path.symlink_metadata() else {
        return Some(Drift::Missing);
    };
    if meta.file_type().is_symlink() {
        if fs::read_link(path).ok().as_ref() != Some(&entry.src) {
            return Some(Drift::Changed);
        }
    } else if !meta.is_file() {
        return Some(Drift::Changed);
    } else if check == Check::Quick
        && let Ok(src) = entry.src.metadata()
    {
        // A hardlink is still the file it was made from; a copy at least
        // has to be the same size.
        return (!same_file(&meta, &src) && meta.len() != src.len()).then_some(Drift::Changed);
    }
    if check == Check::Quick {
        return None;
    }
    (ContentHash::of_file(path).ok() != Some(entry.hash)).then_some(Drift::Changed)
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    false
}

/// The files in `state` that aren't as they were deployed, as far as
/// `check` can tell. Files the game rewrites are expected to change, so
/// they're left out.
pub fn check_files(state: &DeployState, check: Check) -> Result<Vec<(Dest, Drift)>> {
    let roots = state.roots();
    let mut drifted = Vec::new();
    for (dst, entry) in &state.files {
        if entry.rewritten {
            continue;
        }
        if let Some(drift) = drift(&roots.path(dst)?, entry, check) {
            drifted.push((dst.clone(), drift));
        }
    }
    Ok(drifted)
}

/// Whether the file at `path` is there but isn't what was deployed to it.
fn changed_since_deployed(path: &Path, entry: &DeployedFile) -> bool {
    path.is_file() && ContentHash::of_file(path).ok() != Some(entry.hash)
}

fn backup_path(backups: &Path, dst: &Dest) -> PathBuf {
    dst.path.under(&backups.join(dst.root.as_str()))
}

/// Removes the file deployed to `dst`, which is at `target`, and restores
/// whatever it replaced. Returns a warning instead of deleting if the file
/// was changed after deployment.
fn remove(target: &Path, dst: &Dest, entry: &DeployedFile) -> Result<Option<String>> {
    if target.symlink_metadata().is_ok() {
        if !entry.rewritten && ContentHash::of_file(target).ok() != Some(entry.hash) {
            return Ok(Some(format!(
                "{dst} was changed after it was deployed, so it was left in place{}",
                match &entry.backup {
                    Some(backup) => format!(" (the original is at {})", backup.display()),
                    None => String::new(),
                }
            )));
        }
        fs::remove_file(target).at(target)?;
    }
    if let Some(backup) = &entry.backup {
        move_file(backup, target)?;
    }
    Ok(None)
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    Ok(())
}

fn create_dirs(roots: &Roots, dst: &Dest, created: &mut BTreeSet<Dest>) -> Result<()> {
    let mut missing = Vec::new();
    let mut dir = dst.parent();
    while let Some(d) = dir {
        if roots.path(&d)?.is_dir() {
            break;
        }
        dir = d.parent();
        missing.push(d);
    }
    for d in missing.into_iter().rev() {
        let path = roots.path(&d)?;
        fs::create_dir(&path).at(&path)?;
        created.insert(d);
    }
    Ok(())
}

fn remove_empty_dirs(roots: &Roots, created: &mut BTreeSet<Dest>) {
    // Longest paths first, so children go before their parents.
    let mut dirs: Vec<_> = created.iter().cloned().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.path.as_str().len()));
    for dir in dirs {
        let Ok(path) = roots.path(&dir) else {
            continue;
        };
        if !path.exists() || fs::remove_dir(&path).is_ok() {
            created.remove(&dir);
        }
    }
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_err() {
        // Probably different filesystems.
        fs::copy(from, to).at(from)?;
        fs::remove_file(from).at(from)?;
    }
    Ok(())
}

fn link(src: &Path, dst: &Path, method: LinkMethod) -> Result<()> {
    match method {
        LinkMethod::Copy => fs::copy(src, dst).map(drop).at(dst),
        LinkMethod::Hardlink => fs::hard_link(src, dst)
            .or_else(|_| fs::copy(src, dst).map(drop))
            .at(dst),
        LinkMethod::Symlink => symlink(src, dst).at(dst),
    }
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(src, dst)
}

#[cfg(test)]
mod tests;
