//! The core of patchwork: game definitions, profiles, lockfiles, the content
//! store and the deploy engine.
//!
//! Sources and deployers are plugged in through the [`source::Source`] and
//! [`deploy::Deployer`] traits. Neither is allowed to touch the filesystem
//! directly: sources describe where an artifact lives and the host fetches it,
//! and deployers describe where files should end up and the host places them.
//! That boundary is what will let sandboxed (WASM) plugins slot in later.
//!
//! Ways of sharing profiles (zips, URL lists, mod manager codes) plug in
//! through [`share::ProfileFormat`].

pub mod deploy;
pub mod error;
pub mod event;
pub mod fsutil;
pub mod game;
pub mod hash;
pub mod lock;
pub mod matching;
pub mod order;
pub mod paths;
pub mod profile;
pub mod registry;
pub mod relpath;
pub mod search;
pub mod share;
pub mod source;
pub mod store;

mod builtin;
mod patchwork;

pub use error::{Error, Result};
pub use patchwork::{
    Builder, GameInstall, ImportOptions, Imported, KnownSummary, KnownVersions, Patchwork,
};
