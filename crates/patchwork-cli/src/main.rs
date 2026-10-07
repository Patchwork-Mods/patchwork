use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use patchwork_core::Patchwork;
use patchwork_core::deploy::{DeployOp, DeployPlan};
use patchwork_core::event::{Event, Reporter};
use patchwork_core::paths::Paths;
use patchwork_core::profile::ProfileRef;
use patchwork_deployers::RulesDeployer;
use patchwork_sources::LocalSource;

#[derive(Parser)]
#[command(name = "patchwork", version, about = "A universal mod manager")]
struct Cli {
    /// Profile to act on, as <game>/<name>. Defaults to the current profile.
    #[arg(short, long, global = true)]
    profile: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage games.
    #[command(subcommand)]
    Game(GameCommand),
    /// Manage profiles.
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Add a mod to the profile, as `source:id` or a path to a local mod.
    Add {
        spec: String,
        /// What to call the mod in this profile.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
        /// Add it without enabling it.
        #[arg(long)]
        disabled: bool,
    },
    /// Remove a mod from the profile.
    Remove { name: String },
    /// Enable a mod.
    Enable { name: String },
    /// Disable a mod without removing it.
    Disable { name: String },
    /// List the mods in the profile.
    List,
    /// Resolve and fetch every mod in the profile.
    Lock {
        /// Re-resolve mods that are already locked.
        #[arg(long)]
        update: bool,
    },
    /// Deploy the profile's enabled mods to the game.
    Deploy {
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove everything patchwork deployed to a game.
    Undeploy {
        /// Defaults to the profile's game.
        game: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the current profile and what's deployed.
    Status,
}

#[derive(Subcommand)]
enum GameCommand {
    /// List known games.
    List,
    /// Show a game's definition.
    Show { id: String },
    /// Tell patchwork where a game is installed.
    SetPath { id: String, path: PathBuf },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Create a profile (as <game>/<name>) and switch to it.
    New { profile: String },
    /// List profiles.
    List,
    /// Switch to a profile.
    Use { profile: String },
}

struct Printer;

impl Reporter for Printer {
    fn report(&self, event: Event) {
        match event {
            Event::Resolving { name } => eprintln!("  resolving {name}"),
            Event::Fetching { name } => eprintln!("   fetching {name}"),
            Event::Applying(op) => print_op(&op),
            Event::Warning(warning) => eprintln!("warning: {warning}"),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let pw = Patchwork::builder(Paths::from_env()?)
        .source(LocalSource)
        .deployer(RulesDeployer)
        .reporter(Printer)
        .open()?;
    for problem in &pw.registry().problems {
        eprintln!("warning: skipped registry entry: {problem}");
    }

    match cli.command {
        Command::Game(command) => game(&pw, command)?,
        Command::Profile(command) => profile(&pw, command)?,
        Command::Add {
            spec,
            name,
            version,
            disabled,
        } => {
            let profile = selected(&pw, &cli.profile)?;
            let spec = pw.parse_spec(&spec, version)?;
            let added = pw.add_mod(&profile, spec, name, !disabled).await?;
            println!("added {} to {profile}", added.name);
        }
        Command::Remove { name } => {
            let profile = selected(&pw, &cli.profile)?;
            pw.remove_mod(&profile, &name)?;
            println!("removed {name} from {profile}");
        }
        Command::Enable { name } => {
            pw.set_enabled(&selected(&pw, &cli.profile)?, &name, true)?;
            println!("enabled {name}");
        }
        Command::Disable { name } => {
            pw.set_enabled(&selected(&pw, &cli.profile)?, &name, false)?;
            println!("disabled {name}");
        }
        Command::List => {
            let profile = selected(&pw, &cli.profile)?;
            let loaded = pw.load_profile(&profile)?;
            if loaded.mods.is_empty() {
                println!("{profile} has no mods");
            }
            for m in &loaded.mods {
                let mark = if m.enabled { 'x' } else { ' ' };
                let version = m
                    .version
                    .as_deref()
                    .map(|v| format!(" @ {v}"))
                    .unwrap_or_default();
                println!("[{mark}] {}  ({}:{}{version})", m.name, m.source, m.id);
            }
        }
        Command::Lock { update } => {
            let profile = selected(&pw, &cli.profile)?;
            let lock = pw.lock(&profile, update).await?;
            println!("locked {} mods for {profile}", lock.mods.len());
        }
        Command::Deploy { dry_run } => {
            let profile = selected(&pw, &cli.profile)?;
            let plan = pw.plan_deploy(&profile).await?;
            finish(&pw, &plan, dry_run)?;
        }
        Command::Undeploy { game, dry_run } => {
            let game = match game {
                Some(game) => game,
                None => selected(&pw, &cli.profile)?.game,
            };
            let plan = pw.plan_undeploy(&game)?;
            finish(&pw, &plan, dry_run)?;
        }
        Command::Status => status(&pw, &cli.profile)?,
    }
    Ok(())
}

fn game(pw: &Patchwork, command: GameCommand) -> Result<()> {
    match command {
        GameCommand::List => {
            for entry in pw.registry().games() {
                let game = &entry.value;
                let installed = match pw.game_install(&game.id)? {
                    Some(install) => install.path.display().to_string(),
                    None => "not set up".to_owned(),
                };
                println!("{}  {} [{}]  {installed}", game.id, game.name, entry.origin);
            }
        }
        GameCommand::Show { id } => {
            let entry = pw.registry().game(&id)?;
            println!("# origin: {}, hash: {}", entry.origin, entry.hash.short());
            print!("{}", toml::to_string_pretty(&entry.value)?);
        }
        GameCommand::SetPath { id, path } => {
            let install = pw.set_game_path(&id, &path)?;
            println!("{id} is installed at {}", install.path.display());
        }
    }
    Ok(())
}

fn profile(pw: &Patchwork, command: ProfileCommand) -> Result<()> {
    match command {
        ProfileCommand::New { profile } => {
            let profile: ProfileRef = profile.parse()?;
            pw.create_profile(&profile)?;
            pw.use_profile(&profile)?;
            println!("created {profile} and switched to it");
        }
        ProfileCommand::List => {
            let current = pw.current_profile()?;
            for profile in pw.profiles()? {
                let mark = if Some(&profile) == current.as_ref() {
                    '*'
                } else {
                    ' '
                };
                println!("{mark} {profile}");
            }
        }
        ProfileCommand::Use { profile } => {
            let profile: ProfileRef = profile.parse()?;
            pw.use_profile(&profile)?;
            println!("switched to {profile}");
        }
    }
    Ok(())
}

fn status(pw: &Patchwork, explicit: &Option<String>) -> Result<()> {
    let profile = match explicit {
        Some(p) => Some(p.parse()?),
        None => pw.current_profile()?,
    };
    let Some(profile) = profile else {
        println!("no profile selected");
        return Ok(());
    };
    let loaded = pw.load_profile(&profile)?;
    let enabled = loaded.mods.iter().filter(|m| m.enabled).count();
    println!(
        "profile: {profile} ({enabled}/{} mods enabled)",
        loaded.mods.len()
    );
    match pw.game_install(&profile.game)? {
        Some(install) => println!("game:    {}", install.path.display()),
        None => println!(
            "game:    not set up; run `patchwork game set-path {} <dir>`",
            profile.game
        ),
    }
    let state = pw.deploy_state(&profile.game)?;
    match &state.profile {
        Some(deployed) if !state.files.is_empty() => {
            println!("deployed: {deployed} ({} files)", state.files.len())
        }
        _ => println!("deployed: nothing"),
    }
    Ok(())
}

fn finish(pw: &Patchwork, plan: &DeployPlan, dry_run: bool) -> Result<()> {
    for warning in &plan.warnings {
        eprintln!("warning: {warning}");
    }
    if plan.is_noop() {
        println!("nothing to do ({})", in_place(plan.unchanged));
        return Ok(());
    }
    if dry_run {
        plan.ops.iter().for_each(print_op);
        println!(
            "would make {} ({})",
            changes(plan.ops.len()),
            in_place(plan.unchanged)
        );
        return Ok(());
    }
    pw.apply(plan)
        .context("deploy failed partway; what was done is recorded and can be undeployed")?;
    println!(
        "made {} ({})",
        changes(plan.ops.len()),
        in_place(plan.unchanged)
    );
    Ok(())
}

fn changes(n: usize) -> String {
    format!("{n} change{}", if n == 1 { "" } else { "s" })
}

fn in_place(n: usize) -> String {
    format!("{n} file{} already in place", if n == 1 { "" } else { "s" })
}

fn print_op(op: &DeployOp) {
    match op {
        DeployOp::Remove { dst } => println!("  - {dst}"),
        DeployOp::Backup { dst } => println!("  ~ {dst} (backing up original)"),
        DeployOp::Place { dst, mod_name, .. } => println!("  + {dst}  [{mod_name}]"),
    }
}

fn selected(pw: &Patchwork, explicit: &Option<String>) -> Result<ProfileRef> {
    if let Some(profile) = explicit {
        return Ok(profile.parse()?);
    }
    match pw.current_profile()? {
        Some(profile) => Ok(profile),
        None => bail!("no profile selected; create one with `patchwork profile new <game>/<name>`"),
    }
}
