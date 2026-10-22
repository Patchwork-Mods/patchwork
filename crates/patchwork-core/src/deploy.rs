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

use crate::error::IoContext;
use crate::event::{Event, Reporter};
use crate::game::{GameDef, LinkMethod};
use crate::hash::ContentHash;
use crate::registry::Registry;
use crate::relpath::RelPath;
use crate::{Error, Result};

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
}

pub struct DeployContext<'a> {
    pub game: &'a GameDef,
    /// The deployer's settings from the game definition.
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

/// A file that should exist in the game directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub dst: RelPath,
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
    pub dst: RelPath,
    pub src: PathBuf,
    pub mod_name: String,
}

/// Finds where each placement's file is, checking that it's one of `mods`'
/// files or, when `out` is given (for [`Deployer::build`]'s placements), a
/// file the build made there. `kind` is the deployer's, for errors.
pub fn resolve(
    kind: &str,
    placements: Vec<Placement>,
    mods: &[InstalledMod],
    out: Option<&Path>,
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

    /// `mods` are in load order, with dependencies before the mods that need
    /// them.
    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState>;

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
    /// whatever [`Deployer::fingerprint`] covers.
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
        match self.state.files.get(rel) {
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
                .get(&rel)
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

/// What patchwork has put into a game directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployState {
    pub schema: u32,
    pub game: String,
    pub profile: Option<String>,
    pub root: PathBuf,
    #[serde(default)]
    pub files: BTreeMap<RelPath, DeployedFile>,
    /// Directories patchwork created, removed again once they're empty.
    #[serde(default)]
    pub created_dirs: BTreeSet<RelPath>,
    /// [`DeployPlan::inputs`] of the last deploy that finished, so it's
    /// possible to tell whether deploying again would change anything
    /// without working out a whole plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<ContentHash>,
}

impl DeployState {
    pub fn empty(game: &str, root: &Path) -> Self {
        Self {
            schema: DEPLOY_STATE_SCHEMA,
            game: game.to_owned(),
            profile: None,
            root: root.to_owned(),
            files: BTreeMap::new(),
            created_dirs: BTreeSet::new(),
            inputs: None,
        }
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
}

#[derive(Debug, Clone)]
pub enum DeployOp {
    /// Remove a deployed file, restoring the original if one was backed up.
    Remove { dst: RelPath },
    /// Move a file that patchwork didn't create out of the way.
    Backup { dst: RelPath },
    Place {
        dst: RelPath,
        src: PathBuf,
        mod_name: String,
    },
}

#[derive(Debug)]
pub struct DeployPlan {
    pub target: DeployTarget,
    pub profile: Option<String>,
    pub root: PathBuf,
    pub link: LinkMethod,
    pub ops: Vec<DeployOp>,
    /// Files that are already in place.
    pub unchanged: usize,
    pub warnings: Vec<String>,
    /// A hash of everything that went into the plan, recorded in the
    /// [`DeployState`] once it's applied. `None` for undeploying.
    pub inputs: Option<ContentHash>,
}

impl DeployPlan {
    pub fn is_noop(&self) -> bool {
        self.ops.is_empty()
    }
}

/// Works out how to get from `prev` to `desired`. Reads the game directory to
/// find files that would be overwritten, but doesn't change anything.
pub fn plan(
    target: DeployTarget,
    prev: &DeployState,
    placements: Vec<Resolved>,
    mut warnings: Vec<String>,
    profile: Option<String>,
    link: LinkMethod,
) -> DeployPlan {
    // Later placements win, which is what makes load order meaningful.
    let mut wanted: BTreeMap<RelPath, Resolved> = BTreeMap::new();
    for placement in placements {
        if let Some(old) = wanted.get(&placement.dst)
            && old.mod_name != placement.mod_name
        {
            warnings.push(format!(
                "{} is provided by both `{}` and `{}`; using `{}`",
                placement.dst, old.mod_name, placement.mod_name, placement.mod_name
            ));
        }
        wanted.insert(placement.dst.clone(), placement);
    }

    let mut ops = Vec::new();
    for dst in prev.files.keys() {
        if !wanted.contains_key(dst) {
            ops.push(DeployOp::Remove { dst: dst.clone() });
        }
    }

    let mut unchanged = 0;
    for (dst, placement) in wanted {
        match prev.files.get(&dst) {
            Some(existing) if existing.src == placement.src => unchanged += 1,
            Some(_) => {}
            None if dst.under(&prev.root).symlink_metadata().is_ok() => {
                ops.push(DeployOp::Backup { dst: dst.clone() });
            }
            None => {}
        }
        if prev.files.get(&dst).is_none_or(|e| e.src != placement.src) {
            ops.push(DeployOp::Place {
                dst,
                src: placement.src,
                mod_name: placement.mod_name,
            });
        }
    }

    DeployPlan {
        target,
        profile,
        root: prev.root.clone(),
        link,
        ops,
        unchanged,
        warnings,
        inputs: None,
    }
}

/// Carries out a plan, updating `state` as it goes.
///
/// If an operation fails, `state` still reflects everything done up to that
/// point, so the caller should save it either way.
pub fn apply(
    plan: &DeployPlan,
    state: &mut DeployState,
    backups: &Path,
    reporter: &dyn Reporter,
) -> Result<()> {
    if state.root != plan.root {
        return Err(Error::invalid(
            "deploy plan doesn't match the current state",
        ));
    }
    let root = plan.root.clone();
    state.profile = plan.profile.clone();
    // Only set again once everything's done, so a deploy that failed
    // partway is never mistaken for a current one.
    state.inputs = None;

    for op in &plan.ops {
        reporter.report(Event::Applying(op.clone()));
        match op {
            DeployOp::Remove { dst } => {
                if let Some(entry) = state.files.remove(dst)
                    && let Some(warning) = remove(&root, dst, &entry)?
                {
                    reporter.report(Event::Warning(warning));
                }
            }
            DeployOp::Backup { dst } => {
                let from = dst.under(&root);
                let to = dst.under(backups);
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
                    },
                );
            }
            DeployOp::Place { dst, src, mod_name } => {
                let target = dst.under(&root);
                create_dirs(&root, dst, &mut state.created_dirs)?;
                let mut backup = state.files.get(dst).and_then(|e| e.backup.clone());
                if let Some(entry) = state.files.get(dst)
                    && changed_since_deployed(&target, entry)
                {
                    // Most likely a game update put its own file back, which
                    // is now the original to restore.
                    let to = dst.under(backups);
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
                link(src, &target, plan.link)?;
                state.files.insert(
                    dst.clone(),
                    DeployedFile {
                        mod_name: mod_name.clone(),
                        src: src.clone(),
                        hash: ContentHash::of_file(src)?,
                        backup,
                    },
                );
            }
        }
    }

    remove_empty_dirs(&root, &mut state.created_dirs);
    state.inputs = plan.inputs;
    Ok(())
}

/// Whether the file at `path` is there but isn't what was deployed to it.
fn changed_since_deployed(path: &Path, entry: &DeployedFile) -> bool {
    path.is_file() && ContentHash::of_file(path).ok() != Some(entry.hash)
}

/// Removes a deployed file and restores whatever it replaced. Returns a
/// warning instead of deleting if the file was changed after deployment.
fn remove(root: &Path, dst: &RelPath, entry: &DeployedFile) -> Result<Option<String>> {
    let target = dst.under(root);
    if target.symlink_metadata().is_ok() {
        if ContentHash::of_file(&target).ok() != Some(entry.hash) {
            return Ok(Some(format!(
                "{dst} was changed after it was deployed, so it was left in place{}",
                match &entry.backup {
                    Some(backup) => format!(" (the original is at {})", backup.display()),
                    None => String::new(),
                }
            )));
        }
        fs::remove_file(&target).at(&target)?;
    }
    if let Some(backup) = &entry.backup {
        move_file(backup, &target)?;
    }
    Ok(None)
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    Ok(())
}

fn create_dirs(root: &Path, dst: &RelPath, created: &mut BTreeSet<RelPath>) -> Result<()> {
    let mut missing = Vec::new();
    let mut dir = dst.parent();
    while let Some(d) = dir {
        if d.under(root).is_dir() {
            break;
        }
        dir = d.parent();
        missing.push(d);
    }
    for d in missing.into_iter().rev() {
        let path = d.under(root);
        fs::create_dir(&path).at(&path)?;
        created.insert(d);
    }
    Ok(())
}

fn remove_empty_dirs(root: &Path, created: &mut BTreeSet<RelPath>) {
    // Longest paths first, so children go before their parents.
    let mut dirs: Vec<_> = created.iter().cloned().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.as_str().len()));
    for dir in dirs {
        let path = dir.under(root);
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
