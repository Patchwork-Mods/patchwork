use std::fs;
use std::path::{Path, PathBuf};

use crate::deploy::ModFile;
use crate::error::IoContext;
use crate::hash::ContentHash;
use crate::relpath::RelPath;
use crate::source::ArtifactFormat;
use crate::{Error, Result};

/// Content-addressed storage for mod artifacts.
///
/// - `blobs/<hash>`: archives and single files, as downloaded
/// - `trees/<hash>`: unpacked archives (keyed by the archive's hash) and
///   directory mods (keyed by a hash of the whole tree)
///
/// Entries are immutable once written, so any number of profiles can share
/// them.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn blob(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("blobs").join(hash.to_string())
    }

    fn tree(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("trees").join(hash.to_string())
    }

    /// Where an artifact lives in the store: the file as it was fetched, or
    /// the directory for directory mods.
    pub fn path(&self, hash: &ContentHash, format: ArtifactFormat) -> PathBuf {
        match format {
            ArtifactFormat::Archive | ArtifactFormat::File => self.blob(hash),
            ArtifactFormat::Dir => self.tree(hash),
        }
    }

    pub fn contains(&self, hash: &ContentHash, format: ArtifactFormat) -> bool {
        match format {
            ArtifactFormat::Archive | ArtifactFormat::File => self.blob(hash).is_file(),
            ArtifactFormat::Dir => self.tree(hash).is_dir(),
        }
    }

    /// Copies a local artifact into the store.
    pub fn ingest_path(&self, path: &Path, format: ArtifactFormat) -> Result<ContentHash> {
        match format {
            ArtifactFormat::Archive | ArtifactFormat::File => self.ingest_file(path),
            ArtifactFormat::Dir => self.ingest_dir(path),
        }
    }

    fn ingest_file(&self, path: &Path) -> Result<ContentHash> {
        let hash = ContentHash::of_file(path)?;
        let dest = self.blob(&hash);
        if !dest.is_file() {
            let tmp = self.scratch(&dest)?;
            fs::copy(path, &tmp).at(path)?;
            fs::rename(&tmp, &dest).at(&dest)?;
        }
        Ok(hash)
    }

    fn ingest_dir(&self, path: &Path) -> Result<ContentHash> {
        let files = list_tree(path)?;
        // Hash of (path, content hash) pairs in sorted order, so the same
        // tree always gets the same hash.
        let mut hasher = blake3::Hasher::new();
        for (rel, src) in &files {
            hasher.update(rel.as_str().as_bytes());
            hasher.update(&[0]);
            hasher.update(ContentHash::of_file(src)?.to_string().as_bytes());
            hasher.update(&[0]);
        }
        let hash = hasher.finalize().into();
        let dest = self.tree(&hash);
        if !dest.is_dir() {
            let tmp = self.scratch(&dest)?;
            for (rel, src) in &files {
                let target = rel.under(&tmp);
                create_parent(&target)?;
                fs::copy(src, &target).at(src)?;
            }
            fs::rename(&tmp, &dest).at(&dest)?;
        }
        Ok(hash)
    }

    /// Lists an artifact's files, unpacking it first if it hasn't been yet.
    pub fn files(
        &self,
        hash: &ContentHash,
        format: ArtifactFormat,
        file_name: &str,
    ) -> Result<Vec<ModFile>> {
        let tree = match format {
            ArtifactFormat::File => {
                let rel = RelPath::new(file_name).map_err(Error::Invalid)?;
                return Ok(vec![ModFile {
                    rel,
                    src: self.blob(hash),
                }]);
            }
            ArtifactFormat::Dir => self.tree(hash),
            ArtifactFormat::Archive => {
                let tree = self.tree(hash);
                if !tree.is_dir() {
                    let tmp = self.scratch(&tree)?;
                    unpack::unpack(&self.blob(hash), &tmp)?;
                    fs::rename(&tmp, &tree).at(&tree)?;
                }
                tree
            }
        };
        Ok(list_tree(&tree)?
            .into_iter()
            .map(|(rel, src)| ModFile { rel, src })
            .collect())
    }

    /// A fresh temporary path next to `dest`, so the final rename stays on
    /// one filesystem and is atomic.
    fn scratch(&self, dest: &Path) -> Result<PathBuf> {
        let dir = dest.parent().expect("store paths have parents");
        fs::create_dir_all(dir).at(dir)?;
        let name = dest.file_name().unwrap_or_default().to_string_lossy();
        let tmp = dir.join(format!(".{name}.tmp"));
        if tmp.is_dir() {
            fs::remove_dir_all(&tmp).at(&tmp)?;
        } else if tmp.exists() {
            fs::remove_file(&tmp).at(&tmp)?;
        }
        Ok(tmp)
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    Ok(())
}

/// Every regular file under `root`, sorted by relative path.
fn list_tree(root: &Path) -> Result<Vec<(RelPath, PathBuf)>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|e| Error::Io {
            path: e.path().unwrap_or(root).to_owned(),
            source: e.into(),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .expect("walkdir yields paths under its root");
        let rel = RelPath::new(&rel.to_string_lossy()).map_err(Error::Invalid)?;
        files.push((rel, entry.into_path()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

mod unpack;

#[cfg(test)]
mod tests;
