//! Built-in mod sources.

mod github;
mod local;
mod modio;
mod modrinth;
mod nexus;
mod thunderstore;
mod url;

pub use github::GitHub;
pub use local::LocalSource;
pub use modio::ModIo;
pub use modrinth::Modrinth;
pub use nexus::Nexus;
pub use thunderstore::Thunderstore;
pub use url::UrlSource;
