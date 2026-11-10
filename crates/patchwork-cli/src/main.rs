use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use patchwork_core::Patchwork;
use patchwork_core::event::{Event, Reporter};
use patchwork_core::paths::Paths;

mod common;
mod config;
mod daemon;
mod deploy;
mod deps;
mod game;
mod launch;
mod mods;
mod options;
mod profile;
mod selfupdate;
mod sync;

use crate::common::MoveTo;
use crate::config::ConfigCommand;
use crate::daemon::DaemonCommand;
use crate::deploy::{OrderCommand, print_op};
use crate::game::GameCommand;
use crate::profile::ProfileCommand;
use crate::selfupdate::self_update;
use crate::sync::SyncCommand;

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
    /// Settings for sources, such as API keys.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Add a mod to the profile, as `source:id`, a link to its page (or a
    /// download) or a path to a local mod. If the mod has several files
    /// to pick from, like a GitHub release's, asks which.
    Add(mods::Add),
    /// Search a source for mods for the profile's game. With no text, lists
    /// its most popular (or, with --sort, newest, ...) mods.
    Search(mods::Search),
    /// Remove a mod from the profile.
    Remove { name: String },
    /// Enable a mod.
    Enable { name: String },
    /// Disable a mod without removing it.
    Disable { name: String },
    /// Freeze a mod at a version, so updates leave it alone.
    Pin { name: String, version: String },
    /// Let a pinned mod follow the newest version again.
    Unpin { name: String },
    /// List the versions a mod can be pinned to, newest first.
    Versions { name: String },
    /// Show which versions of the game a mod works with, or say so in place
    /// of what its source says, e.g. `supports EngineFixes 1.5.97 ">=1.6.640"`.
    Supports(deps::Supports),
    /// Show what the profile picks about the game it runs (its mod loader,
    /// say), or pick it, e.g. `env loader fabric`. Picking relocks every
    /// mod to suit.
    Env(options::Env),
    /// Show whether the profile is deployed to an instance (a folder of its
    /// own) or to the game's folder, or pick, e.g. `instance on`. Only for
    /// games that have instances, and only for this machine.
    Instance {
        /// `on` or `off`.
        #[arg(value_parser = clap::builder::BoolishValueParser::new())]
        value: Option<bool>,
        /// Go back to the game's default.
        #[arg(long, conflicts_with = "value")]
        reset: bool,
    },
    /// Show the choices to make about a mod, like the options in its FOMOD
    /// installer, and which are picked, or pick them. Until some are picked,
    /// mods get the defaults, like the options an installer recommends.
    #[command(alias = "installer")]
    Options(options::Options),
    /// Show or change settings in config files, like the ones BepInEx
    /// makes for each mod. Without a file, lists the config files there
    /// are. The profile keeps only the settings you change, and deploying
    /// applies them; settings changed in game are kept too.
    Settings(options::Settings),
    /// Show a mod's changelog, newest first.
    Changelog {
        name: String,
        /// Only show what's changed since the version it's locked at.
        #[arg(long)]
        new: bool,
    },
    /// List the mods in the profile, in the order they've been put in.
    /// That's the load order, apart from mods given a priority.
    List {
        /// Only list mods matching this, best match first. Mods are found by
        /// name, and by author and description once their details have been
        /// fetched (by sorting by tags, say). Close misspellings still match.
        query: Vec<String>,
        /// List them in the order they load in, with priorities applied:
        /// later mods win when two provide the same file.
        #[arg(long, conflicts_with = "query")]
        load_order: bool,
    },
    /// Move a mod in the profile's order, which is the load order among
    /// mods with the same priority. Later mods win when two provide the
    /// same file; a mod's dependencies always load before it regardless.
    Move {
        name: String,
        #[command(flatten)]
        to: MoveTo,
    },
    /// Show or set a mod's priority, 0 unless set. Mods with a higher
    /// priority load later, so they win when two provide the same file,
    /// wherever they are in the list; mods with the same priority go by
    /// their place in it. Setting it doesn't move the mod in the list.
    Priority {
        name: String,
        /// The new priority; can be negative.
        #[arg(allow_negative_numbers = true)]
        priority: Option<i32>,
    },
    /// List the dependencies the profile's mods brought in, and which mods
    /// need each. Only locked mods count.
    #[command(alias = "dependencies")]
    Deps {
        /// List the profile's own mods too, with what needs them.
        #[arg(long)]
        all: bool,
    },
    /// List what a mod lists that patchwork didn't install with it, because
    /// its source says it's optional or doesn't say whether it's needed
    /// (Nexus Mods' requirements, say), and whether the profile has each.
    #[command(name = "soft-deps")]
    SoftDeps {
        /// The mod; without one, every mod that lists any.
        name: Option<String>,
        /// Only the ones the profile doesn't have and that aren't ignored.
        #[arg(long)]
        missing: bool,
    },
    /// Stop pointing out one of a mod's soft dependencies (see `patchwork
    /// soft-deps`), given by name or `source:id`, because you've got it
    /// some other way or don't want it.
    Ignore(deps::Ignore),
    /// Have a mod stand in for a dependency other mods need, like a fork of
    /// it, so mods that need it get this one and it isn't downloaded. The
    /// dependency is given by its name in `patchwork deps` or as
    /// `source:id`. Without any, shows what the mod stands in for.
    Replace(deps::Replace),
    /// Put things in the profile's mods in an order of their own, apart from
    /// the mods' order, for games that have them: Skyrim's plugins (.esp,
    /// .esm and .esl files), say.
    #[command(alias = "plugins")]
    Order {
        /// Which list, for games with more than one: its id, or the last
        /// part of it (`plugins` for `bethesda.plugins`).
        #[arg(long)]
        list: Option<String>,
        #[command(subcommand)]
        action: OrderCommand,
    },
    /// Run something an extension offers for the profile's game, like
    /// sorting Skyrim's plugins with LOOT. Without an id, list what there
    /// is. Only locked mods count.
    Action {
        /// Which: its id, or the last part of it (`sort` for `loot.sort`).
        id: Option<String>,
        /// Show what it would change without saving it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show which mods provide the same files, and whose copy of each is
    /// deployed: the one latest in the load order. Only locked mods count.
    Conflicts {
        /// List this mod's files that clash with other mods', rather than
        /// how many each mod has.
        name: Option<String>,
    },
    /// Sort the profile's mods. Mods that tie keep their order.
    Sort(mods::Sort),
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
        /// Read every deployed file to check it hasn't changed, rather than
        /// just that it's there and the right size, and put back any that
        /// have.
        #[arg(long)]
        verify: bool,
    },
    /// Keep what tools (DynDOLOD, Nemesis, Synthesis, BodySlide...) wrote
    /// into the game's folder since the profile was deployed, as a mod, and
    /// deploy again. Without a name, just lists what would be kept.
    Capture {
        /// The mod to keep the files in. It's added to the end of the
        /// profile, or added to if it was captured into before.
        name: Option<String>,
    },
    /// Run a tool (DynDOLOD, Nemesis, Synthesis, BodySlide...) for the
    /// profile, deploying it first if it needs it, and keep what it writes
    /// into the game's folder as a mod, as `capture` does. A Windows tool
    /// runs in the game's Wine prefix.
    Tool(deploy::Tool),
    /// List the profile's past deploys and what changed in each.
    History,
    /// Put the profile's mods and their versions back how they were at a
    /// past deploy, from `patchwork history`. Deploy afterwards to apply it.
    Rollback {
        /// Which deploy. Defaults to the last one, undoing every change
        /// since.
        number: Option<u32>,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Start the profile's game, deploying the profile first if it isn't
    /// deployed as it is now.
    Launch(launch::Launch),
    /// Remove everything patchwork deployed to a game, or to the profile's
    /// instance if it has one.
    Undeploy {
        /// Defaults to wherever the profile is deployed to.
        game: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Use a file you downloaded yourself for a mod that has to be
    /// downloaded from its page, then lock again.
    Provide {
        /// The mod, by its name in the profile or as `source:id`.
        name: String,
        file: PathBuf,
    },
    /// Download the file a link from your browser is for, like the `nxm://`
    /// links Nexus's "Mod Manager Download" button hands out.
    Link { link: String },
    /// Show the current profile and what's deployed.
    Status,
    /// Keep profiles in step with your other computers, share them with
    /// friends, or let others follow them. On its own, syncs every synced
    /// profile once with whoever's online.
    Sync {
        #[command(subcommand)]
        command: Option<SyncCommand>,
    },
    /// The sync daemon, which keeps synced profiles in step in the
    /// background while neither the app nor the CLI is running.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Update patchwork to the newest release.
    SelfUpdate {
        /// Only say whether there's a newer release.
        #[arg(long)]
        check: bool,
    },
}

struct Printer;

impl Reporter for Printer {
    fn report(&self, event: Event) {
        match event {
            Event::Resolving { name } => eprintln!("  resolving {name}"),
            Event::Fetching { name } => eprintln!("   fetching {name}"),
            Event::Building { step } => eprintln!("   building {step}"),
            Event::Applying(op) => print_op(&op),
            Event::Warning(warning) => eprintln!("warning: {warning}"),
            Event::Notice(notice) => eprintln!("note: {notice}"),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            if let Some(patchwork_core::Error::NeedsDownload(_)) = e.downcast_ref() {
                eprintln!(
                    "\nDownload each one from its page, then run `patchwork provide <mod> <file>`,\n\
                     or `patchwork link <link>` with the link a \"Mod Manager Download\" button gives you."
                );
            }
            ExitCode::FAILURE
        }
    }
}

/// Opens patchwork with every extension there is.
pub(crate) fn open(
    paths: Paths,
    reporter: impl Reporter + 'static,
) -> patchwork_core::Result<Patchwork> {
    patchwork_builtin::register(Patchwork::builder(paths))
        .reporter(reporter)
        .open()
}

async fn run(cli: Cli) -> Result<()> {
    // Doesn't need the registry or profiles, so don't let problems there get
    // in the way of fixing them with an update.
    if let Command::SelfUpdate { check } = cli.command {
        return self_update(check).await;
    }
    let paths = Paths::from_env()?;
    let command = match cli.command {
        Command::Daemon(command) => return daemon::run(paths, command).await,
        command => command,
    };

    // the daemon has patchwork open while it runs, so it steps aside until
    // this is done
    let _paused = patchwork_sync::daemon::pause(&paths.sync())?;
    let pw = open(paths, Printer)?;
    for problem in &pw.registry().problems {
        eprintln!("warning: skipped registry entry: {problem}");
    }

    match command {
        Command::Game(command) => game::run(&pw, command)?,
        Command::Config(command) => config::run(&pw, command).await?,
        Command::Sync { command } => sync::run(&pw, command, &cli.profile).await?,
        Command::Profile(command) => profile::run(&pw, command, &cli.profile).await?,
        Command::Add(args) => mods::add(&pw, &cli.profile, args).await?,
        Command::Search(args) => mods::search(&pw, &cli.profile, args).await?,
        Command::Remove { name } => mods::remove(&pw, &cli.profile, name)?,
        Command::Enable { name } => mods::enable(&pw, &cli.profile, name)?,
        Command::Disable { name } => mods::disable(&pw, &cli.profile, name)?,
        Command::Pin { name, version } => mods::pin(&pw, &cli.profile, name, version).await?,
        Command::Unpin { name } => mods::unpin(&pw, &cli.profile, name).await?,
        Command::Supports(args) => deps::supports(&pw, &cli.profile, args)?,
        Command::Env(args) => options::env(&pw, &cli.profile, args).await?,
        Command::Instance { value, reset } => options::instance(&pw, &cli.profile, value, reset)?,
        Command::Options(args) => options::options(&pw, &cli.profile, args)?,
        Command::Settings(args) => options::settings(&pw, &cli.profile, args)?,
        Command::Versions { name } => mods::versions(&pw, &cli.profile, name).await?,
        Command::Changelog { name, new } => mods::changelog(&pw, &cli.profile, name, new).await?,
        Command::List { query, load_order } => mods::list(&pw, &cli.profile, query, load_order)?,
        Command::Move { name, to } => mods::move_mod(&pw, &cli.profile, name, to)?,
        Command::Deps { all } => deps::deps(&pw, &cli.profile, all)?,
        Command::SoftDeps { name, missing } => deps::soft_deps(&pw, &cli.profile, name, missing)?,
        Command::Ignore(args) => deps::ignore(&pw, &cli.profile, args)?,
        Command::Replace(args) => deps::replace(&pw, &cli.profile, args).await?,
        Command::Priority { name, priority } => mods::priority(&pw, &cli.profile, name, priority)?,
        Command::Order { list, action } => deploy::order(&pw, &cli.profile, list, action)?,
        Command::Action { id, dry_run } => deploy::action(&pw, &cli.profile, id, dry_run).await?,
        Command::Conflicts { name } => deps::conflicts(&pw, &cli.profile, name)?,
        Command::Sort(args) => mods::sort(&pw, &cli.profile, args).await?,
        Command::Lock { update } => deploy::lock(&pw, &cli.profile, update).await?,
        Command::Deploy { dry_run, verify } => {
            deploy::deploy(&pw, &cli.profile, dry_run, verify).await?
        }
        Command::Capture { name } => deploy::capture(&pw, &cli.profile, name).await?,
        Command::Tool(args) => deploy::tool(&pw, &cli.profile, args).await?,
        Command::History => deploy::history(&pw, &cli.profile)?,
        Command::Rollback { number, dry_run } => {
            deploy::rollback(&pw, &cli.profile, number, dry_run)?
        }
        Command::Launch(args) => launch::launch(&pw, &cli.profile, args).await?,
        Command::Undeploy { game, dry_run } => deploy::undeploy(&pw, &cli.profile, game, dry_run)?,
        Command::Provide { name, file } => mods::provide(&pw, &cli.profile, name, file).await?,
        Command::Link { link } => mods::link(&pw, link).await?,
        Command::Status => deploy::status(&pw, &cli.profile)?,
        Command::SelfUpdate { .. } | Command::Daemon(_) => unreachable!("handled before opening"),
    }
    Ok(())
}
