//! Updating the `patchwork` CLI itself, from its GitHub releases.
//!
//! The CLI's own installer (made by dist) leaves an install receipt saying
//! where it put it. A copy without a receipt for the running executable came
//! from somewhere else (the Patchwork app, Nix, `cargo install`, a dev build),
//! so it can still be told about new releases but replacing it is left to
//! whatever installed it. The app updates itself, and the CLI it carries,
//! with Tauri's updater instead.

use axoupdater::{AxoUpdater, AxoupdateError, ReleaseSource, ReleaseSourceType};
use serde::Serialize;

use crate::{Error, Result};

const OWNER: &str = "michaelgoldenn";
const REPO: &str = "patchwork-new";

/// Where to send people whose copy can't update itself.
pub const RELEASES_URL: &str = "https://github.com/michaelgoldenn/patchwork-new/releases/latest";

/// This build's version. Every crate shares the workspace's, and they're all
/// released together.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current: String,
    /// The newest release, or `None` if there isn't one yet.
    pub latest: Option<String>,
    /// Whether `latest` is newer than this build.
    pub available: bool,
    /// Whether [`install`] can replace this copy.
    pub installable: bool,
}

/// What dist calls an app is its package's name, e.g. `patchwork-cli`.
fn updater(app: &str) -> Result<(AxoUpdater, bool)> {
    let mut updater = AxoUpdater::new_for(app);
    let installable = updater.load_receipt().is_ok()
        && updater
            .check_receipt_is_for_this_executable()
            .unwrap_or(false);
    if !installable {
        updater.set_release_source(ReleaseSource {
            release_type: ReleaseSourceType::GitHub,
            owner: OWNER.to_owned(),
            name: REPO.to_owned(),
            app_name: app.to_owned(),
        });
        updater
            .set_current_version(CURRENT.parse().map_err(update_err)?)
            .map_err(update_err)?;
    }
    Ok((updater, installable))
}

/// Asks GitHub for the newest release of `app`.
pub async fn check(app: &str) -> Result<UpdateCheck> {
    let (mut updater, installable) = updater(app)?;
    let latest = match updater.query_new_version().await {
        Ok(v) => v.cloned(),
        Err(AxoupdateError::NoStableReleases { .. } | AxoupdateError::ReleaseNotFound { .. }) => {
            None
        }
        Err(e) => return Err(update_err(e)),
    };
    let current: axoupdater::Version = CURRENT.parse().map_err(update_err)?;
    Ok(UpdateCheck {
        current: CURRENT.to_owned(),
        available: latest.as_ref().is_some_and(|l| *l > current),
        latest: latest.map(|v| v.to_string()),
        installable,
    })
}

/// Replaces this copy of `app` with its newest release, returning the version
/// it was updated to, or `None` if it was already the newest. `show_output`
/// passes the installer's own output through to this process's.
pub async fn install(app: &str, show_output: bool) -> Result<Option<String>> {
    let (mut updater, installable) = updater(app)?;
    if !installable {
        return Err(Error::SelfUpdate(format!(
            "this copy of patchwork ({CURRENT}) wasn't installed by its installer; \
             update it the way you installed it"
        )));
    }
    if show_output {
        updater.enable_installer_output();
    } else {
        updater.disable_installer_output();
    }
    let result = updater.run().await.map_err(update_err)?;
    Ok(result.map(|r| r.new_version.to_string()))
}

fn update_err(e: impl std::fmt::Display) -> Error {
    Error::SelfUpdate(e.to_string())
}
