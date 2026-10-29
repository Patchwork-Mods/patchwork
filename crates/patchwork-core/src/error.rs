use std::path::{Path, PathBuf};

use crate::source::PendingDownload;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error(
        "{} uses {kind} schema {found}, but this build only supports up to {supported}; update patchwork",
        path.display()
    )]
    Schema {
        kind: &'static str,
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("reqwest error {url}: {result}")]
    Reqwest {
        url: String,
        #[source]
        result: reqwest::Error,
    },
    #[error("error decoding json!")]
    JsonDecode(#[from] serde_json::Error),
    #[error("unknown game `{0}`")]
    UnknownGame(String),
    #[error("unknown source `{0}`")]
    UnknownSource(String),
    #[error("unknown deployer `{0}`")]
    UnknownDeployer(String),
    #[error("unknown provisioner `{0}`")]
    UnknownProvisioner(String),
    #[error("unknown profile format `{0}`")]
    UnknownFormat(String),
    #[error("{format_id}: {message}")]
    Format { format_id: String, message: String },
    #[error("source `{source_id}`: {message}")]
    Source { source_id: String, message: String },
    #[error("uuid error: {0}")]
    UuidError(#[from] uuid::Error),
    #[error("deployer `{kind}`: {message}")]
    Deployer { kind: String, message: String },
    #[error("provisioner `{kind}`: {message}")]
    Provisioner { kind: String, message: String },
    #[error("extension `{id}`: {message}")]
    Extension { id: String, message: String },
    #[error("another patchwork process is running (lock held on {})", .0.display())]
    Busy(PathBuf),
    #[error("{}", needs_download(.0))]
    NeedsDownload(Vec<PendingDownload>),
    #[error("updating patchwork: {0}")]
    SelfUpdate(String),
    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::Invalid(message.into())
    }

    pub fn parse(path: impl Into<PathBuf>, message: impl ToString) -> Self {
        Error::Parse {
            path: path.into(),
            message: message.to_string(),
        }
    }
}

fn needs_download(pending: &[PendingDownload]) -> String {
    let mut message = "these mods have to be downloaded from their pages first:".to_owned();
    for p in pending {
        message.push_str(&format!("\n  {}: {}", p.name, p.page));
    }
    message
}

/// Attaches the offending path to I/O errors.
pub trait IoContext<T> {
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }
}
