//! The daemon (see `patchwork_daemon`), from the app's side. The
//! app is the daemon too, when started with [`FLAG`], so it needs no console
//! window on Windows and runs from wherever the app is (even an AppImage).
//! While the app's open, the daemon's paused and the app syncs itself.

use std::sync::Mutex;

use jiff::Timestamp;
use patchwork_core::Patchwork;
use patchwork_core::paths::Paths;
use patchwork_core::profile::ProfileRef;
use patchwork_daemon::{self as daemon, DaemonSettings, Paused, Status, service};
use serde::Serialize;
use tauri::State;

use crate::{App, CmdResult, msg};

/// Runs the app as the daemon, rather than opening a window.
pub const FLAG: &str = "--daemon";

/// What [`FLAG`] was when the daemon only synced, which logins set up
/// then still start it with.
pub const OLD_FLAG: &str = "--sync-daemon";

/// How many times the app tries to pause a daemon it's just started, a
/// tenth of a second apart.
const PAUSE_TRIES: u32 = 50;

/// What keeps the daemon paused while the app's open.
#[derive(Default)]
pub struct Daemon(Mutex<Option<Paused>>);

impl Daemon {
    pub fn new(paused: Option<Paused>) -> Self {
        Self(Mutex::new(paused))
    }
}

fn program() -> anyhow::Result<service::Program> {
    service::Program::current(&[FLAG])
}

/// Runs the daemon until it's stopped, opening patchwork with `open`.
pub fn run(open: impl Fn(Paths) -> patchwork_core::Result<Patchwork>) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    tauri::async_runtime::block_on(daemon::serve(&paths, daemon::tasks(), || {
        open(paths.clone())
    }))
}

/// Pauses the daemon so the app can open patchwork, and, if it's installed
/// but isn't running (say, the app's just been updated, which stops it on
/// Windows) or is an older version, starts it, to take over once the app's
/// closed.
pub fn pause_for_app(paths: &Paths) -> Option<Paused> {
    // a package manager may have moved the app since the daemon was set up
    // to start at login
    match program().map(|p| daemon::refresh(paths, &p)) {
        Ok(Ok(_)) => {}
        Ok(Err(e)) | Err(e) => eprintln!("couldn't update the daemon's login entry: {e:#}"),
    }
    let status = daemon::status(paths).unwrap_or_else(|e| {
        eprintln!("couldn't reach the daemon: {e:#}");
        None
    });
    let installed = service::installed().ok().flatten().is_some();
    let current = status
        .as_ref()
        .is_some_and(|s| s.version == env!("CARGO_PKG_VERSION"));
    if installed && !current {
        if status.is_some() {
            let _ = daemon::stop(paths);
        }
        // it waits for the app to close before syncing
        match program() {
            Ok(program) => {
                if let Err(e) = service::start(&program) {
                    eprintln!("couldn't start the daemon: {e:#}");
                }
            }
            Err(e) => eprintln!("couldn't start the daemon: {e:#}"),
        }
        return None;
    }
    daemon::pause(paths).unwrap_or_else(|e| {
        eprintln!("couldn't pause the daemon: {e:#}");
        None
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    /// How it's set up to start at login, if it is.
    installed: Option<&'static str>,
    running: Option<Status>,
    /// The app's version, which a running daemon should match.
    version: &'static str,
}

#[tauri::command]
pub async fn daemon_status(app: State<'_, App>) -> CmdResult<DaemonStatus> {
    let paths = app.pw.paths().clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(DaemonStatus {
            installed: service::installed().map_err(msg)?,
            running: daemon::status(&paths).map_err(msg)?,
            version: env!("CARGO_PKG_VERSION"),
        })
    })
    .await
    .map_err(msg)?
}

/// Installs the daemon, replacing any that's running, and keeps the new
/// one paused while the app's open.
#[tauri::command]
pub async fn daemon_install(app: State<'_, App>) -> CmdResult<()> {
    let paths = app.pw.paths().clone();
    app.daemon.0.lock().unwrap().take();
    let paused = tauri::async_runtime::spawn_blocking(move || {
        daemon::install(&paths, &program().map_err(msg)?).map_err(msg)?;
        Ok::<_, String>(pause_new(&paths))
    })
    .await
    .map_err(msg)??;
    *app.daemon.0.lock().unwrap() = paused;
    Ok(())
}

/// Stops the daemon, and starts it again from this version of the app.
#[tauri::command]
pub async fn daemon_restart(app: State<'_, App>) -> CmdResult<()> {
    let paths = app.pw.paths().clone();
    app.daemon.0.lock().unwrap().take();
    let paused = tauri::async_runtime::spawn_blocking(move || {
        daemon::stop(&paths).map_err(msg)?;
        service::start(&program().map_err(msg)?).map_err(msg)?;
        Ok::<_, String>(pause_new(&paths))
    })
    .await
    .map_err(msg)??;
    *app.daemon.0.lock().unwrap() = paused;
    Ok(())
}

#[tauri::command]
pub async fn daemon_uninstall(app: State<'_, App>) -> CmdResult<()> {
    let paths = app.pw.paths().clone();
    app.daemon.0.lock().unwrap().take();
    tauri::async_runtime::spawn_blocking(move || daemon::uninstall(&paths).map_err(msg))
        .await
        .map_err(msg)?
}

/// Whether the daemon's on at all, and its tasks.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonTasks {
    /// Off means it runs none of them, whatever each says.
    enabled: bool,
    tasks: Vec<DaemonTask>,
}

/// One of the daemon's tasks, for turning on and off.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonTask {
    id: &'static str,
    description: &'static str,
    enabled: bool,
    /// How long it leaves between runs, if `daemon.toml` says.
    every: Option<String>,
}

#[tauri::command]
pub fn daemon_tasks(app: State<'_, App>) -> CmdResult<DaemonTasks> {
    let settings = DaemonSettings::load(app.pw.paths()).map_err(msg)?;
    let tasks = daemon::tasks()
        .iter()
        .map(|task| {
            let own = settings.task(task.id());
            DaemonTask {
                id: task.id(),
                description: task.description(),
                enabled: own.enabled.unwrap_or_else(|| task.default_enabled()),
                every: own.every,
            }
        })
        .collect();
    Ok(DaemonTasks {
        enabled: settings.on(),
        tasks,
    })
}

/// Turns the whole daemon on or off; its tasks keep their own settings.
/// Like [`daemon_set_task`], it takes effect once the app's closed.
#[tauri::command]
pub fn daemon_set_enabled(app: State<'_, App>, enabled: bool) -> CmdResult<()> {
    let paths = app.pw.paths();
    let mut settings = DaemonSettings::load(paths).map_err(msg)?;
    settings.enabled = Some(enabled);
    settings.save(paths).map_err(msg)
}

/// Turns one of the daemon's tasks on or off. The daemon's paused while
/// the app's open, and picks it up once it isn't.
#[tauri::command]
pub fn daemon_set_task(app: State<'_, App>, id: String, enabled: bool) -> CmdResult<()> {
    if !daemon::tasks().iter().any(|t| t.id() == id) {
        return Err(format!("there's no daemon task `{id}`"));
    }
    let paths = app.pw.paths();
    let mut settings = DaemonSettings::load(paths).map_err(msg)?;
    settings.tasks.entry(id).or_default().enabled = Some(enabled);
    settings.save(paths).map_err(msg)
}

/// What the daemon did to a profile while the app was closed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Missed {
    id: String,
    label: String,
    /// When it last changed it.
    at: Timestamp,
    /// It arrived on this computer.
    created: bool,
    changed: bool,
    warnings: Vec<String>,
}

/// What the daemon did while the app was closed, a profile at a time, most
/// recent first. Each is only returned once.
#[tauri::command]
pub fn daemon_missed(app: State<'_, App>) -> CmdResult<Vec<Missed>> {
    let mut missed: Vec<Missed> = Vec::new();
    for m in daemon::sync::take_missed(app.pw.paths()).map_err(msg)? {
        let profile = ProfileRef { uuid: m.profile };
        // removed since, so there's nothing to show
        let Ok(loaded) = app.pw.load_profile(&profile) else {
            continue;
        };
        let id = profile.to_string();
        let i = match missed.iter().position(|s| s.id == id) {
            Some(i) => i,
            None => {
                missed.push(Missed {
                    id,
                    label: loaded.label(),
                    at: m.at,
                    created: false,
                    changed: false,
                    warnings: Vec::new(),
                });
                missed.len() - 1
            }
        };
        let seen = &mut missed[i];
        seen.at = seen.at.max(m.at);
        seen.created |= m.created;
        seen.changed |= m.changed;
        for warning in m.warnings {
            if !seen.warnings.contains(&warning) {
                seen.warnings.push(warning);
            }
        }
    }
    missed.sort_by_key(|m| std::cmp::Reverse(m.at));
    Ok(missed)
}

/// Pauses a daemon that's only just been started, once it's listening.
fn pause_new(paths: &Paths) -> Option<Paused> {
    for _ in 0..PAUSE_TRIES {
        if let Ok(Some(paused)) = daemon::pause(paths) {
            return Some(paused);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}
