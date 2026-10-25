//! The folders a [`DeployTarget`](super::DeployTarget) deploys into. Most
//! games only have their own folder (the `game` root), but some also read
//! files from elsewhere, like the load order Skyrim keeps in AppData.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::relpath::RelPath;
use crate::{Error, Result};

/// The name of one of a target's roots: lowercase letters, digits and `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RootId(String);

impl RootId {
    /// The game's own folder, which every target has.
    pub const GAME: &str = "game";

    pub fn new(id: &str) -> std::result::Result<Self, String> {
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(id.to_owned()))
        } else {
            Err(format!(
                "`{id}` isn't a valid root; use lowercase letters, digits and `-`"
            ))
        }
    }

    pub fn game() -> Self {
        Self(Self::GAME.to_owned())
    }

    pub fn is_game(&self) -> bool {
        self.0 == Self::GAME
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RootId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RootId {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        Self::new(&s)
    }
}

impl From<RootId> for String {
    fn from(id: RootId) -> String {
        id.0
    }
}

/// Where a file goes: a path inside one of the target's roots. Written
/// `appdata:plugins.txt`, or just the path for the `game` root, which is
/// unambiguous because neither root ids nor [`RelPath`]s can contain `:`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Dest {
    pub root: RootId,
    pub path: RelPath,
}

impl Dest {
    pub fn new(root: RootId, path: RelPath) -> Self {
        Self { root, path }
    }

    /// `path` in the game's own folder.
    pub fn game(path: RelPath) -> Self {
        Self::new(RootId::game(), path)
    }

    /// The folder this is in, if it's not directly in its root.
    pub fn parent(&self) -> Option<Dest> {
        Some(Self::new(self.root.clone(), self.path.parent()?))
    }
}

impl fmt::Display for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.root.is_game() {
            write!(f, "{}", self.path)
        } else {
            write!(f, "{}:{}", self.root, self.path)
        }
    }
}

impl fmt::Debug for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl FromStr for Dest {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.split_once(':') {
            Some((root, path)) => Ok(Self::new(RootId::new(root)?, RelPath::new(path)?)),
            None => Ok(Self::game(RelPath::new(s)?)),
        }
    }
}

impl TryFrom<String> for Dest {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        s.parse()
    }
}

impl From<Dest> for String {
    fn from(dest: Dest) -> String {
        dest.to_string()
    }
}

/// Where each of a target's roots is on disk. There's always a `game` root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots(BTreeMap<RootId, PathBuf>);

impl Roots {
    /// Just the game's own folder.
    pub fn game(path: &Path) -> Self {
        Self(BTreeMap::from([(RootId::game(), path.to_owned())]))
    }

    /// Adds another root, or moves one.
    pub fn with(mut self, root: RootId, path: &Path) -> Self {
        self.0.insert(root, path.to_owned());
        self
    }

    pub fn game_path(&self) -> &Path {
        &self.0[&RootId::game()]
    }

    pub fn get(&self, root: &RootId) -> Option<&Path> {
        self.0.get(root).map(PathBuf::as_path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RootId, &Path)> {
        self.0.iter().map(|(id, path)| (id, path.as_path()))
    }

    /// Where `dest` is on disk. An error for a root the target doesn't
    /// have, which is how a deployer naming one gets caught.
    pub fn path(&self, dest: &Dest) -> Result<PathBuf> {
        let root = self.get(&dest.root).ok_or_else(|| {
            Error::invalid(format!(
                "{dest} is in a folder called `{}`, which this game doesn't have",
                dest.root
            ))
        })?;
        Ok(dest.path.under(root))
    }
}
