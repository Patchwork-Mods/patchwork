use async_trait::async_trait;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ResolvedMod, Source, SourceContext,
};
use patchwork_core::{Error, Result};

/// Mods downloaded straight from a URL, as in `url:https://example.com/mod.zip`.
/// A `.zip` is unpacked; anything else is deployed as a single file.
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
        let (stem, ext) = file_name.rsplit_once('.').unwrap_or((&file_name, ""));
        if stem.is_empty() {
            return Err(error(format!("{id} doesn't end in a file name")));
        }
        let format = if ext.eq_ignore_ascii_case("zip") {
            ArtifactFormat::Zip
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
        })
    }

    fn mod_url(&self, _ctx: &SourceContext<'_>, id: &str, _version: Option<&str>) -> Option<String> {
        Some(id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patchwork_core::game::GameDef;

    fn game() -> GameDef {
        toml::from_str(
            r#"
            schema = 1
            id = "g"
            name = "G"
            [deployer]
            kind = "rules"
            "#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn resolves_without_fetching() {
        let game = game();
        let ctx = SourceContext {
            game: &game,
            config: None,
        };
        let zip = UrlSource
            .resolve(&ctx, "https://example.com/files/Cool.Mod.zip?dl=1", None)
            .await
            .unwrap();
        assert_eq!(zip.name, "Cool.Mod");
        assert_eq!(zip.format, ArtifactFormat::Zip);
        let pak = UrlSource
            .resolve(&ctx, "https://example.com/thing_P.pak", None)
            .await
            .unwrap();
        assert_eq!(pak.format, ArtifactFormat::File);

        for bad in ["ftp://example.com/a.zip", "https://example.com/"] {
            assert!(UrlSource.resolve(&ctx, bad, None).await.is_err(), "{bad}");
        }
        assert!(
            UrlSource
                .resolve(&ctx, "https://example.com/a.zip", Some("1.0"))
                .await
                .is_err()
        );
    }
}
