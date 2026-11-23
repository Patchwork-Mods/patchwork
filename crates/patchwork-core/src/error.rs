use std::path::{Path, PathBuf};

use crate::fix::{Blocker, Fix};
use crate::hash::ContentHash;
use crate::lock::LockedMod;
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
    #[error("extension `{id}`: {message}")]
    Extension { id: String, message: String },
    #[error("another patchwork process is running (lock held on {})", .0.display())]
    Busy(PathBuf),
    #[error("{}", needs_download(.0))]
    NeedsDownload(Vec<PendingDownload>),
    #[error("updating patchwork: {0}")]
    SelfUpdate(String),
    /// Called off through a [`crate::cancel::Cancel`].
    #[error("cancelled")]
    Cancelled,
    /// A locked mod's file came back different from the one it was locked
    /// with, at the same version. Usually the mod's host repackaged it;
    /// [`crate::Patchwork::accept_changed`] takes the new file.
    #[error("{0}")]
    ArtifactChanged(Box<ArtifactChanged>),
    /// Something's in the way that the user can clear; see
    /// [`crate::fix`]. Raised before anything was changed, so what failed
    /// can be tried again once it's cleared.
    #[error("{0}")]
    Blocked(Box<Blocked>),
    #[error("{0}")]
    Invalid(String),
}

/// What [`Error::ArtifactChanged`] says changed.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "`{name}` changed since it was locked (expected {}, got {})",
    expected.short(),
    got.short()
)]
pub struct ArtifactChanged {
    /// The mod's key, `<source>:<id>`.
    pub key: String,
    pub name: String,
    pub version: Option<String>,
    pub expected: ContentHash,
    pub got: ContentHash,
}

/// What [`Error::Blocked`] says is in the way.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Blocked {
    /// What went wrong, in a sentence, for where only a message fits.
    pub message: String,
    pub blocker: Blocker,
}

impl Error {
    /// [`Error::Blocked`]: `message` for where only a message fits, and
    /// `title`, `detail` and `fix` for the [`Blocker`].
    pub fn blocked(
        message: impl Into<String>,
        title: impl Into<String>,
        detail: impl Into<String>,
        fix: Fix,
    ) -> Self {
        Error::Blocked(Box::new(Blocked {
            message: message.into(),
            blocker: Blocker {
                title: title.into(),
                detail: detail.into(),
                fix,
            },
        }))
    }

    /// What's in the way, and what clears it, for errors the user can do
    /// something about.
    pub fn blocker(&self) -> Option<Blocker> {
        match self {
            Error::Blocked(b) => Some(b.blocker.clone()),
            Error::ArtifactChanged(changed) => {
                let ArtifactChanged {
                    key, name, version, ..
                } = &**changed;
                let at = version
                    .as_deref()
                    .map(|v| format!(" {v}"))
                    .unwrap_or_default();
                Some(Blocker {
                    title: format!("{name} changed since it was locked"),
                    detail: format!(
                        "{name}{at} downloads as a different file now than the one this \
                         profile was locked with, here or on a PC it syncs with. That usually \
                         means its author or the site it's from repackaged it. Using the new \
                         file keeps {name} at the same version, and PCs this profile syncs \
                         with will use it too."
                    ),
                    fix: Fix::AcceptChanged {
                        keys: vec![key.clone()],
                        names: vec![name.clone()],
                    },
                })
            }
            Error::Schema {
                kind,
                path,
                found,
                supported,
            } => Some(Blocker {
                title: "Patchwork needs updating".to_owned(),
                detail: format!(
                    "{} was saved by a newer version of Patchwork, most likely on another PC \
                     it syncs with. It's {kind} schema {found}, and this version only reads up \
                     to {supported}. Update Patchwork to use it here.",
                    path.display()
                ),
                fix: Fix::UpdatePatchwork,
            }),
            _ => None,
        }
    }

    /// What clears the error, if anything does; see [`Error::blocker`].
    pub fn fix(&self) -> Option<Fix> {
        self.blocker().map(|b| b.fix)
    }

    /// [`Error::ArtifactChanged`] for `locked`, whose file hashes to `got`
    /// now.
    pub fn artifact_changed(locked: &LockedMod, got: ContentHash) -> Self {
        Error::ArtifactChanged(Box::new(ArtifactChanged {
            key: locked.key(),
            name: locked.name.clone(),
            version: locked.version.clone(),
            expected: locked.hash,
            got,
        }))
    }

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
