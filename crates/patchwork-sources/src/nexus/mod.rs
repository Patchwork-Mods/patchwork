use async_trait::async_trait;
use patchwork_core::Result;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ModSpec, ResolvedMod, Source, SourceContext, url_parts,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

const API: &str = "https://api.nexusmods.com/v1";
const API_KEY_VAR: &str = "NEXUS_API_KEY";

/// Mods from Nexus Mods. Ids are the numeric mod id from the mod's page, e.g.
/// `nexus:266` for nexusmods.com/skyrimspecialedition/mods/266. Versions are
/// matched against the version of the mod's files.
///
/// Every Nexus API call needs a personal API key, which is read from
/// `NEXUS_API_KEY` (get one from https://www.nexusmods.com/settings/api-keys).
pub struct Nexus {
    api_key: Option<String>,
    client: reqwest::Client,
}

impl Nexus {
    pub fn new(api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("patchwork/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client should build");
        Nexus { api_key, client }
    }

    /// Reads the API key from `NEXUS_API_KEY`.
    pub fn from_env() -> Self {
        Self::new(std::env::var(API_KEY_VAR).ok().filter(|k| !k.is_empty()))
    }
}

// --- API Responses ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusMod {
    pub mod_id: u64,
    pub domain_name: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub status: Option<String>,
    pub available: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusFiles {
    pub files: Vec<NexusFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusFile {
    pub file_id: u64,
    pub name: Option<String>,
    pub version: Option<String>,
    pub category_name: Option<FileCategory>,
    pub is_primary: Option<bool>,
    pub file_name: String,
    pub uploaded_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FileCategory {
    Main,
    Update,
    Optional,
    OldVersion,
    Miscellaneous,
    Archived,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusDownloadLink {
    pub name: String,
    pub short_name: String,
    #[serde(rename = "URI")]
    pub uri: String,
}

// --- End API Responses ---

fn error(message: impl Into<String>) -> patchwork_core::Error {
    patchwork_core::Error::Source {
        source_id: "nexus".to_owned(),
        message: message.into(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// The game's slug in nexusmods.com URLs, e.g. `skyrimspecialedition`.
    game_domain: String,
}

fn config(ctx: &SourceContext<'_>) -> Result<Config> {
    ctx.config
        .cloned()
        .ok_or_else(|| {
            error(format!(
                "{} isn't on Nexus (no [sources.nexus] in its definition)",
                ctx.game.id
            ))
        })?
        .try_into()
        .map_err(|e| error(format!("bad [sources.nexus] for {}: {e}", ctx.game.id)))
}

/// Picks which of a mod's files `version` refers to. Main files win over
/// optional ones, then the newest upload wins; old and archived files are
/// only considered when a version is asked for.
fn pick_file<'a>(files: &'a [NexusFile], version: Option<&str>) -> Option<&'a NexusFile> {
    let rank = |f: &NexusFile| match f.category_name {
        Some(FileCategory::Main) => 3,
        Some(FileCategory::Update) => 2,
        Some(FileCategory::Optional) => 1,
        _ => 0,
    };
    files
        .iter()
        .filter(|f| match version {
            Some(v) => f.version.as_deref() == Some(v),
            None => !matches!(
                f.category_name,
                Some(FileCategory::OldVersion | FileCategory::Archived)
            ),
        })
        .max_by_key(|f| (f.is_primary.unwrap_or(false), rank(f), f.uploaded_timestamp))
}

// converts a nexus mod file to our internal version of a mod
fn nexus_to_resolved_mod(mod_: &NexusMod, file: &NexusFile, url: String) -> ResolvedMod {
    let id = mod_.mod_id.to_string();
    ResolvedMod {
        name: mod_.name.clone().unwrap_or_else(|| id.clone()),
        id,
        version: file.version.clone(),
        artifact: ArtifactLocation::Url { url },
        format: ArtifactFormat::Zip,
        // nexus doesn't expose requirements in a machine-readable way
        dependencies: Vec::new(),
    }
}

#[async_trait]
impl Source for Nexus {
    fn id(&self) -> &str {
        "nexus"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let config = config(ctx)?;
        let mod_id: u64 = id
            .parse()
            .map_err(|_| error(format!("`{id}` should be a numeric Nexus mod id")))?;
        let api_key = self.api_key.as_deref().ok_or_else(|| {
            error(format!(
                "Nexus needs an API key; set {API_KEY_VAR} (get one from https://www.nexusmods.com/settings/api-keys)"
            ))
        })?;

        let base = format!("{API}/games/{}/mods/{mod_id}", config.game_domain);
        let mod_: NexusMod = self.get_json(&format!("{base}.json"), api_key).await?;
        let files: NexusFiles = self
            .get_json(&format!("{base}/files.json"), api_key)
            .await?;
        let file = pick_file(&files.files, version).ok_or_else(|| match version {
            Some(v) => error(format!("{id} has no file with version {v}")),
            None => error(format!("{id} has no downloadable files")),
        })?;
        if !file.file_name.to_ascii_lowercase().ends_with(".zip") {
            return Err(error(format!(
                "{} is not a zip, and only zips are supported for now",
                file.file_name
            )));
        }

        let links: Vec<NexusDownloadLink> = self
            .get_json(
                &format!("{base}/files/{}/download_link.json", file.file_id),
                api_key,
            )
            .await?;
        let url = links
            .into_iter()
            .next()
            .ok_or_else(|| error(format!("Nexus gave no download links for {id}")))?
            .uri;
        Ok(nexus_to_resolved_mod(&mod_, file, url))
    }

    /// Mod pages don't pin a version, so it's dropped.
    fn mod_url(&self, ctx: &SourceContext<'_>, id: &str, _version: Option<&str>) -> Option<String> {
        let domain = config(ctx).ok()?.game_domain;
        Some(format!("https://www.nexusmods.com/{domain}/mods/{id}"))
    }

    fn parse_url(&self, ctx: &SourceContext<'_>, url: &str) -> Result<Option<ModSpec>> {
        let Some(("nexusmods.com", segments)) = url_parts(url) else {
            return Ok(None);
        };
        let [domain, "mods", id, ..] = segments.as_slice() else {
            return Ok(None);
        };
        if id.parse::<u64>().is_err() {
            return Ok(None);
        }
        let expected = config(ctx)?.game_domain;
        if *domain != expected {
            return Err(error(format!("{url} is a mod for {domain}, not {expected}")));
        }
        Ok(Some(ModSpec {
            source: "nexus".to_owned(),
            id: id.to_string(),
            version: None,
        }))
    }
}

impl Nexus {
    async fn get_json<T: DeserializeOwned>(&self, url: &str, api_key: &str) -> Result<T> {
        let reqwest_error = |result| patchwork_core::Error::Reqwest {
            url: url.to_owned(),
            result,
        };
        let bytes = self
            .client
            .get(url)
            .header("apikey", api_key)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(reqwest_error)?
            .bytes()
            .await
            .map_err(reqwest_error)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}
