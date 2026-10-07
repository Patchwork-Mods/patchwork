use async_trait::async_trait;
use jiff::Timestamp;
use jiff::civil::DateTime;
use patchwork_core::Result;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ModSpec, ResolvedMod, Source, SourceContext,
};
use serde::{Deserialize, Serialize};

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
    pub rating_score: Option<String>,
    pub is_pinned: Option<bool>,
    pub is_deprecated: Option<bool>,
    pub total_downloads: Option<String>,
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
    pub downloads: Option<u32>,
    pub date_created: Option<DateTime>,
    pub website_url: String,
    pub is_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageListingExperimental {
    pub has_nsfw_content: Option<bool>,
    pub categories: Option<String>,
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

// converts a thunderstore mod to our internal version of a mod
fn thunderstore_to_resolved_mod(mod_: PackageVersion) -> Option<ResolvedMod> {
    let artifact = ArtifactLocation::Url {
        url: mod_.package.package_url?,
    };
    let dependencies = mod_
        .version
        .dependencies
        .unwrap_or_default()
        .iter()
        .map(|dep| {
            let ModNameVersion { name, version } = decode_mod_version_name(dep)?;
            Some(ModSpec {
                source: "thunderstore".to_string(),
                id: name,
                version: Some(version),
            })
        })
        .collect::<Option<Vec<_>>>()?;

    Some(ResolvedMod {
        id: mod_.package.name.clone(),
        name: mod_.package.full_name.unwrap_or(mod_.package.name),
        version: None,
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

// --- End API Responses ---

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    community: String,
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
        let config: Config = ctx
            .config
            .cloned()
            .ok_or_else(|| patchwork_core::Error::Source {
                source_id: id.to_string(),
                message: format!(
                    "{} isn't on Thunderstore (no [sources.thunderstore] in its definition)",
                    ctx.game.id
                ),
            })?
            .try_into()
            .map_err(|e| patchwork_core::Error::Source {
                source_id: id.to_string(),
                message: format!("bad [sources.thunderstore] for {}: {e}", ctx.game.id),
            })?;

        let response = get_thunderstore_package(id, &config.community, version).await?;
        thunderstore_to_resolved_mod(response.clone()).ok_or(patchwork_core::Error::Source {
            source_id: "thunderstore".to_string(),
            message: format!("Could not resolve thunderstore mod: {response:?}"),
        })
    }
}

async fn get_thunderstore_package(
    id: &str,
    community: &str,
    version: Option<&str>,
) -> Result<PackageVersion> {
    let url = format!(
        "https://thunderstore.io/api/experimental/package/{}/{}",
        community, id
    );
    let response = reqwest::get(url.clone())
        .await
        .map_err(|e| patchwork_core::Error::Reqwest {
            url: url.clone(),
            result: e,
        })?;
    let package: PackageExperimental =
        serde_json::from_slice(&response.bytes().await.map_err(|result| {
            patchwork_core::Error::Reqwest {
                url: url.clone(),
                result,
            }
        })?)?;

    // kinda wasteful to do the first call if we only need the version package, but who cares
    let version_package = if let Some(v) = version {
        let url = format!("{url}/{v}");
        let response =
            reqwest::get(url.clone())
                .await
                .map_err(|e| patchwork_core::Error::Reqwest {
                    url: url.clone(),
                    result: e,
                })?;
        serde_json::from_slice(
            &response
                .bytes()
                .await
                .map_err(|result| patchwork_core::Error::Reqwest { url, result })?,
        )?
    } else {
        todo! {}
    };
    Ok(PackageVersion {
        package,
        version: version_package,
    })
}
