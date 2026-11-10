//! `patchwork self-update`.

use anyhow::{Result, bail};
use patchwork_core::paths::Paths;
use patchwork_core::update;
use patchwork_sync::daemon::{self, service};

/// What dist calls this app: the package name, not the binary's.
const DIST_APP: &str = "patchwork-cli";

pub(crate) async fn self_update(check: bool) -> Result<()> {
    let found = update::check(DIST_APP).await?;
    let current = &found.current;
    let Some(latest) = found.latest.as_deref().filter(|_| found.available) else {
        println!("patchwork {current} is up to date");
        return Ok(());
    };
    println!("patchwork {latest} is available (you have {current})");
    if !found.installable {
        let how = format!(
            "this copy wasn't installed by patchwork's installer, so update it the way you \
             installed it (if it came with the Patchwork app, updating the app updates it), \
             or get it from {}",
            update::RELEASES_URL
        );
        // Only a failure when asked to do the update.
        if check {
            println!("{how}");
            return Ok(());
        }
        bail!(how);
    }
    if check {
        println!("run `patchwork self-update` to update");
        return Ok(());
    }
    // the daemon would carry on running the old version until the next
    // login, so it's stopped while updating and started again from the new
    // one; worked out first, since where this program is changes once it's
    // been replaced
    let program = service::Program::current(&["daemon", "run"])?;
    let stopped = stop_own_daemon();
    let installed = update::install(DIST_APP, true).await;
    if stopped {
        match service::start(&program) {
            Ok(()) => println!("restarted the sync daemon"),
            Err(e) => eprintln!(
                "warning: couldn't restart the sync daemon ({e:#}); `patchwork daemon start` starts it"
            ),
        }
    }
    match installed? {
        Some(new) => println!("updated patchwork {current} -> {new}"),
        None => println!("patchwork {current} is up to date"),
    }
    Ok(())
}

/// Stops the sync daemon if it's running from this program, so it can be
/// updated. True if it was. A daemon the app runs is left to the app, which
/// restarts it when it's updated.
fn stop_own_daemon() -> bool {
    let Ok(paths) = Paths::from_env() else {
        return false;
    };
    let dir = paths.sync();
    let Ok(Some(status)) = daemon::status(&dir) else {
        return false;
    };
    let ours = std::env::current_exe().and_then(|e| e.canonicalize()).ok();
    let theirs = status.exe.and_then(|e| e.canonicalize().ok());
    if ours.is_none() || ours != theirs {
        return false;
    }
    match daemon::stop(&dir) {
        Ok(stopped) => stopped,
        Err(e) => {
            eprintln!("warning: couldn't stop the sync daemon to update it: {e:#}");
            false
        }
    }
}
