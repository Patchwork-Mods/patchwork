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

pub mod changelog;
pub mod compat;
pub mod conflicts;
pub mod deploy;
pub mod environment;
pub mod error;
pub mod event;
pub mod fsutil;
pub mod game;
pub mod hash;
pub mod issue;
pub mod launch;
pub mod locate;
pub mod lock;
pub mod matching;
pub mod order;
pub mod paths;
pub mod plugins;
pub mod profile;
pub mod registry;
pub mod relpath;
pub mod search;
pub mod settings;
pub mod share;
pub mod source;
pub mod store;
pub mod update;
mod vdf;

mod builtin;
mod patchwork;

use std::sync::LazyLock;
use std::time::Duration;

pub use error::{Error, Result};
pub use patchwork::{
    Builder, DeployStatus, GameInstall, ImportOptions, Imported, KnownSummary, KnownVersions,
    Patchwork,
};

/// The HTTP client every request shares, so connections to a host are kept
/// alive and reused rather than set up again (DNS, TCP, TLS) for each one.
pub fn http() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
    &CLIENT
}

/// GETs `url` with the shared [`http`] client. If the server says it's
/// getting too many requests (as Thunderstore does when a big profile is
/// locked), waits as long as it asks, or a little longer each time, and tries
/// again before giving up.
pub async fn http_get(url: impl reqwest::IntoUrl) -> reqwest::Result<reqwest::Response> {
    const RETRIES: u32 = 5;
    let url = url.into_url()?;
    let mut attempt = 0;
    loop {
        let response = http().get(url.clone()).send().await?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || attempt == RETRIES {
            return Ok(response);
        }
        let wait = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()?.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(1 << attempt))
            .min(Duration::from_secs(30));
        tokio::time::sleep(wait).await;
        attempt += 1;
    }
}
