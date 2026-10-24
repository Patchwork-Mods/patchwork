use std::path::PathBuf;

use async_trait::async_trait;
use patchwork_core::source::{
    ArtifactFormat, ArtifactLocation, ResolvedMod, Source, SourceContext,
};
use patchwork_core::{Error, Result};

/// Mods from the local filesystem: a folder, a `.zip`, or any other single
/// file (deployed as-is).
pub struct LocalSource;

#[async_trait]
impl Source for LocalSource {
    fn id(&self) -> &str {
        "local"
    }

    async fn resolve(
        &self,
        _ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<ResolvedMod> {
        let error = |message: String| Error::Source {
            source_id: "local".to_owned(),
            message,
        };
        if version.is_some() {
            return Err(error("local mods don't have versions".to_owned()));
        }
        let path = expand_home(id)
            .canonicalize()
            .map_err(|e| error(format!("{id}: {e}")))?;
        let format = if path.is_dir() {
            ArtifactFormat::Dir
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            ArtifactFormat::Zip
        } else {
            ArtifactFormat::File
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| error(format!("{} has no file name", path.display())))?;
        Ok(ResolvedMod {
            id: path.to_string_lossy().into_owned(),
            name,
            version: None,
            artifact: ArtifactLocation::Path { path },
            format,
            dependencies: Vec::new(),
            game_versions: None,
        })
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}
