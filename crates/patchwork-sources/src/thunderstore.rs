use async_trait::async_trait;
use jiff::Timestamp;
use patchwork_core::Result;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ModSpec, ResolvedMod, Source, SourceContext,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const API: &str = "https://thunderstore.io/api/experimental";

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
            Some(ModSpec {
                source: "thunderstore".to_string(),
                id: name,
                version: Some(version),
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
            })?;

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
}

async fn get_json<T: DeserializeOwned>(url: String) -> Result<T> {
    let reqwest_error = |result| patchwork_core::Error::Reqwest {
        url: url.clone(),
        result,
    };
    let bytes = reqwest::get(&url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(reqwest_error)?
        .bytes()
        .await
        .map_err(reqwest_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn get_thunderstore_package(id: &str, version: Option<&str>) -> Result<PackageVersion> {
    let (namespace, name) = id
        .split_once('-')
        .ok_or_else(|| error(format!("`{id}` should look like `Namespace-Name`")))?;
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
            vec![ModSpec {
                source: "thunderstore".to_owned(),
                id: "BepInEx-BepInExPack".to_owned(),
                version: Some("5.4.2100".to_owned()),
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
