//! Links to files on file hosts like Dropbox, Google Drive and MediaFire,
//! whose links go to a page about the file rather than the file itself.
//!
//! Hosts are registry data (`hosts/<id>.toml`, see `registry/README.md`),
//! not code, so one that changes its pages can be fixed with a registry
//! update, and any source can use them through [`SourceContext::links`]:
//! [`Links::find`] tells whether a link is on a known host, and the
//! [`HostLink`] it gives says how to get the file right now. The `url`
//! source is the main user, but anything that comes across links on other
//! sites (a forum post, a mod's description) can use the same hosts.
//!
//! [`SourceContext::links`]: crate::source::SourceContext::links

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use regex::{Captures, Regex};
use serde::Deserialize;

use crate::registry::Registry;
use crate::source::{ArtifactFormat, ArtifactLocation};
use crate::{Error, Result};

/// The registry kind hosts are under.
pub const KIND: &str = "hosts";

/// Every known file host, from the registry. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Links {
    hosts: Arc<[Arc<Host>]>,
}

impl Links {
    /// No hosts at all, for tests and callers without a registry.
    pub fn none() -> &'static Links {
        static NONE: std::sync::LazyLock<Links> = std::sync::LazyLock::new(Links::default);
        &NONE
    }

    /// The hosts in `registry`'s `hosts` entries, in id order, with a
    /// problem for each that isn't valid.
    pub fn from_registry(registry: &Registry) -> (Links, Vec<Error>) {
        let mut hosts = Vec::new();
        let mut problems = Vec::new();
        for id in registry.data_ids(KIND) {
            let Some(entry) = registry.data(KIND, id) else {
                continue;
            };
            match Host::parse(id, &entry.value) {
                Ok(host) => hosts.push(Arc::new(host)),
                Err(message) => problems.push(Error::invalid(format!("host `{id}`: {message}"))),
            }
        }
        (
            Links {
                hosts: hosts.into(),
            },
            problems,
        )
    }

    /// Parses hosts straight from TOML, as `(id, text)`, without a registry.
    pub fn parse(hosts: &[(&str, &str)]) -> Result<Links> {
        let hosts = hosts
            .iter()
            .map(|(id, text)| {
                let table: toml::Table = toml::from_str(text)
                    .map_err(|e| Error::invalid(format!("host `{id}`: {}", e.message())))?;
                Host::parse(id, &table)
                    .map(Arc::new)
                    .map_err(|message| Error::invalid(format!("host `{id}`: {message}")))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Links {
            hosts: hosts.into(),
        })
    }

    /// Which host `url` is on, if any knows it. Doesn't fetch anything.
    pub fn find(&self, url: &str) -> Option<HostLink> {
        let url = url.trim();
        self.hosts.iter().find_map(|host| host.find(url))
    }
}

/// A link to a file on a host, from [`Links::find`].
#[derive(Debug, Clone)]
pub struct HostLink {
    host: Arc<Host>,
    /// The link the same file always has on this host, which is what to
    /// keep in profiles. It's the link as given if the host doesn't say.
    pub link: String,
    /// The file's name, if the link says.
    pub file: Option<String>,
    vars: BTreeMap<String, String>,
}

/// Where a host's file can be downloaded right now, from
/// [`HostLink::direct`]. These links often expire, so they aren't kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Direct {
    pub url: String,
    /// The file's name, if the host's pages say.
    pub file: Option<String>,
}

impl HostLink {
    /// The host's id, like `mediafire`.
    pub fn host_id(&self) -> &str {
        &self.host.id
    }

    /// What to call the host, like `MediaFire`.
    pub fn host_name(&self) -> &str {
        &self.host.name
    }

    /// Whether only the user's browser can download from the host, which
    /// turns patchwork away.
    pub fn browser_only(&self) -> bool {
        self.host.browser
    }

    /// What its files usually are, for when the name can't be found out.
    pub fn format(&self) -> Option<ArtifactFormat> {
        self.host.format
    }

    /// Follows the host's pages to where the file can be downloaded now.
    pub async fn direct(&self) -> Result<Direct> {
        let mut vars = self.vars.clone();
        let mut file = self.file.clone();
        let mut found: Option<String> = None;
        for step in &self.host.steps {
            let page = match step.fetch.expand(&vars) {
                Some(page) => page,
                None => found.clone().unwrap_or_else(|| self.link.clone()),
            };
            let (final_url, body) = fetch_page(&page).await?;
            if let Some(name) = &step.name
                && let Some(caps) = name.captures(&body)
                && let Some(m) = caps.name("file").or_else(|| caps.get(1))
            {
                file = Some(unescape_html(m.as_str().trim()));
            }
            let next = step
                .find
                .iter()
                .find_map(|re| re.captures(&body).and_then(|caps| found_url(&caps)))
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "couldn't find the download on {page}; {} may have changed its pages",
                        self.host.name
                    ))
                })?;
            let next = reqwest::Url::parse(&final_url)
                .and_then(|base| base.join(&next))
                .map_err(|e| {
                    Error::invalid(format!("{} links to `{next}`: {e}", self.host.name))
                })?;
            found = Some(next.to_string());
            vars.insert("found".to_owned(), next.to_string());
        }
        let url = self
            .host
            .download
            .expand(&vars)
            .or(found)
            .unwrap_or_else(|| self.link.clone());
        let url = with_query(&url, &self.host.query)?;
        Ok(Direct { url, file })
    }

    /// The file's name: from the link or the host's pages if they say, or
    /// else from what the host sends when the download is started (which is
    /// then stopped). Fails if the host sends a web page instead of a file,
    /// which is what they do when a file needs a sign-in or has run out of
    /// downloads for the day.
    pub async fn file_name(&self) -> Result<String> {
        if let Some(file) = &self.file {
            return Ok(file.clone());
        }
        let direct = self.direct().await?;
        if let Some(file) = direct.file {
            return Ok(file);
        }
        let url = &direct.url;
        let response = crate::http_get(url)
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|result| Error::Reqwest {
                url: url.clone(),
                result,
            })?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        if crate::is_web_page(&response) {
            return Err(Error::invalid(format!(
                "{} sent a web page instead of the file at {}; it may need a sign-in, or be \
                 out of downloads for now",
                self.host.name, self.link
            )));
        }
        let name = header(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|d| disposition_file_name(&d))
            .unwrap_or_else(|| {
                ArtifactLocation::Url {
                    url: response.url().to_string(),
                }
                .file_name()
            });
        if name.is_empty() {
            return Err(Error::invalid(format!(
                "couldn't tell what file {} is",
                self.link
            )));
        }
        Ok(name)
    }
}

/// Whether `err` is a host turning patchwork away, as sites that only want
/// browsers do, so the user has to download the file themselves.
pub fn refused(err: &Error) -> bool {
    use reqwest::StatusCode;
    matches!(err, Error::Reqwest { result, .. } if matches!(
        result.status(),
        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS)
    ))
}

#[derive(Debug)]
struct Host {
    id: String,
    name: String,
    links: Vec<Regex>,
    link: Templates,
    file: Templates,
    steps: Vec<Step>,
    download: Templates,
    query: BTreeMap<String, String>,
    format: Option<ArtifactFormat>,
    browser: bool,
}

#[derive(Debug)]
struct Step {
    fetch: Templates,
    find: Vec<Regex>,
    name: Option<Regex>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostDef {
    name: String,
    links: OneOrMany,
    #[serde(default)]
    link: OneOrMany,
    #[serde(default)]
    file: OneOrMany,
    #[serde(default)]
    steps: Vec<StepDef>,
    #[serde(default)]
    download: OneOrMany,
    #[serde(default)]
    query: BTreeMap<String, String>,
    #[serde(default)]
    format: Option<ArtifactFormat>,
    #[serde(default)]
    browser: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepDef {
    #[serde(default)]
    fetch: OneOrMany,
    find: OneOrMany,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum OneOrMany {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::None => Vec::new(),
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

fn regexes(patterns: OneOrMany) -> std::result::Result<Vec<Regex>, String> {
    patterns
        .into_vec()
        .iter()
        .map(|p| Regex::new(p).map_err(|e| e.to_string()))
        .collect()
}

impl Host {
    fn parse(id: &str, table: &toml::Table) -> std::result::Result<Host, String> {
        let def: HostDef = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| e.message().to_owned())?;
        let links = regexes(def.links)?;
        if links.is_empty() {
            return Err("`links` is empty".to_owned());
        }
        let steps = def
            .steps
            .into_iter()
            .map(|s| {
                let find = regexes(s.find)?;
                if find.is_empty() {
                    return Err("a step's `find` is empty".to_owned());
                }
                Ok(Step {
                    fetch: Templates(s.fetch.into_vec()),
                    find,
                    name: s
                        .name
                        .map(|n| Regex::new(&n).map_err(|e| e.to_string()))
                        .transpose()?,
                })
            })
            .collect::<std::result::Result<_, String>>()?;
        if def.format == Some(ArtifactFormat::Dir) {
            return Err("`format` can't be `dir`".to_owned());
        }
        Ok(Host {
            id: id.to_owned(),
            name: def.name,
            links,
            link: Templates(def.link.into_vec()),
            file: Templates(def.file.into_vec()),
            steps,
            download: Templates(def.download.into_vec()),
            query: def.query,
            format: def.format,
            browser: def.browser,
        })
    }

    fn find(self: &Arc<Self>, url: &str) -> Option<HostLink> {
        let caps = self.links.iter().find_map(|re| re.captures(url))?;
        let mut vars: BTreeMap<String, String> = self
            .links
            .iter()
            .flat_map(|re| re.capture_names().flatten())
            .filter_map(|name| Some((name.to_owned(), caps.name(name)?.as_str().to_owned())))
            .collect();
        vars.insert("url".to_owned(), url.to_owned());
        let link = self.link.expand(&vars).unwrap_or_else(|| url.to_owned());
        vars.insert("link".to_owned(), link.clone());
        let file = self
            .file
            .expand(&vars)
            .map(|f| ArtifactLocation::Url { url: f }.file_name())
            .filter(|f| !f.is_empty());
        Some(HostLink {
            host: self.clone(),
            link,
            file,
            vars,
        })
    }
}

/// Alternatives for a value with `${var}`s in it: the first whose
/// variables all have values is used.
#[derive(Debug, Default)]
struct Templates(Vec<String>);

impl Templates {
    fn expand(&self, vars: &BTreeMap<String, String>) -> Option<String> {
        self.0.iter().find_map(|t| expand(t, vars))
    }
}

/// `template` with each `${var}` replaced, or `None` if one has no value.
fn expand(template: &str, vars: &BTreeMap<String, String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let value = vars.get(&rest[start + 2..end])?;
        if value.is_empty() {
            return None;
        }
        out.push_str(value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The link a `find` pattern caught: its `url` group, its base64 `b64`
/// group decoded, or its first group.
fn found_url(caps: &Captures<'_>) -> Option<String> {
    if let Some(b64) = caps.name("b64") {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.as_str().trim())
            .ok()?;
        return String::from_utf8(bytes).ok();
    }
    let m = caps.name("url").or_else(|| caps.get(1))?;
    Some(unescape_html(m.as_str()))
}

/// GETs a page, giving where it ended up after redirects and its text.
async fn fetch_page(url: &str) -> Result<(String, String)> {
    let reqwest_error = |result| Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let response = crate::http_get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(reqwest_error)?;
    let final_url = response.url().to_string();
    let body = response.text().await.map_err(reqwest_error)?;
    Ok((final_url, body))
}

/// `url` with each of `query`'s parameters set, replacing any it had.
fn with_query(url: &str, query: &BTreeMap<String, String>) -> Result<String> {
    if query.is_empty() {
        return Ok(url.to_owned());
    }
    let mut parsed = reqwest::Url::parse(url)
        .map_err(|e| Error::invalid(format!("`{url}` isn't a link: {e}")))?;
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !query.contains_key(k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .extend_pairs(query);
    Ok(parsed.to_string())
}

/// Decodes the few entities that turn up in links in HTML attributes.
fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let entity = &rest[1..end];
            let c = match entity {
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                "lt" => '<',
                "gt" => '>',
                _ => {
                    let code = match entity.strip_prefix("#x").or(entity.strip_prefix("#X")) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => entity.strip_prefix('#')?.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The file name in a `Content-Disposition` header, preferring the UTF-8
/// `filename*` over the plain `filename`.
fn disposition_file_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                // charset'language'percent-encoded
                let encoded = value.trim().splitn(3, '\'').nth(2)?;
                let name = ArtifactLocation::Url {
                    url: encoded.to_owned(),
                }
                .file_name();
                if !name.is_empty() {
                    return Some(name);
                }
            }
            "filename" => {
                let name = value.trim().trim_matches('"');
                let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
                if !name.is_empty() {
                    plain = Some(name.to_owned());
                }
            }
            _ => {}
        }
    }
    plain
}

#[cfg(test)]
mod tests;
