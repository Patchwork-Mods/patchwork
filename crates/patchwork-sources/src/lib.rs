//! Built-in mod sources.

mod local;
mod nexus;
mod thunderstore;

pub use local::LocalSource;
pub use nexus::Nexus;
pub use thunderstore::Thunderstore;
