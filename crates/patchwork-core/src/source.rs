//! Outlines te way that Sources are defined.
//! the Artifact is the thing that gets moved to the game file
use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::game::GameDef;
use crate::search::{Category, SearchPage, SearchQuery, SortBy};

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

/// What a mod needs of a dependency's version. Sources decide what the
/// versions in their metadata mean; Thunderstore's are minimums, for example.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum VersionReq {
    Any,
    AtLeast(String),
    Exact(String),
}

impl VersionReq {
    pub fn is_any(&self) -> bool {
        *self == VersionReq::Any
    }

    /// Whether `version` meets this, given how the dependency's source
    /// orders versions. `None` means it can't tell.
    pub fn matches(
        &self,
        version: &str,
        compare: impl Fn(&str, &str) -> Option<Ordering>,
    ) -> Option<bool> {
        match self {
            VersionReq::Any => Some(true),
            VersionReq::AtLeast(min) => compare(version, min).map(Ordering::is_ge),
            VersionReq::Exact(want) => compare(version, want).map(Ordering::is_eq),
        }
    }
}

/// Written as `*`, `>=1.2.3` or `=1.2.3`, which is also how lockfiles store it.
impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionReq::Any => write!(f, "*"),
            VersionReq::AtLeast(v) => write!(f, ">={v}"),
            VersionReq::Exact(v) => write!(f, "={v}"),
        }
    }
}

impl FromStr for VersionReq {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim();
        let req = if s == "*" {
            VersionReq::Any
        } else if let Some(v) = s.strip_prefix(">=") {
            VersionReq::AtLeast(v.trim().to_owned())
        } else if let Some(v) = s.strip_prefix('=') {
            VersionReq::Exact(v.trim().to_owned())
        } else {
            return Err(format!(
                "`{s}` isn't a version requirement (expected `*`, `>=x` or `=x`)"
            ));
        };
        match &req {
            VersionReq::AtLeast(v) | VersionReq::Exact(v) if v.is_empty() => {
                Err(format!("`{s}` is missing a version"))
            }
            _ => Ok(req),
        }
    }
}

impl TryFrom<String> for VersionReq {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        s.parse()
    }
}

impl From<VersionReq> for String {
    fn from(req: VersionReq) -> String {
        req.to_string()
    }
}

/// Another mod that a mod needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Dependency {
    pub source: String,
    pub id: String,
    pub req: VersionReq,
}

/// Orders dotted numeric versions like `5.4.2100`, treating missing parts as
/// zero so `1.2` and `1.2.0` are equal. `None` if either has a part that
/// isn't a number.
pub fn compare_dotted(a: &str, b: &str) -> Option<Ordering> {
    let parse = |v: &str| {
        v.split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()
    };
    let (a, b) = (parse(a)?, parse(b)?);
    for i in 0..a.len().max(b.len()) {
        let ord = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if ord.is_ne() {
            return Some(ord);
        }
    }
    Some(Ordering::Equal)
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
    pub dependencies: Vec<Dependency>,
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

    /// Orders two of this source's versions, which is how dependency
    /// requirements get checked. `None` means they can't be compared.
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        compare_dotted(a, b)
    }

    /// Whether [`Source::versions`] can list this source's versions, so UIs
    /// know whether to offer a choice without asking.
    fn lists_versions(&self) -> bool {
        false
    }

    /// Every version of `id` that can be asked for, newest first.
    async fn versions(&self, _ctx: &SourceContext<'_>, _id: &str) -> Result<Vec<String>> {
        Err(crate::Error::Source {
            source_id: self.id().to_owned(),
            message: "can't list versions".to_owned(),
        })
    }

    /// A link to `id`'s page, used when sharing a profile as a list of URLs.
    /// It should pin `version` if the source's URLs can express one.
    fn mod_url(
        &self,
        _ctx: &SourceContext<'_>,
        _id: &str,
        _version: Option<&str>,
    ) -> Option<String> {
        None
    }

    /// The reverse of [`Source::mod_url`]: recognizes a link to one of this
    /// source's mods. `Ok(None)` means it isn't one; a link to this source
    /// that's for a different game is an error.
    fn parse_url(&self, _ctx: &SourceContext<'_>, _url: &str) -> Result<Option<ModSpec>> {
        Ok(None)
    }

    /// The orders [`Source::search`] can return mods in. Empty means the
    /// source can't be searched.
    fn search_sorts(&self) -> &[SortBy] {
        &[]
    }

    /// The categories searches can be narrowed to for this game.
    async fn search_categories(&self, _ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        Ok(Vec::new())
    }

    /// Finds mods for this game. The host checks that `query.sort` is one of
    /// [`Source::search_sorts`] first.
    async fn search(&self, _ctx: &SourceContext<'_>, _query: &SearchQuery) -> Result<SearchPage> {
        Err(crate::Error::Source {
            source_id: self.id().to_owned(),
            message: "can't be searched".to_owned(),
        })
    }
}

/// Splits an `http(s)` URL into its host and non-empty path segments,
/// ignoring any query or fragment. A helper for [`Source::parse_url`].
pub fn url_parts(url: &str) -> Option<(&str, Vec<&str>)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.strip_prefix("www.").unwrap_or(host);
    Some((host, path.split('/').filter(|s| !s.is_empty()).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_dotted_versions() {
        assert_eq!(
            compare_dotted("5.4.2100", "5.4.21"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_dotted("1.2", "1.2.0"), Some(Ordering::Equal));
        assert_eq!(compare_dotted("1.9", "1.10"), Some(Ordering::Less));
        assert_eq!(compare_dotted("5.2SE", "5.1"), None);
    }

    #[test]
    fn version_reqs_round_trip_as_strings() {
        for (text, req) in [
            ("*", VersionReq::Any),
            (">=5.4.2100", VersionReq::AtLeast("5.4.2100".to_owned())),
            ("=1.0", VersionReq::Exact("1.0".to_owned())),
        ] {
            assert_eq!(text.parse::<VersionReq>().unwrap(), req);
            assert_eq!(req.to_string(), text);
        }
        assert!("1.0".parse::<VersionReq>().is_err());
        assert!(">=".parse::<VersionReq>().is_err());
    }

    #[test]
    fn version_reqs_match() {
        let at_least = VersionReq::AtLeast("1.2".to_owned());
        assert_eq!(at_least.matches("1.10", compare_dotted), Some(true));
        assert_eq!(at_least.matches("1.1.9", compare_dotted), Some(false));
        assert_eq!(at_least.matches("beta", compare_dotted), None);
        let exact = VersionReq::Exact("1.2".to_owned());
        assert_eq!(exact.matches("1.2.0", compare_dotted), Some(true));
        assert_eq!(exact.matches("1.3", compare_dotted), Some(false));
        assert_eq!(VersionReq::Any.matches("beta", compare_dotted), Some(true));
    }
}
