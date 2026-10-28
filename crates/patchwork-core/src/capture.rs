//! Finding files that tools wrote into a game's folder, so they can be kept
//! as a mod.
//!
//! Tools that generate LOD, animations or patches (DynDOLOD, Nemesis,
//! Synthesis, BodySlide...) write straight into the game's folder. Those
//! files aren't any mod's, so nothing deploys them again, and deployers
//! that only look at mods (like the Bethesda one's load order) don't see
//! them. Capturing turns them into a local mod of their own.
//!
//! What's in the game's [`CaptureDef::folders`] when a profile is deployed
//! is recorded as its [`Baseline`]: the game's own files, and anything else
//! that was there before patchwork first deployed. Anything new since, and
//! any deployed file that's been replaced, is a tool's output.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::deploy::{Check, DeployState, Dest, Drift, drift};
use crate::game::CaptureDef;
use crate::relpath::RelPath;
use crate::{Error, Result};

/// What a file looked like, closely enough to tell when it's changed
/// without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub len: u64,
    /// Nanoseconds since the Unix epoch, or 0 if the system can't say.
    pub modified: u128,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = path.metadata().ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        Some(Self {
            len: meta.len(),
            modified,
        })
    }
}

/// The files in the game's capture folders that patchwork didn't put
/// there, by path in the game's folder.
pub type Baseline = BTreeMap<RelPath, Stamp>;

/// A file a tool wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    /// Where it is in the game's folder.
    pub path: RelPath,
    /// The mod whose deployed file it replaced, if it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

/// What tools wrote into the game's capture folders since the last deploy.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub files: Vec<OutputFile>,
    /// The game's own files that were changed. They aren't captured, since
    /// the changed file would then be deployed over itself.
    pub changed: Vec<RelPath>,
}

impl ToolOutput {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.changed.is_empty()
    }
}

/// Every file in `def`'s folders under `root` that patchwork didn't deploy
/// (going by `state`), and isn't ignored.
///
/// With a `previous` baseline, only files that were in it are kept, so
/// deploying again after running a tool doesn't make its output look like
/// the game's own. Their stamps are brought up to date, which takes in
/// changes a game update made.
pub fn baseline(
    root: &Path,
    def: &CaptureDef,
    state: &DeployState,
    previous: Option<&Baseline>,
) -> Result<Baseline> {
    let mut baseline = Baseline::new();
    walk(root, def, |rel, path| {
        let known = previous.is_none_or(|p| p.contains_key(&rel));
        if known
            && !state.files.contains_key(&Dest::game(rel.clone()))
            && let Some(stamp) = Stamp::of(path)
        {
            baseline.insert(rel, stamp);
        }
    })?;
    Ok(baseline)
}

/// What's been written to `def`'s folders under `root` since `state` was
/// deployed, given what was there then.
pub fn find(
    root: &Path,
    def: &CaptureDef,
    state: &DeployState,
    baseline: &Baseline,
) -> Result<ToolOutput> {
    let mut output = ToolOutput::default();
    walk(root, def, |rel, path| {
        match state.files.get(&Dest::game(rel.clone())) {
            Some(deployed) => {
                // Files the game rewrites are its own business, and ones
                // without a mod are only moved aside partway through a
                // deploy.
                let replaced = !deployed.rewritten
                    && !deployed.mod_name.is_empty()
                    && drift(path, deployed, Check::Quick) == Some(Drift::Changed);
                if replaced {
                    output.files.push(OutputFile {
                        path: rel,
                        replaces: Some(deployed.mod_name.clone()),
                    });
                }
            }
            None => match baseline.get(&rel) {
                Some(stamp) if Stamp::of(path).as_ref() == Some(stamp) => {}
                Some(_) => output.changed.push(rel),
                None => output.files.push(OutputFile {
                    path: rel,
                    replaces: None,
                }),
            },
        }
    })?;
    Ok(output)
}

/// Calls `f` with each file (or link to one) in `def`'s folders under
/// `root` that isn't ignored, as its path in the game's folder and on disk.
fn walk(root: &Path, def: &CaptureDef, mut f: impl FnMut(RelPath, &Path)) -> Result<()> {
    let ignore = ignore_set(def)?;
    for folder in &def.folders {
        let dir = folder.under(root);
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir).sort_by_file_name() {
            let entry = entry.map_err(|e| Error::Io {
                path: e.path().unwrap_or(&dir).to_owned(),
                source: e.into(),
            })?;
            let kind = entry.file_type();
            if !kind.is_file() && !kind.is_symlink() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(root)
                .expect("walkdir yields paths under its root");
            // Names that can't be a RelPath can't be deployed either.
            let Ok(rel) = RelPath::new(&rel.to_string_lossy()) else {
                continue;
            };
            if !ignore.is_match(rel.as_str()) {
                f(rel, entry.path());
            }
        }
    }
    Ok(())
}

fn ignore_set(def: &CaptureDef) -> Result<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in &def.ignore {
        let glob = GlobBuilder::new(pattern.as_str())
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .map_err(|e| Error::invalid(format!("bad capture pattern: {e}")))?;
        set.add(glob);
    }
    set.build()
        .map_err(|e| Error::invalid(format!("bad capture patterns: {e}")))
}

#[cfg(test)]
mod tests;
