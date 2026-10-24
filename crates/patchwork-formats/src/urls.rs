use std::collections::HashSet;

use async_trait::async_trait;
use patchwork_core::Result;
use patchwork_core::event::Event;
use patchwork_core::lock::mod_key;
use patchwork_core::profile::ProfileMod;
use patchwork_core::share::{FormatContext, ProfileFormat, SharedProfile};
use patchwork_core::source::ModSpec;

const ID: &str = "urls";
/// The source that takes any download link, used when no other source
/// recognizes a URL.
const FALLBACK: &str = "url";

fn error(message: impl Into<String>) -> patchwork_core::Error {
    crate::error(ID, message)
}

/// A comma-separated list of links to each mod's page (or download, for
/// mods added by URL). Easy to paste anywhere, but it only carries enabled
/// mods, and only pins versions where the source's URLs can.
pub struct UrlList;

#[async_trait]
impl ProfileFormat for UrlList {
    fn id(&self) -> &str {
        ID
    }

    fn description(&self) -> &str {
        "a comma-separated list of mod links"
    }

    fn extension(&self) -> Option<&str> {
        None
    }

    fn detect(&self, input: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(input) else {
            return false;
        };
        let mut links = split(text).peekable();
        links.peek().is_some()
            && links.all(|l| l.starts_with("https://") || l.starts_with("http://"))
    }

    async fn export(&self, ctx: &FormatContext<'_>, shared: &SharedProfile) -> Result<Vec<u8>> {
        let mut urls = Vec::new();
        for m in &shared.mods {
            if !m.enabled {
                ctx.warn(format!("{} is disabled; skipped", m.name));
                continue;
            }
            let Some((source, source_ctx)) = ctx.source(&m.source)? else {
                ctx.warn(format!("{} is from an unknown source; skipped", m.name));
                continue;
            };
            let Some(url) = source.mod_url(&source_ctx, &m.id, m.version.as_deref()) else {
                ctx.warn(format!(
                    "{} is a {} mod, which can't be shared as a link; skipped",
                    m.name, m.source
                ));
                continue;
            };
            if let Ok(Some(spec)) = source.parse_url(&source_ctx, &url)
                && spec.version != m.version
                && let Some(version) = &m.version
            {
                ctx.warn(format!(
                    "{}'s link can't pin version {version}; it'll import as the latest",
                    m.name
                ));
            }
            urls.push(url);
        }
        if urls.is_empty() {
            return Err(error("none of the profile's mods can be shared as links"));
        }
        Ok(format!("{}\n", urls.join(",")).into_bytes())
    }

    async fn import(&self, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile> {
        let text = std::str::from_utf8(input).map_err(|_| error("not text"))?;
        let mut seen = HashSet::new();
        let mut mods = Vec::new();
        for url in split(text) {
            let spec = recognize(ctx, url)?;
            let (source, source_ctx) = ctx.source(&spec.source)?.expect("recognized by it");
            ctx.reporter.report(Event::Resolving {
                name: url.to_owned(),
            });
            // Resolved now for a proper name, and so bad links show up early.
            let resolved = source
                .resolve(&source_ctx, &spec.id, spec.version.as_deref())
                .await?;
            if !seen.insert(mod_key(&spec.source, &resolved.id)) {
                continue;
            }
            mods.push(ProfileMod {
                name: resolved.name,
                source: spec.source,
                id: resolved.id,
                version: spec.version,
                enabled: true,
                game_versions: None,
            });
        }
        if mods.is_empty() {
            return Err(error("no links given"));
        }
        Ok(SharedProfile {
            mods,
            ..Default::default()
        })
    }
}

fn split(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
}

/// Asks each source whether `url` is one of theirs, falling back to a plain
/// download.
fn recognize(ctx: &FormatContext<'_>, url: &str) -> Result<ModSpec> {
    for id in ctx.sources.keys().filter(|id| *id != FALLBACK) {
        let (source, source_ctx) = ctx.source(id)?.expect("listed");
        if let Some(spec) = source.parse_url(&source_ctx, url)? {
            return Ok(spec);
        }
    }
    let is_http = url.starts_with("https://") || url.starts_with("http://");
    if is_http && ctx.sources.contains_key(FALLBACK) {
        return Ok(ModSpec {
            source: FALLBACK.to_owned(),
            id: url.to_owned(),
            version: None,
        });
    }
    Err(error(format!("{url} isn't a link patchwork recognizes")))
}
