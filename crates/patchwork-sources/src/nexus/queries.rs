use graphql_client::{GraphQLQuery, Response};
use thiserror::Error;

pub use mods::{
    BaseFilterValue, BaseFilterValueEqualsWildcard, ModFragment, ModsFilter, ModsSort,
    ResponseData, Variables,
};

#[derive(Debug, Error)]
pub enum NexusApiError {
    #[error("Failed to parse configuration: {0}")]
    ParseError(String),
    #[error("Failed to get request: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Failed to Serialize/Deserialize for Nexus: {0}")]
    SerdeJsonError(#[from] serde_json::Error),
}

type DateTime = chrono::DateTime<Utc>;
type JSON = serde_json::Value;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/nexus/schema.json",
    query_path = "src/nexus/queries/mods.graphql",
    response_derives = "Debug, Clone, Serialize",
    variables_derives = "Debug, Clone, Deserialize",
    skip_serializing_none
)]
struct Mods;

#[allow(
    clippy::derivable_impls,
    reason = "`ModsSort` is auto-generated so default() can't be derived (I think)"
)]
impl Default for ModsSort {
    fn default() -> Self {
        Self {
            relevance: None,
            name: None,
            downloads: None,
            unique_downloads: None,
            endorsements: None,
            random: None,
            created_at: None,
            updated_at: None,
            size: None,
            last_comment: None,
        }
    }
}
#[allow(
    clippy::derivable_impls,
    reason = "`ModsFilter` is auto-generated so default() can't be derived (I think)"
)]
impl Default for ModsFilter {
    fn default() -> Self {
        Self {
            filter: None,
            op: None,
            name: None,
            name_stemmed: None,
            mod_id: None,
            id: None,
            game_id: None,
            game_domain_name: None,
            created_at: None,
            updated_at: None,
            has_updated: None,
            uploader_id: None,
            adult_content: None,
            file_size: None,
            downloads: None,
            endorsements: None,
            tag: None,
            description: None,
            author: None,
            uploader: None,
            supports_vortex: None,
            language_name: None,
            category_name: None,
            status: None,
            game_name: None,
            primary_image: None,
            direct_download_enabled: None,
        }
    }
}

impl From<crate::nexus::queries::ModFragment> for patchwork_core::mod_stuff::Mod {
    fn from(value: crate::nexus::queries::ModFragment) -> Self {
        let meta = patchwork_core::mod_stuff::ModMeta {
            name: Some(value.name),
            author: value.author,
            summary: Some(value.summary),
            tags: None,
            last_updated: Some(value.updated_at),
            download_count: Some(value.downloads as u64),
            // FIXME: Include verisons here
            versions: None,
            selected_version: None,
            thumbnail_image_link: value.picture_url,
            // TODO: Add rating support
            positive_ratings: None,
            negative_ratings: None,
        };
        patchwork_core::mod_stuff::Mod {
            meta,
            id: value.id,
            provider_name: "Nexus".to_string(),
        }
    }
}

impl From<SortOrder> for ModsSort {
    fn from(value: SortOrder) -> Self {
        let direction = if value.reversed {
            mods::SortDirection::DESC
        } else {
            mods::SortDirection::ASC
        };
        match value.sort_type.as_str() {
            "relevance" => Self {
                relevance: Some(mods::BaseSortValue { direction }),
                ..Default::default()
            },
            "name" => Self {
                name: Some(mods::BaseSortValue { direction }),
                ..Default::default()
            },
            _ => todo!(),
        }
    }
}

pub async fn fetch_mods(
    filter: Option<ModsFilter>,
    sort: Option<Vec<ModsSort>>,
    offset: Option<i64>,
    count: Option<i64>,
    game_domain_name: String,
) -> Result<Page<patchwork_core::mod_stuff::Mod>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let empty_filter: mods::ModsFilter =
        serde_json::from_value(serde_json::json!({})).expect("Failed to create empty filter");

    let test_filter = ModsFilter {
        game_domain_name: Some(vec![BaseFilterValue {
            value: game_domain_name,
            op: Some(mods::FilterComparisonOperator::EQUALS),
        }]),
        ..(filter.unwrap_or(empty_filter))
    };

    let variables = Variables {
        view_uploader_hidden: None,
        view_user_blocked_content: None,
        facets: None,
        filter: Some(test_filter),
        post_filter: None,
        sort,
        offset,
        count,
    };

    let request_body = Mods::build_query(variables);
    println!("Request: {:#?}", request_body);

    let response = client
        .post("https://api.nexusmods.com/v2/graphql")
        // TODO: do authorization
        // .header("Authorization", "Bearer TOKEN")
        .json(&request_body)
        .send()
        .await?;

    // println!(
    //     "Received response from Nexus: {:#?}",
    //     response.text().await.unwrap()
    // );
    let response_body: Response<ResponseData> = response.json().await?;

    if let Some(errors) = response_body.errors {
        return Err(format!("GraphQL errors: {:?}", errors).into());
    }

    let mods: Vec<patchwork_core::mod_stuff::Mod> = response_body
        .data
        .unwrap()
        .mods
        .nodes
        .into_iter()
        .map(|x| x.into())
        .collect();

    let mod_len = mods.len().clone();
    let page = Page {
        items: mods,
        total: mod_len,
    };
    Ok(page)
}
