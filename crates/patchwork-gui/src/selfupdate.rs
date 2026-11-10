//! Updating the app itself, with Tauri's updater: it reads `latest.json` from
//! the newest GitHub release (made by `.github/workflows/release.yml`, with notes from `CHANGELOG.md`)
//! and installs the bundle that matches how this copy was installed.

use std::sync::Mutex;

use futures_util::future::{AbortHandle, abortable};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;
use tauri::{AppHandle, State};
use tauri_plugin_updater::UpdaterExt;

use crate::{App, CmdResult, msg};

/// Where to get it by hand, when this copy can't update itself.
const RELEASES_URL: &str = "https://github.com/patchwork-mods/patchwork/releases/latest";

/// How this copy was installed, if it came from one of our bundles. Anything
/// else (Nix, a dev build, ...) isn't ours to replace.
pub fn installed_from() -> Option<BundleType> {
    match bundle_type()? {
        // What every macOS build claims, so check it's really in one.
        BundleType::App => std::env::current_exe()
            .ok()?
            .to_string_lossy()
            .contains(".app/Contents/MacOS/")
            .then_some(BundleType::App),
        other => Some(other),
    }
}

/// What's running that an update mustn't cut off by restarting the app:
/// deploys, downloads and the like. An update waits until none of it is
/// running, and none can start once an update has.
#[derive(Default)]
pub struct Work(Mutex<WorkState>);

#[derive(Default)]
struct WorkState {
    running: usize,
    updating: bool,
    /// Cancels the update's download, while there is one.
    download: Option<AbortHandle>,
}

impl Work {
    /// Marks work as running until the guard is dropped.
    pub fn start(&self) -> CmdResult<WorkGuard<'_>> {
        let mut state = self.0.lock().unwrap();
        if state.updating {
            return Err("patchwork is updating".to_owned());
        }
        state.running += 1;
        Ok(WorkGuard(self))
    }

    /// Marks an update as underway until the guard is dropped, which it
    /// only is if the update fails.
    fn start_update(&self) -> CmdResult<UpdateGuard<'_>> {
        let mut state = self.0.lock().unwrap();
        if state.running > 0 {
            return Err(
                "patchwork is busy (deploying or downloading); update once that's finished"
                    .to_owned(),
            );
        }
        state.updating = true;
        Ok(UpdateGuard(self))
    }

    fn set_download(&self, download: Option<AbortHandle>) {
        self.0.lock().unwrap().download = download;
    }
}

pub struct WorkGuard<'a>(&'a Work);

impl Drop for WorkGuard<'_> {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().running -= 1;
    }
}

struct UpdateGuard<'a>(&'a Work);

impl Drop for UpdateGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap();
        state.updating = false;
        state.download = None;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current: String,
    /// The newer release, if there is one.
    latest: Option<String>,
    available: bool,
    /// The newer release's notes (its CHANGELOG.md section), as safe HTML.
    notes_html: Option<String>,
    /// Whether [`install_update`] can replace this copy.
    installable: bool,
    releases_url: &'static str,
}

/// Whether there's a newer release of patchwork than this one.
#[tauri::command]
pub async fn check_for_update(handle: AppHandle) -> CmdResult<UpdateInfo> {
    let update = handle.updater().map_err(msg)?.check().await.map_err(msg)?;
    Ok(UpdateInfo {
        current: handle.package_info().version.to_string(),
        available: update.is_some(),
        notes_html: update
            .as_ref()
            .and_then(|u| u.body.as_deref())
            .filter(|notes| !notes.trim().is_empty())
            .map(crate::changelog::render_markdown),
        latest: update.map(|u| u.version),
        installable: installed_from().is_some(),
        releases_url: RELEASES_URL,
    })
}

/// How an update's going, sent to `install_update`'s channel.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateProgress {
    /// `total` is `None` if the server didn't say how big it is.
    Downloading { downloaded: u64, total: Option<u64> },
    /// Downloaded and being put in place, which can't be cancelled.
    Installing,
}

/// How `install_update` ended, when it didn't end in a restart.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateOutcome {
    /// There turned out to be nothing newer to install.
    UpToDate,
    /// `cancel_update` stopped the download.
    Cancelled,
}

/// Installs the newest release over this one and restarts into it, once
/// nothing in [`Work`] is running, reporting how it's going to `progress`.
/// On Windows the installer closes the app itself, so this doesn't return.
#[tauri::command]
pub async fn install_update(
    handle: AppHandle,
    app: State<'_, App>,
    progress: Channel<UpdateProgress>,
) -> CmdResult<UpdateOutcome> {
    if installed_from().is_none() {
        return Err(
            "this copy of patchwork wasn't installed from one of its installers; \
             update it the way you installed it"
                .to_owned(),
        );
    }
    let _updating = app.work.start_update()?;
    let Some(update) = handle.updater().map_err(msg)?.check().await.map_err(msg)? else {
        return Ok(UpdateOutcome::UpToDate);
    };
    let mut downloaded = 0;
    let (download, abort) = abortable(update.download(
        |chunk, total| {
            downloaded += chunk as u64;
            let _ = progress.send(UpdateProgress::Downloading { downloaded, total });
        },
        || {},
    ));
    app.work.set_download(Some(abort));
    let Ok(bytes) = download.await else {
        return Ok(UpdateOutcome::Cancelled);
    };
    let bytes = bytes.map_err(msg)?;
    app.work.set_download(None);
    let _ = progress.send(UpdateProgress::Installing);
    update.install(bytes).map_err(msg)?;
    handle.restart();
}

/// Stops `install_update` downloading, if it still is.
#[tauri::command]
pub fn cancel_update(app: State<'_, App>) {
    if let Some(download) = app.work.0.lock().unwrap().download.take() {
        download.abort();
    }
}
