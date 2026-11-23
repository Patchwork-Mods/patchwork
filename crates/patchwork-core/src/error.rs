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
    /// Locked mods' files came back different from the ones they were
    /// locked with, at the same version. Usually the mod's host repackaged
    /// it; [`crate::Patchwork::accept_changed`] takes the new file. Never
    /// empty.
    #[error("{}", changed_message(.0))]
    ArtifactChanged(Vec<ArtifactChanged>),
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
    /// Whether it's one of the profile's own mods, under `name`, rather
    /// than something one of them depends on.
    pub in_profile: bool,
    /// Whether the profile pins its version.
    pub pinned: bool,
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
        Self::blocked_by(message, Blocker::new(title, detail, fix))
    }

    /// [`Error::Blocked`] by `blocker`, with `message` for where only a
    /// message fits.
    pub fn blocked_by(message: impl Into<String>, blocker: Blocker) -> Self {
        Error::Blocked(Box::new(Blocked {
            message: message.into(),
            blocker,
        }))
    }

    /// What's in the way, and what clears it, for errors the user can do
    /// something about.
    pub fn blocker(&self) -> Option<Blocker> {
        match self {
            Error::Blocked(b) => Some(b.blocker.clone()),
            Error::ArtifactChanged(changed) => Some(changed_blocker(changed)),
            Error::Schema {
                kind,
                path,
                found,
                supported,
            } => Some(Blocker::new(
                "Patchwork needs updating",
                format!(
                    "{} was saved by a newer version of Patchwork, most likely on another PC \
                     it syncs with. It's {kind} schema {found}, and this version only reads up \
                     to {supported}. Update Patchwork to use it here.",
                    path.display()
                ),
                Fix::UpdatePatchwork,
            )),
            _ => None,
        }
    }

    /// The fix to suggest for the error, if there is one; see
    /// [`Error::blocker`].
    pub fn fix(&self) -> Option<Fix> {
        self.blocker()?.fixes.into_iter().next()
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

impl ArtifactChanged {
    /// `locked` (one of the profile's own mods if `in_profile`), whose file
    /// hashes to `got` now.
    pub fn new(locked: &LockedMod, got: ContentHash, in_profile: bool) -> Self {
        ArtifactChanged {
            key: locked.key(),
            name: locked.name.clone(),
            version: locked.version.clone(),
            expected: locked.hash,
            got,
            in_profile,
            pinned: locked.requested.is_some(),
        }
    }
}

fn changed_message(changed: &[ArtifactChanged]) -> String {
    match changed {
        [one] => one.to_string(),
        _ => format!(
            "{} changed since they were locked",
            and_list(changed.iter().map(|c| format!("`{}`", c.name)))
        ),
    }
}

/// What [`Error::ArtifactChanged`] is in the way of: every fix applies to
/// all the mods, so updating is only offered if none is pinned, and
/// removing if they're all the profile's own.
fn changed_blocker(changed: &[ArtifactChanged]) -> Blocker {
    let keys: Vec<String> = changed.iter().map(|c| c.key.clone()).collect();
    let names: Vec<String> = changed.iter().map(|c| c.name.clone()).collect();
    let (title, detail) = match changed {
        [ArtifactChanged { name, version, .. }] => {
            let at = version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default();
            (
                format!("{name} changed since it was locked"),
                format!(
                    "{name}{at} downloads as a different file now than the one this profile was \
                     locked with, here or on a PC it syncs with. That usually means its author \
                     or the site it's from repackaged it. Using the new file keeps {name} at the \
                     same version, and PCs this profile syncs with will use it too."
                ),
            )
        }
        _ => {
            let list = and_list(names.iter().cloned());
            (
                format!("{} mods changed since they were locked", changed.len()),
                format!(
                    "{list} download as different files now than the ones this profile was \
                     locked with, here or on a PC it syncs with. That usually means their \
                     authors or the sites they're from repackaged them. Using the new files \
                     keeps them at the same versions, and PCs this profile syncs with will use \
                     them too."
                ),
            )
        }
    };
    let mut fixes = vec![Fix::AcceptChanged {
        keys: keys.clone(),
        names: names.clone(),
    }];
    if changed.iter().all(|c| !c.pinned) {
        fixes.push(Fix::UpdateMods {
            keys,
            names: names.clone(),
        });
    }
    if changed.iter().all(|c| c.in_profile) {
        fixes.push(Fix::RemoveMods { names });
    }
    Blocker::with_fixes(title, detail, fixes)
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: impl IntoIterator<Item = String>) -> String {
    let items: Vec<String> = items.into_iter().collect();
    match items.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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
