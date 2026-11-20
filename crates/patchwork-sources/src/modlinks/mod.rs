//! Mods from a ModLinks catalog: the XML files Hollow Knight's mod
//! installers (Scarab, Lumafly) read, from
//! <https://github.com/hk-modding/modlinks>.
//!
//! The game says where its catalogs are:
//!
//! ```toml
//! [sources.modlinks]
//! mods = "https://raw.githubusercontent.com/hk-modding/modlinks/main/ModLinks.xml"
//! api = "https://raw.githubusercontent.com/hk-modding/modlinks/main/ApiLinks.xml"
//! ```
//!
//! Ids are mods' `<Name>`s, which is also how mods name what they depend
//! on, like `modlinks:Randomizer 4`. The modding API (the patched
//! `Assembly-CSharp.dll` and the libraries it loads mods with) is the mod
//! `api`, of kind `api`, so the game's rules can put it somewhere else, and
//! every other mod depends on it. A catalog only has each mod's newest
//! version, so that's the only one there is to ask for; versions locked
//! before keep their download links.
//!
//! A few mods (and the API) have a file for each system the game runs on.
//! The profile's `platform` (`windows`, `linux` or `mac`, from the game's
//! `[env]`) picks, or else the system patchwork is running on.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use patchwork_core::Result;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ResolvedMod, Source, SourceContext, VersionReq,
    compare_dotted, is_archive_name,
};

const ID: &str = "modlinks";
/// The modding API's id and kind.
const API_ID: &str = "api";
const API_NAME: &str = "Modding API";
/// The `[env]` axis that picks which system's file to use.
const PLATFORM: &str = "platform";
/// How long a catalog is reused for. They're over half a megabyte and only
/// change when a mod is updated.
const FRESH_FOR: Duration = Duration::from_secs(10 * 60);
const PAGE_SIZE: usize = 50;

pub struct ModLinks;

/// One `<Manifest>` from a catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    name: String,
    display_name: Option<String>,
    description: Option<String>,
    version: String,
    /// The file, or one per system.
    links: Links,
    dependencies: Vec<String>,
    repository: Option<String>,
    tags: Vec<String>,
    authors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Links {
    One(String),
    /// `(system, link)`, with systems as the catalog spells them
    /// (`Windows`, `Linux`, `Mac`).
    PerSystem(Vec<(String, String)>),
}

impl Manifest {
    fn shown_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// The link for `platform`, or the only one.
    fn link(&self, platform: &str) -> Result<&str> {
        match &self.links {
            Links::One(link) => Ok(link),
            Links::PerSystem(links) => links
                .iter()
                .find(|(system, _)| system.eq_ignore_ascii_case(platform))
                .map(|(_, link)| link.as_str())
                .ok_or_else(|| {
                    let systems: Vec<&str> = links.iter().map(|(s, _)| s.as_str()).collect();
                    error(format!(
                        "{} has no file for {platform}, only for {}",
                        self.name,
                        systems.join(", ")
                    ))
                }),
        }
    }
}

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: ID.to_owned(),
        message: message.into(),
    }
}

// --- Parsing ---

/// The `<Manifest>`s in a ModLinks or ApiLinks file. Ones without a name,
/// version or link are left out.
fn parse(xml: &str) -> Result<Vec<Manifest>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| error(format!("the catalog isn't valid XML: {e}")))?;
    Ok(doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("Manifest"))
        .filter_map(manifest)
        .collect())
}

fn text(node: roxmltree::Node<'_, '_>) -> Option<String> {
    // CDATA sections come through as text, but whitespace around them is
    // its own text node, so join them all.
    let text: String = node
        .children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(name))
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).and_then(text)
}

/// The text of each `<item>` in `<list>`.
fn list(node: roxmltree::Node<'_, '_>, list: &str, item: &str) -> Vec<String> {
    child(node, list)
        .into_iter()
        .flat_map(|l| l.children().filter(move |c| c.has_tag_name(item)))
        .filter_map(text)
        .collect()
}

fn manifest(node: roxmltree::Node<'_, '_>) -> Option<Manifest> {
    let links = match child_text(node, "Link") {
        Some(link) => Links::One(link),
        None => {
            let links: Vec<(String, String)> = child(node, "Links")?
                .children()
                .filter(|c| c.is_element())
                .filter_map(|c| Some((c.tag_name().name().to_owned(), text(c)?)))
                .collect();
            if links.is_empty() {
                return None;
            }
            Links::PerSystem(links)
        }
    };
    Some(Manifest {
        // The API's catalog has no names; [`find`] gives it one.
        name: child_text(node, "Name").unwrap_or_default(),
        display_name: child_text(node, "DisplayName"),
        description: child_text(node, "Description"),
        version: child_text(node, "Version")?,
        links,
        dependencies: list(node, "Dependencies", "Dependency"),
        repository: child_text(node, "Repository"),
        tags: list(node, "Tags", "Tag"),
        authors: list(node, "Authors", "Author"),
    })
}

// --- Catalogs ---

/// What a game's `[sources.modlinks]` says.
struct Config<'a> {
    mods: &'a str,
    api: Option<&'a str>,
}

fn config<'a>(ctx: &SourceContext<'a>) -> Result<Config<'a>> {
    let get = |key| ctx.config?.get(key)?.as_str();
    let mods = get("mods").ok_or_else(|| {
        error(format!(
            "{} has no ModLinks catalog (`[sources.modlinks] mods`)",
            ctx.game.name
        ))
    })?;
    Ok(Config {
        mods,
        api: get("api"),
    })
}

type Catalog = Arc<Vec<Manifest>>;

/// The manifests at `url`, reused for [`FRESH_FOR`].
async fn catalog(url: &str) -> Result<Catalog> {
    static KNOWN: LazyLock<Mutex<HashMap<String, (Instant, Catalog)>>> =
        LazyLock::new(Mutex::default);
    if let Some((at, known)) = KNOWN.lock().unwrap().get(url)
        && at.elapsed() < FRESH_FOR
    {
        return Ok(known.clone());
    }
    let reqwest_error = |result| patchwork_core::Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let xml = patchwork_core::http_get(url)
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(reqwest_error)?
        .text()
        .await
        .map_err(reqwest_error)?;
    let found = Arc::new(parse(&xml)?);
    KNOWN
        .lock()
        .unwrap()
        .insert(url.to_owned(), (Instant::now(), found.clone()));
    Ok(found)
}

/// The manifest for `id`, from the API's catalog for [`API_ID`].
async fn find(ctx: &SourceContext<'_>, id: &str) -> Result<Manifest> {
    let config = config(ctx)?;
    if id == API_ID {
        let url = config
            .api
            .ok_or_else(|| error(format!("{} has no modding API catalog", ctx.game.name)))?;
        let mut api = catalog(url)
            .await?
            .first()
            .cloned()
            .ok_or_else(|| error("the modding API's catalog is empty"))?;
        api.name = API_ID.to_owned();
        api.display_name = Some(API_NAME.to_owned());
        return Ok(api);
    }
    let mods = catalog(config.mods).await?;
    mods.iter()
        .find(|m| m.name == id)
        .or_else(|| mods.iter().find(|m| m.name.eq_ignore_ascii_case(id)))
        .cloned()
        .ok_or_else(|| error(format!("there's no mod `{id}` in the ModLinks catalog")))
}

/// Which system's file to use: the profile's `platform`, or the one
/// patchwork runs on.
fn platform<'a>(ctx: &SourceContext<'a>) -> &'a str {
    ctx.env
        .and_then(|env| env.get(PLATFORM))
        .unwrap_or(match std::env::consts::OS {
            "macos" => "mac",
            "linux" => "linux",
            _ => "windows",
        })
}

fn to_resolved(manifest: &Manifest, platform: &str, has_api: bool) -> Result<ResolvedMod> {
    let link = manifest.link(platform)?;
    let file = link.rsplit('/').next().unwrap_or(link);
    let file = file.split(['?', '#']).next().unwrap_or(file);
    let is_api = manifest.name == API_ID;
    let mut dependencies: Vec<Dependency> = manifest
        .dependencies
        .iter()
        .map(|name| Dependency {
            source: ID.to_owned(),
            id: name.clone(),
            req: VersionReq::Any,
        })
        .collect();
    if has_api && !is_api {
        dependencies.insert(
            0,
            Dependency {
                source: ID.to_owned(),
                id: API_ID.to_owned(),
                req: VersionReq::Any,
            },
        );
    }
    Ok(ResolvedMod {
        id: manifest.name.clone(),
        name: manifest.shown_name().to_owned(),
        version: Some(manifest.version.clone()),
        artifact: ArtifactLocation::Url {
            url: link.to_owned(),
        },
        format: match is_archive_name(file) {
            true => ArtifactFormat::Archive,
            false => ArtifactFormat::File,
        },
        dependencies,
        soft_dependencies: Vec::new(),
        game_versions: None,
        env: Default::default(),
        kind: is_api.then(|| API_ID.to_owned()),
    })
}

fn to_summary(manifest: &Manifest) -> ModSummary {
    ModSummary {
        source: ID.to_owned(),
        id: manifest.name.clone(),
        name: manifest.shown_name().to_owned(),
        file: None,
        url: manifest.repository.clone(),
        author: (!manifest.authors.is_empty()).then(|| manifest.authors.join(", ")),
        author_url: None,
        summary: manifest.description.clone(),
        icon_url: None,
        downloads: None,
        rating: None,
        updated: None,
        latest_version: Some(manifest.version.clone()),
        categories: manifest.tags.iter().map(|t| category(t)).collect(),
    }
}

fn category(tag: &str) -> Category {
    Category {
        id: tag.to_owned(),
        name: tag.to_owned(),
    }
}

/// Whether every word of `text` is in `manifest`'s names, description or
/// authors, ignoring case.
fn matches_text(manifest: &Manifest, text: &str) -> bool {
    let shown = [
        Some(manifest.name.as_str()),
        manifest.display_name.as_deref(),
        manifest.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(manifest.authors.iter().map(String::as_str))
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    text.split_whitespace()
        .all(|word| shown.contains(&word.to_lowercase()))
}

fn has_tag(manifest: &Manifest, tags: &[String]) -> bool {
    manifest
        .tags
        .iter()
        .any(|t| tags.iter().any(|c| c.eq_ignore_ascii_case(t)))
}

/// How well `manifest`'s name matches search `text`: 0 if the name starts
/// with it, 1 if it has every word, 2 if only the rest does.
fn rank(manifest: &Manifest, text: &str) -> u8 {
    let text = text.to_lowercase();
    let names = [
        Some(manifest.name.as_str()),
        manifest.display_name.as_deref(),
    ];
    let names = names.into_iter().flatten().map(str::to_lowercase);
    names
        .map(|name| match name.starts_with(&text) {
            true => 0,
            false if text.split_whitespace().all(|w| name.contains(w)) => 1,
            false => 2,
        })
        .min()
        .unwrap_or(2)
}

/// One page of the mods in `mods` that `query` asks for: by name with no
/// search text, or else ones whose name matches first, then in the
/// catalog's order.
fn search_page(mods: &[Manifest], query: &SearchQuery) -> SearchPage {
    let mut found: Vec<&Manifest> = mods
        .iter()
        .filter(|m| !m.name.is_empty())
        .filter(|m| query.text().is_none_or(|text| matches_text(m, text)))
        .filter(|m| query.categories.is_empty() || has_tag(m, &query.categories))
        .filter(|m| !has_tag(m, &query.excluded_categories))
        .collect();
    match query.text() {
        Some(text) => found.sort_by_cached_key(|m| rank(m, text)),
        None => found.sort_by_cached_key(|m| m.shown_name().to_lowercase()),
    }
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
        sort: SortBy::Relevance,
    }
}

#[async_trait]
impl Source for ModLinks {
    fn id(&self) -> &str {
        ID
    }

    fn name(&self) -> &str {
        "ModLinks"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let manifest = find(ctx, id).await?;
        if let Some(version) = version
            && version != manifest.version
        {
            return Err(error(format!(
                "the ModLinks catalog only has the newest version of {}, {}, not {version}",
                manifest.name, manifest.version
            )));
        }
        to_resolved(&manifest, platform(ctx), config(ctx)?.api.is_some())
    }

    /// Mods' versions are four numbers, and the API's one.
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        compare_dotted(a, b)
    }

    fn lists_versions(&self) -> bool {
        true
    }

    /// Just the newest, which is all a catalog has.
    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        Ok(vec![find(ctx, id).await?.version])
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        Ok(Some(to_summary(&find(ctx, id).await?)))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &[SortBy::Relevance]
    }

    /// The catalog's tags.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        let mods = catalog(config(ctx)?.mods).await?;
        let mut tags: Vec<&str> = mods
            .iter()
            .flat_map(|m| &m.tags)
            .map(String::as_str)
            .collect();
        tags.sort_unstable();
        tags.dedup();
        Ok(tags.into_iter().map(category).collect())
    }

    /// Every mod in the catalog has every word of the search text in its
    /// name, description or authors. The API isn't listed, since every mod
    /// brings it along.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let mods = catalog(config(ctx)?.mods).await?;
        Ok(search_page(&mods, query))
    }
}

#[cfg(test)]
mod tests;
