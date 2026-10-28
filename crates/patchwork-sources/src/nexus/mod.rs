use async_trait::async_trait;
use patchwork_core::Result;
use patchwork_core::changelog::ChangelogEntry;
use patchwork_core::compat::{self, GameVersions};
use patchwork_core::game::GameDef;
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::settings::{SettingKind, SettingSpec};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Download, Handoff, ModSpec, ResolvedMod, Source,
    SourceContext, compare_dotted, url_parts,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

mod search;
#[cfg(test)]
mod tests;

const API: &str = "https://api.nexusmods.com/v1";

const API_KEY: SettingSpec = SettingSpec::new("api_key", "API key", SettingKind::Secret)
    .help(
        "Needed for anything but searching. Only Premium accounts can download \
         through it; without Premium, downloads start on the mod's page.",
    )
    .link("https://www.nexusmods.com/settings/api-keys")
    .env("NEXUS_API_KEY");

/// Mods from Nexus Mods. Ids are the numeric mod id from the mod's page, e.g.
/// `nexus:266` for nexusmods.com/skyrimspecialedition/mods/266. Versions are
/// matched against the version of the mod's files.
///
/// Everything but searching needs a personal API key, the `api_key` setting
/// (or `NEXUS_API_KEY`; get one from
/// https://www.nexusmods.com/settings/api-keys). Only premium accounts can
/// download through the API; everyone else is sent to the file's page to
/// start the download there.
pub struct Nexus {
    client: reqwest::Client,
}

impl Default for Nexus {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("patchwork/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client should build");
        Nexus { client }
    }
}

// --- API Responses ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusUser {
    pub name: String,
    #[serde(default)]
    pub is_premium: bool,
}

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
    pub description: Option<String>,
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

/// A mod's changelogs: each version's notes as a list of lines, in the
/// order Nexus sends them, which is kept because versions are free text.
#[derive(Debug, Clone, Default)]
pub struct NexusChangelogs(pub Vec<(String, Vec<String>)>);

impl<'de> Deserialize<'de> for NexusChangelogs {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = NexusChangelogs;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of versions to lists of changes")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut versions = Vec::new();
                while let Some(entry) = map.next_entry::<String, Vec<String>>()? {
                    versions.push(entry);
                }
                Ok(NexusChangelogs(versions))
            }
            // mods without any come back as an empty list rather than map
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(NexusChangelogs::default())
            }
        }
        deserializer.deserialize_any(Visitor)
    }
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

/// What an `nxm://` link is for, which the site hands to mod managers when
/// a user clicks "Mod Manager Download". `key` and `expires` are what lets
/// accounts without premium get a download link through the API.
#[derive(Debug, PartialEq, Eq)]
struct NxmLink<'a> {
    domain: &'a str,
    id: &'a str,
    file: &'a str,
    key: &'a str,
    expires: &'a str,
}

/// Reads links like
/// `nxm://skyrimspecialedition/mods/12604/files/2000?key=..&expires=..&user_id=..`.
fn parse_nxm(link: &str) -> Option<NxmLink<'_>> {
    let rest = link.strip_prefix("nxm://")?;
    let (path, query) = rest.split_once('?')?;
    let [domain, "mods", id, "files", file] = path.split('/').collect::<Vec<_>>()[..] else {
        return None;
    };
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    };
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !numeric(id) || !numeric(file) {
        return None;
    }
    Some(NxmLink {
        domain,
        id,
        file,
        key: param("key")?,
        expires: param("expires")?,
    })
}

/// The page where a user can download `file` of mod `id` themselves.
fn file_page(domain: &str, id: &str, file: &str) -> String {
    format!("https://www.nexusmods.com/{domain}/mods/{id}?tab=files&file_id={file}")
}

/// Picks which of a mod's files `version` refers to. Without a version,
/// files that `fit` the game best win, then main files over optional ones,
/// then the newest upload; old and archived files are only considered when
/// a version is asked for.
fn pick_file<'a>(
    files: &'a [NexusFile],
    version: Option<&str>,
    fit: impl Fn(&NexusFile) -> u8,
) -> Option<&'a NexusFile> {
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
        .max_by_key(|f| {
            let fit = if version.is_some() { 0 } else { fit(f) };
            (
                fit,
                f.is_primary.unwrap_or(false),
                rank(f),
                f.uploaded_timestamp,
            )
        })
}

/// Which versions of the game `file` is for, going by the words in its
/// name and description, since Nexus has nowhere to say.
fn game_versions(game: &GameDef, file: &NexusFile) -> Option<GameVersions> {
    let texts = [
        file.name.as_deref(),
        Some(file.file_name.as_str()),
        file.description.as_deref(),
    ];
    compat::guess(game, texts.into_iter().flatten())
}

/// How well `file` suits the installed game: 2 if it says it's for this
/// version, 0 if it says it's for others, and 1 if it can't be told.
fn fit(game: &GameDef, installed: Option<&str>, file: &NexusFile) -> u8 {
    let matches = installed.and_then(|v| game_versions(game, file)?.matches(v));
    match matches {
        Some(true) => 2,
        None => 1,
        Some(false) => 0,
    }
}

/// The distinct versions among `files`, newest upload first. Nexus versions
/// are free text, so upload time is the only reliable order.
fn file_versions(mut files: Vec<NexusFile>) -> Vec<String> {
    files.sort_by_key(|f| std::cmp::Reverse(f.uploaded_timestamp));
    let mut versions: Vec<String> = Vec::new();
    for v in files.into_iter().filter_map(|f| f.version) {
        if !v.is_empty() && !versions.contains(&v) {
            versions.push(v);
        }
    }
    versions
}

/// Turns a mod's changelogs into entries, newest first: by version number
/// if every version is one, or else the reverse of the order Nexus lists
/// them in, which is oldest first.
fn changelog_entries(changelogs: NexusChangelogs) -> Vec<ChangelogEntry> {
    let mut versions = changelogs.0;
    let numbered = versions
        .iter()
        .all(|(v, _)| compare_dotted(v, "0").is_some());
    if numbered {
        versions.sort_by(|(a, _), (b, _)| compare_dotted(b, a).unwrap());
    } else {
        versions.reverse();
    }
    versions
        .into_iter()
        .map(|(version, lines)| ChangelogEntry {
            version: Some(version),
            notes: lines
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .collect()
}

// converts a nexus mod file to our internal version of a mod
fn nexus_to_resolved_mod(game: &GameDef, mod_: &NexusMod, file: &NexusFile) -> ResolvedMod {
    let id = mod_.mod_id.to_string();
    ResolvedMod {
        name: mod_.name.clone().unwrap_or_else(|| id.clone()),
        id,
        version: file.version.clone(),
        // download links expire, so they're asked for at download time
        artifact: ArtifactLocation::Source {
            file: file.file_id.to_string(),
            name: file.file_name.clone(),
        },
        format: ArtifactFormat::Archive,
        // nexus doesn't expose requirements in a machine-readable way
        dependencies: Vec::new(),
        game_versions: game_versions(game, file),
        env: Default::default(),
    }
}

#[async_trait]
impl Source for Nexus {
    fn id(&self) -> &str {
        "nexus"
    }

    fn name(&self) -> &str {
        "Nexus Mods"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[API_KEY]
    }

    async fn check_settings(&self, settings: Option<&toml::Table>) -> Result<Option<String>> {
        let Some(api_key) = api_key(settings) else {
            return Ok(None);
        };
        let url = format!("{API}/users/validate.json");
        let response = self.get(&url, api_key).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(error("Nexus doesn't accept this API key".to_owned()));
        }
        let user: NexusUser = decode(&url, response).await?;
        Ok(Some(if user.is_premium {
            format!("Signed in as {} (Premium)", user.name)
        } else {
            format!(
                "Signed in as {}; without Premium, downloads start on the mod's page",
                user.name
            )
        }))
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let (base, api_key) = self.mod_api(ctx, id)?;
        let mod_: NexusMod = self.get_json(&format!("{base}.json"), api_key).await?;
        let files: NexusFiles = self
            .get_json(&format!("{base}/files.json"), api_key)
            .await?;
        let installed = ctx.env.and_then(|env| env.version.as_deref());
        let file =
            pick_file(&files.files, version, |f| fit(ctx.game, installed, f)).ok_or_else(|| {
                match version {
                    Some(v) => error(format!("{id} has no file with version {v}")),
                    None => error(format!("{id} has no downloadable files")),
                }
            })?;
        if !file.file_name.to_ascii_lowercase().ends_with(".zip") {
            return Err(error(format!(
                "{} is not a zip, and only zips are supported for now",
                file.file_name
            )));
        }
        Ok(nexus_to_resolved_mod(ctx.game, &mod_, file))
    }

    async fn download(&self, ctx: &SourceContext<'_>, id: &str, file: &str) -> Result<Download> {
        let (base, api_key) = self.mod_api(ctx, id)?;
        let url = format!("{base}/files/{file}/download_link.json");
        let response = self.get(&url, api_key).await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            let domain = config(ctx)?.game_domain;
            return Ok(Download::NeedsUser {
                page: file_page(&domain, id, file),
            });
        }
        let links: Vec<NexusDownloadLink> = decode(&url, response).await?;
        Ok(Download::Url(first_link(links, id)?))
    }

    /// Takes the `nxm://` links the site hands out for "Mod Manager
    /// Download".
    async fn accept_link(&self, ctx: &SourceContext<'_>, link: &str) -> Result<Option<Handoff>> {
        let Some(nxm) = parse_nxm(link) else {
            return Ok(None);
        };
        if !nxm.domain.eq_ignore_ascii_case(&config(ctx)?.game_domain) {
            return Ok(None);
        }
        let (base, api_key) = self.mod_api(ctx, nxm.id)?;
        // the key and expiry came from a URL, so they're still encoded
        let url = format!(
            "{base}/files/{}/download_link.json?key={}&expires={}",
            nxm.file, nxm.key, nxm.expires
        );
        let links: Vec<NexusDownloadLink> = self.get_json(&url, api_key).await?;
        Ok(Some(Handoff {
            id: nxm.id.to_owned(),
            file: nxm.file.to_owned(),
            url: first_link(links, nxm.id)?,
        }))
    }

    fn lists_versions(&self) -> bool {
        true
    }

    /// The versions of the mod's files, newest upload first.
    async fn versions(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Vec<String>> {
        let (base, api_key) = self.mod_api(ctx, id)?;
        let files: NexusFiles = self
            .get_json(&format!("{base}/files.json"), api_key)
            .await?;
        Ok(file_versions(files.files))
    }

    /// The mod's changelogs, which are for the whole mod rather than any one
    /// version of it, so `version` makes no difference.
    async fn changelog(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        _version: Option<&str>,
    ) -> Result<Vec<ChangelogEntry>> {
        let (base, api_key) = self.mod_api(ctx, id)?;
        let changelogs: NexusChangelogs = self
            .get_json(&format!("{base}/changelogs.json"), api_key)
            .await?;
        Ok(changelog_entries(changelogs))
    }

    /// Doesn't need an API key.
    async fn summary(&self, ctx: &SourceContext<'_>, id: &str) -> Result<Option<ModSummary>> {
        let mod_id: u64 = id
            .parse()
            .map_err(|_| error(format!("`{id}` should be a numeric Nexus mod id")))?;
        self.mod_summary(&config(ctx)?.game_domain, mod_id).await
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
            return Err(error(format!(
                "{url} is a mod for {domain}, not {expected}"
            )));
        }
        Ok(Some(ModSpec {
            source: "nexus".to_owned(),
            id: id.to_string(),
            version: None,
        }))
    }

    fn search_sorts(&self) -> &[SortBy] {
        &SortBy::ALL
    }

    /// Category ids are their names, e.g. `User Interface`.
    async fn search_categories(&self, ctx: &SourceContext<'_>) -> Result<Vec<Category>> {
        self.categories(&config(ctx)?.game_domain).await
    }

    /// Pages are 20 mods. Without a sort, text searches are by relevance
    /// and everything else is most downloaded first.
    async fn search(&self, ctx: &SourceContext<'_>, query: &SearchQuery) -> Result<SearchPage> {
        self.search_mods(&config(ctx)?.game_domain, query).await
    }
}

impl Nexus {
    /// The API URL for mod `id` in the game's domain, and the API key that
    /// requests for it need.
    fn mod_api<'a>(&self, ctx: &SourceContext<'a>, id: &str) -> Result<(String, &'a str)> {
        let config = config(ctx)?;
        let mod_id: u64 = id
            .parse()
            .map_err(|_| error(format!("`{id}` should be a numeric Nexus mod id")))?;
        let api_key = api_key(ctx.settings).ok_or_else(|| {
            error(
                "Nexus needs an API key; add one in Settings, or with `patchwork config set nexus.api_key <key>` (get one from https://www.nexusmods.com/settings/api-keys)"
                    .to_owned(),
            )
        })?;
        Ok((
            format!("{API}/games/{}/mods/{mod_id}", config.game_domain),
            api_key,
        ))
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str, api_key: &str) -> Result<T> {
        decode(url, self.get(url, api_key).await?).await
    }

    /// Sends an API request, whatever status it gets back.
    async fn get(&self, url: &str, api_key: &str) -> Result<reqwest::Response> {
        self.client
            .get(url)
            .header("apikey", api_key)
            .send()
            .await
            .map_err(|result| patchwork_core::Error::Reqwest {
                url: url.to_owned(),
                result,
            })
    }
}

/// The user's API key, if they've set one.
fn api_key(settings: Option<&toml::Table>) -> Option<&str> {
    settings?.get(API_KEY.key)?.as_str()
}

fn first_link(links: Vec<NexusDownloadLink>, id: &str) -> Result<String> {
    links
        .into_iter()
        .next()
        .map(|link| link.uri)
        .ok_or_else(|| error(format!("Nexus gave no download links for {id}")))
}

/// The JSON body of a successful response to `url`.
async fn decode<T: DeserializeOwned>(url: &str, response: reqwest::Response) -> Result<T> {
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
