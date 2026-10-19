//! Searching through the v2 GraphQL API, which, unlike v1, doesn't need an
//! API key.
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Nexus, error};

const GRAPHQL: &str = "https://api.nexusmods.com/v2/graphql";
pub(super) const PAGE_SIZE: u32 = 20;

const MODS_QUERY: &str = "
query Mods($filter: ModsFilter, $sort: [ModsSort!], $offset: Int, $count: Int) {
  mods(filter: $filter, sort: $sort, offset: $offset, count: $count) {
    totalCount
    nodes {
      modId name summary author version downloads endorsements updatedAt thumbnailUrl
      modCategory { name }
      uploader { name }
    }
  }
}";

/// Mods by id; unlike `mods`, it doesn't need the game's numeric id.
const MODS_BY_ID_QUERY: &str = "
query ModsById($ids: [CompositeDomainWithIdInput!]!) {
  legacyModsByDomain(ids: $ids) {
    nodes {
      modId name summary author version downloads endorsements updatedAt thumbnailUrl
      modCategory { name }
      uploader { name }
    }
  }
}";

const CATEGORIES_QUERY: &str = "
query Categories($filter: ModsFilter, $facets: ModsFacet) {
  mods(filter: $filter, facets: $facets, count: 0) { facetsData }
}";

// --- API Responses ---
#[derive(Debug, Deserialize)]
struct GraphqlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ModsData<T> {
    mods: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModsByIdData {
    legacy_mods_by_domain: ModNodes,
}

#[derive(Debug, Deserialize)]
struct ModNodes {
    nodes: Vec<ModNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModsPage {
    pub total_count: u64,
    pub nodes: Vec<ModNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModNode {
    pub mod_id: u64,
    pub name: String,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub downloads: Option<u64>,
    pub endorsements: Option<u64>,
    pub updated_at: Option<Timestamp>,
    pub thumbnail_url: Option<String>,
    pub mod_category: Option<ModCategory>,
    pub uploader: Option<Uploader>,
}

/// Who uploaded a mod, which isn't always who `author` credits.
#[derive(Debug, Deserialize)]
pub(super) struct Uploader {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModCategory {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Facets {
    facets_data: FacetsData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FacetsData {
    /// Category name to how many mods are in it.
    category_name: serde_json::Map<String, Value>,
}
// --- End API Responses ---

pub(super) fn node_to_summary(game_domain: &str, node: ModNode) -> ModSummary {
    ModSummary {
        source: "nexus".to_owned(),
        id: node.mod_id.to_string(),
        url: Some(format!(
            "https://www.nexusmods.com/{game_domain}/mods/{}",
            node.mod_id
        )),
        // `author` is free text, so link the uploader's mods instead
        author_url: node
            .uploader
            .map(|u| format!("https://www.nexusmods.com/profile/{}/mods", u.name)),
        name: node.name,
        author: node.author,
        summary: node.summary,
        icon_url: node.thumbnail_url,
        downloads: node.downloads,
        rating: node.endorsements,
        updated: node.updated_at,
        latest_version: node.version,
        categories: node
            .mod_category
            .into_iter()
            .map(|c| Category {
                id: c.name.clone(),
                name: c.name,
            })
            .collect(),
    }
}

/// The `ModsFilter` for a search. Adult mods are left out, as they are on the
/// site by default.
pub(super) fn mods_filter(game_domain: &str, query: &SearchQuery) -> Value {
    // each group in `filter` is ORed within and ANDed with the rest
    let mut groups = Vec::new();
    if let Some(text) = query.text() {
        // the wildcard match finds names containing the text, and the stemmed
        // one is what makes relevance ranking work
        groups.push(json!({
            "op": "OR",
            "name": [{ "value": text, "op": "WILDCARD" }],
            "nameStemmed": [{ "value": text, "op": "MATCHES" }],
        }));
    }
    if !query.categories.is_empty() {
        let categories: Vec<_> = query
            .categories
            .iter()
            .map(|c| json!({ "value": c, "op": "EQUALS" }))
            .collect();
        groups.push(json!({ "op": "OR", "categoryName": categories }));
    }
    json!({
        "gameDomainName": [{ "value": game_domain, "op": "EQUALS" }],
        "adultContent": [{ "value": false, "op": "EQUALS" }],
        "filter": groups,
    })
}

/// The sort the site uses: by relevance when there's text to be relevant to,
/// otherwise most downloaded.
pub(super) fn effective_sort(query: &SearchQuery) -> SortBy {
    query.sort.unwrap_or(if query.text().is_some() {
        SortBy::Relevance
    } else {
        SortBy::Downloads
    })
}

fn mods_sort(sort: SortBy) -> Value {
    let field = match sort {
        SortBy::Relevance => "relevance",
        SortBy::Downloads => "downloads",
        SortBy::Updated => "updatedAt",
        SortBy::Newest => "createdAt",
        SortBy::Rating => "endorsements",
    };
    json!([{ field: { "direction": "DESC" } }])
}

impl Nexus {
    pub(super) async fn search_mods(
        &self,
        game_domain: &str,
        query: &SearchQuery,
    ) -> Result<SearchPage> {
        let sort = effective_sort(query);
        let offset = query.page * PAGE_SIZE;
        let variables = json!({
            "filter": mods_filter(game_domain, query),
            "sort": mods_sort(sort),
            "offset": offset,
            "count": PAGE_SIZE,
        });
        let page = self
            .graphql::<ModsData<ModsPage>>(MODS_QUERY, variables)
            .await?
            .mods;
        let has_more = u64::from(offset) + (page.nodes.len() as u64) < page.total_count;
        Ok(SearchPage {
            total: Some(page.total_count),
            has_more,
            items: page
                .nodes
                .into_iter()
                .map(|node| node_to_summary(game_domain, node))
                .collect(),
            sort,
        })
    }

    /// Nexus has no list of a game's mod categories, so this asks how many
    /// mods are in each one, which comes out most populated first.
    pub(super) async fn categories(&self, game_domain: &str) -> Result<Vec<Category>> {
        let variables = json!({
            "filter": { "gameDomainName": [{ "value": game_domain, "op": "EQUALS" }] },
            "facets": { "categoryName": [] },
        });
        let facets = self
            .graphql::<ModsData<Facets>>(CATEGORIES_QUERY, variables)
            .await?
            .mods;
        let mut counted: Vec<_> = facets
            .facets_data
            .category_name
            .into_iter()
            .map(|(name, count)| (count.as_u64().unwrap_or(0), name))
            .collect();
        counted.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(counted
            .into_iter()
            .map(|(_, name)| Category {
                id: name.clone(),
                name,
            })
            .collect())
    }

    /// Mod `mod_id` as a search would show it, or `None` if there's no such
    /// mod.
    pub(super) async fn mod_summary(
        &self,
        game_domain: &str,
        mod_id: u64,
    ) -> Result<Option<ModSummary>> {
        let variables = json!({ "ids": [{ "gameDomain": game_domain, "modId": mod_id }] });
        let data: ModsByIdData = self.graphql(MODS_BY_ID_QUERY, variables).await?;
        Ok(data
            .legacy_mods_by_domain
            .nodes
            .into_iter()
            .find(|node| node.mod_id == mod_id)
            .map(|node| node_to_summary(game_domain, node)))
    }

    async fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        let reqwest_error = |result| patchwork_core::Error::Reqwest {
            url: GRAPHQL.to_owned(),
            result,
        };
        let bytes = self
            .client
            .post(GRAPHQL)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(reqwest_error)?
            .bytes()
            .await
            .map_err(reqwest_error)?;
        let response: GraphqlResponse<T> = serde_json::from_slice(&bytes)?;
        if let Some(e) = response.errors.first() {
            return Err(error(format!("search failed: {}", e.message)));
        }
        response
            .data
            .ok_or_else(|| error("search returned nothing"))
    }
}
