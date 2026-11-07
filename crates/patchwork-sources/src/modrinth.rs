//! Mods, resource packs and shaders from Modrinth, which is for Minecraft.
//!
//! Ids are Modrinth's project ids, e.g. `modrinth:AANobbMI`. Slugs
//! (`modrinth:sodium`) work anywhere an id is asked for, and are turned into
//! ids when a mod is added. Versions are version numbers, like
//! `mc1.21.1-0.8.13-fabric`.
//!
//! Projects have a file for each Minecraft version and mod loader, so which
//! one suits a profile comes from what it picks for the game's `[env]`: the
//! `version` axis is the Minecraft version, and the `loader` axis the mod
//! loader. Only loaders among the `loader` axis's choices count as mod
//! loaders; a project's others (`datapack`, `iris`, `minecraft`, ...) are
//! what it's for rather than what it runs on.
//!
//! Each mod's kind is its project type (`mod`, `resourcepack`, `shader`,
//! `datapack`, ...), which deployer rules can match with `kinds`, since a
//! resource pack and a shader pack are both just a zip.
//!
//! ```toml
//! [sources.modrinth]
//! # What searches find: any of `mod`, `resourcepack`, `shader`, `datapack`
//! # and `modpack`. Just `mod` if left out.
//! project_types = ["mod", "resourcepack", "shader"]
//!
//! # Modrinth's loaders whose files a pick of the `loader` axis runs, when
//! # it runs more than its own: Quilt runs Fabric mods, say.
//! [sources.modrinth.loaders]
//! quilt = ["quilt", "fabric"]
//! ```

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::changelog::ChangelogEntry;
use patchwork_core::compat::{EnvSupport, GameVersions};
use patchwork_core::game::{EnvDefs, GameDef};
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ModSpec, Need, ResolvedMod, SoftDependency,
    Source, SourceContext, VersionReq, compare_dotted, url_parts,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const ID: &str = "modrinth";
const API: &str = "https://api.modrinth.com/v2";
const SITE: &str = "https://modrinth.com";
/// The `[env]` axis with the mod loader in it.
const LOADER: &str = "loader";
const PAGE_SIZE: u32 = 20;

/// Every kind of project Modrinth has pages for, as they appear in its URLs.
const PROJECT_TYPES: &[&str] = &[
    "mod",
    "resourcepack",
    "shader",
    "datapack",
    "modpack",
    "plugin",
    "project",
];

pub struct Modrinth;

// --- API Responses ---

#[derive(Debug, Clone, Deserialize)]
struct Project {
    id: String,
    slug: String,
    title: String,
    description: Option<String>,
    project_type: String,
    icon_url: Option<String>,
    downloads: Option<u64>,
    followers: Option<u64>,
    updated: Option<Timestamp>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Version {
    id: String,
    project_id: String,
    version_number: String,
    changelog: Option<String>,
    /// `release`, `beta` or `alpha`.
    version_type: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    files: Vec<VersionFile>,
    #[serde(default)]
    dependencies: Vec<VersionDependency>,
}

#[derive(Debug, Clone, Deserialize)]
struct VersionFile {
    url: String,
    primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct VersionDependency {
    version_id: Option<String>,
    project_id: Option<String>,
    /// `required`, `optional`, `incompatible` or `embedded`.
    dependency_type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchResult {
    hits: Vec<Hit>,
    total_hits: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Hit {
    project_id: String,
    slug: String,
    title: String,
    description: Option<String>,
    project_type: String,
    author: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    downloads: Option<u64>,
    follows: Option<u64>,
    icon_url: Option<String>,
    date_modified: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
struct CategoryTag {
    name: String,
    project_type: String,
    /// `categories`, or what else it's about, like `resolutions`.
    header: String,
}

// --- End API Responses ---

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default = "default_project_types")]
    project_types: Vec<String>,
    #[serde(default)]
    loaders: BTreeMap<String, Vec<String>>,
}

impl Config {
    /// Modrinth's loaders whose files run on `loader`.
    fn accepts(&self, loader: &str) -> Vec<String> {
        self.loaders
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(loader))
            .map_or_else(|| vec![loader.to_owned()], |(_, accepts)| accepts.clone())
    }
}

fn default_project_types() -> Vec<String> {
    vec!["mod".to_owned()]
}

fn config(ctx: &SourceContext<'_>) -> Result<Config> {
    ctx.config
        .cloned()
        .ok_or_else(|| {
            error(format!(
                "{} isn't on Modrinth (no [sources.modrinth] in its definition)",
                ctx.game.id
            ))
        })?
        .try_into()
        .map_err(|e| error(format!("bad [sources.modrinth] for {}: {e}", ctx.game.id)))
}

/// Which of `loaders` are the game's mod loaders: choices of its `loader`
/// axis, or all of them if it has no choices. None without a `loader` axis.
fn mod_loaders(game: &GameDef, loaders: &[String]) -> Vec<String> {
    let Some(axis) = game.env.get(LOADER) else {
        return Vec::new();
    };
    loaders
        .iter()
        .filter(|l| {
            axis.choices.is_empty() || axis.choices.iter().any(|c| c.eq_ignore_ascii_case(l))
        })
        .cloned()
        .collect()
}

/// The picks of the game's `loader` axis that `version_loaders` (a
/// version's loaders on Modrinth) run on, going by what each accepts.
fn supported_picks(game: &GameDef, config: &Config, version_loaders: &[String]) -> Vec<String> {
    let found = mod_loaders(game, version_loaders);
    let Some(axis) = game.env.get(LOADER) else {
        return found;
    };
    if found.is_empty() || axis.choices.is_empty() {
        return found;
    }
    axis.choices
        .iter()
        .filter(|choice| {
            config
                .accepts(choice)
                .iter()
                .any(|a| found.iter().any(|f| f.eq_ignore_ascii_case(a)))
        })
        .cloned()
        .collect()
}

/// What a project's versions have to be for to suit the profile.
#[derive(Debug, Default, PartialEq, Eq)]
struct Filter {
    game_version: Option<String>,
    /// The profile's mod loader, for projects that run on one.
    loader: Option<String>,
    /// Modrinth's loaders whose files run on `loader`.
    accepts: Vec<String>,
}

impl Filter {
    /// For a project whose loaders on Modrinth are `loaders`; `None` for
    /// any project.
    fn new(ctx: &SourceContext<'_>, config: &Config, loaders: Option<&[String]>) -> Self {
        let env = ctx.env;
        let runs_on_loader = loaders.is_none_or(|l| !mod_loaders(ctx.game, l).is_empty());
        let loader = env
            .and_then(|e| e.get(LOADER))
            .filter(|_| runs_on_loader)
            .map(str::to_owned);
        Filter {
            game_version: env.and_then(|e| e.version.clone()),
            accepts: loader
                .as_deref()
                .map(|l| config.accepts(l))
                .unwrap_or_default(),
            loader,
        }
    }

    fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if let Some(v) = &self.game_version {
            params.push(("game_versions", json_list([v])));
        }
        if !self.accepts.is_empty() {
            params.push(("loaders", json_list(&self.accepts)));
        }
        params
    }

    /// Says what it is, like `Minecraft 1.21.1 with fabric`.
    fn describe(&self, game: &GameDef) -> String {
        let mut text = match &self.game_version {
            Some(v) => format!("{} {v}", game.name),
            None => game.name.clone(),
        };
        if let Some(l) = &self.loader {
            text.push_str(&format!(" with {l}"));
        }
        text
    }
}

fn json_list<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    serde_json::to_string(&items.into_iter().collect::<Vec<_>>()).expect("strings serialize")
}

/// The version to use out of `versions` (newest first): the one numbered
/// (or with the id) `wanted`, or the newest release, or failing that the
/// newest beta or alpha.
fn pick(versions: Vec<Version>, wanted: Option<&str>) -> Option<Version> {
    match wanted {
        Some(w) => versions
            .into_iter()
            .find(|v| v.version_number == w || v.id == w),
        None => {
            let release = versions.iter().position(|v| v.version_type == "release");
            versions.into_iter().nth(release.unwrap_or(0))
        }
    }
}

/// `project` at `version`, given the version numbers of the dependencies
/// that ask for one in particular, by version id.
fn to_resolved(
    game: &GameDef,
    config: &Config,
    project: &Project,
    version: Version,
    numbers: &HashMap<String, (String, String)>,
) -> Result<ResolvedMod> {
    let file = version
        .files
        .iter()
        .find(|f| f.primary)
        .or(version.files.first())
        .ok_or_else(|| {
            error(format!(
                "version {} of {} has no files",
                version.version_number, project.slug
            ))
        })?;
    let mut dependencies = Vec::new();
    for dep in version
        .dependencies
        .iter()
        .filter(|d| d.dependency_type == "required")
    {
        let pinned = dep.version_id.as_ref().and_then(|v| numbers.get(v));
        let Some(project_id) = dep
            .project_id
            .clone()
            .or_else(|| pinned.map(|(project, _)| project.clone()))
        else {
            // a file that isn't on Modrinth, which nothing can be done about
            continue;
        };
        dependencies.push(Dependency {
            source: ID.to_owned(),
            id: project_id,
            req: match pinned {
                Some((_, number)) => VersionReq::Exact(number.clone()),
                None => VersionReq::Any,
            },
        });
    }
    // Optional ones are only shown; Modrinth names them by project, which
    // is all a soft dependency needs.
    let soft_dependencies = version
        .dependencies
        .iter()
        .filter(|d| d.dependency_type == "optional")
        .filter_map(|dep| {
            let pinned = dep.version_id.as_ref().and_then(|v| numbers.get(v));
            let project_id = dep
                .project_id
                .clone()
                .or_else(|| pinned.map(|(project, _)| project.clone()))?;
            Some(SoftDependency {
                key: Some(format!("{ID}:{project_id}")),
                name: None,
                need: Need::Optional,
                notes: None,
                url: None,
            })
        })
        .collect();
    let loaders = supported_picks(game, config, &version.loaders);
    let env = match loaders.is_empty() {
        true => EnvSupport::default(),
        false => EnvSupport::default().with(LOADER, loaders),
    };
    let kind = kind(project, &version);
    let game_versions = match version.game_versions.is_empty() {
        true => None,
        false => GameVersions::try_from(version.game_versions.clone()).ok(),
    };
    Ok(ResolvedMod {
        id: project.id.clone(),
        name: project.slug.clone(),
        version: Some(version.version_number),
        artifact: ArtifactLocation::Url {
            url: file.url.clone(),
        },
        format: ArtifactFormat::File,
        dependencies,
        soft_dependencies,
        game_versions,
        env,
        kind: Some(kind),
    })
}

/// Gives `resolved`'s soft dependencies, which Modrinth only names by
/// project id, their projects' titles and pages. They're only shown, so
/// they go without if Modrinth can't be asked.
async fn name_soft_dependencies(resolved: &mut ResolvedMod) {
    let prefix = format!("{ID}:");
    let ids: Vec<String> = resolved
        .soft_dependencies
        .iter()
        .filter_map(|d| d.key.as_deref()?.strip_prefix(&prefix).map(str::to_owned))
        .collect();
    if ids.is_empty() {
        return;
    }
    let Ok(url) = with_params(&format!("{API}/projects"), &[("ids", json_list(&ids))]) else {
        return;
    };
    let Ok(projects) = get_json::<Vec<Project>>(&url).await else {
        return;
    };
    for dep in &mut resolved.soft_dependencies {
        let id = dep.key.as_deref().and_then(|k| k.strip_prefix(&prefix));
        if let Some(project) = projects.iter().find(|p| Some(p.id.as_str()) == id) {
            dep.name = Some(project.title.clone());
            dep.url = Some(format!("https://modrinth.com/project/{}", project.slug));
        }
    }
}

/// What `version` of `project` is: its project type, except that
/// datapacks are `mod` projects whose only loader is `datapack`.
fn kind(project: &Project, version: &Version) -> String {
    let datapack = !version.loaders.is_empty() && version.loaders.iter().all(|l| l == "datapack");
    match datapack {
        true => "datapack".to_owned(),
        false => project.project_type.clone(),
    }
}

/// Fails if the game picks its version but the profile hasn't, since
/// there's no telling which of a project's files would do.
fn check_version_picked(ctx: &SourceContext<'_>) -> Result<()> {
    let picked = ctx.env.and_then(|e| e.version.as_deref()).is_some();
    match ctx.game.env.get(EnvDefs::VERSION) {
        Some(axis) if !picked => Err(error(format!(
            "pick the profile's {} first, so the right files can be found",
            axis.name
        ))),
        _ => Ok(()),
    }
}

fn hit_to_summary(game: &GameDef, hit: Hit) -> ModSummary {
    let loaders = mod_loaders(game, &hit.categories);
    ModSummary {
        source: ID.to_owned(),
        id: hit.project_id,
        url: Some(format!("{SITE}/{}/{}", hit.project_type, hit.slug)),
        author_url: hit.author.as_ref().map(|a| format!("{SITE}/user/{a}")),
        author: hit.author,
        name: hit.title,
        summary: hit.description,
        icon_url: hit.icon_url,
        downloads: hit.downloads,
        rating: hit.follows,
        updated: hit.date_modified,
        latest_version: None,
        // loaders are in there too, but they aren't what it's about
        categories: hit
            .categories
            .into_iter()
            .filter(|c| !loaders.contains(c))
            .map(category)
            .collect(),
    }
}

fn project_to_summary(project: Project) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        url: Some(format!("{SITE}/{}/{}", project.project_type, project.slug)),
        id: project.id,
        name: project.title,
        author: None,
        author_url: None,
        summary: project.description,
        icon_url: project.icon_url,
        downloads: project.downloads,
        rating: project.followers,
        updated: project.updated,
        latest_version: None,
        categories: project.categories.into_iter().map(category).collect(),
    }
}

/// `game-mechanics` as `Game mechanics`.
fn category(id: String) -> Category {
    let words = id.replace('-', " ");
    let mut chars = words.chars();
    let name = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
    };
    Category { id, name }
}

fn index(sort: SortBy) -> &'static str {
    match sort {
        SortBy::Relevance => "relevance",
        SortBy::Downloads => "downloads",
        SortBy::Updated => "updated",
        SortBy::Newest => "newest",
        SortBy::Rating => "follows",
    }
}

/// The search's facets: lists that are all required, of which any one
/// item will do.
fn facets(types: &[String], filter: &Filter, query: &SearchQuery) -> Vec<Vec<String>> {
    let mut facets = vec![
        types
            .iter()
            .map(|t| format!("project_type:{t}"))
            .collect::<Vec<_>>(),
    ];
    if let Some(v) = &filter.game_version {
        facets.push(vec![format!("versions:{v}")]);
    }
    // only mods run on the loader; other kinds of project are fine as they are
    if !filter.accepts.is_empty() && types.iter().any(|t| t == "mod") {
        let mut any: Vec<String> = (filter.accepts.iter())
            .map(|l| format!("categories:{l}"))
            .collect();
        any.extend(
            types
                .iter()
                .filter(|t| *t != "mod")
                .map(|t| format!("project_type:{t}")),
        );
        facets.push(any);
    }
    if !query.categories.is_empty() {
        facets.push(
            query
                .categories
                .iter()
                .map(|c| format!("categories:{c}"))
                .collect(),
        );
    }
    for c in &query.excluded_categories {
        facets.push(vec![format!("categories!={c}")]);
    }
    facets
}

/// Recognizes project pages (`/mod/<slug>`, `/shader/<slug>/version/<v>`,
/// ...), returning the slug and version.
fn parse_project_url(url: &str) -> Option<(String, Option<String>)> {
    let (host, segments) = url_parts(url)?;
    if host != "modrinth.com" {
        return None;
    }
    match segments.as_slice() {
        [kind, slug, rest @ ..] if PROJECT_TYPES.contains(kind) => {
            let version = match rest {
                [] => None,
                ["version", version] => Some(version.to_string()),
                // the tabs on a project's page
                [_] => None,
                _ => return None,
            };
            Some((slug.to_string(), version))
        }
        _ => None,
    }
}

#[async_trait]
impl Source for Modrinth {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "Modrinth"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        check_version_picked(ctx)?;
        let project = project(id).await?;
        let filter = Filter::new(ctx, &config, Some(&project.loaders));
        let versions = project_versions(&project.id, &filter).await?;
        let chosen = match (pick(versions, version), version) {
            (Some(v), _) => v,
            // A pinned version that doesn't suit is still what was asked
            // for; whatever it doesn't suit shows up as an issue.
            (None, Some(wanted)) => {
                get_json(&format!("{API}/project/{}/version/{wanted}", project.id))
                    .await
                    .map_err(|_| error(format!("{} has no version {wanted}", project.slug)))?
            }
            (None, None) => {
                return Err(error(format!(
                    "{} has no version for {}",
                    project.slug,
                    filter.describe(ctx.game)
                )));
            }
        };
        let pinned: Vec<&String> = chosen
            .dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
            .filter_map(|d| d.version_id.as_ref())
            .collect();
        let numbers: HashMap<String, (String, String)> = if pinned.is_empty() {
            HashMap::new()
        } else {
            let url = with_params(&format!("{API}/versions"), &[("ids", json_list(pinned))])?;
            let found: Vec<Version> = get_json(&url).await?;
            found
                .into_iter()
                .map(|v| (v.id, (v.project_id, v.version_number)))
                .collect()
        };
        let mut resolved = to_resolved(ctx.game, &config, &project, chosen, &numbers)?;
        name_soft_dependencies(&mut resolved).await;
        Ok(resolved)
    }

    /// Version numbers are free-form, and only ones that are numbers can
    /// be put in order.
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        if a == b {
            return Some(Ordering::Equal);
        }
        let plain = |v: &'_ str| v.strip_prefix('v').unwrap_or(v).to_owned();
        compare_dotted(&plain(a), &plain(b))
    }

    fn lists_versions(&self) -> bool {
        true
    }

    /// Only the versions that suit the profile.
    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let config = config(ctx)?;
        let project = project(id).await?;
        let filter = Filter::new(ctx, &config, Some(&project.loaders));
        let mut numbers: Vec<String> = Vec::new();
        for v in project_versions(&project.id, &filter).await? {
            if !numbers.contains(&v.version_number) {
                numbers.push(v.version_number);
            }
        }
        Ok(numbers)
    }

    async fn summary(&self, _ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        Ok(Some(project_to_summary(project(id).await?)))
    }

    /// Each version's notes, for the versions that suit the profile, from
    /// `version` back.
    async fn changelog(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let config = config(ctx)?;
        let project = project(id).await?;
        let filter = Filter::new(ctx, &config, Some(&project.loaders));
        let versions = project_versions(&project.id, &filter).await?;
        let from = version
            .and_then(|w| versions.iter().position(|v| v.version_number == w))
            .unwrap_or(0);
        Ok(versions
            .into_iter()
            .skip(from)
            .filter_map(|v| {
                let notes = v.changelog?.trim().to_owned();
                (!notes.is_empty()).then_some(ChangelogEntry {
                    version: Some(v.version_number),
                    notes,
                })
            })
            .collect())
    }

    fn mod_url(&self, _ctx: &SourceContext<'_>, id: &str, version: Option<&str>) -> Option<String> {
        let mut url = format!("{SITE}/project/{id}");
        if let Some(version) = version {
            url.push_str(&format!("/version/{version}"));
        }
        Some(url)
    }

    fn parse_url(&self, _ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        Ok(parse_project_url(url).map(|(id, version)| ModSpec {
            source: ID.to_owned(),
            id,
            version,
        }))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &SortBy::ALL
    }

    /// Category ids are Modrinth's, e.g. `game-mechanics`.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        let types = config(ctx)?.project_types;
        let mut found: Vec<Category> = Vec::new();
        for tag in all_categories().await? {
            // datapacks are listed with mods'
            let for_type = types
                .iter()
                .any(|t| *t == tag.project_type || (t == "datapack" && tag.project_type == "mod"));
            if tag.header == "categories" && for_type && !found.iter().any(|c| c.id == tag.name) {
                found.push(category(tag.name));
            }
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(found)
    }

    /// Pages are 20 projects, of the game definition's `project_types`,
    /// that suit the profile. Without a sort, text searches are by
    /// relevance and the rest most downloaded first.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let config = config(ctx)?;
        let filter = Filter::new(ctx, &config, None);
        let types = config.project_types;
        let text = query.text();
        let sort = query.sort.unwrap_or(match text {
            Some(_) => SortBy::Relevance,
            None => SortBy::Downloads,
        });
        let mut params = vec![
            (
                "facets",
                serde_json::to_string(&facets(&types, &filter, query)).expect("strings serialize"),
            ),
            ("index", index(sort).to_owned()),
            ("offset", (query.page * PAGE_SIZE).to_string()),
            ("limit", PAGE_SIZE.to_string()),
        ];
        if let Some(text) = text {
            params.push(("query", text.to_owned()));
        }
        let result: SearchResult =
            get_json(&with_params(&format!("{API}/search"), &params)?).await?;
        let shown = u64::from(query.page * PAGE_SIZE) + result.hits.len() as u64;
        Ok(SearchPage {
            total: Some(result.total_hits),
            has_more: shown < result.total_hits,
            items: result
                .hits
                .into_iter()
                .map(|hit| hit_to_summary(ctx.game, hit))
                .collect(),
            sort,
        })
    }
}

async fn project(id: &str) -> Result<Project> {
    let url = format!("{API}/project/{id}");
    let response = send(&url).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(error(format!("there's no project `{id}`")));
    }
    read_json(&url, response).await
}

/// The project's versions that suit `filter`, newest first.
async fn project_versions(id: &str, filter: &Filter) -> Result<Vec<Version>> {
    get_json(&with_params(
        &format!("{API}/project/{id}/version"),
        &filter.params(),
    )?)
    .await
}

/// Every category, which is only fetched once.
async fn all_categories() -> Result<Vec<CategoryTag>> {
    static KNOWN: LazyLock<Mutex<Option<Vec<CategoryTag>>>> = LazyLock::new(Mutex::default);
    if let Some(known) = KNOWN.lock().unwrap().as_ref() {
        return Ok(known.clone());
    }
    let tags: Vec<CategoryTag> = get_json(&format!("{API}/tag/category")).await?;
    *KNOWN.lock().unwrap() = Some(tags.clone());
    Ok(tags)
}

fn with_params(url: &str, params: &[(&str, String)]) -> Result<String> {
    reqwest::Url::parse_with_params(url, params)
        .map(String::from)
        .map_err(|e| error(format!("bad url: {e}")))
}

async fn send(url: &str) -> Result<reqwest::Response> {
    patchwork_core::http_get(url)
        .await
        .map_err(|result| patchwork_core::Error::Reqwest {
            url: url.to_owned(),
            result,
        })
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    read_json(url, send(url).await?).await
}

async fn read_json<T: DeserializeOwned>(url: &str, response: reqwest::Response) -> Result<T> {
    let reqwest_error = |result| patchwork_core::Error::Reqwest {
        url: url.to_owned(),
        result,
    };
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
