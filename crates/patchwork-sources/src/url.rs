use async_trait::async_trait;
use patchwork_core::links::{self, HostLink};
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, Download, ResolvedMod, Source, SourceContext, is_archive_name,
};
use patchwork_core::{Error, Result};

/// Mods downloaded straight from a URL, as in `url:https://example.com/mod.zip`.
/// A `.zip`, `.7z` or `.rar` is unpacked; anything else is deployed as a
/// single file.
///
/// Links to a file host's page about a file (Dropbox, Google Drive,
/// MediaFire, ModDB; see [`patchwork_core::links`]) work too: the mod's id
/// is the page's link, and each download follows it to wherever the file
/// is then. Hosts that turn patchwork away leave the download to the user.
///
/// A URL is checked for a file when it's resolved (a web page is turned
/// down), but nothing checks whether what's at it changes, other than the
/// lockfile's hash.
pub struct UrlSource;

fn error(message: String) -> Error {
    Error::Source {
        source_id: "url".to_owned(),
        message,
    }
}

/// What to call a file in a profile: its name without the extension.
fn stem(file_name: &str) -> &str {
    file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem)
}

fn format_of(file_name: &str) -> ArtifactFormat {
    if is_archive_name(file_name) {
        ArtifactFormat::Archive
    } else {
        ArtifactFormat::File
    }
}

fn resolved(
    id: String,
    name: String,
    artifact: ArtifactLocation,
    format: ArtifactFormat,
) -> ResolvedMod {
    ResolvedMod {
        id,
        name,
        version: None,
        artifact,
        format,
        dependencies: Vec::new(),
        soft_dependencies: Vec::new(),
        game_versions: None,
        env: Default::default(),
        kind: None,
    }
}

/// Starts downloading `url` (then stops) to check there's a file there, so
/// a link to a web page, like a shared folder or a mod's page, is turned
/// down when it's added rather than when it's deployed.
async fn check_file(url: &str) -> Result<()> {
    let response = patchwork_core::http_get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|result| Error::Reqwest {
            url: url.to_owned(),
            result,
        })?;
    if patchwork_core::is_web_page(&response) {
        return Err(error(format!(
            "{url} is a web page, not a file; link to the file itself (if it's a folder, \
             open it and copy the link to the file you want)"
        )));
    }
    Ok(())
}

/// A link on a file host. Its file is a [`ArtifactLocation::Source`] one,
/// since the host's download links expire.
async fn resolve_hosted(hosted: HostLink) -> Result<ResolvedMod> {
    let file = if hosted.browser_only() {
        hosted.file.clone()
    } else {
        match hosted.file_name().await {
            Ok(file) => Some(file),
            Err(e) if links::refused(&e) => hosted.file.clone(),
            Err(e) => return Err(e),
        }
    };
    let (name, file_name, format) = match file {
        Some(file) => (stem(&file).to_owned(), file.clone(), format_of(&file)),
        None => {
            let format = hosted.format().ok_or_else(|| {
                error(format!(
                    "{} won't let patchwork see what file {} is; download it yourself and \
                     add it as a local file",
                    hosted.host_name(),
                    hosted.link
                ))
            })?;
            // the last part of a page's link is usually the file's title
            let title = ArtifactLocation::Url {
                url: hosted.link.trim_end_matches('/').to_owned(),
            }
            .file_name();
            (title.clone(), title, format)
        }
    };
    if name.is_empty() {
        return Err(error(format!("couldn't tell what file {} is", hosted.link)));
    }
    let artifact = ArtifactLocation::Source {
        file: hosted.link.clone(),
        name: file_name,
    };
    Ok(resolved(hosted.link, name, artifact, format))
}

#[async_trait]
impl Source for UrlSource {
    fn id(&self) -> &str {
        "url"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        if version.is_some() {
            return Err(error("direct downloads don't have versions".to_owned()));
        }
        if !(id.starts_with("https://") || id.starts_with("http://")) {
            return Err(error(format!("`{id}` isn't an http(s) URL")));
        }
        if let Some(hosted) = ctx.links.find(id) {
            return resolve_hosted(hosted).await;
        }
        let artifact = ArtifactLocation::Url { url: id.to_owned() };
        let file_name = artifact.file_name();
        let name = stem(&file_name);
        if name.is_empty() {
            return Err(error(format!("{id} doesn't end in a file name")));
        }
        check_file(id).await?;
        let format = format_of(&file_name);
        Ok(resolved(id.to_owned(), name.to_owned(), artifact, format))
    }

    async fn download(&self, ctx: &SourceContext<'_>, _id: &str, file: &str) -> Result<Download> {
        let hosted = ctx.links.find(file).ok_or_else(|| {
            error(format!(
                "no file host patchwork knows has {file} any more; add the mod again"
            ))
        })?;
        let needs_user = || Download::NeedsUser {
            page: hosted.link.clone(),
        };
        if hosted.browser_only() {
            return Ok(needs_user());
        }
        match hosted.direct().await {
            Ok(direct) => Ok(Download::Url(direct.url)),
            Err(e) if links::refused(&e) => Ok(needs_user()),
            Err(e) => Err(e),
        }
    }

    fn mod_url(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        _version: Option<&str>,
    ) -> Option<String> {
        Some(id.to_owned())
    }
}

#[cfg(test)]
mod tests;
