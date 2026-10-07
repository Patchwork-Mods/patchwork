//! Outlines te way that Sources are defined.
//! the Artifact is the thing that gets moved to the game file
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::game::GameDef;

/// Where the host should fetch an artifact from. Sources only describe this;
/// the host does the fetching, hashing and storing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ArtifactLocation {
    Path { path: PathBuf },
    Url { url: String },
}

impl ArtifactLocation {
    pub fn file_name(&self) -> String {
        match self {
            ArtifactLocation::Path { path } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ArtifactLocation::Url { url } => {
                let path = url.split(['?', '#']).next().unwrap_or(url);
                path.rsplit('/').next().unwrap_or_default().to_owned()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactFormat {
    /// Unpacked into a directory tree.
    Zip,
    /// A directory, copied as-is.
    Dir,
    /// A single file that's deployed as it is, like a `.pak` or `.ftl`.
    File,
}

/// A request for a mod from a particular source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModSpec {
    pub source: String,
    pub id: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedMod {
    /// The canonical id, which is what gets stored in profiles and lockfiles.
    pub id: String,
    /// A suggested name for the mod within a profile.
    pub name: String,
    pub version: Option<String>,
    pub artifact: ArtifactLocation,
    pub format: ArtifactFormat,
    pub dependencies: Vec<ModSpec>,
}

pub struct SourceContext<'a> {
    pub game: &'a GameDef,
    /// This source's table from the game definition's `[sources]`, if any.
    pub config: Option<&'a toml::Table>,
}

/// Somewhere mods come from (local files, thunderstore, github, etc.)
#[async_trait]
pub trait Source: Send + Sync {
    /// The prefix users write, as in `local:~/mod.zip`.
    fn id(&self) -> &str;

    /// Works out exactly which artifact `id` at `version` refers to.
    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod>;
}
