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
            let file = tmp.path().join("blob");
            fs::copy(path, &file).at(path)?;
            publish(&file, &dest)?;
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
                let target = rel.under(tmp.path());
                create_parent(&target)?;
                fs::copy(src, &target).at(src)?;
            }
            publish(tmp.path(), &dest)?;
        }
        Ok(hash)
    }

    /// Where an artifact's files are, if it's a directory or an archive
    /// that's been unpacked. Doesn't unpack anything.
    pub fn unpacked(&self, hash: &ContentHash, format: ArtifactFormat) -> Option<PathBuf> {
        match format {
            ArtifactFormat::File => None,
            ArtifactFormat::Dir | ArtifactFormat::Archive => {
                Some(self.tree(hash)).filter(|t| t.is_dir())
            }
        }
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
                    unpack::unpack(&self.blob(hash), tmp.path())?;
                    publish(tmp.path(), &tree)?;
                }
                tree
            }
        };
        Ok(list_tree(&tree)?
            .into_iter()
            .map(|(rel, src)| ModFile { rel, src })
            .collect())
    }

    /// A fresh scratch directory next to `dest`, so moving what's built in
    /// it into place stays on one filesystem and is atomic. Each call gets
    /// its own, since the GUI can fill the same entry from two commands at
    /// once, and it's removed when dropped.
    fn scratch(&self, dest: &Path) -> Result<tempfile::TempDir> {
        let dir = dest.parent().expect("store paths have parents");
        fs::create_dir_all(dir).at(dir)?;
        let name = dest.file_name().unwrap_or_default().to_string_lossy();
        tempfile::Builder::new()
            .prefix(&format!(".{name}."))
            .suffix(".tmp")
            .tempdir_in(dir)
            .at(dir)
    }
}

/// Moves `tmp` into place as `dest`. If something else put `dest` there
/// first it's the same content, since entries are named by their hash, so
/// that one is kept.
fn publish(tmp: &Path, dest: &Path) -> Result<()> {
    match fs::rename(tmp, dest) {
        Err(_) if dest.exists() => Ok(()),
        renamed => renamed.at(dest),
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    Ok(())
}

/// Every regular file under `root`, sorted by relative path.
pub fn list_tree(root: &Path) -> Result<Vec<(RelPath, PathBuf)>> {
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
