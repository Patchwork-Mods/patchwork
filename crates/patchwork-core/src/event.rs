use crate::deploy::DeployOp;

/// Progress reported by long-running operations. The core never prints; the
/// CLI (and later the GUI) decides how to show these.
#[derive(Debug, Clone)]
pub enum Event {
    Resolving { name: String },
    Fetching { name: String },
    Building { step: String },
    Applying(DeployOp),
    Warning(String),
}

pub trait Reporter: Send + Sync {
    fn report(&self, event: Event);
}

pub struct Silent;

impl Reporter for Silent {
    fn report(&self, _: Event) {}
}
