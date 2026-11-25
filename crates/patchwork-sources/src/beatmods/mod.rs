//! Mods from BeatMods (<https://beatmods.com>), where Beat Saber's mods
//! are, through its `v3` API.
//!
//! The game says which of BeatMods' games it is:
//!
//! ```toml
//! [sources.beatmods]
//! game = "beatsaber"
//! platforms = ["universal", "steam"]   # the default
//! ```
//!
//! Ids are projects' `nameId`s, like `beatmods:BSIPA`; a project's number
//! (from a link to its page) works too, and resolves to its `nameId`.
//! Every version a project has had can be asked for. With none asked for,
//! it's the newest one made for the game version that's installed, since
//! Beat Saber's mods only work on the versions they're made for; with the
//! version unknown, the newest one there is. Only verified versions are
//! used, and only ones for the `platforms` the game lists.
//! When the installed game version has no mods at all (as after an
//! update, until they catch up), resolving is blocked with a blocker that
//! says so and names the version to switch to through Steam's Betas.
//!
//! Dependencies name projects by number, with npm-style ranges like
//! `^4.3.6`, which are taken as minimums: there's rarely more than one
//! version of a mod for a game version, so the upper bound seldom matters.

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::compat::GameVersions;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ModSpec, ResolvedMod, Source, SourceContext,
    VersionReq, compare_dotted, url_parts,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

mod assets;

pub use assets::BeatModsAssets;

const ID: &str = "beatmods";
const SITE: &str = "https://beatmods.com";
const API: &str = "https://beatmods.com/api/v3";
const HOST: &str = "beatmods.com";
/// The only status of version used.
const VERIFIED: &str = "verified";
/// Which platforms' versions are used unless the game says otherwise.
const PLATFORMS: [&str; 2] = ["universal", "steam"];
/// How long API responses are reused for. The full list of mods is most of
/// a megabyte and only changes when one is updated.
const FRESH_FOR: Duration = Duration::from_secs(10 * 60);
const PAGE_SIZE: usize = 50;

pub struct BeatMods;

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

// --- The API's JSON ---

/// One of `/mods`' results: a project and one of its versions.
#[derive(Debug, Clone, Deserialize)]
struct Entry {
    project: Project,
    version: Version,
}

/// `/mods/{id}`: a project and every version it has.
#[derive(Debug, Clone, Deserialize)]
struct ProjectPage {
    project: Project,
    #[serde(default)]
    versions: Vec<Version>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: u64,
    name: String,
    name_id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    icon_file_url: Option<String>,
    #[serde(default)]
    updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Author {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    semver: String,
    #[serde(default)]
    supported_game_versions: Vec<GameVersion>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    dependencies: Vec<Dep>,
    #[serde(default)]
    platform: String,
    zip_hash: String,
    #[serde(default)]
    download_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GameVersion {
    version: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Dep {
    #[serde(rename = "pId")]
    project: u64,
    #[serde(default)]
    sv: String,
}

/// `/games/{game}`.
#[derive(Debug, Clone, Deserialize)]
struct GamePage {
    game: GameInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct GameInfo {
    #[serde(default)]
    categories: Vec<String>,
}

impl Project {
    fn page(&self) -> String {
        format!("{SITE}/mods/{}", self.id)
    }

    fn authors(&self) -> Option<String> {
        let names: Vec<&str> = self
            .authors
            .iter()
            .filter_map(|a| a.display_name.as_deref().or(a.username.as_deref()))
            .collect();
        (!names.is_empty()).then(|| names.join(", "))
    }
}

impl Version {
    /// Where to download it: the file itself, or failing that the link by
    /// its hash, which redirects there.
    fn url(&self) -> String {
        match &self.download_url {
            Some(url) if !url.is_empty() => url.clone(),
            _ => format!("{SITE}/cdn/mod/{}.zip", self.zip_hash),
        }
    }

    fn supports(&self, game_version: &str) -> bool {
        self.supported_game_versions
            .iter()
            .any(|v| v.version == game_version)
    }

    /// The game versions it's for, oldest first (BeatMods lists them in
    /// no order).
    fn game_list(&self) -> Vec<&str> {
        let mut versions: Vec<&str> = self
            .supported_game_versions
            .iter()
            .map(|v| v.version.as_str())
            .collect();
        versions.sort_by(|a, b| compare_dotted(a, b).unwrap_or_else(|| a.cmp(b)));
        versions.dedup();
        versions
    }

    fn game_versions(&self) -> Option<GameVersions> {
        let versions = self.game_list();
        match versions.is_empty() {
            true => None,
            false => GameVersions::try_from(versions).ok(),
        }
    }
}

// --- Versions ---

/// Orders semantic versions like `4.3.6`, `1.0.0-beta.2` and
/// `4.3.6+1.29.1`: by their numbers, then a pre-release before the release
/// itself. Build metadata (after `+`) doesn't count, as semver says.
fn compare_semver(a: &str, b: &str) -> Option<Ordering> {
    let split = |v: &str| {
        let v = v.trim().trim_start_matches('v');
        let v = v.split_once('+').map_or(v, |(v, _)| v);
        match v.split_once('-') {
            Some((core, pre)) => (core.to_owned(), Some(pre.to_owned())),
            None => (v.to_owned(), None),
        }
    };
    let ((a_core, a_pre), (b_core, b_pre)) = (split(a), split(b));
    match compare_dotted(&a_core, &b_core)? {
        Ordering::Equal => {}
        ord => return Some(ord),
    }
    Some(match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_pre_release(&a, &b),
    })
}

/// Orders pre-release tags like `beta.2` and `beta.10` as semver does: a
/// part at a time, numbers by value and before words, and fewer parts
/// first when the rest are the same.
fn compare_pre_release(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        let ord = match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if ord.is_ne() {
            return ord;
        }
    }
}

/// What a dependency's npm-style range (`^4.3.6`, `~1.2.0`, `>=1.0.0`,
/// `1.2.3`, `*`) asks for. Carets and tildes are taken as minimums (see
/// the module's docs); anything that isn't a plain version is any.
fn version_req(range: &str) -> VersionReq {
    let range = range.trim();
    // Only plain versions, not `1.x` or `1.0.0 - 2.0.0`.
    let version = |v: &str| {
        let v = v.trim();
        compare_semver(v, v).is_some().then(|| v.to_owned())
    };
    if let Some(min) = ["^", "~", ">="]
        .iter()
        .find_map(|op| range.strip_prefix(op))
        .and_then(version)
    {
        return VersionReq::AtLeast(min);
    }
    let exact = range.strip_prefix('=').unwrap_or(range);
    version(exact).map_or(VersionReq::Any, VersionReq::Exact)
}

// --- Fetching ---

/// What a game's `[sources.beatmods]` says.
#[derive(Debug)]
struct Config<'a> {
    game: &'a str,
    platforms: Vec<&'a str>,
}

fn config<'a>(ctx: &SourceContext<'a>) -> Result<Config<'a>> {
    let table = ctx.config;
    let game = table.and_then(|t| t.get("game")?.as_str()).ok_or_else(|| {
        error(format!(
            "{} has no BeatMods game (`[sources.beatmods] game`)",
            ctx.game.name
        ))
    })?;
    let platforms = match table.and_then(|t| t.get("platforms")?.as_array()) {
        Some(list) => list.iter().filter_map(|p| p.as_str()).collect(),
        None => PLATFORMS.to_vec(),
    };
    Ok(Config { game, platforms })
}

impl Config<'_> {
    fn allows(&self, version: &Version) -> bool {
        version.status == VERIFIED
            && self
                .platforms
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&version.platform))
    }
}

/// The game version mods are being picked for, if it's known.
fn game_version<'a>(ctx: &SourceContext<'a>) -> Option<&'a str> {
    ctx.env.and_then(|env| env.version.as_deref())
}

/// The JSON at `url`, reused for [`FRESH_FOR`].
async fn get<T: DeserializeOwned + Send + Sync + 'static>(url: &str) -> Result<Arc<T>> {
    find_json(url)
        .await?
        .ok_or_else(|| error(format!("{url} wasn't found")))
}

/// [`get`], with `None` if there's nothing at `url`.
async fn find_json<T: DeserializeOwned + Send + Sync + 'static>(
    url: &str,
) -> Result<Option<Arc<T>>> {
    type Known = HashMap<String, (Instant, Arc<dyn Any + Send + Sync>)>;
    static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);
    if let Some((at, known)) = KNOWN.lock().unwrap().get(url)
        && at.elapsed() < FRESH_FOR
        && let Ok(known) = known.clone().downcast::<T>()
    {
        return Ok(Some(known));
    }
    let reqwest_error = |result| patchwork_core::Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let response = patchwork_core::http_get(url).await.map_err(reqwest_error)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let text = response
        .error_for_status()
        .map_err(reqwest_error)?
        .text()
        .await
        .map_err(reqwest_error)?;
    let found: Arc<T> = Arc::new(
        serde_json::from_str(&text)
            .map_err(|e| error(format!("BeatMods sent something unexpected for {url}: {e}")))?,
    );
    KNOWN
        .lock()
        .unwrap()
        .insert(url.to_owned(), (Instant::now(), found.clone()));
    Ok(Some(found))
}

fn api_url(path: &str, params: &[(&str, &str)]) -> Result<String> {
    let url = format!("{API}{path}");
    match params.is_empty() {
        true => reqwest::Url::parse(&url),
        false => reqwest::Url::parse_with_params(&url, params),
    }
    .map(String::from)
    .map_err(|e| error(format!("can't make a link to {path}: {e}")))
}

/// Projects' numbers by `nameId`, from every list fetched, so
/// [`Source::mod_url`] (which can't fetch anything) can link to them.
static NUMBERS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Mutex::default);

/// The verified mods for `game_version`, or, with none, every mod with its
/// newest verified version.
async fn list(config: &Config<'_>, game_version: Option<&str>) -> Result<Arc<Vec<Entry>>> {
    let mut params = vec![("gameName", config.game), ("status", VERIFIED)];
    if let Some(v) = game_version {
        params.push(("gameVersion", v));
    }
    let entries: Arc<Vec<Entry>> = get(&api_url("/mods", &params)?).await?;
    let mut numbers = NUMBERS.lock().unwrap();
    for entry in entries.iter() {
        numbers.insert(entry.project.name_id.clone(), entry.project.id);
    }
    Ok(entries)
}

/// The project numbered `number`, with its verified versions.
async fn project(number: u64) -> Result<Arc<ProjectPage>> {
    let url = api_url(&format!("/mods/{number}"), &[("status", VERIFIED)])?;
    let page: Arc<ProjectPage> = get(&url).await?;
    NUMBERS
        .lock()
        .unwrap()
        .insert(page.project.name_id.clone(), page.project.id);
    Ok(page)
}

/// The number of the project `id` names: by `nameId` (ignoring case if
/// nothing matches exactly), or a number as it is.
fn find_number(entries: &[Entry], id: &str) -> Option<u64> {
    entries
        .iter()
        .find(|e| e.project.name_id == id)
        .or_else(|| {
            entries
                .iter()
                .find(|e| e.project.name_id.eq_ignore_ascii_case(id))
        })
        .map(|e| e.project.id)
        .or_else(|| id.parse().ok())
}

async fn find(config: &Config<'_>, id: &str) -> Result<Arc<ProjectPage>> {
    let all = list(config, None).await?;
    let number =
        find_number(&all, id).ok_or_else(|| error(format!("there's no mod `{id}` on BeatMods")))?;
    project(number).await
}

/// The `nameId` of the project numbered `number`.
async fn name_id(config: &Config<'_>, number: u64) -> Result<String> {
    let all = list(config, None).await?;
    if let Some(entry) = all.iter().find(|e| e.project.id == number) {
        return Ok(entry.project.name_id.clone());
    }
    Ok(project(number).await?.project.name_id.clone())
}

// --- Picking ---

/// The newest of `versions`.
fn newest<'p>(versions: impl Iterator<Item = &'p Version>) -> Option<&'p Version> {
    versions.max_by(|a, b| compare_semver(&a.semver, &b.semver).unwrap_or(Ordering::Equal))
}

/// The version of `page` to use: the one numbered `wanted`, or else the
/// newest that's for `game_version` (or the newest of all, with it
/// unknown). Only versions `config` allows count. `game` is the game's
/// name, for errors.
fn pick<'p>(
    config: &Config<'_>,
    page: &'p ProjectPage,
    wanted: Option<&str>,
    game_version: Option<&str>,
    game: &str,
) -> Result<&'p Version> {
    let allowed = || page.versions.iter().filter(|v| config.allows(v));
    let name = &page.project.name_id;
    if let Some(wanted) = wanted {
        // A pinned version that's for another game version is still what
        // was asked for; that shows up as an issue.
        return allowed()
            .find(|v| v.semver == wanted)
            .ok_or_else(|| error(format!("{name} has no version {wanted} on BeatMods")));
    }
    let Some(gv) = game_version else {
        return newest(allowed()).ok_or_else(|| error(format!("{name} has no versions")));
    };
    newest(allowed().filter(|v| v.supports(gv))).ok_or_else(|| {
        let latest = newest(allowed()).map(|v| {
            let versions = v.game_list();
            match versions.is_empty() {
                true => format!(" (its newest is {})", v.semver),
                false => format!(
                    " (its newest, {}, is for {})",
                    v.semver,
                    versions.join(", ")
                ),
            }
        });
        error(format!(
            "{name} has no version for {game} {gv} yet{}",
            latest.unwrap_or_default()
        ))
    })
}

/// The newest game version most mods are made for, with how many are, from
/// `entries` (every mod with its newest version): the newest with at least
/// half as many as the version with the most, so a few mods out early for
/// a new version don't count as it having mods.
fn moddable(config: &Config<'_>, entries: &[Entry]) -> Option<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for entry in usable(config, entries) {
        for version in entry.version.game_list() {
            *counts.entry(version).or_default() += 1;
        }
    }
    let most = counts.values().copied().max()?;
    counts
        .into_iter()
        .filter(|&(_, n)| n * 2 >= most)
        .max_by(|(a, _), (b, _)| compare_dotted(a, b).unwrap_or_else(|| a.cmp(b)))
        .map(|(version, n)| (version.to_owned(), n))
}

/// What's in the way when `game` version `gv` has no mods at all, `best`
/// being the version to switch to and how many mods it has (see
/// [`moddable`]). Only the user can change the game's version, through
/// Steam, so it's cleared by trying again.
fn no_mods_for(game: &str, gv: &str, best: Option<(&str, usize)>) -> patchwork_core::Error {
    let message = format!("{game} {gv} has no mods on BeatMods yet");
    let title = format!("No mods for {game} {gv} yet");
    let wait = "mods only work on the game version they're made for, so after an update it \
                takes a while for them to catch up";
    let (message, detail) = match best {
        Some((best, count)) => (
            format!(
                "{message}; switch to {best}, the newest version most mods are for, under \
                 Betas in the game's properties in Steam"
            ),
            format!(
                "{game}'s {wait}, and BeatMods doesn't have any for {gv} yet. To use mods \
                 now, switch to {best}, the newest version most of them are for ({count} \
                 mods): in Steam, open {game}'s Properties, pick {best} under Betas, and \
                 try again once it's downloaded. Steam won't update the game while it's on \
                 that branch. Otherwise, wait for mods for {gv} to come out and try again \
                 then."
            ),
        ),
        None => (
            message,
            format!(
                "{game}'s {wait}, and BeatMods doesn't have any for {gv} yet. Wait for \
                 them to come out, or pick an older version under Betas in {game}'s \
                 Properties in Steam, and try again."
            ),
        ),
    };
    patchwork_core::Error::blocked_by(
        error(message).to_string(),
        patchwork_core::fix::Blocker::try_again(title, detail),
    )
}

/// The error for no version of a mod being for `gv`, when it's because the
/// game version has no mods at all.
async fn no_mods_yet(
    config: &Config<'_>,
    gv: &str,
    game: &str,
) -> Result<Option<patchwork_core::Error>> {
    if !usable(config, &list(config, Some(gv)).await?).is_empty() {
        return Ok(None);
    }
    let all = list(config, None).await?;
    let best = moddable(config, &all);
    let best = best.as_ref().map(|(v, n)| (v.as_str(), *n));
    Ok(Some(no_mods_for(game, gv, best)))
}

/// `project` at `version`, with its dependencies named by `names`
/// (project numbers to `nameId`s).
fn to_resolved(
    project: &Project,
    version: &Version,
    names: &HashMap<u64, String>,
) -> Result<ResolvedMod> {
    let dependencies = version
        .dependencies
        .iter()
        .map(|dep| {
            let id = names.get(&dep.project).cloned().ok_or_else(|| {
                error(format!(
                    "{} needs BeatMods' mod number {}, which couldn't be found",
                    project.name_id, dep.project
                ))
            })?;
            Ok(Dependency {
                source: ID.to_owned(),
                id,
                req: version_req(&dep.sv),
            })
        })
        .collect::<Result<_>>()?;
    Ok(ResolvedMod {
        id: project.name_id.clone(),
        name: project.name.clone(),
        version: Some(version.semver.clone()),
        artifact: ArtifactLocation::Url { url: version.url() },
        format: ArtifactFormat::Archive,
        dependencies,
        soft_dependencies: Vec::new(),
        game_versions: version.game_versions(),
        env: Default::default(),
        kind: None,
    })
}

// --- Searching ---

fn to_summary(project: &Project, version: Option<&Version>) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        id: project.name_id.clone(),
        name: project.name.clone(),
        file: None,
        url: Some(project.page()),
        author: project.authors(),
        author_url: None,
        summary: project.summary.clone().filter(|s| !s.trim().is_empty()),
        icon_url: project.icon_file_url.clone(),
        downloads: None,
        rating: None,
        updated: project.updated_at,
        latest_version: version.map(|v| v.semver.clone()),
        categories: project.category.iter().map(|c| category(c)).collect(),
    }
}

fn category(name: &str) -> Category {
    Category {
        id: name.to_owned(),
        name: name.to_owned(),
    }
}

/// `entries` with one for each project, the ones `config` allows.
fn usable<'e>(config: &Config<'_>, entries: &'e [Entry]) -> Vec<&'e Entry> {
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .filter(|e| config.allows(&e.version))
        .filter(|e| seen.insert(e.project.id))
        .collect()
}

/// Whether every word of `text` is in `project`'s names, summary or
/// authors, ignoring case.
fn matches_text(project: &Project, text: &str) -> bool {
    let shown = [
        Some(project.name.as_str()),
        Some(project.name_id.as_str()),
        project.summary.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::to_owned)
    .chain(project.authors())
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    text.split_whitespace()
        .all(|word| shown.contains(&word.to_lowercase()))
}

fn in_category(project: &Project, categories: &[String]) -> bool {
    project
        .category
        .as_ref()
        .is_some_and(|c| categories.iter().any(|w| w.eq_ignore_ascii_case(c)))
}

/// How well `project`'s name matches search `text`: 0 if a name starts
/// with it, 1 if one has every word, 2 if only the rest does.
fn rank(project: &Project, text: &str) -> u8 {
    let text = text.to_lowercase();
    [&project.name, &project.name_id]
        .into_iter()
        .map(|name| name.to_lowercase())
        .map(|name| match name.starts_with(&text) {
            true => 0,
            false if text.split_whitespace().all(|w| name.contains(w)) => 1,
            false => 2,
        })
        .min()
        .unwrap_or(2)
}

/// One page of `entries` that `query` asks for.
fn search_page(config: &Config<'_>, entries: &[Entry], query: &SearchQuery) -> SearchPage {
    let mut found: Vec<&Entry> = usable(config, entries)
        .into_iter()
        .filter(|e| {
            query
                .text()
                .is_none_or(|text| matches_text(&e.project, text))
        })
        .filter(|e| query.categories.is_empty() || in_category(&e.project, &query.categories))
        .filter(|e| !in_category(&e.project, &query.excluded_categories))
        .collect();
    let sort = match (query.sort, query.text()) {
        (Some(SortBy::Updated), _) => {
            found.sort_by_key(|e| std::cmp::Reverse(e.project.updated_at));
            SortBy::Updated
        }
        (_, Some(text)) => {
            found.sort_by_cached_key(|e| (rank(&e.project, text), e.project.name.to_lowercase()));
            SortBy::Relevance
        }
        (_, None) => {
            found.sort_by_cached_key(|e| e.project.name.to_lowercase());
            SortBy::Relevance
        }
    };
    let start = query.page as usize * PAGE_SIZE;
    SearchPage {
        total: Some(found.len() as u64),
        has_more: found.len() > start + PAGE_SIZE,
        items: found
            .into_iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(|e| to_summary(&e.project, Some(&e.version)))
            .collect(),
        sort,
    }
}

#[async_trait]
impl Source for BeatMods {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "BeatMods"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        let page = find(&config, id).await?;
        let gv = game_version(ctx);
        let chosen = match pick(&config, &page, version, gv, &ctx.game.name) {
            Ok(chosen) => chosen,
            // Explain it as the game version's, if no mod has a version
            // for it, rather than this mod's.
            Err(e) => {
                return Err(match (version, gv) {
                    (None, Some(gv)) => {
                        no_mods_yet(&config, gv, &ctx.game.name).await?.unwrap_or(e)
                    }
                    _ => e,
                });
            }
        };
        let mut names = HashMap::new();
        for dep in &chosen.dependencies {
            names.insert(dep.project, name_id(&config, dep.project).await?);
        }
        to_resolved(&page.project, chosen, &names)
    }

    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        compare_semver(a, b)
    }

    fn lists_versions(&self) -> bool {
        true
    }

    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let config = config(ctx)?;
        let page = find(&config, id).await?;
        let mut versions: Vec<&Version> =
            page.versions.iter().filter(|v| config.allows(v)).collect();
        versions.sort_by(|a, b| compare_semver(&b.semver, &a.semver).unwrap_or(Ordering::Equal));
        let mut numbers: Vec<String> = Vec::new();
        for v in versions {
            if !numbers.contains(&v.semver) {
                numbers.push(v.semver.clone());
            }
        }
        Ok(numbers)
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let config = config(ctx)?;
        let page = find(&config, id).await?;
        let newest = pick(&config, &page, None, game_version(ctx), &ctx.game.name)
            .or_else(|_| pick(&config, &page, None, None, &ctx.game.name))
            .ok();
        Ok(Some(to_summary(&page.project, newest)))
    }

    /// Its page, by number, once a list it's in has been fetched.
    fn mod_url(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        _version: Option<&str>,
    ) -> Option<String> {
        let number = match id.parse::<u64>() {
            Ok(n) => n,
            Err(_) => *NUMBERS.lock().unwrap().get(id)?,
        };
        Some(format!("{SITE}/mods/{number}"))
    }

    /// `https://beatmods.com/mods/{number}`.
    fn parse_url(&self, _ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        let Some((HOST, path)) = url_parts(url) else {
            return Ok(None);
        };
        Ok(match path.as_slice() {
            ["mods", number] if number.parse::<u64>().is_ok() => Some(ModSpec {
                source: ID.to_owned(),
                id: (*number).to_owned(),
                version: None,
            }),
            _ => None,
        })
    }

    fn search_sorts(&self) -> &[SortBy] {
        &[SortBy::Relevance, SortBy::Updated]
    }

    /// The game's categories on BeatMods.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        let config = config(ctx)?;
        let page: Arc<GamePage> = get(&api_url(&format!("/games/{}", config.game), &[])?).await?;
        Ok(page.game.categories.iter().map(|c| category(c)).collect())
    }

    /// The mods for the installed game version (or every mod, with it
    /// unknown) that have every word of the search text in their names,
    /// summary or authors.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let config = config(ctx)?;
        let entries = list(&config, game_version(ctx)).await?;
        Ok(search_page(&config, &entries, query))
    }
}

#[cfg(test)]
mod tests;
