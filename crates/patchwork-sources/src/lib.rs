//! Built-in mod sources.

mod beatmods;
mod beatsaver;
mod github;
mod local;
mod modio;
mod modlinks;
mod modrinth;
mod nexus;
mod pattern;
mod thunderstore;
mod url;

pub use beatmods::{BeatMods, BeatModsAssets};
pub use beatsaver::BeatSaver;
pub use github::GitHub;
pub use local::LocalSource;
pub use modio::ModIo;
pub use modlinks::ModLinks;
pub use modrinth::Modrinth;
pub use nexus::Nexus;
pub use thunderstore::Thunderstore;
pub use url::UrlSource;
