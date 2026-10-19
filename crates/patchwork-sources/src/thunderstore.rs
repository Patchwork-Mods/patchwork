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
        if !query.categories.is_empty() {
            // the listing filters by numeric id, so look up the slugs
            let known = community_categories(&community).await?;
            for wanted in &query.categories {
                let category = known
                    .iter()
                    .find(|c| c.slug == *wanted)
                    .ok_or_else(|| error(format!("{community} has no category `{wanted}`")))?;
                params.push(("included_categories", category.id.clone()));
            }
        }
        let url =
            reqwest::Url::parse_with_params(&format!("{CYBERSTORM}/listing/{community}/"), &params)
                .map_err(|e| error(format!("bad search url: {e}")))?;
        let response =
            reqwest::get(url.clone())
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

async fn community_categories(community: &str) -> Result<Vec<ListedCategory>> {
    let filters: CommunityFilters =
        get_json(format!("{CYBERSTORM}/community/{community}/filters/")).await?;
    Ok(filters.package_categories)
}

async fn get_json<T: DeserializeOwned>(url: String) -> Result<T> {
    let response = reqwest::get(&url)
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
mod thunderstore_test {
    use super::*;
    use patchwork_core::game::GameDef;

    // a response for notnotnotswipez/MoreCompany
    const PACKAGE: &str = r#"{
        "namespace": "notnotnotswipez",
        "name": "MoreCompany",
        "full_name": "notnotnotswipez-MoreCompany",
        "owner": "notnotnotswipez",
        "package_url": "https://thunderstore.io/package/notnotnotswipez/MoreCompany/",
        "date_created": "2023-11-16T04:46:27.899881Z",
        "date_updated": "2026-04-25T01:28:42.076106Z",
        "rating_score": -1,
        "is_pinned": false,
        "is_deprecated": false,
        "total_downloads": -1,
        "latest": {
            "namespace": "notnotnotswipez",
            "name": "MoreCompany",
            "version_number": "1.14.0",
            "full_name": "notnotnotswipez-MoreCompany-1.14.0",
            "description": "A stable lobby player count expansion mod. With cosmetics!",
            "icon": "https://ccdn.thunderstore.io/live/repository/icons/notnotnotswipez-MoreCompany-1.14.0.png",
            "dependencies": ["BepInEx-BepInExPack-5.4.2100"],
            "download_url": "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.14.0/",
            "downloads": 917031,
            "date_created": "2026-04-25T01:28:40.943994Z",
            "website_url": "https://github.com/notnotnotswipez/MoreCompany",
            "is_active": true
        },
        "community_listings": [{
            "has_nsfw_content": false,
            "categories": ["Mods", "BepInEx", "Tweaks & Quality Of Life"],
            "community": "lethal-company",
            "review_status": "approved"
        }]
    }"#;

    fn lethal_company() -> GameDef {
        toml::from_str(
            r#"
            schema = 1
            id = "lethal-company"
            name = "Lethal Company"

            [sources.thunderstore]
            community = "lethal-company"

            [deployer]
            kind = "rules"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn converts_api_response() {
        let package: PackageExperimental = serde_json::from_str(PACKAGE).unwrap();
        let version = package.latest.clone();
        let resolved = thunderstore_to_resolved_mod(PackageVersion { package, version }).unwrap();

        assert_eq!(resolved.id, "notnotnotswipez-MoreCompany");
        assert_eq!(resolved.name, "notnotnotswipez-MoreCompany");
        assert_eq!(resolved.version.as_deref(), Some("1.14.0"));
        assert_eq!(resolved.format, ArtifactFormat::Zip);
        assert_eq!(
            resolved.artifact,
            ArtifactLocation::Url {
                url: "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.14.0/"
                    .to_owned()
            }
        );
        assert_eq!(
            resolved.dependencies,
            vec![Dependency {
                source: "thunderstore".to_owned(),
                id: "BepInEx-BepInExPack".to_owned(),
                req: VersionReq::AtLeast("5.4.2100".to_owned()),
            }]
        );
    }

    #[test]
    fn decodes_dependency_strings() {
        let ModNameVersion { name, version } =
            decode_mod_version_name("BepInEx-BepInExPack-5.4.2100").unwrap();
        assert_eq!(name, "BepInEx-BepInExPack");
        assert_eq!(version, "5.4.2100");
        assert!(decode_mod_version_name("nohyphens").is_none());
    }

    #[test]
    fn urls_round_trip() {
        let game = lethal_company();
        let ctx = SourceContext {
            game: &game,
            config: game.sources.get("thunderstore"),
        };
        let url = Thunderstore
            .mod_url(&ctx, "notnotnotswipez-MoreCompany", Some("1.11.0"))
            .unwrap();
        assert_eq!(
            url,
            "https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/v/1.11.0/"
        );
        let spec = Thunderstore.parse_url(&ctx, &url).unwrap().unwrap();
        assert_eq!(spec.id, "notnotnotswipez-MoreCompany");
        assert_eq!(spec.version.as_deref(), Some("1.11.0"));

        let spec = Thunderstore
            .parse_url(
                &ctx,
                "https://thunderstore.io/package/download/BepInEx/BepInExPack/5.4.2100/",
            )
            .unwrap()
            .unwrap();
        assert_eq!(spec.id, "BepInEx-BepInExPack");
        assert_eq!(spec.version.as_deref(), Some("5.4.2100"));

        let spec = Thunderstore
            .parse_url(
                &ctx,
                "https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/?tab=wiki",
            )
            .unwrap()
            .unwrap();
        assert_eq!(spec.version, None);

        assert!(
            Thunderstore
                .parse_url(&ctx, "https://example.com/c/x/p/a/b/")
                .unwrap()
                .is_none()
        );
        let err = Thunderstore
            .parse_url(&ctx, "https://thunderstore.io/c/riskofrain2/p/a/b/")
            .unwrap_err();
        assert!(err.to_string().contains("riskofrain2"), "{err}");
    }

    // a trimmed /api/cyberstorm/listing/lethal-company/?q=morecompany
    const LISTING: &str = r#"{
        "count": 145,
        "next": "https://thunderstore.io/api/cyberstorm/listing/lethal-company/?page=2&q=morecompany",
        "previous": null,
        "results": [{
            "categories": [
                {"id": "658", "name": "Mods", "slug": "mods"},
                {"id": "872", "name": "Tweaks & Quality Of Life", "slug": "tweaks-and-quality-of-life"}
            ],
            "community_identifier": "lethal-company",
            "description": "A stable lobby player count expansion mod. With cosmetics!",
            "download_count": 26998136,
            "icon_url": "https://ccdn.thunderstore.io/live/repository/icons/notnotnotswipez-MoreCompany-1.14.0.png",
            "is_deprecated": false,
            "is_nsfw": false,
            "is_pinned": false,
            "last_updated": "2026-04-25T01:28:42.076106Z",
            "name": "MoreCompany",
            "namespace": "notnotnotswipez",
            "rating_count": 641,
            "size": 10530391,
            "datetime_created": "2023-11-16T04:46:28.298342Z"
        }]
    }"#;

    #[test]
    fn converts_listings() {
        let listing: Listing = serde_json::from_str(LISTING).unwrap();
        assert_eq!(listing.count, 145);
        assert!(listing.next.is_some());
        let summary = listed_to_summary(
            "lethal-company",
            listing.results.into_iter().next().unwrap(),
        );
        assert_eq!(summary.id, "notnotnotswipez-MoreCompany");
        assert_eq!(
            summary.url.as_deref(),
            Some("https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/")
        );
        assert_eq!(
            summary.author_url.as_deref(),
            Some("https://thunderstore.io/c/lethal-company/p/notnotnotswipez/")
        );
        assert_eq!(summary.name, "MoreCompany");
        assert_eq!(summary.author.as_deref(), Some("notnotnotswipez"));
        assert_eq!(summary.downloads, Some(26998136));
        assert_eq!(summary.rating, Some(641));
        assert_eq!(
            summary.categories[1],
            Category {
                id: "tweaks-and-quality-of-life".to_owned(),
                name: "Tweaks & Quality Of Life".to_owned(),
            }
        );
        assert_eq!(
            summary.spec(),
            ModSpec {
                source: "thunderstore".to_owned(),
                id: "notnotnotswipez-MoreCompany".to_owned(),
                version: None,
            }
        );
    }

    #[test]
    fn sorts_versions_newest_first() {
        let versions: Vec<ListedVersion> = serde_json::from_str(
            r#"[
                {"version_number": "5.4.2304", "datetime_created": "2025-11-05T17:14:09Z"},
                {"version_number": "5.4.2305", "datetime_created": "2026-03-17T05:05:21Z"},
                {"version_number": "5.4.2100", "datetime_created": "2023-01-01T00:00:00Z"}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            sort_versions(versions),
            ["5.4.2305", "5.4.2304", "5.4.2100"]
        );
    }

    #[tokio::test]
    async fn needs_community_config() {
        let mut game = lethal_company();
        game.sources.clear();
        let ctx = SourceContext {
            game: &game,
            config: None,
        };
        let err = Thunderstore
            .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't on Thunderstore"), "{err}");
    }

    // These talk to thunderstore.io; run them with `cargo test -- --ignored`.

    #[tokio::test]
    #[ignore = "needs network"]
    async fn test_get_mod() {
        let game = lethal_company();
        let ctx = SourceContext {
            game: &game,
            config: game.sources.get("thunderstore"),
        };

        let latest = Thunderstore
            .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
            .await
            .unwrap();
        assert_eq!(latest.id, "notnotnotswipez-MoreCompany");
        assert!(latest.version.is_some());

        let pinned = Thunderstore
            .resolve(&ctx, "notnotnotswipez-MoreCompany", Some("1.11.0"))
            .await
            .unwrap();
        assert_eq!(pinned.version.as_deref(), Some("1.11.0"));
        assert_eq!(
            pinned.artifact,
            ArtifactLocation::Url {
                url: "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.11.0/"
                    .to_owned()
            }
        );
        assert!(
            pinned
                .dependencies
                .iter()
                .any(|d| d.id == "BepInEx-BepInExPack")
        );

        let versions = Thunderstore
            .versions(&ctx, "notnotnotswipez-MoreCompany")
            .await
            .unwrap();
        assert_eq!(versions.first(), latest.version.as_ref());
        assert!(versions.iter().any(|v| v == "1.11.0"));
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn searches() {
        let game = lethal_company();
        let ctx = SourceContext {
            game: &game,
            config: game.sources.get("thunderstore"),
        };

        let home = Thunderstore
            .search(&ctx, &SearchQuery::default())
            .await
            .unwrap();
        assert_eq!(home.sort, SortBy::Downloads);
        assert!(home.has_more);
        let bepinex = home
            .items
            .iter()
            .find(|m| m.id == "BepInEx-BepInExPack")
            .unwrap();
        assert_eq!(
            bepinex.url.as_deref(),
            Some("https://thunderstore.io/c/lethal-company/p/BepInEx/BepInExPack/")
        );
        assert_eq!(
            bepinex.author_url.as_deref(),
            Some("https://thunderstore.io/c/lethal-company/p/BepInEx/")
        );

        let query = SearchQuery {
            text: Some("morecompany".to_owned()),
            categories: vec!["cosmetics".to_owned()],
            ..Default::default()
        };
        let found = Thunderstore.search(&ctx, &query).await.unwrap();
        assert!(!found.items.is_empty());
        assert!(
            found
                .items
                .iter()
                .all(|m| { m.categories.iter().any(|c| c.id == "cosmetics") })
        );

        let past_end = SearchQuery {
            page: 10_000,
            ..query
        };
        let empty = Thunderstore.search(&ctx, &past_end).await.unwrap();
        assert!(empty.items.is_empty() && !empty.has_more);

        let categories = Thunderstore.search_categories(&ctx).await.unwrap();
        assert!(categories.iter().any(|c| c.id == "moons"));
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn rejects_other_communities() {
        let mut game = lethal_company();
        game.sources.get_mut("thunderstore").unwrap().insert(
            "community".to_owned(),
            toml::Value::String("riskofrain2".to_owned()),
        );
        let ctx = SourceContext {
            game: &game,
            config: game.sources.get("thunderstore"),
        };
        let err = Thunderstore
            .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't listed"), "{err}");
    }
}
