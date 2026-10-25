use async_trait::async_trait;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ResolvedMod, Source, SourceContext, is_archive_name,
};
use patchwork_core::{Error, Result};

/// Mods downloaded straight from a URL, as in `url:https://example.com/mod.zip`.
/// A `.zip`, `.7z` or `.rar` is unpacked; anything else is deployed as a
/// single file.
///
/// Nothing checks whether what's at the URL changes, other than the lockfile's
/// hash.
pub struct UrlSource;

#[async_trait]
impl Source for UrlSource {
    fn id(&self) -> &str {
        "url"
    }

    async fn resolve(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let error = |message: String| Error::Source {
            source_id: "url".to_owned(),
            message,
        };
        if version.is_some() {
            return Err(error("direct downloads don't have versions".to_owned()));
        }
        if !(id.starts_with("https://") || id.starts_with("http://")) {
            return Err(error(format!("`{id}` isn't an http(s) URL")));
        }
        let artifact = ArtifactLocation::Url { url: id.to_owned() };
        let file_name = artifact.file_name();
        let stem = file_name
            .rsplit_once('.')
            .map_or(&*file_name, |(stem, _)| stem);
        if stem.is_empty() {
            return Err(error(format!("{id} doesn't end in a file name")));
        }
        let format = if is_archive_name(&file_name) {
            ArtifactFormat::Archive
        } else {
            ArtifactFormat::File
        };
        Ok(ResolvedMod {
            id: id.to_owned(),
            name: stem.to_owned(),
            version: None,
            artifact,
            format,
            dependencies: Vec::new(),
            game_versions: None,
        })
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
