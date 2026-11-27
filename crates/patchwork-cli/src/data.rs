//! `patchwork data`: where patchwork keeps its own files, and moving its
//! data folder somewhere else.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use patchwork_core::Patchwork;
use patchwork_core::cancel::Cancel;
use patchwork_core::paths::{Locations, Paths};
use patchwork_daemon::service;

use crate::open;

#[derive(Subcommand)]
pub(crate) enum DataCommand {
    /// Move the data folder (profiles, mods' files, backups and what's
    /// deployed) somewhere else, such as a bigger drive, bringing over what's
    /// in it. What's deployed stays deployed. The folder can be a new one, or
    /// one with other things in, as long as none of them have the same names
    /// as what's in the data folder.
    Move {
        to: PathBuf,
        /// Only say what it would move.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Runs `command`, or with none says where everything is. Opens patchwork
/// itself, since moving needs the daemon stopped rather than paused.
pub(crate) fn run(paths: Paths, command: Option<DataCommand>) -> Result<()> {
    match command {
        None => {
            let pw = open(paths, crate::Printer)?;
            show(&pw)
        }
        Some(DataCommand::Move { to, dry_run }) => move_data(paths, to, dry_run),
    }
}

fn show(pw: &Patchwork) -> Result<()> {
    let paths = pw.paths();
    let moved = Locations::load(paths)?.data.is_some();
    println!("settings   {}", paths.config.display());
    println!(
        "data       {}{}",
        paths.data.display(),
        if moved { " (moved)" } else { "" }
    );
    println!("  mods     {}", paths.store().display());
    println!("cache      {}", paths.cache.display());
    Ok(())
}

fn move_data(paths: Paths, to: PathBuf, dry_run: bool) -> Result<()> {
    let to = std::path::absolute(&to)?;
    if dry_run {
        let _paused = patchwork_daemon::pause(&paths)?;
        let pw = open(paths, crate::Printer)?;
        return print_plan(&pw, &to).map(drop);
    }
    // it has files in the data folder open while it runs, even paused
    let was_running = patchwork_daemon::stop(&paths)?;
    let pw = open(paths, crate::Printer)?;
    print_plan(&pw, &to)?;
    let mut shown = 0;
    let moved = pw.move_data(&to, &Cancel::never(), &mut |p| {
        let percent = (p.done * 100).checked_div(p.total).unwrap_or(100);
        if percent != shown {
            shown = percent;
            eprint!("\r{percent}%");
            let _ = std::io::stderr().flush();
        }
    });
    eprintln!();
    drop(pw);
    if was_running {
        restart_daemon();
    }
    moved?;
    println!(
        "Moved patchwork's data to {}. The old folder is removed the next time patchwork opens.",
        to.display()
    );
    Ok(())
}

/// Says what moving to `to` would do, failing if it can't.
fn print_plan(pw: &Patchwork, to: &std::path::Path) -> Result<()> {
    let plan = pw.plan_data_move(to)?;
    if !plan.conflicts.is_empty() {
        bail!(
            "{} already has {}, which the data folder has too; pick an empty folder, or move those \
             out of the way first",
            to.display(),
            plan.conflicts.join(", ")
        );
    }
    let how = match plan.same_drive {
        Some(true) => "moving",
        Some(false) => "copying",
        None => "moving or copying",
    };
    println!(
        "{how} {} files ({:.1} MB) from {} to {}",
        plan.files,
        plan.bytes as f64 / 1e6,
        plan.from.display(),
        plan.to.display()
    );
    Ok(())
}

fn restart_daemon() {
    let started = service::Program::current(&["daemon", "run"]).and_then(|p| service::start(&p));
    if let Err(e) = started {
        eprintln!(
            "warning: couldn't start the daemon again ({e:#}); `patchwork daemon start` does"
        );
    }
}
