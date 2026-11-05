//! Nicknames the user gives other machines, so they don't have to tell
//! them apart by endpoint id. They're kept in `<data>/sync/names.toml` and
//! never synced: each person names the machines they see for themselves,
//! and a name means nothing to anyone else.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iroh::PublicKey;
use serde::{Deserialize, Serialize};

/// The longest a nickname can be, in characters.
pub const MAX_NAME: usize = 64;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Names {
    /// Nicknames by endpoint id.
    #[serde(default)]
    names: BTreeMap<String, String>,
}

/// The nicknames given to machines from `dir` (see [`crate::Node::spawn`]),
/// by endpoint id.
pub fn peer_names(dir: &Path) -> Result<BTreeMap<String, String>> {
    let path = names_file(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).context(path.display().to_string()),
    };
    let names: Names = toml::from_str(&text).with_context(|| path.display().to_string())?;
    Ok(names.names)
}

/// Calls the machine `peer` `name` from now on, or forgets what it was
/// called if `name` is blank.
pub fn set_peer_name(dir: &Path, peer: &PublicKey, name: &str) -> Result<()> {
    let name = name.trim();
    if name.chars().count() > MAX_NAME {
        bail!("a name can be at most {MAX_NAME} characters");
    }
    if name.chars().any(char::is_control) {
        bail!("a name can't have control characters");
    }
    let mut names = peer_names(dir)?;
    if name.is_empty() {
        names.remove(&peer.to_string());
    } else {
        names.insert(peer.to_string(), name.to_owned());
    }
    let path = names_file(dir);
    let text = toml::to_string(&Names { names })?;
    fs::write(&path, text).with_context(|| path.display().to_string())
}

fn names_file(dir: &Path) -> PathBuf {
    dir.join("names.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn names_are_set_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let peer = SecretKey::generate().public();
        assert!(peer_names(dir.path()).unwrap().is_empty());

        set_peer_name(dir.path(), &peer, "  Sam's laptop ").unwrap();
        let names = peer_names(dir.path()).unwrap();
        assert_eq!(names.get(&peer.to_string()).unwrap(), "Sam's laptop");

        set_peer_name(dir.path(), &peer, " ").unwrap();
        assert!(peer_names(dir.path()).unwrap().is_empty());

        assert!(set_peer_name(dir.path(), &peer, "a\nb").is_err());
        assert!(set_peer_name(dir.path(), &peer, &"x".repeat(MAX_NAME + 1)).is_err());
    }
}
