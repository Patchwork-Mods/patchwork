//! Mods from mod.io, for games that load mod.io's files from a folder of
//! their own as well as through their in-game mod browser.
//!
//! Ids are mods' `name_id`s, the last part of their pages' URLs: `modio:cool-mod`
//! for mod.io/g/corekeeper/m/cool-mod. Numeric mod ids work anywhere an id
//! is asked for, and are turned into `name_id`s when a mod is added.
//! Versions are what authors call their files, which is free text and can
//! be blank; files without one go by their numeric file id.
//!
//! Every request needs a read-only API key from https://mod.io/me/access,
//! the `api_key` setting (or `MODIO_API_KEY`). Games choose whether apps
//! other than their own may use mod.io for them, and some only let signed-in
//! players download, which an API key can't do; both are reported as errors
//! naming the game.
//!
//! ```toml
//! [sources.modio]
//! game = "corekeeper"   # the game's name_id, from mod.io/g/<name_id>
//! id = 5289             # its numeric id; looked up from `game` if left out
//! platform = "windows"  # whose files to get, for games with a file per platform
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::changelog::ChangelogEntry;
use patchwork_core::compat::EnvSupport;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::settings::{SettingKind, SettingSpec};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, Download, ModSpec, ResolvedMod, Source,
    SourceContext, VersionReq, compare_dotted, is_archive_name, url_parts,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const ID: &str = "modio";
const SITE: &str = "https://mod.io";
/// For finding a game's id from its `name_id`; everything else goes to the
/// game's own host.
const GLOBAL_API: &str = "https://api.mod.io/v1";
const PAGE_SIZE: u32 = 20;
/// The most files mod.io lists at once.
const MAX_LIMIT: u32 = 100;

/// `api_access_options` bits.
const ACCESS_OPEN: u32 = 1;
const ACCESS_AUTHORISED_DOWNLOADS: u32 = 4;
const ACCESS_PAID_DOWNLOADS: u32 = 8;

const API_KEY: SettingSpec = SettingSpec::new("api_key", "API key", SettingKind::Secret)
    .help("mod.io answers nothing without one. Make a read-only key under API Access.")
    .link("https://mod.io/me/access")
    .env("MODIO_API_KEY");

pub struct ModIo;

// --- API Responses ---

#[derive(Debug, Clone, Deserialize)]
struct List<T> {
    data: Vec<T>,
    #[serde(default)]
    result_total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct Game {
    id: u64,
    name: String,
    name_id: String,
    #[serde(default)]
    api_access_options: u32,
    #[serde(default)]
    tag_options: Vec<TagOption>,
}

#[derive(Debug, Clone, Deserialize)]
struct TagOption {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Mod {
    id: u64,
    name: String,
    name_id: String,
    summary: Option<String>,
    profile_url: Option<String>,
    logo: Option<Logo>,
    submitted_by: Option<User>,
    date_updated: Option<i64>,
    modfile: Option<Modfile>,
    stats: Option<Stats>,
    #[serde(default)]
    tags: Vec<Tag>,
    /// Whether it has any dependencies.
    #[serde(default)]
    dependencies: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Logo {
    thumb_320x180: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct User {
    username: String,
    profile_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Stats {
    downloads_total: Option<u64>,
    ratings_positive: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct Tag {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Modfile {
    id: u64,
    #[serde(default)]
    date_added: i64,
    filename: String,
    version: Option<String>,
    changelog: Option<String>,
    download: Option<FileDownload>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileDownload {
    binary_url: String,
}

/// One of a mod's dependencies, which mod.io sends as much of a mod as this.
#[derive(Debug, Clone, Deserialize)]
struct DependencyMod {
    name_id: String,
}

// --- End API Responses ---

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    game: String,
    id: Option<u64>,
    #[serde(default = "default_platform")]
    platform: String,
}

fn default_platform() -> String {
    "windows".to_owned()
}

fn config(ctx: &SourceContext<'_>) -> Result<Config> {
    ctx.config
        .cloned()
        .ok_or_else(|| {
            error(format!(
                "{} isn't on mod.io (no [sources.modio] in its definition)",
                ctx.game.id
            ))
        })?
        .try_into()
        .map_err(|e| error(format!("bad [sources.modio] for {}: {e}", ctx.game.id)))
}

/// The user's API key, if they've set one.
fn api_key(settings: Option<&toml::Table>) -> Option<&str> {
    settings?
        .get(API_KEY.key)?
        .as_str()
        .filter(|k| !k.trim().is_empty())
}

fn need_api_key<'a>(ctx: &SourceContext<'a>) -> Result<&'a str> {
    api_key(ctx.settings).ok_or_else(|| {
        error(
            "mod.io needs an API key; add one in Settings, or with `patchwork config set modio.api_key <key>` (make a read-only one at https://mod.io/me/access)",
        )
    })
}

/// What a request needs: where the game's API is, and how to sign it.
struct Api<'a> {
    base: String,
    key: &'a str,
    platform: String,
    game: Arc<Game>,
}

impl<'a> Api<'a> {
    /// The game's API, once it's known that mod.io lets apps like this one
    /// use it.
    async fn new(ctx: &SourceContext<'a>) -> Result<Api<'a>> {
        let config = config(ctx)?;
        let key = need_api_key(ctx)?;
        let game = game(&config, key).await?;
        if game.api_access_options & ACCESS_OPEN == 0 {
            return Err(error(format!(
                "{} doesn't let apps other than its own get its mods from mod.io",
                game.name
            )));
        }
        Ok(Api {
            base: game_api(game.id),
            key,
            platform: config.platform,
            game,
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        get_json(
            &format!("{}{path}", self.base),
            self.key,
            &self.platform,
            params,
        )
        .await
    }

    /// The mod `id` names: a `name_id`, or a numeric id.
    async fn mod_(&self, id: &str) -> Result<Mod> {
        if let Ok(number) = id.parse::<u64>() {
            let path = format!("/games/{}/mods/{number}", self.game.id);
            return self.get(&path, &[]).await.map_err(|e| {
                not_found(e, || format!("there's no mod {id} for {}", self.game.name))
            });
        }
        let path = format!("/games/{}/mods", self.game.id);
        let found: List<Mod> = self.get(&path, &[("name_id", id.to_owned())]).await?;
        found
            .data
            .into_iter()
            .next()
            .ok_or_else(|| error(format!("there's no mod `{id}` for {}", self.game.name)))
    }

    /// Every one of the mod's files, newest first.
    async fn files(&self, mod_id: u64) -> Result<Vec<Modfile>> {
        let path = format!("/games/{}/mods/{mod_id}/files", self.game.id);
        let mut files = Vec::new();
        loop {
            let page: List<Modfile> = self
                .get(
                    &path,
                    &[
                        ("_limit", MAX_LIMIT.to_string()),
                        ("_offset", files.len().to_string()),
                    ],
                )
                .await?;
            let done = page.data.len() < MAX_LIMIT as usize;
            files.extend(page.data);
            if done {
                break;
            }
        }
        files.sort_by(|a, b| b.date_added.cmp(&a.date_added).then(b.id.cmp(&a.id)));
        Ok(files)
    }

    async fn file(&self, mod_id: u64, file: &str) -> Result<Modfile> {
        let path = format!("/games/{}/mods/{mod_id}/files/{file}", self.game.id);
        self.get(&path, &[])
            .await
            .map_err(|e| not_found(e, || format!("mod {mod_id} has no file {file}")))
    }

    async fn dependencies(&self, mod_id: u64) -> Result<Vec<DependencyMod>> {
        let path = format!("/games/{}/mods/{mod_id}/dependencies", self.game.id);
        let found: List<DependencyMod> = self.get(&path, &[]).await?;
        Ok(found.data)
    }

    fn summary(&self, m: Mod) -> ModSummary {
        mod_summary(&self.game.name_id, m)
    }
}

/// The API host for the game with id `id`.
fn game_api(id: u64) -> String {
    format!("https://g-{id}.modapi.io/v1")
}

/// The game `config` names, which is only asked about once.
async fn game(config: &Config, key: &str) -> Result<Arc<Game>> {
    static KNOWN: LazyLock<Mutex<HashMap<String, Arc<Game>>>> = LazyLock::new(Mutex::default);
    let cache_key = format!("{}#{:?}", config.game, config.id);
    if let Some(known) = KNOWN.lock().unwrap().get(&cache_key) {
        return Ok(known.clone());
    }
    let game: Game = match config.id {
        Some(id) => get_json(
            &format!("{}/games/{id}", game_api(id)),
            key,
            &config.platform,
            &[],
        )
        .await
        .map_err(|e| not_found(e, || format!("mod.io has no game {id}")))?,
        None => {
            let found: List<Game> = get_json(
                &format!("{GLOBAL_API}/games"),
                key,
                &config.platform,
                &[("name_id", config.game.clone())],
            )
            .await?;
            found
                .data
                .into_iter()
                .next()
                .ok_or_else(|| error(format!("mod.io has no game `{}`", config.game)))?
        }
    };
    let game = Arc::new(game);
    KNOWN.lock().unwrap().insert(cache_key, game.clone());
    Ok(game)
}

/// Turns a 404 into a clearer error, and leaves others as they are.
fn not_found(e: patchwork_core::Error, message: impl FnOnce() -> String) -> patchwork_core::Error {
    match &e {
        patchwork_core::Error::Reqwest { result, .. }
            if result.status() == Some(reqwest::StatusCode::NOT_FOUND) =>
        {
            error(message())
        }
        _ => e,
    }
}

/// What a file is called as a version: its version, or its id if it
/// hasn't one.
fn file_version(file: &Modfile) -> String {
    file.version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map_or_else(|| file.id.to_string(), str::to_owned)
}

/// The newest of `files` (newest first) whose version, or id, is `wanted`.
fn pick<'a>(files: &'a [Modfile], wanted: &str) -> Option<&'a Modfile> {
    files
        .iter()
        .find(|f| file_version(f) == wanted)
        .or_else(|| files.iter().find(|f| f.id.to_string() == wanted))
}

fn to_resolved(m: &Mod, file: &Modfile, dependencies: Vec<DependencyMod>) -> ResolvedMod {
    let format = match is_archive_name(&file.filename) {
        true => ArtifactFormat::Archive,
        false => ArtifactFormat::File,
    };
    ResolvedMod {
        id: m.name_id.clone(),
        name: m.name_id.clone(),
        version: Some(file_version(file)),
        // Download links can expire, so they're asked for when needed.
        artifact: ArtifactLocation::Source {
            file: file.id.to_string(),
            name: file.filename.clone(),
        },
        format,
        dependencies: dependencies
            .into_iter()
            .map(|d| Dependency {
                source: ID.to_owned(),
                id: d.name_id,
                // mod.io's dependencies are on mods, not versions of them
                req: VersionReq::Any,
            })
            .collect(),
        soft_dependencies: Vec::new(),
        game_versions: None,
        env: EnvSupport::default(),
        kind: None,
    }
}

fn mod_url(game: &str, name_id: &str) -> String {
    format!("{SITE}/g/{game}/m/{name_id}")
}

fn mod_summary(game: &str, m: Mod) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        url: Some(
            m.profile_url
                .clone()
                .unwrap_or_else(|| mod_url(game, &m.name_id)),
        ),
        author: m.submitted_by.as_ref().map(|u| u.username.clone()),
        author_url: m.submitted_by.and_then(|u| u.profile_url),
        name: m.name,
        summary: m.summary,
        icon_url: m.logo.and_then(|l| l.thumb_320x180),
        downloads: m.stats.as_ref().and_then(|s| s.downloads_total),
        rating: m.stats.as_ref().and_then(|s| s.ratings_positive),
        updated: m.date_updated.and_then(|t| Timestamp::from_second(t).ok()),
        latest_version: m.modfile.as_ref().map(file_version),
        file: None,
        categories: m
            .tags
            .into_iter()
            .map(|t| Category {
                id: t.name.clone(),
                name: t.name,
            })
            .collect(),
        id: m.name_id,
    }
}

/// mod.io's sort for `sort`, newest or biggest first.
fn sort_field(sort: SortBy) -> &'static str {
    match sort {
        // mod.io doesn't rank by relevance, so this is never asked for
        SortBy::Relevance | SortBy::Downloads => "-downloads_total",
        SortBy::Updated => "-date_updated",
        SortBy::Newest => "-date_live",
        SortBy::Rating => "-ratings_weighted_aggregate",
    }
}

fn search_params(query: &SearchQuery, sort: SortBy) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("_sort", sort_field(sort).to_owned()),
        ("_limit", PAGE_SIZE.to_string()),
        ("_offset", (query.page * PAGE_SIZE).to_string()),
    ];
    if let Some(text) = query.text() {
        params.push(("_q", text.to_owned()));
    }
    if !query.categories.is_empty() {
        params.push(("tags-in", query.categories.join(",")));
    }
    if !query.excluded_categories.is_empty() {
        params.push(("tags-not-in", query.excluded_categories.join(",")));
    }
    params
}

/// Recognizes mod pages, `mod.io/g/<game>/m/<mod>`, returning the game's
/// and the mod's `name_id`s.
fn parse_mod_url(url: &str) -> Option<(String, String)> {
    let (host, segments) = url_parts(url)?;
    if host != "mod.io" {
        return None;
    }
    match segments.as_slice() {
        ["g", game, "m", name_id, ..] => Some((game.to_string(), name_id.to_string())),
        _ => None,
    }
}

#[async_trait]
impl Source for ModIo {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "mod.io"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[API_KEY]
    }

    async fn check_settings(&self, settings: Option<&toml::Table>) -> Result<Option<String>> {
        let Some(key) = api_key(settings) else {
            return Ok(None);
        };
        let _: List<Game> = get_json(
            &format!("{GLOBAL_API}/games"),
            key,
            &default_platform(),
            &[("_limit", "1".to_owned())],
        )
        .await?;
        Ok(Some("mod.io accepts this API key".to_owned()))
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let api = Api::new(ctx).await?;
        if api.game.api_access_options & (ACCESS_AUTHORISED_DOWNLOADS | ACCESS_PAID_DOWNLOADS) != 0
        {
            return Err(error(format!(
                "{} only lets signed-in players download its mods, which patchwork can't do yet",
                api.game.name
            )));
        }
        let m = api.mod_(id).await?;
        let file = match version {
            // the file the author has made the current one
            None => m
                .modfile
                .clone()
                .ok_or_else(|| error(format!("{} has no file for {}", m.name_id, api.platform)))?,
            Some(wanted) => {
                let files = api.files(m.id).await?;
                pick(&files, wanted)
                    .cloned()
                    .ok_or_else(|| error(format!("{} has no version {wanted}", m.name_id)))?
            }
        };
        let dependencies = match m.dependencies {
            true => api.dependencies(m.id).await?,
            false => Vec::new(),
        };
        Ok(to_resolved(&m, &file, dependencies))
    }

    /// A fresh link each time, since games can make them expire.
    async fn download(&self, ctx: &SourceContext<'_>, id: &str, file: &str) -> Result<Download> {
        let api = Api::new(ctx).await?;
        let m = api.mod_(id).await?;
        let found = api.file(m.id, file).await?;
        let download = found
            .download
            .ok_or_else(|| error(format!("mod.io gave no download link for {id}")))?;
        Ok(Download::Url(download.binary_url))
    }

    /// Versions are free text, so only ones that are numbers can be put in
    /// order.
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        if a == b {
            return Some(Ordering::Equal);
        }
        let plain = |v: &str| {
            let v = v.trim();
            v.strip_prefix(['v', 'V']).unwrap_or(v).to_owned()
        };
        compare_dotted(&plain(a), &plain(b))
    }

    fn lists_versions(&self) -> bool {
        true
    }

    /// Every file's version, newest first.
    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let api = Api::new(ctx).await?;
        let m = api.mod_(id).await?;
        let mut versions: Vec<String> = Vec::new();
        for file in api.files(m.id).await? {
            let version = file_version(&file);
            if !versions.contains(&version) {
                versions.push(version);
            }
        }
        Ok(versions)
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let api = Api::new(ctx).await?;
        let m = api.mod_(id).await?;
        Ok(Some(api.summary(m)))
    }

    /// Each file's changelog, from `version` back.
    async fn changelog(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let api = Api::new(ctx).await?;
        let m = api.mod_(id).await?;
        let files = api.files(m.id).await?;
        let from = version
            .and_then(|w| files.iter().position(|f| file_version(f) == w))
            .unwrap_or(0);
        Ok(files
            .into_iter()
            .skip(from)
            .filter_map(|f| {
                let version = file_version(&f);
                let notes = f.changelog?.trim().to_owned();
                (!notes.is_empty()).then_some(ChangelogEntry {
                    version: Some(version),
                    notes,
                })
            })
            .collect())
    }

    /// Mod pages don't pin a version, so it's dropped.
    fn mod_url(&self, ctx: &SourceContext<'_>, id: &str, _version: Option<&str>) -> Option<String> {
        let game = config(ctx).ok()?.game;
        Some(mod_url(&game, id))
    }

    fn parse_url(&self, ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        let Some((game, name_id)) = parse_mod_url(url) else {
            return Ok(None);
        };
        let expected = config(ctx)?.game;
        if !game.eq_ignore_ascii_case(&expected) {
            return Err(error(format!("{url} is a mod for {game}, not {expected}")));
        }
        Ok(Some(ModSpec {
            source: ID.to_owned(),
            id: name_id,
            version: None,
        }))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &[
            SortBy::Downloads,
            SortBy::Updated,
            SortBy::Newest,
            SortBy::Rating,
        ]
    }

    /// Category ids are the game's tags, e.g. `Weapons`.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        let api = Api::new(ctx).await?;
        let mut found: Vec<Category> = Vec::new();
        for option in api.game.tag_options.iter().filter(|o| !o.hidden) {
            for tag in &option.tags {
                if !found.iter().any(|c| c.id == *tag) {
                    found.push(Category {
                        id: tag.clone(),
                        name: tag.clone(),
                    });
                }
            }
        }
        Ok(found)
    }

    /// Pages are 20 mods, most downloaded first unless asked otherwise.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let api = Api::new(ctx).await?;
        let sort = query.sort.unwrap_or(SortBy::Downloads);
        let path = format!("/games/{}/mods", api.game.id);
        let found: List<Mod> = api.get(&path, &search_params(query, sort)).await?;
        let shown = u64::from(query.page * PAGE_SIZE) + found.data.len() as u64;
        Ok(SearchPage {
            total: found.result_total,
            has_more: found
                .result_total
                .map_or(found.data.len() == PAGE_SIZE as usize, |total| {
                    shown < total
                }),
            items: found.data.into_iter().map(|m| api.summary(m)).collect(),
            sort,
        })
    }
}

/// GETs `url` and reads its JSON. The API key goes in the query, so it's
/// kept out of errors, which name the URL.
async fn get_json<T: DeserializeOwned>(
    url: &str,
    key: &str,
    platform: &str,
    params: &[(&str, String)],
) -> Result<T> {
    let shown =
        reqwest::Url::parse_with_params(url, params).map_err(|e| error(format!("bad url: {e}")))?;
    let mut signed = shown.clone();
    signed.query_pairs_mut().append_pair("api_key", key);
    let shown = String::from(shown);
    let reqwest_error = |result: reqwest::Error| patchwork_core::Error::Reqwest {
        url: shown.clone(),
        result: result.without_url(),
    };
    let response = patchwork_core::http()
        .get(signed)
        .header("X-Modio-Platform", platform)
        .send()
        .await
        .map_err(reqwest_error)?;
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED => {
            return Err(error("mod.io doesn't accept this API key"));
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            return Err(error(
                "mod.io says this API key has made too many requests; try again in a minute",
            ));
        }
        _ => {}
    }
    let bytes = response
        .error_for_status()
        .map_err(reqwest_error)?
        .bytes()
        .await
        .map_err(reqwest_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests;
