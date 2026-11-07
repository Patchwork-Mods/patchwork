//! Outlines te way that Sources are defined.
//! the Artifact is the thing that gets moved to the game file
use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::changelog::ChangelogEntry;
use crate::compat::{EnvSupport, GameVersions};
use crate::environment::GameEnv;
use crate::game::GameDef;
use crate::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use crate::settings::SettingSpec;

/// Where the host should fetch an artifact from. Sources only describe this;
/// the host does the fetching, hashing and storing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ArtifactLocation {
    Path {
        path: PathBuf,
    },
    Url {
        url: String,
    },
    /// A file that only its source knows how to download, e.g. because its
    /// download links expire. `file` is the source's own id for it, and
    /// `name` its file name. The host asks [`Source::download`] for it. It's
    /// always a single downloaded file, so never [`ArtifactFormat::Dir`].
    Source {
        file: String,
        name: String,
    },
    /// A local mod's file as it was locked, which can't be downloaded from
    /// anywhere: it's in the store, or at the path it was added from on the
    /// machine that added it. The host records this in lockfiles in place
    /// of [`ArtifactLocation::Path`], which only means something on one
    /// machine; sources don't return it.
    Local {
        name: String,
    },
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
                let name = path.rsplit('/').next().unwrap_or_default();
                percent_decode(name).unwrap_or_else(|| name.to_owned())
            }
            ArtifactLocation::Source { name, .. } | ArtifactLocation::Local { name } => {
                name.clone()
            }
        }
    }
}

/// `name` with its `%XX` escapes decoded, as long as that makes a file
/// name: `None` if it'd have a slash in it, or isn't UTF-8.
fn percent_decode(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                i += 2;
                u8::from_str_radix(hex, 16).ok()?
            }
            byte => byte,
        };
        decoded.push(byte);
        i += 1;
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.contains(['/', '\\'])).then_some(decoded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactFormat {
    /// An archive that's unpacked into a directory tree: a zip, 7z or RAR,
    /// told apart by its contents. Saved as `zip`, which is all it used to
    /// be, so lockfiles from then still read the same.
    #[serde(rename = "zip", alias = "archive")]
    Archive,
    /// A directory, copied as-is.
    Dir,
    /// A single file that's deployed as it is, like a `.pak` or `.ftl`.
    File,
}

impl ArtifactFormat {
    /// What a file or folder on disk is: a folder, an archive going by its
    /// name, or any other file.
    pub fn of_path(path: &Path) -> Self {
        if path.is_dir() {
            ArtifactFormat::Dir
        } else if is_archive_name(&path.to_string_lossy()) {
            ArtifactFormat::Archive
        } else {
            ArtifactFormat::File
        }
    }
}

/// How to get an [`ArtifactLocation::Source`] file right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Download {
    Url(String),
    /// The user has to start the download themselves from `page`, e.g.
    /// because the site only lets its paying users download through its API.
    NeedsUser {
        page: String,
    },
}

/// One of a game's [`ArtifactLocation::Source`] files.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceFile {
    pub game: String,
    pub source: String,
    /// The mod's id.
    pub id: String,
    pub file: String,
}

impl SourceFile {
    /// `source:id` of the mod.
    pub fn key(&self) -> String {
        crate::lock::mod_key(&self.source, &self.id)
    }
}

/// A download that's waiting on the user, from [`Download::NeedsUser`].
/// Once they've downloaded it, give it to [`crate::Patchwork::provide`], or
/// pass the link their browser hands back to [`crate::Patchwork::accept_link`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDownload {
    /// The mod's name in the profile.
    pub name: String,
    pub page: String,
    pub file: SourceFile,
}

/// Where to download one of a source's files, from a link that the user's
/// browser handed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// The mod's id.
    pub id: String,
    pub file: String,
    pub url: String,
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

/// How sure a source is that a mod needs a [`SoftDependency`]. Ones it's
/// sure of are [`Dependency`]s, which are installed along with the mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Need {
    /// The source says the mod works without it.
    Optional,
    /// The source lists it but doesn't say whether it's needed, like
    /// Nexus Mods' requirements, which authors use for both.
    Unknown,
}

/// Something a mod lists that patchwork doesn't install by itself, since
/// it may not be needed; frontends show these so the user can add them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftDependency {
    /// `source:id`, or none for something patchwork can't get, like a
    /// download on another site (see `url`) or a game's DLC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// What the source calls it, if it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub need: Need,
    /// What the mod's author says about it, like what it's for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// A page about it; the only way to get ones without a `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl SoftDependency {
    /// What [`crate::profile::ProfileMod::ignores`] names it by: its key,
    /// or failing that its page or name.
    pub fn ident(&self) -> &str {
        self.key
            .as_deref()
            .or(self.url.as_deref())
            .or(self.name.as_deref())
            .unwrap_or_default()
    }

    /// What to call it: its name, key or page.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.key.as_deref())
            .or(self.url.as_deref())
            .unwrap_or_default()
    }
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
    /// What it needs, which is installed with it.
    pub dependencies: Vec<Dependency>,
    /// What it lists that it may not need; see [`SoftDependency`].
    pub soft_dependencies: Vec<SoftDependency>,
    /// Which versions of the game this is for, if the source can tell.
    /// [`crate::compat::guess`] helps sources that only have free text to
    /// go on.
    pub game_versions: Option<GameVersions>,
    /// Which values of the game's other `[env]` axes this works with, if
    /// the source can tell, like `loader = ["fabric", "quilt"]`.
    pub env: EnvSupport,
    /// What sort of thing this is, in the source's own terms, if it has
    /// more than one: Modrinth's `mod`, `resourcepack` or `shader`, say.
    /// Deployers can tell artifacts apart by it when their files can't.
    pub kind: Option<String>,
}

pub struct SourceContext<'a> {
    pub game: &'a GameDef,
    /// This source's table from the game definition's `[sources]`, if any.
    pub config: Option<&'a toml::Table>,
    /// What the user has set of the source's [`Source::settings`], if
    /// anything.
    pub settings: Option<&'a toml::Table>,
    /// What the profile mods are being resolved for runs on (the game
    /// install's version, and what the profile picks for the game's
    /// `[env]`), when anything's known, so sources with a file for each
    /// version of the game or mod loader can pick the right one.
    pub env: Option<&'a GameEnv>,
}

/// Somewhere mods come from (local files, thunderstore, github, etc.)
#[async_trait]
pub trait Source: Send + Sync {
    /// The prefix users write, as in `local:~/mod.zip`.
    fn id(&self) -> &str;

    /// What to call it in a UI.
    fn name(&self) -> &str {
        self.id()
    }

    /// The settings users can give it, which apply to every game. It gets
    /// what's been set in [`SourceContext::settings`].
    fn settings(&self) -> &[SettingSpec] {
        &[]
    }

    /// Checks that `settings` work (that an API key is valid, say) and
    /// describes what they amount to, such as the account they sign in to.
    async fn check_settings(&self, _settings: Option<&toml::Table>) -> Result<Option<String>> {
        Ok(None)
    }

    /// Works out exactly which artifact `id` at `version` refers to.
    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod>;

    /// How to download `file`, a [`ArtifactLocation::Source`] that
    /// [`Source::resolve`] returned for `id`. Only sources that return those
    /// need this.
    async fn download(&self, _ctx: &SourceContext<'_>, _id: &str, _file: &str) -> Result<Download> {
        Err(crate::Error::Source {
            source_id: self.id().to_owned(),
            message: "has no downloads of its own".to_owned(),
        })
    }

    /// Recognizes a link that the user's browser hands back once they start
    /// a download from a [`Download::NeedsUser`] page, like Nexus's
    /// `nxm://` links. `Ok(None)` means it isn't one, or is for another game.
    async fn accept_link(&self, _ctx: &SourceContext<'_>, _link: &str) -> Result<Option<Handoff>> {
        Ok(None)
    }

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

    /// What a search would show for `id` (its name, picture, summary, ...),
    /// so UIs can show mods that have already been added the same way.
    /// `Ok(None)` means the source has nothing to show beyond the id.
    async fn summary(&self, _ctx: &SourceContext<'_>, _id: &str) -> Result<Option<ModSummary>> {
        Ok(None)
    }

    /// `id`'s changelog as of `version` (the newest if `None`), newest entry
    /// first. Empty means the source has none for it.
    async fn changelog(
        &self,
        _ctx: &SourceContext<'_>,
        _id: &str,
        _version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        Ok(Vec::new())
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
mod tests;

/// Whether a file name looks like an archive patchwork can unpack.
pub fn is_archive_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    ["zip", "7z", "rar"]
        .iter()
        .any(|a| ext.eq_ignore_ascii_case(a))
}
