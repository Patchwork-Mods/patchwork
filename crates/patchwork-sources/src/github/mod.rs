//! Mods from the files attached to a GitHub repo's releases.
//!
//! Ids are `owner/repo/pattern`, where the pattern is the name of the file
//! to take from each release with `*` for the part that changes, like
//! `github:someone/SomeMod/SomeMod-*.zip`; see [`pattern`]. Without a
//! pattern (`github:someone/SomeMod`) a repo whose releases have just one
//! file uses that; otherwise [`Source::choices`] lists the newest release's
//! files, each with the pattern guessed from its name, for the user to pick
//! from. Versions are release tags.
//!
//! The newest release that isn't a prerelease is used, or the newest
//! prerelease if no other release has a matching file. Only the newest 100
//! releases are looked at, and only public repos can be downloaded from.
//!
//! GitHub itself isn't searched, but a game can list mods to show on the
//! Add mods tab (and to search among), each an id as above with an
//! optional name to show instead of the repo's:
//!
//! ```toml
//! [sources.github]
//! mods = [
//!     "someone/SomeMod",
//!     { id = "someone/Big-Releases/Big.*.-.Data.zip", name = "Big (data)" },
//! ]
//! ```
//!
//! GitHub allows 60 requests an hour without signing in. The `token`
//! setting (or `GITHUB_TOKEN`) raises that to 5000; a fine-grained token
//! with no permissions at all is enough.

use crate::pattern;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::changelog::ChangelogEntry;
use patchwork_core::search::{ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::settings::{SettingKind, SettingSpec};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, FileChoice, ModSpec, ResolvedMod, Source, SourceContext,
    compare_dotted, is_archive_name, url_parts,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const ID: &str = "github";
const API: &str = "https://api.github.com";
const SITE: &str = "https://github.com";
/// How long a repo's releases are reused for, so adding a mod and then
/// showing its details and changelog doesn't use up the hourly allowance.
const FRESH_FOR: Duration = Duration::from_secs(60);

const TOKEN: SettingSpec = SettingSpec::new("token", "Access token", SettingKind::Secret)
    .help(
        "Optional. GitHub allows 60 requests an hour without one, and 5000 with \
         one. A fine-grained token with no permissions is enough.",
    )
    .link("https://github.com/settings/personal-access-tokens/new")
    .env("GITHUB_TOKEN");

/// The first parts of github.com links that aren't users or organizations.
const NOT_OWNERS: &[&str] = &[
    "about",
    "apps",
    "collections",
    "enterprise",
    "explore",
    "features",
    "issues",
    "login",
    "marketplace",
    "notifications",
    "orgs",
    "pricing",
    "pulls",
    "search",
    "settings",
    "sponsors",
    "topics",
    "users",
];

pub struct GitHub;

// --- API Responses ---

#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    size: Option<u64>,
    browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Repo {
    name: String,
    description: Option<String>,
    html_url: String,
    stargazers_count: Option<u64>,
    pushed_at: Option<Timestamp>,
    owner: Owner,
}

#[derive(Debug, Clone, Deserialize)]
struct Owner {
    login: String,
    avatar_url: Option<String>,
    html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct User {
    login: String,
}

// --- Ids ---

/// A mod's id, split up.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModId<'a> {
    owner: &'a str,
    repo: &'a str,
    pattern: Option<&'a str>,
}

impl<'a> ModId<'a> {
    fn parse(id: &'a str) -> Result<Self> {
        let mut parts = id.splitn(3, '/');
        let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
            return Err(error(format!(
                "`{id}` isn't `owner/repo` or `owner/repo/file pattern`"
            )));
        };
        let pattern = parts.next().filter(|p| !p.is_empty());
        for name in [owner, repo] {
            if !is_name(name) {
                return Err(error(format!("`{name}` in `{id}` isn't a GitHub name")));
            }
        }
        Ok(ModId {
            owner,
            repo,
            pattern,
        })
    }

    fn repo(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    fn with(&self, pattern: &str) -> String {
        format!("{}/{}/{pattern}", self.owner, self.repo)
    }
}

/// Whether `name` could be a GitHub user, organization or repo.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

// --- Picking releases and files ---

/// Whether `release` has a file `pattern` picks, or any file at all
/// without one.
fn fits(release: &Release, pattern: Option<&str>) -> bool {
    match pattern {
        Some(p) => release.assets.iter().any(|a| pattern::matches(p, &a.name)),
        None => !release.assets.is_empty(),
    }
}

/// The release to use when none is asked for: the newest that fits and
/// isn't a prerelease, or the newest prerelease that fits.
fn newest<'a>(releases: &'a [Release], pattern: Option<&str>) -> Option<&'a Release> {
    let mut fitting = releases.iter().filter(|r| fits(r, pattern));
    let first = fitting.clone().next();
    fitting.find(|r| !r.prerelease).or(first)
}

/// The release `version` asks for, or the [`newest`] if it doesn't.
fn release<'a>(
    releases: &'a [Release],
    id: &ModId<'_>,
    version: Option<&str>,
) -> Result<&'a Release> {
    match version {
        Some(tag) => releases
            .iter()
            .find(|r| r.tag_name == tag)
            .ok_or_else(|| error(format!("{} has no release `{tag}`", id.repo()))),
        None => newest(releases, id.pattern).ok_or_else(|| {
            error(match id.pattern {
                Some(p) => format!("no release of {} has a file matching `{p}`", id.repo()),
                None => format!("{} has no releases with files", id.repo()),
            })
        }),
    }
}

/// The file `pattern` picks from `release`. When it matches several, the
/// one named for the release's version wins, then the shortest name.
fn pick_asset<'a>(release: &'a Release, pattern: &str) -> Option<&'a Asset> {
    let matching: Vec<&Asset> = release
        .assets
        .iter()
        .filter(|a| pattern::matches(pattern, &a.name))
        .collect();
    let expected = pattern::tag_version(&release.tag_name).map(|v| pattern::fill(pattern, v));
    matching
        .iter()
        .find(|a| {
            expected
                .as_ref()
                .is_some_and(|e| a.name.eq_ignore_ascii_case(e))
        })
        .or_else(|| matching.iter().min_by_key(|a| a.name.len()))
        .copied()
}

fn to_resolved(id: &ModId<'_>, pattern: &str, release: &Release, asset: &Asset) -> ResolvedMod {
    let format = match is_archive_name(&asset.name) {
        true => ArtifactFormat::Archive,
        false => ArtifactFormat::File,
    };
    ResolvedMod {
        id: id.with(pattern),
        name: id.repo.to_owned(),
        version: Some(release.tag_name.clone()),
        artifact: ArtifactLocation::Url {
            url: asset.browser_download_url.clone(),
        },
        format,
        dependencies: Vec::new(),
        soft_dependencies: Vec::new(),
        game_versions: None,
        env: Default::default(),
        kind: None,
    }
}

/// What to add for each of `release`'s files.
fn choices(id: &ModId<'_>, release: &Release) -> Vec<FileChoice> {
    release
        .assets
        .iter()
        .map(|asset| {
            let pattern = pattern::infer(&asset.name, &release.tag_name);
            let later = match pattern.contains('*') {
                true => format!("updates pick {pattern}"),
                false => format!("updates pick files named exactly {pattern}"),
            };
            FileChoice {
                id: id.with(&pattern),
                name: asset.name.clone(),
                size: asset.size,
                description: Some(format!("From {}; {later}", release.tag_name)),
            }
        })
        .collect()
}

// --- Links ---

/// The `#file=` part of a link [`GitHub::mod_url`] made, which carries the
/// pattern along so a shared link adds the same file.
const FILE_FRAGMENT: &str = "file=";

fn parse_link(url: &str) -> Option<String> {
    let (host, segments) = url_parts(url)?;
    if host != "github.com" {
        return None;
    }
    let [owner, repo, rest @ ..] = segments.as_slice() else {
        return None;
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if NOT_OWNERS.contains(owner) || !is_name(owner) || !is_name(repo) {
        return None;
    }
    let fragment = url.split_once('#').map(|(_, f)| f);
    let pattern = match (rest, fragment.and_then(|f| f.strip_prefix(FILE_FRAGMENT))) {
        (_, Some(pattern)) if !pattern.is_empty() && !pattern.contains('/') => pattern.to_owned(),
        (["releases", "download", tag, file], _) => pattern::infer(file, tag),
        _ => return Some(format!("{owner}/{repo}")),
    };
    Some(format!("{owner}/{repo}/{pattern}"))
}

// --- Requests ---

fn token(settings: Option<&toml::Table>) -> Option<&str> {
    settings?
        .get(TOKEN.key)?
        .as_str()
        .filter(|t| !t.trim().is_empty())
}

async fn get_json<T: DeserializeOwned>(
    settings: Option<&toml::Table>,
    url: &str,
    not_found: impl FnOnce() -> String,
) -> Result<T> {
    let reqwest_error = |result| patchwork_core::Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let mut request = patchwork_core::http()
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = token(settings) {
        request = request.bearer_auth(token.trim());
    }
    let response = request.send().await.map_err(reqwest_error)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(error(not_found()));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(error("GitHub doesn't accept the access token"));
    }
    if let Some(message) = rate_limited(&response) {
        return Err(error(message));
    }
    let bytes = response
        .error_for_status()
        .map_err(reqwest_error)?
        .bytes()
        .await
        .map_err(reqwest_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// What to say if GitHub turned a request down for being over its limit.
fn rate_limited(response: &reqwest::Response) -> Option<String> {
    let status = response.status();
    if status != reqwest::StatusCode::FORBIDDEN && status != reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return None;
    }
    let header = |name| response.headers().get(name)?.to_str().ok();
    if header("x-ratelimit-remaining") != Some("0") {
        return None;
    }
    let minutes = header("x-ratelimit-reset")
        .and_then(|r| r.parse::<i64>().ok())
        .map(|reset| (reset - Timestamp::now().as_second()).max(0) / 60 + 1);
    let when = match minutes {
        Some(1) => "in a minute".to_owned(),
        Some(m) => format!("in {m} minutes"),
        None => "later".to_owned(),
    };
    Some(format!(
        "GitHub's limit on requests is used up; try again {when}, or set an access token \
         (patchwork config set github.token) for a higher limit"
    ))
}

type Releases = Arc<Vec<Release>>;

/// `id`'s releases, newest first, leaving out drafts.
async fn releases(ctx: &SourceContext<'_>, id: &ModId<'_>) -> Result<Releases> {
    static KNOWN: LazyLock<Mutex<HashMap<String, (Instant, Releases)>>> =
        LazyLock::new(Mutex::default);
    let key = id.repo().to_ascii_lowercase();
    if let Some((at, known)) = KNOWN.lock().unwrap().get(&key)
        && at.elapsed() < FRESH_FOR
    {
        return Ok(known.clone());
    }
    let url = format!("{API}/repos/{}/releases?per_page=100", id.repo());
    let mut found: Vec<Release> = get_json(ctx.settings, &url, || no_repo(id)).await?;
    found.retain(|r| !r.draft);
    let found = Arc::new(found);
    KNOWN
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), found.clone()));
    Ok(found)
}

/// `id`'s repo, reused for a while like [`releases`].
async fn repo(ctx: &SourceContext<'_>, id: &ModId<'_>) -> Result<Repo> {
    static KNOWN: LazyLock<Mutex<HashMap<String, (Instant, Repo)>>> = LazyLock::new(Mutex::default);
    let key = id.repo().to_ascii_lowercase();
    if let Some((at, known)) = KNOWN.lock().unwrap().get(&key)
        && at.elapsed() < FRESH_FOR
    {
        return Ok(known.clone());
    }
    let url = format!("{API}/repos/{}", id.repo());
    let found: Repo = get_json(ctx.settings, &url, || no_repo(id)).await?;
    KNOWN
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), found.clone()));
    Ok(found)
}

fn to_summary(id: &ModId<'_>, repo: Repo) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        id: match id.pattern {
            Some(p) => id.with(p),
            None => id.repo(),
        },
        file: id.pattern.and_then(|p| pattern::label(p, &repo.name, true)),
        name: repo.name,
        url: Some(repo.html_url),
        author: Some(repo.owner.login),
        author_url: repo.owner.html_url,
        summary: repo.description,
        icon_url: repo.owner.avatar_url,
        downloads: None,
        rating: repo.stargazers_count,
        updated: repo.pushed_at,
        latest_version: None,
        categories: Vec::new(),
    }
}

/// One of the mods a game lists in `[sources.github] mods`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum Listed {
    Id(String),
    Named { id: String, name: Option<String> },
}

impl Listed {
    fn id(&self) -> &str {
        match self {
            Listed::Id(id) | Listed::Named { id, .. } => id,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Listed::Id(_) => None,
            Listed::Named { name, .. } => name.as_deref(),
        }
    }
}

/// The mods the game's `[sources.github]` lists.
fn listed(config: Option<&toml::Table>) -> Result<Vec<Listed>> {
    let Some(mods) = config.and_then(|c| c.get("mods")) else {
        return Ok(Vec::new());
    };
    mods.clone().try_into().map_err(|e| {
        error(format!(
            "the game's `[sources.github] mods` isn't a list of ids: {e}"
        ))
    })
}

/// Whether every word of `text` is in what `summary` shows, ignoring case.
fn matches_text(summary: &ModSummary, text: &str) -> bool {
    let shown = [
        Some(summary.name.as_str()),
        summary.file.as_deref(),
        summary.summary.as_deref(),
        summary.author.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    text.split_whitespace()
        .all(|word| shown.contains(&word.to_lowercase()))
}

fn no_repo(id: &ModId<'_>) -> String {
    format!("there's no GitHub repo {} (or it's private)", id.repo())
}

#[async_trait]
impl Source for GitHub {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "GitHub"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[TOKEN]
    }

    async fn check_settings(&self, settings: Option<&toml::Table>) -> Result<Option<String>> {
        if token(settings).is_none() {
            return Ok(None);
        }
        let user: User = get_json(settings, &format!("{API}/user"), || {
            "GitHub doesn't know this token".to_owned()
        })
        .await?;
        Ok(Some(format!("Signed in as {}", user.login)))
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let id = ModId::parse(id)?;
        let releases = releases(ctx, &id).await?;
        let release = release(&releases, &id, version)?;
        if let Some(pattern) = id.pattern {
            let asset = pick_asset(release, pattern).ok_or_else(|| {
                error(format!(
                    "{} of {} has no file matching `{pattern}`",
                    release.tag_name,
                    id.repo()
                ))
            })?;
            return Ok(to_resolved(&id, pattern, release, asset));
        }
        match release.assets.as_slice() {
            [] => Err(error(format!(
                "{} of {} has no files",
                release.tag_name,
                id.repo()
            ))),
            [only] => {
                let pattern = pattern::infer(&only.name, &release.tag_name);
                Ok(to_resolved(&id, &pattern, release, only))
            }
            _ => {
                let ids: Vec<String> = choices(&id, release)
                    .into_iter()
                    .map(|c| format!("{ID}:{}", c.id))
                    .collect();
                Err(error(format!(
                    "{}'s releases have more than one file, so say which: {}",
                    id.repo(),
                    ids.join(", ")
                )))
            }
        }
    }

    /// The newest release's files, if `id` has no pattern.
    async fn choices(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<FileChoice>> {
        let id = ModId::parse(id)?;
        if id.pattern.is_some() {
            return Ok(Vec::new());
        }
        let releases = releases(ctx, &id).await?;
        Ok(choices(&id, release(&releases, &id, None)?))
    }

    /// Release tags, which are ordered by the version in them (`v1.10` is
    /// after `v1.9`) as long as that's only numbers and dots.
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        if a == b {
            return Some(Ordering::Equal);
        }
        compare_dotted(pattern::tag_version(a)?, pattern::tag_version(b)?)
    }

    fn lists_versions(&self) -> bool {
        true
    }

    /// The tags of the releases with a file the pattern picks.
    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let id = ModId::parse(id)?;
        let releases = releases(ctx, &id).await?;
        Ok(releases
            .iter()
            .filter(|r| fits(r, id.pattern))
            .map(|r| r.tag_name.clone())
            .collect())
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let id = ModId::parse(id)?;
        let repo = repo(ctx, &id).await?;
        Ok(Some(to_summary(&id, repo)))
    }

    /// The notes of the releases with a file the pattern picks, from
    /// `version` back.
    async fn changelog(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let id = ModId::parse(id)?;
        let releases = releases(ctx, &id).await?;
        let fitting: Vec<&Release> = releases.iter().filter(|r| fits(r, id.pattern)).collect();
        let from = version
            .and_then(|v| fitting.iter().position(|r| r.tag_name == v))
            .unwrap_or(0);
        Ok(fitting
            .into_iter()
            .skip(from)
            .filter_map(|r| {
                let notes = r.body.as_deref()?.trim().to_owned();
                (!notes.is_empty()).then(|| ChangelogEntry {
                    version: Some(r.tag_name.clone()),
                    notes,
                })
            })
            .collect())
    }

    /// The repo's releases, or the release for `version`, with the pattern
    /// after `#file=` so the link adds the same file again; GitHub ignores
    /// it.
    fn mod_url(&self, _ctx: &SourceContext<'_>, id: &str, version: Option<&str>) -> Option<String> {
        let id = ModId::parse(id).ok()?;
        let mut url = format!("{SITE}/{}/releases", id.repo());
        if let Some(tag) = version {
            url.push_str(&format!("/tag/{tag}"));
        }
        if let Some(pattern) = id.pattern {
            url.push_str(&format!("#{FILE_FRAGMENT}{pattern}"));
        }
        Some(url)
    }

    /// Any link into a repo. A link to one of a release's files picks that
    /// file; any other leaves it to [`Source::choices`]. Links never pin a
    /// version, since the point of adding from GitHub is following its
    /// releases (and "latest release" links land on a tag's page).
    fn parse_url(&self, _ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        Ok(parse_link(url).map(|id| ModSpec {
            source: ID.to_owned(),
            id,
            version: None,
        }))
    }

    /// Only the mods the game lists can be searched, so only games that
    /// list some offer it.
    fn search_sorts(&self) -> &[SortBy] {
        &[SortBy::Relevance]
    }

    /// The mods the game lists, in its order, that have every word of the
    /// search text in their name, file, description or owner.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let sort = SortBy::Relevance;
        if query.page > 0 {
            return Ok(SearchPage::empty(sort));
        }
        let mut items = Vec::new();
        for listed in listed(ctx.config)? {
            let id = ModId::parse(listed.id())?;
            let mut summary = to_summary(&id, repo(ctx, &id).await?);
            if let Some(name) = listed.name() {
                summary.name = name.to_owned();
                summary.file = None;
            }
            if query.text().is_none_or(|text| matches_text(&summary, text)) {
                items.push(summary);
            }
        }
        Ok(SearchPage {
            total: Some(items.len() as u64),
            items,
            has_more: false,
            sort,
        })
    }
}

#[cfg(test)]
mod tests;
