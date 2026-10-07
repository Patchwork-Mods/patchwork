use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A normalized relative path that can't escape the directory it's joined
/// onto. Used for everything that comes from archives or deployers, so a
/// malicious mod or plugin can't write outside the game directory.
///
/// Always uses `/` as the separator, whatever the platform.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

impl RelPath {
    pub fn new(path: &str) -> Result<Self, String> {
        let path = path.replace('\\', "/");
        if path.starts_with('/') {
            return Err(format!("`{path}` is absolute"));
        }
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(format!("`{path}` points outside its directory")),
                // Rules out drive letters and NTFS alternate data streams.
                p if p.contains(':') => return Err(format!("`{path}` contains `:`")),
                p => parts.push(p),
            }
        }
        if parts.is_empty() {
            return Err(format!("`{path}` is empty"));
        }
        Ok(Self(parts.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    pub fn parent(&self) -> Option<RelPath> {
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| Self(parent.to_owned()))
    }

    pub fn under(&self, root: &Path) -> PathBuf {
        let mut path = root.to_owned();
        path.extend(self.components());
        path
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl TryFrom<String> for RelPath {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl From<RelPath> for String {
    fn from(path: RelPath) -> Self {
        path.0
    }
}

#[cfg(test)]
mod tests {
    use super::RelPath;

    #[test]
    fn normalizes() {
        assert_eq!(RelPath::new("a\\b/./c//").unwrap().as_str(), "a/b/c");
    }

    #[test]
    fn rejects_escapes() {
        for bad in [
            "",
            "/etc/passwd",
            "a/../../b",
            "..",
            "C:/Windows",
            "\\\\server\\x",
        ] {
            assert!(RelPath::new(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn parent_and_name() {
        let p = RelPath::new("BepInEx/plugins/x.dll").unwrap();
        assert_eq!(p.file_name(), "x.dll");
        assert_eq!(p.parent().unwrap().as_str(), "BepInEx/plugins");
        assert_eq!(RelPath::new("x").unwrap().parent(), None);
    }
}
