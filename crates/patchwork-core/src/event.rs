use crate::deploy::DeployOp;

/// Progress reported by long-running operations. The core never prints; the
/// CLI (and later the GUI) decides how to show these.
#[derive(Debug, Clone)]
pub enum Event {
    Resolving {
        name: String,
    },
    Fetching {
        name: String,
    },
    /// How far a download for `name` has got, sent every so often while it
    /// runs (and once at its start and end). `total` is `None` when the
    /// server doesn't say how big it is.
    Downloading {
        name: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// A download for `name` stopped, whether it finished or failed.
    Downloaded {
        name: String,
    },
    Building {
        step: String,
    },
    Applying(DeployOp),
    Warning(String),
    /// A mod in `profile` locked for the first time lists the mods in
    /// `missing` without saying whether it needs them, and the profile
    /// doesn't have them. Not a problem, just worth knowing, once.
    MaybeNeeded {
        profile: String,
        name: String,
        missing: Vec<String>,
    },
}

pub trait Reporter: Send + Sync {
    fn report(&self, event: Event);
}

pub struct Silent;

impl Reporter for Silent {
    fn report(&self, _: Event) {}
}
