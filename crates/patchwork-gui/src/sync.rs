//! Keeping profiles in step with the user's other computers, and sharing
//! them with friends or for others to follow (see `patchwork_sync`). While
//! patchwork's open, and once there's anything to sync, the iroh node runs
//! in the background and tells the window `patchwork://synced` whenever a
//! sync changes a profile, so it can show it. Synced changes are never
//! deployed.

use patchwork_sync::{Node, Outcome};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

use crate::{App, CmdResult, msg, profile_ref};

/// How long joining waits for the profile to arrive.
const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub struct Sync {
    node: OnceCell<Node>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Synced {
    profile: Option<String>,
    changed: bool,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Joined {
    Profile {
        id: String,
        /// What didn't work, like mods that couldn't be downloaded.
        warnings: Vec<String>,
    },
    /// Linked to the user's other computers, whose profiles arrive in the
    /// background.
    Devices { profiles: usize },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// Whether this machine can change the profile, or only follows it.
    writable: bool,
}

/// Starts the node if any profile is synced; otherwise it waits until one
/// is shared or joined.
pub fn start_if_synced(handle: &AppHandle) {
    let app = handle.state::<App>();
    let dir = app.pw.paths().sync();
    let synced = patchwork_sync::synced_profiles(&dir).is_ok_and(|s| !s.is_empty())
        || patchwork_sync::devices_linked(&dir).unwrap_or(false);
    if synced {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = node(&handle).await {
                emit(&handle, Err(e));
            }
        });
    }
}

/// The node, started (along with its background syncing) the first time.
async fn node(handle: &AppHandle) -> Result<&Node, String> {
    let app = handle.state::<App>().inner();
    let mut started = false;
    let node = app
        .sync
        .node
        .get_or_try_init(|| async {
            started = true;
            Node::spawn(&app.pw.paths().sync()).await.map_err(msg)
        })
        .await?;
    if started {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let app = handle.state::<App>();
            let node = app.sync.node.get().expect("started above");
            if let Err(e) = node.run(&app.pw, |outcome| emit(&handle, outcome)).await {
                emit(&handle, Err(e));
            }
        });
    }
    Ok(node)
}

fn emit(handle: &AppHandle, outcome: Result<Outcome, impl std::fmt::Display>) {
    let synced = match outcome {
        Ok(mut o) => {
            if o.moved {
                o.warnings.push(
                    "the profile moved to a new document; another computer syncing it hands \
                     that over when you're both online, unless you were removed or only \
                     follow it, in which case you need a new ticket"
                        .to_owned(),
                );
            }
            Synced {
                profile: Some(o.profile.to_string()),
                changed: o.applied.is_some_and(|a| a.changed),
                warnings: o.warnings,
            }
        }
        Err(e) => Synced {
            profile: None,
            changed: false,
            warnings: vec![format!("sync failed: {e}")],
        },
    };
    let _ = handle.emit("patchwork://synced", synced);
}

/// Whether the profile is synced, and how.
#[tauri::command]
pub fn sync_status(app: State<'_, App>, id: String) -> CmdResult<Option<SyncStatus>> {
    let r = profile_ref(&id)?;
    let synced = patchwork_sync::synced_profiles(&app.pw.paths().sync()).map_err(msg)?;
    Ok(synced
        .into_iter()
        .find(|s| s.profile == r.uuid)
        .map(|s| SyncStatus {
            writable: s.writable,
        }))
}

/// A ticket for friends to join the profile with (`writable`), or for
/// others to follow it.
#[tauri::command]
pub async fn sync_share(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    writable: bool,
) -> CmdResult<String> {
    let node = node(&handle).await?;
    node.share(
        &app.pw,
        &profile_ref(&id)?,
        writable,
        patchwork_sync::Addresses::Included,
    )
    .await
    .map_err(msg)
}

/// Joins a profile shared from another computer, or links this one to the
/// user's others. The window opens a joined profile, so it isn't sent as
/// `patchwork://synced`.
#[tauri::command]
pub async fn sync_join(
    handle: AppHandle,
    app: State<'_, App>,
    ticket: String,
) -> CmdResult<Joined> {
    let node = node(&handle).await?;
    let joined = node
        .join(&app.pw, &ticket, JOIN_TIMEOUT)
        .await
        .map_err(msg)?;
    Ok(match joined {
        patchwork_sync::Joined::Profile(outcome) => Joined::Profile {
            id: outcome.profile.to_string(),
            warnings: outcome.warnings,
        },
        patchwork_sync::Joined::Devices { profiles } => Joined::Devices { profiles },
    })
}

/// Whether this computer is linked to the user's others.
#[tauri::command]
pub fn sync_linked(app: State<'_, App>) -> CmdResult<bool> {
    patchwork_sync::devices_linked(&app.pw.paths().sync()).map_err(msg)
}

/// A ticket to link another of the user's computers to this one.
#[tauri::command]
pub async fn sync_link(handle: AppHandle, app: State<'_, App>) -> CmdResult<String> {
    let node = node(&handle).await?;
    node.link_devices(&app.pw).await.map_err(msg)
}

/// Stops linking this computer to the user's others.
#[tauri::command]
pub async fn sync_unlink(handle: AppHandle) -> CmdResult<()> {
    let node = node(&handle).await?;
    node.unlink_devices().await.map_err(msg)
}

/// Stops syncing the profile here.
#[tauri::command]
pub async fn sync_leave(handle: AppHandle, app: State<'_, App>, id: String) -> CmdResult<()> {
    let node = node(&handle).await?;
    node.leave(&app.pw, &profile_ref(&id)?).await.map_err(msg)
}
