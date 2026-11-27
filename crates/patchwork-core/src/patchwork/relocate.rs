//! Moving patchwork's data folder somewhere else, such as a bigger drive.
//!
//! What's in it goes over as it is: renamed when the new place is on the
//! same drive, otherwise copied. A few files keep absolute paths into it
//! (what's deployed, and where local mods were captured to), and deployed
//! symlinks point into it, so those are rewritten to the new place. Only
//! then does `<config>/locations.toml` (see [`Locations`]) point at the new
//! folder; anything going wrong before that puts everything back as it was.
//!
//! The folder can't be removed while this process still has it open, so
//! it's removed the next time patchwork opens ([`remove_leftover`]). The
//! [`Patchwork`] that moved it mustn't be used afterwards: its paths are the
//! old ones.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use super::Patchwork;
use crate::cancel::Cancel;
use crate::deploy::DeployState;
use crate::error::IoContext;
use crate::event::{Event, Reporter};
use crate::fsutil::{self, StateLock};
use crate::paths::{Locations, Paths};
use crate::{Error, Result};

/// The state lock's file, which stays behind with the process holding it.
const LOCK: &str = ".lock";

/// What moving the data folder to `to` would do; see
/// [`Patchwork::plan_data_move`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataMove {
    pub from: PathBuf,
    pub to: PathBuf,
    /// How much there is to move.
    pub bytes: u64,
    pub files: u64,
    /// What's in the data folder that `to` already has something called
    /// too. It can't be moved there while there's any.
    pub conflicts: Vec<String>,
    /// Whether `to` has nothing in it yet (or isn't there), so it would
    /// hold only patchwork's data.
    pub empty: bool,
    /// Whether `to` is on the same drive, so moving is quick rather than a
    /// copy. `None` where that can't be told beforehand.
    pub same_drive: Option<bool>,
}

/// How far a move has got, in bytes.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MoveProgress {
    pub done: u64,
    pub total: u64,
}

impl Patchwork {
    /// Checks that the data folder can be moved to `to`, which can be a new
    /// folder or one that's already there, and says how much that is.
    pub fn plan_data_move(&self, to: &Path) -> Result<DataMove> {
        let from = &self.paths.data;
        if !to.is_absolute() {
            return Err(Error::invalid(format!(
                "{} isn't a full path; pick a folder",
                to.display()
            )));
        }
        if to.exists() && !to.is_dir() {
            return Err(Error::invalid(format!("{} isn't a folder", to.display())));
        }
        let real_from = fs::canonicalize(from).at(from)?;
        let real_to = canonical_ish(to);
        if real_to == real_from {
            return Err(Error::invalid("that's where patchwork's data already is"));
        }
        if real_to.starts_with(&real_from) || real_from.starts_with(&real_to) {
            return Err(Error::invalid(
                "the data folder can't move into itself, or into a folder it's in",
            ));
        }
        if real_to.starts_with(fs::canonicalize(&self.paths.config).unwrap_or_default()) {
            return Err(Error::invalid(
                "the data folder can't go in patchwork's settings folder",
            ));
        }
        let entries = entries(from)?;
        let conflicts = entries
            .iter()
            .filter(|name| to.join(name).symlink_metadata().is_ok())
            .cloned()
            .collect();
        let (mut bytes, mut files) = (0, 0);
        for name in &entries {
            let (b, f) = tree_size(&from.join(name))?;
            bytes += b;
            files += f;
        }
        Ok(DataMove {
            from: from.clone(),
            to: to.to_owned(),
            bytes,
            files,
            conflicts,
            empty: fs::read_dir(to).map_or(true, |mut d| d.next().is_none()),
            same_drive: same_drive(from, to),
        })
    }

    /// Moves the data folder to `to` (see the module docs), reporting how
    /// far it's got. Stops with [`Error::Cancelled`] once `cancel` is, with
    /// everything back as it was. Patchwork has to be opened again
    /// afterwards to use the new folder.
    pub fn move_data(
        &self,
        to: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(MoveProgress),
    ) -> Result<()> {
        self.move_data_by(to, true, cancel, progress)
    }

    /// [`Patchwork::move_data`], copying everything when `rename` is false,
    /// as between drives.
    pub(super) fn move_data_by(
        &self,
        to: &Path,
        rename: bool,
        cancel: &Cancel,
        progress: &mut dyn FnMut(MoveProgress),
    ) -> Result<()> {
        let plan = self.plan_data_move(to)?;
        if !plan.conflicts.is_empty() {
            return Err(Error::invalid(format!(
                "{} already has {}, which patchwork's data folder has too. Pick an empty \
                 folder, or move those out of the way first",
                to.display(),
                plan.conflicts.join(", ")
            )));
        }
        let created = !to.exists();
        fs::create_dir_all(to).at(to)?;
        let mut moving = Moving {
            from: plan.from.clone(),
            to: to.to_owned(),
            rename,
            done: 0,
            total: plan.bytes,
            renamed: Vec::new(),
            copied: Vec::new(),
            rewritten: Vec::new(),
            relinked: Vec::new(),
            linked: HashMap::new(),
        };
        match moving.run(self, cancel, progress) {
            Ok(()) => {
                let mut locations = Locations::load(&self.paths)?;
                locations.data = Some(to.to_owned());
                locations.leftover = Some(plan.from);
                if let Err(e) = locations.save(&self.paths) {
                    moving.undo(self.reporter.as_ref());
                    if created {
                        let _ = fs::remove_dir(to);
                    }
                    return Err(e);
                }
                Ok(())
            }
            Err(e) => {
                moving.undo(self.reporter.as_ref());
                if created {
                    let _ = fs::remove_dir(to);
                }
                Err(e)
            }
        }
    }
}

/// Removes the data folder that was moved away from, once nothing has it
/// open. Called on opening patchwork, with the state lock held.
pub(super) fn remove_leftover(paths: &Paths, reporter: &dyn Reporter) {
    let Ok(mut locations) = Locations::load(paths) else {
        return;
    };
    let Some(old) = locations.leftover.clone() else {
        return;
    };
    if old != paths.data && old.join(LOCK).is_file() {
        // something that opened patchwork before it moved still has it
        let Ok(lock) = StateLock::acquire(&old.join(LOCK)) else {
            return;
        };
        drop(lock);
        if let Err(e) = fs::remove_dir_all(&old) {
            reporter.report(Event::Warning(format!(
                "couldn't remove {}, where patchwork's data was before it moved ({e}); \
                 it can be deleted",
                old.display()
            )));
        }
    }
    locations.leftover = None;
    if let Err(e) = locations.save(paths) {
        reporter.report(Event::Warning(e.to_string()));
    }
}

/// What a move has done so far, to undo it if it doesn't finish.
struct Moving {
    from: PathBuf,
    to: PathBuf,
    /// Whether to try renaming entries before copying them.
    rename: bool,
    done: u64,
    total: u64,
    /// Entries of the data folder renamed into `to`.
    renamed: Vec<String>,
    /// Entries copied into `to`, wholly or partly.
    copied: Vec<String>,
    /// Files rewritten for the new place, with what they were before.
    rewritten: Vec<(PathBuf, Vec<u8>)>,
    /// Symlinks pointed at the new place, with where they pointed before.
    relinked: Vec<(PathBuf, PathBuf)>,
    /// Files with more than one name, by device and inode, and where the
    /// first was copied to, so the rest are linked to it again.
    linked: HashMap<(u64, u64), PathBuf>,
}

impl Moving {
    fn run(
        &mut self,
        pw: &Patchwork,
        cancel: &Cancel,
        progress: &mut dyn FnMut(MoveProgress),
    ) -> Result<()> {
        progress(self.progress());
        for name in entries(&self.from)? {
            cancel.check()?;
            let (src, dst) = (self.from.join(&name), self.to.join(&name));
            if self.rename && fs::rename(&src, &dst).is_ok() {
                self.done += tree_size(&dst)?.0;
                self.renamed.push(name);
                progress(self.progress());
                continue;
            }
            self.copied.push(name);
            self.copy_tree(&src, &dst, cancel, progress)?;
        }
        cancel.check()?;
        self.rewrite_deployments()?;
        self.rewrite_origins(pw)?;
        self.relink_inside()?;
        self.relink_deployed()?;
        Ok(())
    }

    fn progress(&self) -> MoveProgress {
        MoveProgress {
            done: self.done.min(self.total),
            total: self.total,
        }
    }

    fn rebase(&self, path: &Path) -> Option<PathBuf> {
        Some(self.to.join(path.strip_prefix(&self.from).ok()?))
    }

    fn copy_tree(
        &mut self,
        src: &Path,
        dst: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(MoveProgress),
    ) -> Result<()> {
        for entry in WalkDir::new(src).follow_links(false) {
            let entry = entry.map_err(|e| walk_error(src, e))?;
            let rel = entry.path().strip_prefix(src).expect("walked from src");
            let out = if rel.as_os_str().is_empty() {
                dst.to_owned()
            } else {
                dst.join(rel)
            };
            let kind = entry.file_type();
            if kind.is_dir() {
                fs::create_dir_all(&out).at(&out)?;
            } else if kind.is_symlink() {
                // pointed at the new place later, with the rest
                let target = fs::read_link(entry.path()).at(entry.path())?;
                make_symlink(&target, &out)?;
            } else {
                cancel.check()?;
                let meta = entry.metadata().map_err(|e| walk_error(src, e))?;
                if let Some(first) = self.already_copied(&meta, &out)
                    && fs::hard_link(&first, &out).is_ok()
                {
                    continue;
                }
                fs::copy(entry.path(), &out).at(&out)?;
                self.done += meta.len();
                progress(self.progress());
            }
        }
        Ok(())
    }

    /// Where another name for the file `meta` is about was copied to, if it
    /// has one. Records `out` as its copy otherwise.
    #[cfg(unix)]
    fn already_copied(&mut self, meta: &fs::Metadata, out: &Path) -> Option<PathBuf> {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() < 2 {
            return None;
        }
        let key = (meta.dev(), meta.ino());
        if let Some(first) = self.linked.get(&key) {
            return Some(first.clone());
        }
        self.linked.insert(key, out.to_owned());
        None
    }

    #[cfg(not(unix))]
    fn already_copied(&mut self, _: &fs::Metadata, _: &Path) -> Option<PathBuf> {
        None
    }

    /// Saves `bytes` to `path`, keeping what was there to put back.
    fn rewrite(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        let before = fs::read(path).at(path)?;
        fsutil::atomic_write(path, bytes)?;
        self.rewritten.push((path.to_owned(), before));
        Ok(())
    }

    /// Points what's recorded as deployed at the new place: where files
    /// came from, where originals were backed up to, and profiles' own
    /// folders.
    fn rewrite_deployments(&mut self) -> Result<()> {
        let dir = self.to.join("deployments");
        let Ok(list) = fs::read_dir(&dir) else {
            return Ok(());
        };
        for entry in list {
            let path = entry.at(&dir)?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(mut state) = fsutil::read_json::<DeployState>(&path)? else {
                continue;
            };
            let mut changed = false;
            let mut fix = |p: &mut PathBuf| {
                if let Some(new) = self.rebase(p) {
                    *p = new;
                    changed = true;
                }
            };
            fix(&mut state.root);
            state.roots.values_mut().for_each(&mut fix);
            for file in state.files.values_mut() {
                fix(&mut file.src);
                if let Some(backup) = &mut file.backup {
                    fix(backup);
                }
            }
            if changed {
                let bytes =
                    serde_json::to_vec_pretty(&state).map_err(|e| Error::parse(&path, e))?;
                self.rewrite(&path, &bytes)?;
            }
        }
        Ok(())
    }

    /// Local mods captured into the data folder were added from there.
    fn rewrite_origins(&mut self, pw: &Patchwork) -> Result<()> {
        let file = self.to.join(
            pw.paths
                .local_mods()
                .strip_prefix(&self.from)
                .expect("local mods are in the data folder"),
        );
        if let Some(text) = super::local::rebase_origins(&file, &|p| self.rebase(p))? {
            self.rewrite(&file, text.as_bytes())?;
        }
        Ok(())
    }

    /// Symlinks in the moved folder that point into the old one, such as a
    /// profile's own folder deployed by symlinking.
    fn relink_inside(&mut self) -> Result<()> {
        let mut links = Vec::new();
        for entry in WalkDir::new(&self.to).follow_links(false) {
            let entry = entry.map_err(|e| walk_error(&self.to, e))?;
            if entry.file_type().is_symlink() {
                links.push(entry.into_path());
            }
        }
        for link in links {
            self.relink(&link)?;
        }
        Ok(())
    }

    /// Files deployed into games' folders by symlinking.
    fn relink_deployed(&mut self) -> Result<()> {
        let dir = self.to.join("deployments");
        let Ok(list) = fs::read_dir(&dir) else {
            return Ok(());
        };
        for entry in list {
            let path = entry.at(&dir)?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(state) = fsutil::read_json::<DeployState>(&path)? else {
                continue;
            };
            let roots = state.roots();
            for dest in state.files.keys() {
                let Ok(link) = roots.path(dest) else { continue };
                // in the moved folder, these were done already
                if !link.starts_with(&self.to) {
                    self.relink(&link)?;
                }
            }
        }
        Ok(())
    }

    /// Points `link` at the new place, if it's a symlink into the old one.
    fn relink(&mut self, link: &Path) -> Result<()> {
        let Ok(target) = fs::read_link(link) else {
            return Ok(());
        };
        let Some(new) = self.rebase(&target) else {
            return Ok(());
        };
        fs::remove_file(link).at(link)?;
        if let Err(e) = make_symlink(&new, link) {
            let _ = make_symlink(&target, link);
            return Err(e);
        }
        self.relinked.push((link.to_owned(), target));
        Ok(())
    }

    /// Puts everything back as it was before the move, as far as it can.
    fn undo(&mut self, reporter: &dyn Reporter) {
        let warn = |what: String| reporter.report(Event::Warning(what));
        for (link, target) in self.relinked.drain(..).rev() {
            let _ = fs::remove_file(&link);
            if let Err(e) = make_symlink(&target, &link) {
                warn(format!(
                    "couldn't put back the link {}: {e}",
                    link.display()
                ));
            }
        }
        for (path, before) in self.rewritten.drain(..).rev() {
            if let Err(e) = fsutil::atomic_write(&path, &before) {
                warn(format!("couldn't put back {}: {e}", path.display()));
            }
        }
        for name in self.renamed.drain(..) {
            let (src, dst) = (self.to.join(&name), self.from.join(&name));
            if let Err(e) = fs::rename(&src, &dst) {
                warn(format!(
                    "couldn't move {} back to {}: {e}",
                    src.display(),
                    dst.display()
                ));
            }
        }
        for name in self.copied.drain(..) {
            let _ = fs::remove_dir_all(self.to.join(&name))
                .or_else(|_| fs::remove_file(self.to.join(&name)));
        }
    }
}

/// What's in the data folder to move: everything but the state lock,
/// which stays with this process.
fn entries(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).at(dir)? {
        let name = entry.at(dir)?.file_name().to_string_lossy().into_owned();
        if name != LOCK {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Bytes and files under `path`, not following symlinks.
fn tree_size(path: &Path) -> Result<(u64, u64)> {
    let (mut bytes, mut files) = (0, 0);
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|e| walk_error(path, e))?;
        if entry.file_type().is_file() {
            bytes += entry.metadata().map_err(|e| walk_error(path, e))?.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}

fn walk_error(root: &Path, e: walkdir::Error) -> Error {
    let path = e.path().unwrap_or(root).to_owned();
    match e.into_io_error() {
        Some(source) => Error::Io { path, source },
        None => Error::invalid(format!("{} loops back on itself", path.display())),
    }
}

/// `path` with as much of it as exists made canonical, for comparing a
/// folder that may not exist yet.
fn canonical_ish(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut here = path;
    loop {
        if let Ok(real) = fs::canonicalize(here) {
            return rest.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (here.parent(), here.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                here = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

/// The nearest folder at or above `path` that exists.
fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.exists())
}

#[cfg(unix)]
fn same_drive(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let dev = |p: &Path| fs::metadata(existing_ancestor(p)?).ok().map(|m| m.dev());
    Some(dev(a)? == dev(b)?)
}

#[cfg(not(unix))]
fn same_drive(_: &Path, _: &Path) -> Option<bool> {
    None
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).at(link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    let resolved = link.parent().map_or(target.to_owned(), |p| p.join(target));
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).at(link)
    } else {
        std::os::windows::fs::symlink_file(target, link).at(link)
    }
}
