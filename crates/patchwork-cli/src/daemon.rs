//! `patchwork daemon`: the background daemon and its tasks.

use anyhow::Result;
use clap::Subcommand;
use patchwork_core::event::Silent;
use patchwork_core::paths::Paths;
use patchwork_daemon::{self as daemon, DaemonSettings, service, settings};

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
    /// List the daemon's tasks, and whether each is on.
    Tasks,
    /// Turn the daemon on, so it runs whichever of its tasks are on.
    On,
    /// Turn the daemon off, so it runs none of its tasks; each keeps its
    /// own setting for when it's back on.
    Off,
    /// Turn one of the daemon's tasks on.
    Enable { task: String },
    /// Turn one of the daemon's tasks off.
    Disable { task: String },
    /// Say how long a periodic task leaves between runs, such as `6h`, or
    /// with `default` go back to its own schedule.
    Every { task: String, every: String },
    /// Turn a task's desktop notifications on or off, such as the update
    /// check's for mods you follow (`patchwork follow`).
    Notify {
        task: String,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Run the daemon here until it's stopped. This is what `install` has
    /// the system run.
    Run,
}

pub(crate) async fn run(paths: Paths, command: DaemonCommand) -> Result<()> {
    let program = || service::Program::current(&["daemon", "run"]);
    match command {
        DaemonCommand::Status => {
            match service::installed()? {
                Some(how) => println!("installed as {how}: starts when you log in"),
                None => {
                    println!("not installed: `patchwork daemon install` starts it when you log in")
                }
            }
            let Some(status) = daemon::status(&paths)? else {
                println!("not running");
                return Ok(());
            };
            let state = match status.state {
                daemon::State::Starting => "starting",
                daemon::State::Running => "running tasks",
                daemon::State::Paused => "paused while patchwork is open elsewhere",
                daemon::State::Waiting => "waiting for patchwork to close elsewhere",
                daemon::State::Idle => "idle: no task has anything to do for now",
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
            for task in &status.tasks {
                println!("  {}: {}", task.id, task_state(task));
            }
            for activity in status.activity.iter().rev().take(10).rev() {
                let mark = if activity.error { " error:" } else { "" };
                println!("  {}{mark} {}", activity.at, activity.message);
            }
            println!("log: {}", status.log.display());
        }
        DaemonCommand::Install => {
            daemon::install(&paths, &program()?)?;
            println!(
                "installed; the daemon runs its tasks in the background whenever you're \
                 logged in and neither the app nor the CLI is running (`patchwork daemon tasks` \
                 lists them)"
            );
        }
        DaemonCommand::Uninstall => {
            daemon::uninstall(&paths)?;
            println!(
                "uninstalled; profiles only sync while the app or `patchwork sync run` is running"
            );
        }
        DaemonCommand::Start => {
            if daemon::status(&paths)?.is_some() {
                println!("already running");
            } else {
                service::start(&program()?)?;
                println!("started");
            }
        }
        DaemonCommand::Stop => {
            if daemon::stop(&paths)? {
                println!("stopped");
            } else {
                println!("not running");
            }
        }
        DaemonCommand::Tasks => {
            let settings = DaemonSettings::load(&paths)?;
            if !settings.on() {
                println!("the daemon is off (`patchwork daemon on`), so none of these run");
            }
            for task in daemon::tasks() {
                let own = settings.task(task.id());
                let on = own.enabled.unwrap_or_else(|| task.default_enabled());
                let every = own
                    .every
                    .as_ref()
                    .map(|e| format!(", every {e}"))
                    .unwrap_or_default();
                let notify = match task.notifies() {
                    Some(_) if own.notifies() => ", notifies",
                    Some(_) => ", doesn't notify",
                    None => "",
                };
                println!(
                    "{} ({}{every}{notify}): {}",
                    task.id(),
                    if on { "on" } else { "off" },
                    task.description()
                );
            }
        }
        DaemonCommand::On => {
            settings::set_on(&paths, true)?;
            println!("the daemon is on");
        }
        DaemonCommand::Off => {
            settings::set_on(&paths, false)?;
            println!("the daemon is off; none of its tasks run until `patchwork daemon on`");
        }
        DaemonCommand::Enable { task } => {
            settings::set_enabled(&paths, known_task(&task)?, true)?;
            println!("{task} is on");
        }
        DaemonCommand::Disable { task } => {
            settings::set_enabled(&paths, known_task(&task)?, false)?;
            println!("{task} is off");
        }
        DaemonCommand::Every { task, every } => {
            let every = (every != "default").then_some(every.as_str());
            settings::set_every(&paths, known_task(&task)?, every)?;
            match every {
                Some(every) => println!("{task} runs every {every}"),
                None => println!("{task} runs on its own schedule"),
            }
        }
        DaemonCommand::Notify { task, on } => {
            let id = known_task(&task)?;
            if !daemon::tasks()
                .iter()
                .any(|t| t.id() == id && t.notifies().is_some())
            {
                anyhow::bail!("{task} doesn't send notifications");
            }
            settings::set_notify(&paths, id, on == "on")?;
            println!("{task}'s notifications are {on}");
        }
        DaemonCommand::Run => {
            #[cfg(windows)]
            hide_console();
            let opening = paths.clone();
            tokio::select! {
                result = daemon::serve(&paths, daemon::tasks(), || open(opening.clone(), Silent)) => result?,
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
    Ok(())
}

/// `task` if the daemon has a task by that name.
fn known_task(task: &str) -> Result<&str> {
    if daemon::tasks().iter().any(|t| t.id() == task) {
        Ok(task)
    } else {
        let all: Vec<_> = daemon::tasks().iter().map(|t| t.id()).collect();
        anyhow::bail!(
            "there's no daemon task `{task}`; there's {}",
            all.join(", ")
        )
    }
}

fn task_state(task: &daemon::TaskStatus) -> String {
    if !task.enabled {
        "off".to_owned()
    } else if task.running {
        "running".to_owned()
    } else if let Some(next) = task.next {
        format!("next at {next}")
    } else {
        "waiting for something to do".to_owned()
    }
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
