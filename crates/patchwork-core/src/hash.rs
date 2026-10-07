use std::fmt;
use std::fs::File;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::error::IoContext;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn of_file(path: &Path) -> Result<Self> {
        let file = File::open(path).at(path)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(file).at(path)?;
        Ok(hasher.finalize().into())
    }

    /// An abbreviated form for display.
    pub fn short(&self) -> String {
        self.to_string()[..12].to_owned()
    }
}

impl From<blake3::Hash> for ContentHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl FromStr for ContentHash {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        blake3::Hash::from_hex(s)
            .map(Into::into)
            .map_err(|e| format!("invalid content hash `{s}`: {e}"))
    }
}

impl TryFrom<String> for ContentHash {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> Self {
        hash.to_string()
    }
}
