//! Beat Saber's custom maps from BeatSaver (<https://beatsaver.com>).
//!
//! The game says which mod every map needs to load, and which mods the
//! features maps' difficulties can use come from (`""` for one patchwork
//! can't get), which are suggested with the maps that use them:
//!
//! ```toml
//! [sources.beatsaver]
//! needs = "beatmods:SongCore"
//! uses = { ne = "beatmods:NoodleExtensions", chroma = "beatmods:Chroma", cinema = "" }
//! ```
//!
//! Ids are maps' keys, like `beatsaver:25f`, as in `!bsr 25f` and links to
//! them. A map is a zip of its song and difficulties, with no versions to
//! choose from: when its mapper publishes a new one it replaces the old,
//! and its link (named by the new one's hash) changes with it, so updating
//! picks it up. Maps are of kind `map`, for the game's deploy rules to give
//! each a folder of its own.
//!
//! The features a difficulty uses are suggestions, not dependencies:
//! BeatSaver doesn't say whether a map needs them or only looks better with
//! them, and they're often not out yet for the newest game version, which
//! would stop every deploy.

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ModSpec, Need, ResolvedMod, SoftDependency,
    Source, SourceContext, VersionReq, url_parts,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const ID: &str = "beatsaver";
const SITE: &str = "https://beatsaver.com";
const API: &str = "https://api.beatsaver.com";
const HOST: &str = "beatsaver.com";
/// The scheme of BeatSaver's one-click install links, which Mod Assistant
/// and BSManager handle: `beatsaver://25f`.
const ONE_CLICK: &str = "beatsaver://";
/// What the kind of a map is, for the game's deploy rules.
const KIND: &str = "map";
/// The only state of a map's version that's playable by everyone.
const PUBLISHED: &str = "Published";
/// How long API responses are reused for.
const FRESH_FOR: Duration = Duration::from_secs(10 * 60);
const PAGE_SIZE: u32 = 20;

pub struct BeatSaver;

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

// --- The API's JSON ---

/// `/search/text/{page}`.
#[derive(Debug, Clone, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    docs: Vec<Map>,
    #[serde(default)]
    info: Option<SearchInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchInfo {
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    pages: Option<u32>,
}

/// `/maps/id/{key}`, or one of a search's results.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Map {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    uploader: Option<User>,
    #[serde(default)]
    metadata: Option<Metadata>,
    #[serde(default)]
    stats: Option<Stats>,
    #[serde(default)]
    versions: Vec<MapVersion>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    last_published_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
struct User {
    id: u64,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    #[serde(default)]
    song_author_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Stats {
    #[serde(default)]
    upvotes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapVersion {
    #[serde(default)]
    state: String,
    #[serde(default)]
    created_at: Option<Timestamp>,
    #[serde(default)]
    diffs: Vec<Difficulty>,
    #[serde(rename = "downloadURL")]
    download_url: String,
    #[serde(default, rename = "coverURL")]
    cover_url: Option<String>,
}

/// One of a version's difficulties, and the features it uses.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Difficulty {
    chroma: bool,
    cinema: bool,
    me: bool,
    ne: bool,
    vivify: bool,
}

/// The features a difficulty can use, as `[sources.beatsaver] uses` names
/// them: what each is called and what it's for, and whether maps can do
/// without it (as with lights and videos) or might not be playable at all.
const FEATURES: &[(&str, &str, &str, Need)] = &[
    (
        "ne",
        "Noodle Extensions",
        "moves notes and walls",
        Need::Unknown,
    ),
    (
        "me",
        "Mapping Extensions",
        "places notes off the grid",
        Need::Unknown,
    ),
    (
        "vivify",
        "Vivify",
        "adds custom models and effects",
        Need::Unknown,
    ),
    (
        "chroma",
        "Chroma",
        "colors its lights, notes and walls",
        Need::Optional,
    ),
    (
        "cinema",
        "Cinema",
        "plays a video behind it",
        Need::Optional,
    ),
];

impl Difficulty {
    fn uses(&self, feature: &str) -> bool {
        match feature {
            "chroma" => self.chroma,
            "cinema" => self.cinema,
            "me" => self.me,
            "ne" => self.ne,
            "vivify" => self.vivify,
            _ => false,
        }
    }
}

impl Map {
    fn page(&self) -> String {
        format!("{SITE}/maps/{}", self.id)
    }

    /// The version everyone can play: the newest published one.
    fn published(&self) -> Option<&MapVersion> {
        self.versions
            .iter()
            .filter(|v| v.state == PUBLISHED)
            .max_by_key(|v| v.created_at)
    }

    /// The first line of its description, or failing that who the song's
    /// by, as a summary.
    fn summary(&self) -> Option<String> {
        let line = self
            .description
            .as_deref()
            .and_then(|d| d.lines().find(|l| !l.trim().is_empty()));
        match line {
            Some(line) => Some(line.trim().to_owned()),
            None => self
                .metadata
                .as_ref()?
                .song_author_name
                .clone()
                .filter(|a| !a.trim().is_empty()),
        }
    }
}

// --- The game's config ---

/// What a game's `[sources.beatsaver]` says.
#[derive(Debug)]
struct Config<'a> {
    /// The mod every map needs (`source:id`), if any.
    needs: Option<&'a str>,
    /// The mods features come from, by feature, or `None` for one
    /// patchwork can't get. Features not here aren't mentioned.
    uses: Vec<(&'a str, Option<&'a str>)>,
}

fn config<'a>(ctx: &SourceContext<'a>) -> Result<Config<'a>> {
    let table = ctx.config;
    let needs = match table.and_then(|t| t.get("needs")) {
        None => None,
        Some(needs) => Some(needs.as_str().ok_or_else(|| {
            error(format!(
                "the mod {}'s maps need (`[sources.beatsaver] needs`) should be a `source:id`",
                ctx.game.name
            ))
        })?),
    };
    let uses = match table.and_then(|t| t.get("uses")) {
        None => Vec::new(),
        Some(uses) => uses
            .as_table()
            .ok_or_else(|| {
                error(format!(
                    "{}'s `[sources.beatsaver] uses` should be a table of features",
                    ctx.game.name
                ))
            })?
            .iter()
            .map(|(feature, from)| {
                let from = from.as_str().ok_or_else(|| {
                    error(format!(
                        "the mod {} uses for {feature} should be a `source:id` or \"\"",
                        ctx.game.name
                    ))
                })?;
                Ok((feature.as_str(), Some(from).filter(|f| !f.is_empty())))
            })
            .collect::<Result<_>>()?,
    };
    Ok(Config {
        needs: needs.filter(|n| !n.is_empty()),
        uses,
    })
}

/// `source:id` as a dependency on any version of it.
fn dependency(key: &str) -> Result<Dependency> {
    let (source, id) = key
        .split_once(':')
        .ok_or_else(|| error(format!("`{key}`, a mod maps need, should be a `source:id`")))?;
    Ok(Dependency {
        source: source.to_owned(),
        id: id.to_owned(),
        req: VersionReq::Any,
    })
}

impl Config<'_> {
    /// What `version` suggests, for the features its difficulties use.
    fn suggests(&self, version: &MapVersion) -> Vec<SoftDependency> {
        let total = version.diffs.len();
        FEATURES
            .iter()
            .filter_map(|&(feature, name, does, need)| {
                let key = self.uses.iter().find(|(f, _)| *f == feature)?.1;
                let using = version.diffs.iter().filter(|d| d.uses(feature)).count();
                let which = match (using, total) {
                    (0, _) => return None,
                    (1, 1) => "Its difficulty uses".to_owned(),
                    (u, t) if u == t => "All its difficulties use".to_owned(),
                    (1, t) => format!("1 of its {t} difficulties uses"),
                    (u, t) => format!("{u} of its {t} difficulties use"),
                };
                Some(SoftDependency {
                    key: key.map(str::to_owned),
                    name: Some(name.to_owned()),
                    need,
                    notes: Some(format!("{which} {name}, which {does}.")),
                    url: None,
                })
            })
            .collect()
    }
}

// --- Fetching ---

/// The JSON at `url`, reused for [`FRESH_FOR`], or `None` if there's
/// nothing there.
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
    let found: Arc<T> = Arc::new(serde_json::from_str(&text).map_err(|e| {
        error(format!(
            "BeatSaver sent something unexpected for {url}: {e}"
        ))
    })?);
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

/// `id` as a map's key: hex digits, lowercased. `None` if it isn't one.
fn key(id: &str) -> Option<String> {
    let id = id.trim();
    (!id.is_empty() && id.len() <= 8 && id.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| id.to_ascii_lowercase())
}

fn not_a_key(id: &str) -> patchwork_core::Error {
    error(format!(
        "`{id}` isn't a BeatSaver map; they go by their key, like 25f in `!bsr 25f`"
    ))
}

async fn find(id: &str) -> Result<Arc<Map>> {
    let key = key(id).ok_or_else(|| not_a_key(id))?;
    find_json(&api_url(&format!("/maps/id/{key}"), &[])?)
        .await?
        .ok_or_else(|| error(format!("there's no map {key} on BeatSaver")))
}

fn to_resolved(config: &Config<'_>, map: &Map) -> Result<ResolvedMod> {
    let version = map.published().ok_or_else(|| {
        error(format!(
            "{} ({}) isn't published on BeatSaver, so only its mapper's testers can play it",
            map.name, map.id
        ))
    })?;
    Ok(ResolvedMod {
        id: map.id.clone(),
        name: map.name.clone(),
        version: None,
        artifact: ArtifactLocation::Url {
            url: version.download_url.clone(),
        },
        format: ArtifactFormat::Archive,
        dependencies: config
            .needs
            .map(dependency)
            .transpose()?
            .into_iter()
            .collect(),
        soft_dependencies: config.suggests(version),
        game_versions: None,
        env: Default::default(),
        kind: Some(KIND.to_owned()),
    })
}

// --- Searching ---

/// BeatSaver's tags, as search categories: its styles, then its genres.
const TAGS: &[(&str, &str)] = &[
    ("accuracy", "Accuracy"),
    ("balanced", "Balanced"),
    ("challenge", "Challenge"),
    ("dance-style", "Dance"),
    ("fitness", "Fitness"),
    ("speed", "Speed"),
    ("tech", "Tech"),
    ("poodle", "Poodle"),
    ("alternative", "Alternative"),
    ("ambient", "Ambient"),
    ("anime", "Anime"),
    ("classical-orchestral", "Classical & orchestral"),
    ("comedy-meme", "Comedy & meme"),
    ("dance", "Dance music"),
    ("drum-and-bass", "Drum and bass"),
    ("dubstep", "Dubstep"),
    ("electronic", "Electronic"),
    ("folk-acoustic", "Folk & acoustic"),
    ("funk-disco", "Funk & disco"),
    ("hardcore", "Hardcore"),
    ("hip-hop-rap", "Hip hop & rap"),
    ("holiday", "Holiday"),
    ("house", "House"),
    ("indie", "Indie"),
    ("instrumental", "Instrumental"),
    ("j-pop", "J-pop"),
    ("j-rock", "J-rock"),
    ("jazz", "Jazz"),
    ("k-pop", "K-pop"),
    ("kids-family", "Kids & family"),
    ("metal", "Metal"),
    ("nightcore", "Nightcore"),
    ("pop", "Pop"),
    ("punk", "Punk"),
    ("rb", "R&B"),
    ("rock", "Rock"),
    ("soul", "Soul"),
    ("speedcore", "Speedcore"),
    ("swing", "Swing"),
    ("techno", "Techno"),
    ("trance", "Trance"),
    ("tv-movie-soundtrack", "TV & film soundtracks"),
    ("video-game-soundtrack", "Video game soundtracks"),
    ("vocaloid", "Vocaloid"),
];

fn category(tag: &str) -> Category {
    Category {
        id: tag.to_owned(),
        name: TAGS
            .iter()
            .find(|(t, _)| *t == tag)
            .map_or(tag, |(_, name)| name)
            .to_owned(),
    }
}

fn to_summary(map: &Map) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        id: map.id.clone(),
        name: map.name.clone(),
        file: None,
        url: Some(map.page()),
        author: map.uploader.as_ref().map(|u| u.name.clone()),
        author_url: map
            .uploader
            .as_ref()
            .map(|u| format!("{SITE}/profile/{}", u.id)),
        summary: map.summary(),
        icon_url: map.published().and_then(|v| v.cover_url.clone()),
        downloads: None,
        rating: map.stats.as_ref().and_then(|s| s.upvotes),
        updated: map.last_published_at,
        latest_version: None,
        categories: map
            .tags
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| category(t))
            .collect(),
    }
}

/// BeatSaver's tag query for `query`'s categories: any of the ones it
/// asks for (`a|b`), and none it leaves out (`!c`), all together (`,`).
fn tag_query(query: &SearchQuery) -> Option<String> {
    let tag = |t: &String| t.to_ascii_lowercase();
    let wanted = query
        .categories
        .iter()
        .map(tag)
        .collect::<Vec<_>>()
        .join("|");
    let parts: Vec<String> = (!wanted.is_empty())
        .then_some(wanted)
        .into_iter()
        .chain(
            query
                .excluded_categories
                .iter()
                .map(|t| format!("!{}", tag(t))),
        )
        .collect();
    (!parts.is_empty()).then(|| parts.join(","))
}

/// The search's sort, and what BeatSaver calls it: the one asked for, or
/// by relevance when there's text, and newest first when there isn't, as
/// BeatSaver shows them.
fn order(query: &SearchQuery) -> (SortBy, &'static str) {
    match (query.sort, query.text()) {
        (Some(SortBy::Rating), _) => (SortBy::Rating, "Rating"),
        (Some(SortBy::Newest), _) | (None, None) => (SortBy::Newest, "Latest"),
        _ => (SortBy::Relevance, "Relevance"),
    }
}

/// The link to search for `query`, and the sort it uses.
fn search_url(query: &SearchQuery) -> Result<(String, SortBy)> {
    let (sort, order) = order(query);
    let page_size = PAGE_SIZE.to_string();
    let tags = tag_query(query);
    let mut params = vec![("order", order), ("pageSize", page_size.as_str())];
    if let Some(text) = query.text() {
        params.push(("q", text));
    }
    if let Some(tags) = &tags {
        params.push(("tags", tags));
    }
    let url = api_url(&format!("/search/text/{}", query.page), &params)?;
    Ok((url, sort))
}

fn to_page(response: &SearchResponse, query: &SearchQuery, sort: SortBy) -> SearchPage {
    let info = response.info.as_ref();
    SearchPage {
        total: info.and_then(|i| i.total),
        has_more: info
            .and_then(|i| i.pages)
            .is_some_and(|pages| query.page + 1 < pages),
        items: response.docs.iter().map(to_summary).collect(),
        sort,
    }
}

/// The key in a link to a map's page (`https://beatsaver.com/maps/25f`)
/// or a one-click install link (`beatsaver://25f`).
fn linked_key(url: &str) -> Option<String> {
    match url.strip_prefix(ONE_CLICK) {
        Some(rest) => key(rest.trim_end_matches('/')),
        None => match url_parts(url)? {
            (HOST, path) => match path[..] {
                ["maps", id] | ["beatmap", id] => key(id),
                _ => None,
            },
            _ => None,
        },
    }
}

#[async_trait]
impl Source for BeatSaver {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "BeatSaver"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        let map = find(id).await?;
        if let Some(version) = version {
            return Err(error(format!(
                "{} has no version {version}: BeatSaver's maps only have the one their mapper last published",
                map.name
            )));
        }
        to_resolved(&config, &map)
    }

    /// There's only ever the one version, so no two to order.
    fn compare_versions(&self, _a: &str, _b: &str) -> Option<Ordering> {
        None
    }

    async fn summary(&self, _ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        Ok(Some(to_summary(&*find(id).await?)))
    }

    fn mod_url(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        _version: Option<&str>,
    ) -> Option<String> {
        key(id).map(|key| format!("{SITE}/maps/{key}"))
    }

    fn parse_url(&self, _ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        Ok(linked_key(url).map(|key| ModSpec {
            source: ID.to_owned(),
            id: key,
            version: None,
        }))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &[SortBy::Relevance, SortBy::Newest, SortBy::Rating]
    }

    async fn search_categories(&self, _ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        Ok(TAGS.iter().map(|(tag, _)| category(tag)).collect())
    }

    async fn search(&self, _ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let (url, sort) = search_url(query)?;
        let response: Arc<SearchResponse> = find_json(&url)
            .await?
            .ok_or_else(|| error(format!("{url} wasn't found")))?;
        Ok(to_page(&response, query, sort))
    }
}

#[cfg(test)]
mod tests;
