/// This file is all about turning a set of mods into files in a game directory and back again
///
/// A [`Deployer`] only says where each file should go. The host then diffs
/// that against what's already deployed to get a [`DeployPlan`], which can be
/// shown to the user before [`apply`] carries it out. Everything applied is
/// recorded in a [`DeployState`], so it can be removed exactly and any game
/// files that were overwritten can be restored.
use std::collections::{BTreeMap, BTreeSet};
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

/// A file that should exist in the game directory.
#[derive(Debug, Clone)]
pub struct Placement {
    pub dst: RelPath,
    pub src: PathBuf,
    pub mod_name: String,
}

#[derive(Debug, Default)]
pub struct DesiredState {
    pub placements: Vec<Placement>,
    pub warnings: Vec<String>,
}

/// Knows how a particular game or mod loader expects mods to be laid out.
///
/// Deployers must not touch the filesystem: everything they need is in
/// `mods`, and everything they decide goes in the returned [`DesiredState`].
pub trait Deployer: Send + Sync {
    fn kind(&self) -> &str;

    /// `mods` are in load order, with dependencies before the mods that need
    /// them.
    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState>;
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
    desired: DesiredState,
    profile: Option<String>,
    link: LinkMethod,
) -> DeployPlan {
    let mut warnings = desired.warnings;

    // Later placements win, which is what makes load order meaningful.
    let mut wanted: BTreeMap<RelPath, Placement> = BTreeMap::new();
    for placement in desired.placements {
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
                if target.symlink_metadata().is_ok() {
                    fs::remove_file(&target).at(&target)?;
                }
                link(src, &target, plan.link)?;
                let backup = state.files.get(dst).and_then(|e| e.backup.clone());
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
