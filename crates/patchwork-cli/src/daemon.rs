//! `patchwork daemon`: the background sync daemon.

use anyhow::Result;
use clap::Subcommand;
use patchwork_core::event::Silent;
use patchwork_core::paths::Paths;
use patchwork_sync::daemon::{self, service};

use crate::open;

#[derive(Subcommand)]
pub(crate) enum DaemonCommand {
    /// Say whether the daemon's set up and running, and what it's done
    /// lately.
    Status,
    /// Have the daemon start whenever you log in, and start it now.
    Install,
    /// Stop the daemon, and stop it starting when you log in.
    Uninstall,
    /// Start the daemon now, if it isn't running.
    Start,
    /// Stop the daemon until it's next started.
    Stop,
    /// Run the daemon here until it's stopped. This is what `install` has
    /// the system run.
    Run,
}

pub(crate) async fn run(paths: Paths, command: DaemonCommand) -> Result<()> {
    let dir = paths.sync();
    let program = || service::Program::current(&["daemon", "run"]);
    match command {
        DaemonCommand::Status => {
            match service::installed()? {
                Some(how) => println!("installed as {how}: starts when you log in"),
                None => {
                    println!("not installed: `patchwork daemon install` starts it when you log in")
                }
            }
            let Some(status) = daemon::status(&dir)? else {
                println!("not running");
                return Ok(());
            };
            let state = match status.state {
                daemon::State::Starting => "starting",
                daemon::State::Syncing => "syncing",
                daemon::State::Paused => "paused while patchwork is open elsewhere",
                daemon::State::Waiting => "waiting for patchwork to close elsewhere",
                daemon::State::Idle => "idle: nothing is synced on this computer",
            };
            println!(
                "running (pid {}, patchwork {}, since {}): {state}",
                status.pid, status.version, status.started
            );
            if status.version != env!("CARGO_PKG_VERSION") {
                println!(
                    "it's a different version from this one; `patchwork daemon stop` then `start` restarts it"
                );
            }
            for activity in status.activity.iter().rev().take(10).rev() {
                let mark = if activity.error { " error:" } else { "" };
                println!("  {}{mark} {}", activity.at, activity.message);
            }
            println!("log: {}", status.log.display());
        }
        DaemonCommand::Install => {
            daemon::install(&dir, &program()?)?;
            println!(
                "installed; the daemon syncs in the background whenever you're logged in \
                 and neither the app nor the CLI is running"
            );
        }
        DaemonCommand::Uninstall => {
            daemon::uninstall(&dir)?;
            println!(
                "uninstalled; profiles only sync while the app or `patchwork sync run` is running"
            );
        }
        DaemonCommand::Start => {
            if daemon::status(&dir)?.is_some() {
                println!("already running");
            } else {
                service::start(&program()?)?;
                println!("started");
            }
        }
        DaemonCommand::Stop => {
            if daemon::stop(&dir)? {
                println!("stopped");
            } else {
                println!("not running");
            }
        }
        DaemonCommand::Run => {
            #[cfg(windows)]
            hide_console();
            let opening = paths.clone();
            tokio::select! {
                result = daemon::serve(&dir, || open(opening.clone(), Silent)) => result?,
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
    Ok(())
}

/// Lets go of the console window a login item's given, so the daemon
/// doesn't leave one open.
#[cfg(windows)]
fn hide_console() {
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }
    // SAFETY: takes no arguments, and only detaches this process from its
    // console
    unsafe {
        FreeConsole();
    }
}
