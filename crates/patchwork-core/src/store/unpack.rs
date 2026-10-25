//! Unpacking the archives mods come in. What kind an archive is comes from
//! its first few bytes rather than its name, which sites don't always get
//! right.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::IoContext;
use crate::relpath::RelPath;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Zip,
    SevenZip,
    Rar,
}

impl Kind {
    fn of(start: &[u8]) -> Option<Self> {
        if start.starts_with(b"PK\x03\x04") || start.starts_with(b"PK\x05\x06") {
            Some(Kind::Zip)
        } else if start.starts_with(b"7z\xBC\xAF\x27\x1C") {
            Some(Kind::SevenZip)
        // RAR 1.5 to 4, and RAR 5
        } else if start.starts_with(b"Rar!\x1A\x07\x00")
            || start.starts_with(b"Rar!\x1A\x07\x01\x00")
        {
            Some(Kind::Rar)
        } else {
            None
        }
    }
}

/// Unpacks `archive` into `dest`, which shouldn't exist yet.
pub(super) fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let mut start = Vec::with_capacity(8);
    File::open(archive)
        .and_then(|f| f.take(8).read_to_end(&mut start))
        .at(archive)?;
    fs::create_dir_all(dest).at(dest)?;
    match Kind::of(&start) {
        Some(Kind::Zip) => zip(archive, dest),
        Some(Kind::SevenZip) => seven_zip(archive, dest),
        Some(Kind::Rar) => rar(archive, dest),
        None => Err(Error::parse(
            archive,
            "isn't a zip, 7z or RAR archive, which are the kinds patchwork can unpack",
        )),
    }
}

/// Where the entry called `name` goes in `dest`, with its folder made.
/// Checked here rather than left to each format's library, because plenty
/// of mod archives use `\` as the separator, and so that none of them can
/// write outside `dest`.
fn target(archive: &Path, dest: &Path, name: &str) -> Result<PathBuf> {
    let rel = RelPath::new(name).map_err(|e| Error::parse(archive, format!("bad entry: {e}")))?;
    let target = rel.under(dest);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    Ok(target)
}

/// Copies an entry's contents to `target`, blaming the archive for what
/// goes wrong reading them and `target` for what goes wrong writing.
fn write_entry(archive: &Path, mut data: impl Read, target: &Path) -> Result<()> {
    let mut out = File::create(target).at(target)?;
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match data.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::parse(archive, e)),
        };
        out.write_all(&buf[..n]).at(target)?;
    }
}

fn password_protected(archive: &Path) -> Error {
    Error::parse(
        archive,
        "is password-protected, which patchwork can't unpack",
    )
}

fn zip(archive: &Path, dest: &Path) -> Result<()> {
    let bad = |e: zip::result::ZipError| match e {
        zip::result::ZipError::UnsupportedArchive(zip::result::ZipError::PASSWORD_REQUIRED) => {
            password_protected(archive)
        }
        e => Error::parse(archive, e),
    };
    let file = File::open(archive).at(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(bad)?;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(bad)?;
        if entry.is_dir() {
            continue;
        }
        let target = target(archive, dest, entry.name())?;
        write_entry(archive, entry, &target)?;
    }
    Ok(())
}

fn seven_zip(archive: &Path, dest: &Path) -> Result<()> {
    use sevenz_rust2::{ArchiveReader, Error as SevenZipError, Password};
    let bad = |e: SevenZipError| match e {
        SevenZipError::PasswordRequired | SevenZipError::MaybeBadPassword(_) => {
            password_protected(archive)
        }
        // what it says when it's built without decryption
        SevenZipError::UnsupportedCompressionMethod(method) if method.starts_with("AES") => {
            password_protected(archive)
        }
        e => Error::parse(archive, e),
    };
    let mut reader = ArchiveReader::open(archive, Password::empty()).map_err(bad)?;
    // Errors of our own can't go through the callback, so the first one is
    // kept here and everything after it is skipped.
    let mut failed = None;
    reader
        .for_each_entries(|entry, data| {
            if failed.is_some() {
                return Ok(false);
            }
            let result = if entry.is_directory() || entry.is_anti_item() {
                // read anyway, since the next entry's data follows on
                io::copy(data, &mut io::sink())
                    .map(drop)
                    .map_err(|e| Error::parse(archive, e))
            } else {
                target(archive, dest, entry.name())
                    .and_then(|target| write_entry(archive, data, &target))
            };
            match result {
                Ok(()) => Ok(true),
                Err(e) => {
                    failed = Some(e);
                    Ok(false)
                }
            }
        })
        .map_err(bad)?;
    failed.map_or(Ok(()), Err)
}

fn rar(archive: &Path, dest: &Path) -> Result<()> {
    let bad = |e: unrar_ng::error::UnrarError| Error::parse(archive, e);
    let mut open = unrar_ng::Archive::new(archive)
        .open_for_processing()
        .map_err(bad)?;
    while let Some(header) = open.read_header().map_err(bad)? {
        let entry = header.entry();
        if entry.is_encrypted() {
            return Err(password_protected(archive));
        }
        open = if entry.is_file() {
            let target = target(archive, dest, &entry.filename.to_string_lossy())?;
            let next = header.extract_to(&target).map_err(bad)?;
            // RAR archives can hold symlinks, which unrar makes unless they
            // look unsafe. Even a safe-looking one could have a later entry
            // written through it, so none are kept.
            if target.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
                fs::remove_file(&target).at(&target)?;
            }
            next
        } else {
            header.skip().map_err(bad)?
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests;
