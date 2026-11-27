//! Where things are on disk, for the window to show and open: patchwork's
//! own folders (Settings → Folders), each game's folders, and profiles'
//! own folders. Folders are looked up here from what's asked for, rather
//! than taken as paths from the window, so it can only open ones patchwork
//! knows about. Also moving the data folder; see
//! [`patchwork_core::Patchwork::move_data`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patchwork_core::cancel::Cancel;
use patchwork_core::deploy::DeployTarget;
use patchwork_core::paths::Locations;
use patchwork_core::{DataMove, MoveProgress};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{App, CmdResult, msg};

/// How often a move's progress goes to the window.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// One of patchwork's own folders.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFolder {
    id: &'static str,
    path: String,
    /// Whether it's there yet; some are only made once they're needed.
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFolders {
    folders: Vec<AppFolder>,
    /// Whether the data folder was moved from where it usually is.
    moved: bool,
    /// Set when `$PATCHWORK_HOME` keeps everything in one place.
    home: Option<String>,
}

fn app_folder(app: &App, id: &str) -> CmdResult<PathBuf> {
    let paths = app.pw.paths();
    Ok(match id {
        "config" => paths.config.clone(),
        "data" => paths.data.clone(),
        "store" => paths.store(),
        "cache" => paths.cache.clone(),
        "profiles" => paths.profiles(),
        "backups" => paths.backups(),
        _ => return Err(msg(format!("patchwork has no folder called {id}"))),
    })
}

const APP_FOLDERS: [&str; 6] = ["config", "data", "store", "profiles", "backups", "cache"];

#[tauri::command]
pub fn app_folders(app: State<'_, App>) -> CmdResult<AppFolders> {
    let folders = APP_FOLDERS
        .into_iter()
        .map(|id| {
            let path = app_folder(&app, id)?;
            Ok(AppFolder {
                id,
                exists: path.is_dir(),
                path: path.display().to_string(),
            })
        })
        .collect::<CmdResult<_>>()?;
    Ok(AppFolders {
        folders,
        moved: Locations::load(app.pw.paths()).map_err(msg)?.data.is_some(),
        home: std::env::var_os("PATCHWORK_HOME").map(|h| PathBuf::from(h).display().to_string()),
    })
}

/// How many bytes are in one of patchwork's folders, which can take a while
/// for the store.
#[tauri::command]
pub async fn folder_size(app: State<'_, App>, id: String) -> CmdResult<u64> {
    let path = app_folder(&app, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    })
    .await
    .map_err(msg)
}

fn open_dir(path: &Path) -> CmdResult<()> {
    if !path.is_dir() {
        return Err(msg(format!("{} isn't there", path.display())));
    }
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(msg)
}

#[tauri::command]
pub fn open_app_folder(app: State<'_, App>, id: String) -> CmdResult<()> {
    let path = app_folder(&app, &id)?;
    // made on first use, but there's nothing wrong with looking before
    if id != "config" && id != "data" {
        let _ = std::fs::create_dir_all(&path);
    }
    open_dir(&path)
}

/// Opens one of a game's folders: `install`, `prefix` (its Wine prefix),
/// or one of its other roots by id, wherever patchwork finds it.
#[tauri::command]
pub fn open_game_folder(app: State<'_, App>, game: String, folder: String) -> CmdResult<()> {
    let install = app.pw.game_install(&game).map_err(msg)?;
    let Some(install) = install else {
        return Err(msg(format!("{game} has no install folder set")));
    };
    if folder == "install" {
        return open_dir(&install.path);
    }
    let roots = app
        .pw
        .resolve_roots(&DeployTarget::Game(game.clone()))
        .map_err(msg)?;
    let Some(roots) = roots else {
        return Err(msg(format!("{game} has no folder called {folder}")));
    };
    if folder == "prefix" {
        let Some(prefix) = roots.prefix() else {
            return Err(msg("patchwork hasn't found the game's Wine prefix"));
        };
        return open_dir(prefix);
    }
    let root = roots
        .iter()
        .find(|(id, _)| id.as_str() == folder)
        .map(|(_, root)| root.path.clone());
    match root {
        Some(Ok(path)) => open_dir(&path),
        Some(Err(problem)) => Err(msg(problem)),
        None => Err(msg(format!("{game} has no folder called {folder}"))),
    }
}

/// Opens the folder of its own a profile deploys to.
#[tauri::command]
pub fn open_profile_folder(app: State<'_, App>, profile: String) -> CmdResult<()> {
    let profile = app.pw.find_profile(&profile).map_err(msg)?;
    open_dir(&app.pw.instance_dir(&profile))
}

/// What moving the data folder to `to` would do, to ask before doing it.
#[tauri::command]
pub async fn plan_data_move(handle: AppHandle, to: String) -> CmdResult<DataMove> {
    // sizing the store reads every file in it
    tauri::async_runtime::spawn_blocking(move || {
        handle.state::<App>().pw.plan_data_move(Path::new(&to))
    })
    .await
    .map_err(msg)?
    .map_err(msg)
}

/// Moves the data folder to `to`, sending progress as
/// `patchwork://data-move`, then restarts the app to use it. Syncing and
/// the daemon stop first, since they have files in it open, so this only
/// returns if the move fails (or is cancelled), and then the app has to be
/// restarted for them to start again; the window says so.
#[tauri::command]
pub async fn move_data(handle: AppHandle, app: State<'_, App>, to: String) -> CmdResult<()> {
    let _work = app.work.start()?;
    if app.job.lock().unwrap().is_some() {
        return Err(msg("wait for the deploy to finish first"));
    }
    let plan = app.pw.plan_data_move(Path::new(&to)).map_err(msg)?;
    if !plan.conflicts.is_empty() {
        return Err(msg(format!(
            "{to} already has {}, which patchwork's data folder has too",
            plan.conflicts.join(", ")
        )));
    }
    let cancel = Cancel::default();
    *app.job.lock().unwrap() = Some(cancel.clone());
    crate::sync::stop(&app).await;
    crate::daemon::stop_for_move(&app);
    let window = handle.clone();
    let moved = tauri::async_runtime::spawn_blocking(move || {
        let app = window.state::<App>();
        let mut last = Instant::now() - PROGRESS_EVERY;
        app.pw
            .move_data(Path::new(&to), &cancel, &mut |p: MoveProgress| {
                if last.elapsed() >= PROGRESS_EVERY || p.done == p.total {
                    last = Instant::now();
                    let _ = window.emit("patchwork://data-move", p);
                }
            })
    })
    .await
    .map_err(msg)?;
    app.job.lock().unwrap().take();
    moved.map_err(msg)?;
    handle.restart();
}

/// Restarts the app, after a move that didn't finish, so syncing and the
/// daemon start again.
#[tauri::command]
pub fn restart_app(handle: AppHandle) {
    handle.restart();
}
