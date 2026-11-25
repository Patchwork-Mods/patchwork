//! Sabers, avatars, platforms and notes from BeatMods' assets
//! (<https://beatmods.com/assets>), which took over from ModelSaber.
//!
//! The game says which of BeatMods' games it is, and which types of asset
//! to offer, each with the mod that loads it (or `""` for none), which is
//! added with it:
//!
//! ```toml
//! [sources.beatmods-assets]
//! game = "beatsaber"
//! types = { saber_saber = "beatmods:CustomSabersLite", note_bloq = "beatmods:CustomNotes" }
//! ```
//!
//! Ids are assets' numbers, like `beatmods-assets:8078`. An asset is a
//! single file (a `.saber`, `.bloq` and so on), with no versions: when its
//! author uploads a new file it replaces the old one. Its link carries the
//! file's hash, so updating picks the new file up. Only verified assets
//! are used. The game's deploy rules say where each type of file goes.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ModSpec, ResolvedMod, Source, SourceContext,
    VersionReq, url_parts,
};
use serde::Deserialize;

use super::{HOST, PAGE_SIZE, SITE, VERIFIED, api_url, find_json, get};

const ID: &str = "beatmods-assets";
/// The scheme of the one-click install links on BeatMods' asset pages,
/// which Mod Assistant and BSManager handle:
/// `modelsaber://saber/8078/iSF-HoloPoint.saber`.
const ONE_CLICK: &str = "modelsaber://";

pub struct BeatModsAssets;

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

// --- The API's JSON ---

/// `/assets`: every asset, in one page.
#[derive(Debug, Clone, Deserialize)]
struct AssetList {
    #[serde(default)]
    assets: Vec<Asset>,
}

/// One of `/assets`' results, or `/assets/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asset {
    id: u64,
    name: String,
    game_name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    uploader: Option<Uploader>,
    #[serde(default)]
    icon_urls: Vec<String>,
    file_hash: String,
    download_url: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Uploader {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

/// What BeatMods' asset types are called, plural, as search categories.
/// Types not here go by their own names.
const TYPE_NAMES: &[(&str, &str)] = &[
    ("saber_saber", "Sabers"),
    ("saber_whacker", "Whackers"),
    ("avatar_avatar", "Avatars"),
    ("platform_plat", "Platforms"),
    ("note_bloq", "Notes"),
    ("note_cyoob", "Notes (Cyoob)"),
    ("wall_pixie", "Walls (Pixie)"),
    ("wall_box", "Walls (Box)"),
    ("health-bar_energy", "Health bars"),
    ("sound_ogg", "Sounds (OGG)"),
    ("sound_mp3", "Sounds (MP3)"),
    ("banner_png", "Banners"),
    ("chroma-environment_dat", "Chroma environments"),
    ("camera2-config_json", "Camera2 configs"),
    ("counters-plus-config_json", "Counters+ configs"),
    (
        "hitscorevisualizer-config_json",
        "HitScoreVisualizer configs",
    ),
];

fn type_name(kind: &str) -> &str {
    TYPE_NAMES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(kind, |(_, name)| name)
}

fn category(kind: &str) -> Category {
    Category {
        id: kind.to_owned(),
        name: type_name(kind).to_owned(),
    }
}

impl Asset {
    fn page(&self) -> String {
        format!("{SITE}/assets/{}", self.id)
    }

    fn author(&self) -> Option<String> {
        let uploader = self.uploader.as_ref()?;
        uploader
            .display_name
            .clone()
            .or_else(|| uploader.username.clone())
    }

    /// Its file's name: the end of its download link, which BeatMods
    /// leaves unescaped (so it can have `#` or `%` in it).
    fn file_name(&self) -> &str {
        self.download_url
            .rsplit_once('/')
            .map_or(self.download_url.as_str(), |(_, name)| name)
    }

    /// Where to download its file: its download link escaped properly,
    /// with the file's hash after it. The link stays the same when the
    /// file's replaced, and patchwork takes a link to always be one file,
    /// so the hash makes a new file a new link. BeatMods ignores it.
    fn url(&self) -> Result<String> {
        let (folder, _) = self
            .download_url
            .rsplit_once('/')
            .ok_or_else(|| error(format!("asset {} has no download link", self.id)))?;
        let mut url = reqwest::Url::parse(&format!("{folder}/")).map_err(|e| {
            error(format!(
                "asset {}'s download link `{}` isn't one: {e}",
                self.id, self.download_url
            ))
        })?;
        url.path_segments_mut()
            .map_err(|()| error(format!("asset {}'s download link isn't one", self.id)))?
            .pop_if_empty()
            .push(self.file_name());
        url.query_pairs_mut().append_pair("v", &self.file_hash);
        Ok(url.into())
    }

    /// The first line of its description, as a summary.
    fn summary(&self) -> Option<String> {
        let line = self
            .description
            .as_deref()?
            .lines()
            .find(|l| !l.trim().is_empty())?;
        Some(line.trim().to_owned())
    }
}

// --- The game's config ---

/// What a game's `[sources.beatmods-assets]` says.
#[derive(Debug)]
struct Config<'a> {
    game: &'a str,
    /// The types of asset to offer, by name, with the mod each needs
    /// (`source:id`), if any.
    types: Vec<(&'a str, Option<&'a str>)>,
}

fn config<'a>(ctx: &SourceContext<'a>) -> Result<Config<'a>> {
    let table = ctx.config;
    let game = table.and_then(|t| t.get("game")?.as_str()).ok_or_else(|| {
        error(format!(
            "{} has no BeatMods game (`[sources.beatmods-assets] game`)",
            ctx.game.name
        ))
    })?;
    let types = table
        .and_then(|t| t.get("types")?.as_table())
        .ok_or_else(|| {
            error(format!(
                "{} doesn't say which BeatMods assets to offer (`[sources.beatmods-assets] types`)",
                ctx.game.name
            ))
        })?
        .iter()
        .map(|(kind, needs)| {
            let needs = needs.as_str().ok_or_else(|| {
                error(format!(
                    "the mod {} needs for {kind} assets should be a `source:id` or \"\"",
                    ctx.game.name
                ))
            })?;
            Ok((kind.as_str(), Some(needs).filter(|n| !n.is_empty())))
        })
        .collect::<Result<_>>()?;
    Ok(Config { game, types })
}

impl Config<'_> {
    fn offers(&self, kind: &str) -> bool {
        self.types.iter().any(|(k, _)| *k == kind)
    }

    /// Whether `asset` is one to use: verified, for this game, and of a
    /// type it offers.
    fn allows(&self, asset: &Asset) -> bool {
        asset.status == VERIFIED && asset.game_name == self.game && self.offers(&asset.kind)
    }

    /// The mod `kind` of asset needs to load.
    fn needs(&self, kind: &str) -> Result<Option<Dependency>> {
        let Some(needs) = self
            .types
            .iter()
            .find(|(k, _)| *k == kind)
            .and_then(|(_, n)| *n)
        else {
            return Ok(None);
        };
        let (source, id) = needs.split_once(':').ok_or_else(|| {
            error(format!(
                "`{needs}`, the mod {kind} assets need, should be a `source:id`"
            ))
        })?;
        Ok(Some(Dependency {
            source: source.to_owned(),
            id: id.to_owned(),
            req: VersionReq::Any,
        }))
    }
}

// --- Fetching ---

async fn list() -> Result<Arc<AssetList>> {
    get(&api_url("/assets", &[("status", VERIFIED)])?).await
}

/// Asset `id`, checking it's one `config` allows.
async fn find(config: &Config<'_>, ctx: &SourceContext<'_>, id: &str) -> Result<Arc<Asset>> {
    let number: u64 = id.parse().map_err(|_| {
        error(format!(
            "`{id}` isn't a BeatMods asset; they go by number, as in links to them"
        ))
    })?;
    let asset: Arc<Asset> = find_json(&api_url(&format!("/assets/{number}"), &[])?)
        .await?
        .ok_or_else(|| error(format!("there's no asset {number} on BeatMods")))?;
    check(config, &asset, &ctx.game.name)?;
    Ok(asset)
}

/// That `asset` is one `config` allows, or why not. `game` is the game's
/// name, for errors.
fn check(config: &Config<'_>, asset: &Asset, game: &str) -> Result<()> {
    let name = &asset.name;
    if asset.game_name != config.game {
        return Err(error(format!(
            "{name} is a BeatMods asset for another game ({})",
            asset.game_name
        )));
    }
    if asset.status != VERIFIED {
        return Err(error(format!(
            "{name} isn't verified on BeatMods (it's {})",
            asset.status
        )));
    }
    if !config.offers(&asset.kind) {
        return Err(error(format!(
            "{name} is one of BeatMods' {}, which patchwork can't install for {game} yet",
            type_name(&asset.kind).to_lowercase(),
        )));
    }
    Ok(())
}

fn to_resolved(config: &Config<'_>, asset: &Asset) -> Result<ResolvedMod> {
    Ok(ResolvedMod {
        id: asset.id.to_string(),
        name: asset.name.clone(),
        version: None,
        artifact: ArtifactLocation::Url { url: asset.url()? },
        format: ArtifactFormat::File,
        dependencies: config.needs(&asset.kind)?.into_iter().collect(),
        soft_dependencies: Vec::new(),
        game_versions: None,
        env: Default::default(),
        kind: Some(asset.kind.clone()),
    })
}

// --- Searching ---

fn to_summary(asset: &Asset) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        id: asset.id.to_string(),
        name: asset.name.clone(),
        file: Some(asset.file_name().to_owned()),
        url: Some(asset.page()),
        author: asset.author(),
        author_url: None,
        summary: asset.summary(),
        icon_url: asset.icon_urls.first().cloned(),
        downloads: None,
        rating: None,
        updated: asset.updated_at,
        latest_version: None,
        categories: vec![category(&asset.kind)],
    }
}

/// Whether every word of `text` is in `asset`'s name, description,
/// uploader or tags, ignoring case.
fn matches_text(asset: &Asset, text: &str) -> bool {
    let shown = [Some(asset.name.as_str()), asset.description.as_deref()]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .chain(asset.author())
        .chain(asset.tags.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    text.split_whitespace()
        .all(|word| shown.contains(&word.to_lowercase()))
}

/// How well `asset`'s name matches search `text`: 0 if it starts with it,
/// 1 if it has every word, 2 if only the rest does.
fn rank(asset: &Asset, text: &str) -> u8 {
    let (name, text) = (asset.name.to_lowercase(), text.to_lowercase());
    match name.starts_with(&text) {
        true => 0,
        false if text.split_whitespace().all(|w| name.contains(w)) => 1,
        false => 2,
    }
}

fn in_types(asset: &Asset, types: &[String]) -> bool {
    types.iter().any(|t| t.eq_ignore_ascii_case(&asset.kind))
}

/// One page of `assets` that `query` asks for.
fn search_page(config: &Config<'_>, assets: &[Asset], query: &SearchQuery) -> SearchPage {
    let mut seen = HashSet::new();
    let mut found: Vec<&Asset> = assets
        .iter()
        .filter(|a| config.allows(a) && seen.insert(a.id))
        .filter(|a| query.text().is_none_or(|text| matches_text(a, text)))
        .filter(|a| query.categories.is_empty() || in_types(a, &query.categories))
        .filter(|a| !in_types(a, &query.excluded_categories))
        .collect();
    let sort = match (query.sort, query.text()) {
        (Some(SortBy::Updated), _) => {
            found.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
            SortBy::Updated
        }
        (_, Some(text)) => {
            found.sort_by_cached_key(|a| (rank(a, text), a.name.to_lowercase()));
            SortBy::Relevance
        }
        // newest first, as BeatMods shows them, since there are thousands
        (_, None) => {
            found.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
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
            .map(to_summary)
            .collect(),
        sort,
    }
}

/// The asset number in a link to an asset's page
/// (`https://beatmods.com/assets/8078`) or a one-click install link
/// (`modelsaber://saber/8078/iSF-HoloPoint.saber`).
fn linked_number(url: &str) -> Option<&str> {
    let number = match url.strip_prefix(ONE_CLICK) {
        Some(rest) => *rest.split('/').collect::<Vec<_>>().get(1)?,
        None => match url_parts(url)? {
            (HOST, path) => match path[..] {
                ["assets", number] => number,
                _ => return None,
            },
            _ => return None,
        },
    };
    number.parse::<u64>().is_ok().then_some(number)
}

#[async_trait]
impl Source for BeatModsAssets {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "BeatMods assets"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        let asset = find(&config, ctx, id).await?;
        if let Some(version) = version {
            return Err(error(format!(
                "{} has no version {version}: BeatMods' assets don't have versions",
                asset.name
            )));
        }
        to_resolved(&config, &asset)
    }

    /// There's only ever the one file, so no two versions to order.
    fn compare_versions(&self, _a: &str, _b: &str) -> Option<Ordering> {
        None
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let config = config(ctx)?;
        Ok(Some(to_summary(&*find(&config, ctx, id).await?)))
    }

    fn mod_url(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        _version: Option<&str>,
    ) -> Option<String> {
        id.parse::<u64>()
            .is_ok()
            .then(|| format!("{SITE}/assets/{id}"))
    }

    fn parse_url(&self, _ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        Ok(linked_number(url).map(|number| ModSpec {
            source: ID.to_owned(),
            id: number.to_owned(),
            version: None,
        }))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &[SortBy::Relevance, SortBy::Updated]
    }

    /// The types of asset the game offers.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        let config = config(ctx)?;
        Ok(config
            .types
            .iter()
            .map(|(kind, _)| category(kind))
            .collect())
    }

    /// The assets the game offers that have every word of the search text
    /// in their name, description, uploader or tags.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let config = config(ctx)?;
        Ok(search_page(&config, &list().await?.assets, query))
    }
}

#[cfg(test)]
mod tests;
