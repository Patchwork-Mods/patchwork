//! Launching the game.

use anyhow::{Context, Result};
use patchwork_core::launch::LaunchOption;
use patchwork_core::{DeployStatus, Patchwork};

use crate::common::selected;
use crate::deploy::finish;

pub(crate) fn print_launch_options(options: &[LaunchOption]) {
    for option in options {
        let default = if option.default { " (default)" } else { "" };
        match &option.unavailable {
            Some(why) => println!("{}  {}  [unavailable: {why}]", option.id, option.name),
            None => println!("{}  {}{default}", option.id, option.name),
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct Launch {
    /// Which way to start it, from `--list`. Defaults to the game's
    /// default one.
    option: Option<String>,
    /// What to run it with this time, from `game runners`, instead of
    /// what's picked for the game.
    #[arg(long)]
    runner: Option<String>,
    /// List the ways the game can be started.
    #[arg(long, conflicts_with_all = ["option", "runner", "dry_run", "no_deploy"])]
    list: bool,
    /// Show the command that would start the game without running it.
    #[arg(long)]
    dry_run: bool,
    /// Start the game with whatever's deployed now.
    #[arg(long)]
    no_deploy: bool,
}

/// `patchwork launch`.
pub(crate) async fn launch(pw: &Patchwork, explicit: &Option<String>, args: Launch) -> Result<()> {
    let Launch {
        option,
        runner,
        list,
        dry_run,
        no_deploy,
    } = args;
    let (profile, loaded) = selected(pw, explicit)?;
    let options = pw.profile_launch_options(&loaded)?.with_context(|| {
        format!(
            "patchwork doesn't know where {} is installed; run `patchwork game set-path {} <dir>`",
            loaded.game, loaded.game
        )
    })?;
    if list {
        print_launch_options(&options);
        return Ok(());
    }
    if pw.uses_instance(&loaded)? {
        // says why it can't, before anything's deployed for nothing
        pw.plan_profile_launch(&profile, option.as_deref(), runner.as_deref())?;
    }
    if !no_deploy && pw.deploy_status(&profile)? != DeployStatus::Deployed {
        if dry_run {
            println!("would deploy {} first", loaded.label());
        } else {
            let plan = pw.plan_deploy(&profile).await?;
            finish(pw, &plan, false)?;
        }
    }
    let launch = pw.plan_profile_launch(&profile, option.as_deref(), runner.as_deref())?;
    let name = options
        .iter()
        .find(|o| o.id == launch.option)
        .map_or(launch.option.as_str(), |o| o.name.as_str());
    if dry_run {
        println!(
            "would start {name} {}, in {}:",
            launch.via,
            launch.cwd.display()
        );
        println!("{launch}");
        if let Some(o) = &launch.prefix_overrides {
            println!(
                "after making sure Wine in {} loads the game's own {}",
                o.prefix.display(),
                o.dlls.join(", ")
            );
        }
        return Ok(());
    }
    if launch.needs_steam() && patchwork_core::launch::steam_running() == Some(false) {
        eprintln!("warning: Steam doesn't look to be running, and the game may need it");
    }
    let started = launch.spawn(&pw.paths().launch_log(&loaded.game))?;
    if !started.added_overrides.is_empty() {
        println!(
            "told Wine in the game's prefix to load the game's own {}",
            started.added_overrides.join(", ")
        );
    }
    println!("started {name} {}", launch.via);
    Ok(())
}
