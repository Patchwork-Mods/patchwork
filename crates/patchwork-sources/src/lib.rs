//! Built-in mod sources.

mod local;
mod nexus;
mod thunderstore;
mod url;

pub use local::LocalSource;
pub use nexus::Nexus;
pub use thunderstore::Thunderstore;
pub use url::UrlSource;
