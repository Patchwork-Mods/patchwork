use std::fs::{self, File};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::IoContext;
use crate::{Error, Result};

/// Writes via a temporary file and a rename, so a crash never leaves a
/// half-written state file behind.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).at(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, contents).at(&tmp)?;
    fs::rename(&tmp, path).at(path)
}

/// Copies a directory and everything in it to `dst`, which must not exist.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).at(dst)?;
    for entry in fs::read_dir(src).at(src)? {
        let entry = entry.at(src)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().at(&from)?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).at(&from)?;
        }
    }
    Ok(())
}

/// Reads a TOML file, returning `None` if it doesn't exist.
pub fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|e| Error::parse(path, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).at(path),
    }
}

pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value).map_err(|e| Error::parse(path, e))?;
    atomic_write(path, text.as_bytes())
}

/// Reads a JSON file, returning `None` if it doesn't exist.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::parse(path, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).at(path),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::parse(path, e))?;
    atomic_write(path, &bytes)
}

/// An exclusive lock on patchwork's state, held for the lifetime of the value.
#[derive(Debug)]
pub struct StateLock(#[allow(dead_code)] File);

impl StateLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).at(dir)?;
        }
        let file = File::create(path).at(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Busy(path.to_owned())),
            Err(fs::TryLockError::Error(e)) => Err(e).at(path),
        }
    }
}
