use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Dependency, ModSpec, ResolvedMod, Source, SourceContext,
    VersionReq, compare_dotted, url_parts,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const API: &str = "https://thunderstore.io/api/experimental";
/// The API behind thunderstore.io's own pages, which is the only one that can
/// search. It isn't documented, so it may change under us.
const CYBERSTORM: &str = "https://thunderstore.io/api/cyberstorm";

/// Mods from Thunderstore. Ids are `Namespace-Name`, the same form Thunderstore
/// uses in dependency strings, e.g. `thunderstore:BepInEx-BepInExPack`.
pub struct Thunderstore;

#[derive(Debug, Clone)]
pub struct PackageVersion {
    package: PackageExperimental,
    version: PackageVersionExperimental,
}

// --- API Responses ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageExperimental {
    pub namespace: Option<String>,
    pub name: String,
    pub full_name: Option<String>,
    pub owner: Option<String>,
    pub package_url: Option<String>,
    pub date_created: Option<Timestamp>,
    pub date_updated: Option<Timestamp>,
    pub rating_score: Option<i64>,
    pub is_pinned: Option<bool>,
    pub is_deprecated: Option<bool>,
    pub total_downloads: Option<i64>,
    pub latest: PackageVersionExperimental,
    pub community_listings: Vec<PackageListingExperimental>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersionExperimental {
    pub namespace: Option<String>,
    pub name: String,
    pub version_number: String,
    pub full_name: Option<String>,
    pub description: String,
    pub icon: Option<String>,
    pub dependencies: Option<Vec<String>>,
    pub download_url: Option<String>,
    pub downloads: Option<u64>,
    pub date_created: Option<Timestamp>,
    pub website_url: Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageListingExperimental {
    pub has_nsfw_content: Option<bool>,
    pub categories: Option<Vec<String>>,
    pub community: Option<String>,
    pub review_status: Option<ReviewStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStatus {
    Unreviewed,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Deserialize)]
struct Listing {
    count: u64,
    next: Option<String>,
    results: Vec<ListedPackage>,
}

#[derive(Debug, Clone, Deserialize)]
struct ListedPackage {
    namespace: String,
    name: String,
    description: Option<String>,
    icon_url: Option<String>,
    download_count: Option<u64>,
    rating_count: Option<u64>,
    last_updated: Option<Timestamp>,
    #[serde(default)]
    categories: Vec<ListedCategory>,
}

/// One package's page in the listing, which has a bit more than a search
/// result.
#[derive(Debug, Clone, Deserialize)]
struct ListedPackageDetail {
    #[serde(flatten)]
    package: ListedPackage,
    latest_version_number: Option<String>,
    /// When the latest version was uploaded, which is when it was last
    /// updated.
    version_created: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
struct ListedCategory {
    id: String,
    name: String,
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ListedVersion {
    version_number: String,
    datetime_created: Option<Timestamp>,
}

#[derive(Debug, Clone, Deserialize)]
struct CommunityFilters {
    package_categories: Vec<ListedCategory>,
}

// --- End API Responses ---

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: "thunderstore".to_owned(),
        message: message.into(),
    }
}

// converts a thunderstore mod to our internal version of a mod
fn thunderstore_to_resolved_mod(mod_: PackageVersion) -> Option<ResolvedMod> {
    let PackageVersion { package, version } = mod_;
    let artifact = ArtifactLocation::Url {
        url: version.download_url?,
    };
    let dependencies = version
        .dependencies
        .unwrap_or_default()
        .iter()
        .map(|dep| {
            let ModNameVersion { name, version } = decode_mod_version_name(dep)?;
            // these are the versions a mod was built against, and mod
            // managers treat them as minimums
            Some(Dependency {
                source: "thunderstore".to_string(),
                id: name,
                req: VersionReq::AtLeast(version),
            })
        })
        .collect::<Option<Vec<_>>>()?;

    let id = format!("{}-{}", package.namespace?, package.name);
    Some(ResolvedMod {
        name: package.full_name.unwrap_or_else(|| id.clone()),
        id,
        version: Some(version.version_number),
        artifact,
        format: ArtifactFormat::Zip,
        dependencies,
    })
}

struct ModNameVersion {
    name: String,
    version: String,
}
// Converts a mod in the "name-version" syntax used in the Thunderstore API to an actual mod name and version
fn decode_mod_version_name(name_version: &str) -> Option<ModNameVersion> {
    let (name, version) = name_version.rsplit_once('-')?;
    Some(ModNameVersion {
        name: name.to_string(),
        version: version.to_string(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    community: String,
}

fn config(ctx: &SourceContext<'_>) -> Result<Config> {
    ctx.config
        .cloned()
        .ok_or_else(|| {
            error(format!(
                "{} isn't on Thunderstore (no [sources.thunderstore] in its definition)",
                ctx.game.id
            ))
        })?
        .try_into()
        .map_err(|e| {
            error(format!(
                "bad [sources.thunderstore] for {}: {e}",
                ctx.game.id
            ))
        })
}

/// Recognizes package pages (`/c/<community>/p/<namespace>/<name>/`, maybe
/// with `v/<version>/`) and download links
/// (`/package/download/<namespace>/<name>/<version>/`), returning the
/// community if the URL has one, the id and the version.
fn parse_package_url(url: &str) -> Option<(Option<&str>, String, Option<String>)> {
    let (host, segments) = url_parts(url)?;
    if host != "thunderstore.io" {
        return None;
    }
    match segments.as_slice() {
        ["c", community, "p", namespace, name, rest @ ..] => {
            let version = match rest {
                [] => None,
                ["v", version] => Some(version.to_string()),
                _ => return None,
            };
            Some((Some(community), format!("{namespace}-{name}"), version))
        }
        ["package", "download", namespace, name, version] => Some((
            None,
            format!("{namespace}-{name}"),
            Some(version.to_string()),
        )),
        _ => None,
    }
}

fn listed_to_summary(community: &str, package: ListedPackage) -> ModSummary {
    let author_url = format!(
        "https://thunderstore.io/c/{community}/p/{}/",
        package.namespace
    );
    ModSummary {
        source: "thunderstore".to_owned(),
        id: format!("{}-{}", package.namespace, package.name),
        url: Some(format!("{author_url}{}/", package.name)),
        author_url: Some(author_url),
        name: package.name,
        author: Some(package.namespace),
        summary: package.description,
        icon_url: package.icon_url,
        downloads: package.download_count,
        rating: package.rating_count,
        updated: package.last_updated,
        // the listing doesn't say; it's only needed once a mod's picked
        latest_version: None,
        categories: package
            .categories
            .into_iter()
            .map(|c| Category {
                id: c.slug,
                name: c.name,
            })
            .collect(),
    }
}

fn detail_to_summary(community: &str, mut detail: ListedPackageDetail) -> ModSummary {
    detail.package.last_updated = detail.package.last_updated.or(detail.version_created);
    ModSummary {
        latest_version: detail.latest_version_number,
        ..listed_to_summary(community, detail.package)
    }
}

/// Version numbers, newest first. Thunderstore doesn't return them in any
/// particular order, and requires them to be `major.minor.patch`.
fn sort_versions(mut versions: Vec<ListedVersion>) -> Vec<String> {
    versions.sort_by(|a, b| {
        compare_dotted(&b.version_number, &a.version_number)
            .unwrap_or_else(|| b.datetime_created.cmp(&a.datetime_created))
    });
    versions.into_iter().map(|v| v.version_number).collect()
}

fn ordering(sort: SortBy) -> &'static str {
    match sort {
        SortBy::Downloads => "most-downloaded",
        SortBy::Updated => "last-updated",
        SortBy::Newest => "newest",
        SortBy::Rating => "top-rated",
        // not in search_sorts
        SortBy::Relevance => "most-downloaded",
    }
}

#[async_trait]
impl Source for Thunderstore {
    fn id(&self) -> &str {
        "thunderstore"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        let response = get_thunderstore_package(id, version).await?;
        if !response
            .package
            .community_listings
            .iter()
            .any(|l| l.community.as_deref() == Some(config.community.as_str()))
        {
            return Err(error(format!(
                "{id} isn't listed in the {} community",
                config.community
            )));
        }
        thunderstore_to_resolved_mod(response.clone())
            .ok_or_else(|| error(format!("could not resolve thunderstore mod: {response:?}")))
    }

    fn lists_versions(&self) -> bool {
        true
    }

    async fn versions(&self, _ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let (namespace, name) = split_id(id)?;
        let versions: Vec<ListedVersion> =
            get_json(format!("{CYBERSTORM}/package/{namespace}/{name}/versions/")).await?;
        Ok(sort_versions(versions))
    }

    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let community = config(ctx)?.community;
        let (namespace, name) = split_id(id)?;
        let detail: ListedPackageDetail = get_json(format!(
            "{CYBERSTORM}/listing/{community}/{namespace}/{name}/"
        ))
        .await?;
        Ok(Some(detail_to_summary(&community, detail)))
    }

    fn mod_url(&self, ctx: &SourceContext<'_>, id: &str, version: Option<&str>) -> Option<String> {
        let community = config(ctx).ok()?.community;
        let (namespace, name) = id.split_once('-')?;
        let mut url = format!("https://thunderstore.io/c/{community}/p/{namespace}/{name}/");
        if let Some(version) = version {
            url.push_str(&format!("v/{version}/"));
        }
        Some(url)
    }

    fn parse_url(&self, ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        let Some((community, id, version)) = parse_package_url(url) else {
            return Ok(None);
        };
        let expected = config(ctx)?.community;
        if let Some(community) = community
            && community != expected
        {
            return Err(error(format!(
                "{url} is in the {community} community, not {expected}"
            )));
        }
        Ok(Some(ModSpec {
            source: "thunderstore".to_owned(),
            id,
            version,
        }))
    }

    /// Thunderstore can't rank by relevance; text searches match names and
    /// descriptions and are sorted like everything else.
    fn search_sorts(&self) -> &[SortBy] {
        &[
            SortBy::Downloads,
            SortBy::Updated,
            SortBy::Newest,
            SortBy::Rating,
        ]
    }

    /// Category ids are their slugs, e.g. `tweaks-and-quality-of-life`.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        Ok(community_categories(&config(ctx)?.community)
            .await?
            .into_iter()
            .map(|c| Category {
                id: c.slug,
                name: c.name,
            })
            .collect())
    }

    /// Pages are 20 mods. Without a sort, mods come most downloaded first.
    /// Deprecated and NSFW mods are left out, as they are on the site.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        let community = config(ctx)?.community;
        let sort = query.sort.unwrap_or(SortBy::Downloads);
        let mut params = vec![
            ("ordering", ordering(sort).to_owned()),
            ("page", (query.page + 1).to_string()),
        ];
        if let Some(text) = query.text() {
            params.push(("q", text.to_owned()));
        }
        if !query.categories.is_empty() || !query.excluded_categories.is_empty() {
            // the listing filters by numeric id, so look up the slugs
            let known = community_categories(&community).await?;
            let included = query.categories.iter().map(|c| ("included_categories", c));
            let excluded = (query.excluded_categories.iter()).map(|c| ("excluded_categories", c));
            for (param, wanted) in included.chain(excluded) {
                let category = known
                    .iter()
                    .find(|c| c.slug == *wanted)
                    .ok_or_else(|| error(format!("{community} has no category `{wanted}`")))?;
                params.push((param, category.id.clone()));
            }
        }
        let url =
            reqwest::Url::parse_with_params(&format!("{CYBERSTORM}/listing/{community}/"), &params)
                .map_err(|e| error(format!("bad search url: {e}")))?;
        let response = patchwork_core::http_get(url.clone())
            .await
            .map_err(|result| patchwork_core::Error::Reqwest {
                url: url.to_string(),
                result,
            })?;
        // asking for a page past the end is a 404
        if query.page > 0 && response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(SearchPage::empty(sort));
        }
        let listing: Listing = read_json(url.as_str(), response).await?;
        Ok(SearchPage {
            total: Some(listing.count),
            has_more: listing.next.is_some(),
            items: listing
                .results
                .into_iter()
                .map(|package| listed_to_summary(&community, package))
                .collect(),
            sort,
        })
    }
}

/// A community's categories, which are only fetched once since every search
/// by category needs them.
async fn community_categories(community: &str) -> Result<Vec<ListedCategory>> {
    static KNOWN: LazyLock<Mutex<HashMap<String, Vec<ListedCategory>>>> =
        LazyLock::new(Mutex::default);
    if let Some(known) = KNOWN.lock().unwrap().get(community) {
        return Ok(known.clone());
    }
    let filters: CommunityFilters =
        get_json(format!("{CYBERSTORM}/community/{community}/filters/")).await?;
    KNOWN
        .lock()
        .unwrap()
        .insert(community.to_owned(), filters.package_categories.clone());
    Ok(filters.package_categories)
}

async fn get_json<T: DeserializeOwned>(url: String) -> Result<T> {
    let response =
        patchwork_core::http_get(&url)
            .await
            .map_err(|result| patchwork_core::Error::Reqwest {
                url: url.clone(),
                result,
            })?;
    read_json(&url, response).await
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

fn split_id(id: &str) -> Result<(&str, &str)> {
    id.split_once('-')
        .ok_or_else(|| error(format!("`{id}` should look like `Namespace-Name`")))
}

async fn get_thunderstore_package(id: &str, version: Option<&str>) -> Result<PackageVersion> {
    let (namespace, name) = split_id(id)?;
    // the trailing slashes matter, thunderstore 502s without them
    let url = format!("{API}/package/{namespace}/{name}/");
    // we need the package either way for its community listings
    let package: PackageExperimental = get_json(url.clone()).await?;
    let version = match version {
        Some(v) => get_json(format!("{url}{v}/")).await?,
        None => package.latest.clone(),
    };
    Ok(PackageVersion { package, version })
}

#[cfg(test)]
mod tests;
