use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use patchwork_core::action::ActionReport;
use patchwork_core::capture::ToolOutput;
use patchwork_core::compat::GameVersions;
use patchwork_core::conflicts::Conflicts;
use patchwork_core::deploy::{Check, DeployOp, DeployPlan, DeployTarget, Dest, RootId, RootOrigin};
use patchwork_core::event::{Event, Reporter, Silent};
use patchwork_core::issue::Severity;
use patchwork_core::launch::{self, LaunchOption};
use patchwork_core::list::List;
use patchwork_core::locate::Locator;
use patchwork_core::lock::mod_key;
use patchwork_core::options::{Answer, Field, FieldKind, Form, ItemState, Pick, Value};
use patchwork_core::order::{self, Position, SortKey};
use patchwork_core::paths::Paths;
use patchwork_core::profile::{Profile, ProfileMod, ProfileRef};
use patchwork_core::search::{SearchQuery, SortBy};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::source::Need;
use patchwork_core::update;
use patchwork_core::{
    ConfigFileInfo, ConfigView, DeployStatus, GameInstall, ImportOptions, ModOptionsView, Patchwork,
};
use patchwork_sync::daemon::{self, service};
use patchwork_sync::{Addresses, Joined, Node};

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
    /// Search a source for mods for the profile's game. With no text, lists
    /// its most popular (or, with --sort, newest, ...) mods.
    Search {
        text: Vec<String>,
        /// Which source to search. Needed if the game has more than one.
        #[arg(short, long)]
        source: Option<String>,
        /// Search for this game instead of the profile's.
        #[arg(long)]
        game: Option<String>,
        /// relevance, downloads, updated, newest or rating.
        #[arg(long)]
        sort: Option<SortBy>,
        /// Only show mods in this category, by id or name; repeat for any of
        /// several.
        #[arg(short, long = "category")]
        categories: Vec<String>,
        /// Leave out mods in this category, by id or name; repeat to leave out
        /// several.
        #[arg(short = 'x', long = "exclude")]
        excluded_categories: Vec<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        /// List the categories and sorts instead of searching.
        #[arg(long)]
        list_categories: bool,
    },
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
    Supports {
        name: String,
        /// Version ranges, any of which will do: `1.5.97`, `1.6.*`,
        /// `">=1.6.640 <1.7"`.
        versions: Vec<String>,
        /// Go back to what the mod's source says.
        #[arg(long, conflicts_with = "versions")]
        reset: bool,
    },
    /// Show what the profile picks about the game it runs (its mod loader,
    /// say), or pick it, e.g. `env loader fabric`. Picking relocks every
    /// mod to suit.
    Env {
        /// Which axis; all of them if left out.
        axis: Option<String>,
        value: Option<String>,
        /// Go back to the game's default.
        #[arg(long, requires = "axis", conflicts_with = "value")]
        reset: bool,
    },
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
    Options {
        name: String,
        /// Which choices, for mods with more than one kind: their id, or the
        /// last part of it (`installer` for `fomod.installer`).
        #[arg(long = "for", value_name = "ID")]
        of: Option<String>,
        /// Pick options in a group, as `GROUP=OPTION`, or `PAGE/GROUP=OPTION`
        /// if groups on different pages (an installer's steps) share a name.
        /// Separate several options with commas, or give none to pick none.
        /// Repeat for more groups; the picks are saved.
        #[arg(long = "pick", value_name = "GROUP=OPTIONS")]
        picks: Vec<String>,
        /// Set a setting, as `SETTING=VALUE`, or `GROUP/SETTING=VALUE` (or
        /// `PAGE/GROUP/SETTING=VALUE`) if more than one has its name.
        /// Repeat for more settings; they're saved.
        #[arg(long = "set", value_name = "SETTING=VALUE")]
        sets: Vec<String>,
        /// Save the options as they're shown, keeping the defaults in groups
        /// nothing's been picked in yet.
        #[arg(long)]
        save: bool,
        /// Forget the options picked, going back to the defaults.
        #[arg(long, conflicts_with_all = ["picks", "sets", "save"])]
        reset: bool,
    },
    /// Show or change settings in config files, like the ones BepInEx
    /// makes for each mod. Without a file, lists the config files there
    /// are. The profile keeps only the settings you change, and deploying
    /// applies them; settings changed in game are kept too.
    Settings {
        /// A config file, by its path as listed, or the end of it (like its
        /// name) if that's enough to tell which.
        file: Option<String>,
        /// Only list the config files this mod deploys.
        #[arg(long = "mod", value_name = "NAME", conflicts_with = "file")]
        of: Option<String>,
        /// Set a setting, as `SETTING=VALUE`, or `GROUP/SETTING=VALUE` if
        /// more than one group has it. Repeat for more; they're saved.
        #[arg(long = "set", value_name = "SETTING=VALUE", requires = "file")]
        sets: Vec<String>,
        /// Forget the settings changed in the file, so it's deployed as it is.
        #[arg(long, requires = "file", conflicts_with = "sets")]
        reset: bool,
    },
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
    Ignore {
        name: String,
        dependency: String,
        /// Point it out again.
        #[arg(long)]
        undo: bool,
    },
    /// Have a mod stand in for a dependency other mods need, like a fork of
    /// it, so mods that need it get this one and it isn't downloaded. The
    /// dependency is given by its name in `patchwork deps` or as
    /// `source:id`. Without any, shows what the mod stands in for.
    Replace {
        name: String,
        dependencies: Vec<String>,
        /// Stop it standing in for the dependencies given, or for all of
        /// them if none are.
        #[arg(long)]
        undo: bool,
    },
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
    Sort {
        /// name, source, tags, dependencies, dependents or updated, optionally with
        /// `:desc`. Repeat to break ties with the next key.
        #[arg(short, long = "by", required = true)]
        by: Vec<SortKey>,
        /// With `--by tags`, the tags to put first, most important first.
        /// Repeat or separate with commas.
        #[arg(short, long = "tag", value_delimiter = ',')]
        tags: Vec<String>,
        /// Show the new order without saving it.
        #[arg(long)]
        dry_run: bool,
    },
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
    Tool {
        /// The mod to keep what it writes in. Defaults to `<tool> Output`,
        /// after the exe's name.
        #[arg(long)]
        into: Option<String>,
        /// Show how it would be run without running it.
        #[arg(long)]
        dry_run: bool,
        /// The tool's exe.
        exe: PathBuf,
        /// Arguments for the tool.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
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
    Launch {
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
    },
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

#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct MoveTo {
    #[arg(long)]
    top: bool,
    #[arg(long)]
    bottom: bool,
    /// The position to move it to, counting from 1 as `list` does.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    to: Option<u32>,
    #[arg(long)]
    before: Option<String>,
    #[arg(long)]
    after: Option<String>,
}

impl MoveTo {
    fn position(self) -> Position {
        if self.top {
            Position::Top
        } else if self.bottom {
            Position::Bottom
        } else if let Some(n) = self.to {
            Position::Index(n as usize - 1)
        } else if let Some(name) = self.before {
            Position::Before(name)
        } else if let Some(name) = self.after {
            Position::After(name)
        } else {
            unreachable!("clap requires one of them")
        }
    }
}

#[derive(Subcommand)]
enum OrderCommand {
    /// List what's in the list, in order, from the profile's locked mods.
    List,
    /// Move something in the list. Some things always come before others
    /// (Skyrim's masters before other plugins), so they stay among their
    /// own kind.
    Move {
        name: String,
        #[command(flatten)]
        to: MoveTo,
    },
    /// Turn something in the list on.
    Enable { name: String },
    /// Leave something's files in place but turn it off.
    Disable { name: String },
}

#[derive(Subcommand)]
enum GameCommand {
    /// List known games.
    List,
    /// Show a game's definition.
    Show { id: String },
    /// Tell patchwork where a game is installed.
    SetPath { id: String, path: PathBuf },
    /// Look for where games are installed, in every Steam library. Give a
    /// game to look for only that one.
    Detect {
        id: Option<String>,
        /// Use what's found, if it's in one place. Without a game, this sets
        /// up every game that's found and isn't set up yet.
        #[arg(long)]
        set: bool,
    },
    /// List a game's settings: how mods are linked into it, its deployer's
    /// settings, how it's started, and where its other folders are.
    Settings { id: String },
    /// List what can run a game, and what each one's settings are.
    Runners { id: String },
    /// Change one of a game's settings (see `game settings`). `prefix` is
    /// the Wine prefix a Windows game runs in, `root.<id>` moves one of its
    /// other folders, `launch` is the way to start it by default (see
    /// `launch --list`), `runner` is what to run it with, and
    /// `runner.<runner>.<setting>` changes a runner's setting (see `game
    /// runners`).
    Set {
        id: String,
        key: String,
        value: String,
    },
    /// Go back to what the game's definition says for a setting.
    Unset { id: String, key: String },
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Link another of your own computers to this one: every profile on
    /// either, including ones made later, is kept in step on both. Prints a
    /// ticket to `join` with there, then keeps everything in step until
    /// stopped with Ctrl-C.
    Link,
    /// Stop linking this computer to your others. Profiles synced so far
    /// stay synced; stop each with `leave`.
    Unlink,
    /// Share the profile with friends, printing a ticket for them to
    /// `join` with, then keep it in step until stopped with Ctrl-C. Anyone
    /// with the ticket can add, remove and change mods, and every change
    /// goes to everyone.
    Share {
        /// Let others follow the profile instead: they get your mods and
        /// your changes, and can add mods of their own, which stay on their
        /// computer. Only you can change what everyone gets. Letting people
        /// change it can't be taken back once they have the ticket.
        #[arg(long)]
        read_only: bool,
        /// Leave this computer's IP addresses out of the ticket, so it can
        /// be posted publicly. Others reach it through its relay instead,
        /// and may still learn them once connected.
        #[arg(long)]
        hide_addresses: bool,
    },
    /// Join a profile someone shared, or link this computer to another of
    /// yours, with the ticket from `share` or `link`. The other computer
    /// has to be online. Mods are downloaded, but not deployed.
    Join { ticket: String },
    /// List the profiles this computer syncs.
    List,
    /// Stop syncing the profile here. It stays as it is, here and
    /// elsewhere, and linked computers no longer pick it up.
    Leave,
    /// List the other computers that can change the profile, by the id
    /// each prints with `patchwork sync id`, and the name you gave each.
    Peers,
    /// Give another computer a name to know it by here (see `peers`), or
    /// forget its name if none is given. Names aren't synced.
    Name { id: String, name: Vec<String> },
    /// Stop the computers with these ids (see `peers`) syncing the profile
    /// with anyone else. Everyone else moves to a new document, which
    /// they're told about when they're next online; whoever follows the
    /// profile needs a new ticket.
    Remove {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Keep every synced profile in step until stopped with Ctrl-C. Changes
    /// from other machines are saved and downloaded, never deployed.
    Run,
    /// Print the id other computers know this one by.
    Id,
    /// Give this computer a new id, e.g. if its old one got out alongside
    /// its addresses. Profiles stay synced, and the others are told the old
    /// id is gone the next time this one syncs. Tickets handed out before
    /// still work, but only while someone else syncing it is online.
    ResetId,
}

#[derive(Subcommand)]
enum DaemonCommand {
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

#[derive(Subcommand)]
enum ConfigCommand {
    /// List the settings sources take, and what they're set to.
    List,
    /// Change a setting, given as <source>.<key>, e.g. `nexus.api_key`.
    /// Leave out the value to type it in instead, which keeps secrets out
    /// of your shell's history.
    Set { name: String, value: Option<String> },
    /// Clear a setting.
    Unset { name: String },
    /// Check that a source's settings work, e.g. that its API key is valid.
    Check { source: String },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Create a profile (as <game>/<name>) and switch to it.
    New { profile: String },
    /// List profiles.
    List,
    /// Switch to a profile.
    Use { profile: String },
    /// Delete a profile, along with its lockfile and its deploy history. A
    /// deployed profile has to be undeployed first.
    Remove { profile: String },
    /// Rename a profile; it stays for the same game.
    Rename { profile: String, name: String },
    /// Copy a profile, mods and all, to a new name for the same game.
    Copy { profile: String, name: String },
    /// Export the profile to share it (see `profile formats`).
    Export {
        #[arg(short, long, default_value = "patchwork")]
        format: String,
        /// Where to write it; `-` for stdout. Files default to
        /// `<name>.<ext>`, and text (codes, links) to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import a profile from a file, a code or a list of links (`-` reads
    /// stdin), and switch to it.
    Import {
        input: String,
        /// Worked out from the input if not given.
        #[arg(short, long)]
        format: Option<String>,
        /// The game it's for, if the format doesn't say.
        #[arg(long)]
        game: Option<String>,
        /// What to call it, instead of the name it was shared with.
        #[arg(long)]
        name: Option<String>,
        /// Update the profile if it's already here, rather than failing.
        #[arg(long)]
        replace: bool,
    },
    /// List the formats profiles can be exported to and imported from.
    Formats,
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
fn open(paths: Paths, reporter: impl Reporter + 'static) -> patchwork_core::Result<Patchwork> {
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
        Command::Daemon(command) => return daemon(paths, command).await,
        command => command,
    };

    // the daemon has patchwork open while it runs, so it steps aside until
    // this is done
    let _paused = daemon::pause(&paths.sync())?;
    let pw = open(paths, Printer)?;
    for problem in &pw.registry().problems {
        eprintln!("warning: skipped registry entry: {problem}");
    }

    match command {
        Command::Game(command) => game(&pw, command)?,
        Command::Config(command) => config(&pw, command).await?,
        Command::Sync { command } => sync(&pw, command, &cli.profile).await?,
        Command::Profile(command) => profile(&pw, command, &cli.profile).await?,
        Command::Add {
            spec,
            name,
            version,
            disabled,
        } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let spec = pw.parse_spec(&spec, version)?;
            let added = pw.add_mod(&profile, spec, name, !disabled).await?;
            println!("added {} to {}", added.name, loaded.label());
        }
        Command::Search {
            text,
            source,
            game,
            sort,
            categories,
            excluded_categories,
            page,
            list_categories,
        } => {
            let game = match game {
                Some(game) => game,
                None => selected(&pw, &cli.profile)?.1.game,
            };
            let source = search_source(&pw, &game, source)?;
            if list_categories {
                let sorts: Vec<_> = pw
                    .search_sorts(&source)?
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                println!("sorts: {}", sorts.join(", "));
                for category in pw.search_categories(&game, &source).await? {
                    if category.id == category.name {
                        println!("  {}", category.name);
                    } else {
                        println!("  {:<32} {}", category.id, category.name);
                    }
                }
                return Ok(());
            }
            let query = SearchQuery {
                text: Some(text.join(" ")),
                sort,
                categories,
                excluded_categories,
                page: page - 1,
            };
            search(&pw, &game, &source, &query).await?;
        }
        Command::Remove { name } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            pw.remove_mod(&profile, &name)?;
            println!("removed {name} from {}", loaded.label());
        }
        Command::Enable { name } => {
            pw.set_enabled(&selected(&pw, &cli.profile)?.0, &name, true)?;
            println!("enabled {name}");
        }
        Command::Disable { name } => {
            pw.set_enabled(&selected(&pw, &cli.profile)?.0, &name, false)?;
            println!("disabled {name}");
        }
        Command::Pin { name, version } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            pw.set_version(&profile, &name, Some(version.clone()))?;
            let lock = pw.lock(&profile, false).await?;
            println!(
                "pinned {name} to {version} ({} mods locked)",
                lock.mods.len()
            );
        }
        Command::Unpin { name } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            pw.set_version(&profile, &name, None)?;
            let lock = pw.lock(&profile, false).await?;
            let version = lock
                .mods
                .iter()
                .find(|m| m.name == name)
                .and_then(|m| m.version.as_deref())
                .unwrap_or("its newest version");
            println!("unpinned {name}; it's now at {version}");
        }
        Command::Supports {
            name,
            versions,
            reset,
        } => {
            let (profile, mut loaded) = selected(&pw, &cli.profile)?;
            if reset || !versions.is_empty() {
                let versions = if reset {
                    None
                } else {
                    Some(GameVersions::try_from(versions).map_err(anyhow::Error::msg)?)
                };
                pw.set_game_versions(&profile, &name, versions)?;
                loaded = pw.load_profile(&profile)?;
            }
            let m = loaded
                .mods
                .iter()
                .find(|m| m.name == name)
                .with_context(|| format!("{} has no mod named `{name}`", loaded.label()))?;
            let locked = pw.load_lock(&profile)?.and_then(|lock| {
                let key = mod_key(&m.source, &m.id);
                lock.mods.into_iter().find(|l| l.key() == key)
            });
            match (&m.game_versions, locked.and_then(|l| l.game_versions)) {
                (Some(v), _) => println!("{name} works with {v} (set in the profile)"),
                (None, Some(v)) => println!("{name} works with {v} (going by its source)"),
                (None, None) => {
                    println!("{name} works with any version, as far as patchwork knows")
                }
            }
        }
        Command::Env { axis, value, reset } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            if let Some(axis) = &axis
                && (reset || value.is_some())
            {
                pw.set_env(&profile, axis, value.as_deref())?;
                let lock = pw.lock(&profile, false).await?;
                println!("relocked {} mods", lock.mods.len());
            }
            let loaded = pw.load_profile(&profile).unwrap_or(loaded);
            let registry = pw.registry();
            let game = &registry.game(&loaded.game)?.value;
            if game.env.is_empty() {
                println!("{} has nothing to pick", game.name);
            }
            for (id, def) in game.env.iter() {
                if axis.as_deref().is_some_and(|a| a != id) {
                    continue;
                }
                let picked = match (loaded.env.get(id), &def.default) {
                    (Some(v), _) => v.clone(),
                    (None, Some(d)) => format!("{d} (default)"),
                    (None, None) => "not picked".to_owned(),
                };
                println!("{} ({id}): {picked}", def.name);
                if !def.choices.is_empty() {
                    println!("  choices: {}", def.choices.join(", "));
                }
            }
            if let Some(axis) = &axis
                && game.env.get(axis).is_none()
            {
                game.env.check(axis, "")?;
            }
        }
        Command::Instance { value, reset } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            if reset || value.is_some() {
                pw.set_instance(&profile, value)?;
            }
            let registry = pw.registry();
            let game = &registry.game(&loaded.game)?.value;
            match &game.instances {
                None => println!(
                    "{} profiles are deployed to the game's folder; they can't have instances",
                    game.name
                ),
                Some(_) if pw.uses_instance(&loaded)? => println!(
                    "{} is deployed to its own instance, {}",
                    loaded.label(),
                    pw.instance_dir(&profile).display()
                ),
                Some(_) => println!("{} is deployed to the game's folder", loaded.label()),
            }
        }
        Command::Options {
            name,
            of,
            picks,
            sets,
            save,
            reset,
        } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let of = pick_options(&pw, &profile, &name, of.as_deref())?;
            let of = of.as_deref();
            let mut view = pw.mod_options(&profile, &name, of, None)?;
            if reset {
                pw.set_mod_options(&profile, &name, of, None)?;
                view = pw.mod_options(&profile, &name, of, None)?;
            } else if save || !picks.is_empty() || !sets.is_empty() {
                let mut answers = view.form.answers.clone();
                for pick in &picks {
                    // Each pick can show pages that the next is on.
                    let now = pw.mod_options(&profile, &name, of, Some(&answers))?;
                    let picked = find_pick(&now, pick)?;
                    match answers.iter_mut().find(|a| a.group == picked.group) {
                        Some(a) => *a = picked,
                        None => answers.push(picked),
                    }
                }
                for set in &sets {
                    let now = pw.mod_options(&profile, &name, of, Some(&answers))?;
                    let (group, field, value) = find_set(&now.form, set)?;
                    match answers.iter_mut().find(|a| a.group == group) {
                        Some(a) => {
                            a.values.insert(field, value);
                        }
                        None => answers.push(Answer::values(group, [(field, value)].into())),
                    }
                }
                pw.set_mod_options(&profile, &name, of, Some(&answers))?;
                view = pw.mod_options(&profile, &name, of, None)?;
            }
            print_options(&name, &view);
        }
        Command::Settings {
            file,
            of,
            sets,
            reset,
        } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let files = pw.config_files(&profile)?;
            let Some(file) = file else {
                let shown: Vec<_> = files
                    .iter()
                    .filter(|f| of.as_ref().is_none_or(|m| f.from.as_ref() == Some(m)))
                    .collect();
                if shown.is_empty() {
                    println!("No config files.");
                }
                for f in shown {
                    print_config_file(f);
                }
                return Ok(());
            };
            let path = find_config_file(&files, &file)?;
            if reset {
                pw.set_config(&profile, &path, None)?;
            } else if !sets.is_empty() {
                let mut answers = pw.config_form(&profile, &path, None)?.form.answers;
                for set in &sets {
                    let now = pw.config_form(&profile, &path, Some(&answers))?;
                    let (group, field, value) = find_set(&now.form, set)?;
                    match answers.iter_mut().find(|a| a.group == group) {
                        Some(a) => {
                            a.values.insert(field, value);
                        }
                        None => answers.push(Answer::values(group, [(field, value)].into())),
                    }
                }
                pw.set_config(&profile, &path, Some(&answers))?;
            }
            print_config(&pw.config_form(&profile, &path, None)?);
        }
        Command::Versions { name } => {
            let (_, loaded) = selected(&pw, &cli.profile)?;
            let m =
                loaded.mods.iter().find(|m| m.name == name).ok_or_else(|| {
                    anyhow::anyhow!("{} has no mod named `{name}`", loaded.label())
                })?;
            for v in pw.versions(&pw.lookup(&loaded)?, &m.source, &m.id).await? {
                let mark = if m.version.as_ref() == Some(&v) {
                    " (pinned)"
                } else {
                    ""
                };
                println!("{v}{mark}");
            }
        }
        Command::Changelog { name, new } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let m =
                loaded.mods.iter().find(|m| m.name == name).ok_or_else(|| {
                    anyhow::anyhow!("{} has no mod named `{name}`", loaded.label())
                })?;
            let entries = if new {
                let key = mod_key(&m.source, &m.id);
                let locked = pw
                    .load_lock(&profile)?
                    .and_then(|lock| lock.mods.into_iter().find(|l| l.key() == key))
                    .and_then(|l| l.version);
                let Some(locked) = locked else {
                    bail!("`{name}` isn't locked at a version yet; run `patchwork lock`");
                };
                pw.changes(
                    &pw.lookup(&loaded)?,
                    &m.source,
                    &m.id,
                    Some(&locked),
                    m.version.as_deref(),
                )
                .await?
            } else {
                pw.changelog(&pw.lookup(&loaded)?, &m.source, &m.id, None)
                    .await?
            };
            if entries.is_empty() && new {
                println!("nothing new in {name}'s changelog");
            } else if entries.is_empty() {
                println!("no changelog for {name}");
            }
            for (i, entry) in entries.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                if let Some(version) = &entry.version {
                    println!("== {version} ==");
                }
                println!("{}", entry.notes);
            }
        }
        Command::List { query, load_order } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            // only for showing ids, so a broken one isn't an error here
            let lock = pw.load_lock(&profile).ok().flatten();
            let ids = |m: &ProfileMod| pw.display_id(lock.as_ref(), m);
            if loaded.mods.is_empty() {
                println!("{} has no mods", loaded.label());
            } else if load_order {
                print_mods(order::load_order(&loaded.mods).into_iter().enumerate(), ids);
            } else if query.is_empty() {
                print_mods(loaded.mods.iter().enumerate(), ids);
            } else {
                let query = query.join(" ");
                let found = pw.find_mods(&profile, &query)?;
                if found.is_empty() {
                    println!("no mods in {} match \"{query}\"", loaded.label());
                }
                print_mods(found.iter().map(|(i, m)| (*i, m)), ids);
            }
        }
        Command::Move { name, to } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let index = pw.move_mod(&profile, &name, &to.position())?;
            println!("moved {name} to {} of {}", index + 1, loaded.mods.len());
        }
        Command::Deps { all } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let Some(lock) = pw.load_lock(&profile)? else {
                println!("{} isn't locked yet; run `patchwork lock`", loaded.label());
                return Ok(());
            };
            let info: Vec<_> = lock
                .dependency_info(&loaded)
                .into_iter()
                .filter(|i| all || !i.in_profile)
                .collect();
            if info.is_empty() {
                println!("{} has no dependencies", loaded.label());
            }
            for i in info {
                let what = match (&i.replaced_by, &i.version) {
                    (Some(with), _) => format!("replaced by {with}"),
                    (None, Some(version)) => version.clone(),
                    (None, None) => String::new(),
                };
                let needed = if i.needed_by.is_empty() {
                    String::new()
                } else {
                    format!("  needed by {}", i.needed_by.join(", "))
                };
                println!("{}  {what}  ({}){needed}", i.name, i.key);
            }
        }
        Command::SoftDeps { name, missing } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let Some(lock) = pw.load_lock(&profile)? else {
                println!("{} isn't locked yet; run `patchwork lock`", loaded.label());
                return Ok(());
            };
            if let Some(name) = &name
                && loaded.find(name).is_none()
            {
                anyhow::bail!("{} has no mod named `{name}`", loaded.label());
            }
            let mut any = false;
            for m in loaded
                .mods
                .iter()
                .filter(|m| name.as_ref().is_none_or(|n| *n == m.name))
            {
                let soft: Vec<_> = lock
                    .soft_dependencies(&loaded, &mod_key(&m.source, &m.id))
                    .into_iter()
                    .filter(|d| !missing || d.missing())
                    .collect();
                if soft.is_empty() {
                    continue;
                }
                any = true;
                println!("{}:", m.name);
                for d in soft {
                    let need = match d.dependency.need {
                        Need::Optional => "optional",
                        Need::Unknown => "maybe needed",
                    };
                    let status = match (&d.installed, d.ignored) {
                        (Some(by), _) => format!("have ({by})"),
                        (None, true) => "ignored".to_owned(),
                        (None, false) => "missing".to_owned(),
                    };
                    let label = d.dependency.label();
                    let place = d
                        .dependency
                        .key
                        .as_deref()
                        .or(d.dependency.url.as_deref())
                        .filter(|p| *p != label)
                        .map(|p| format!("  ({p})"))
                        .unwrap_or_default();
                    println!("  {label}{place}  {need}, {status}");
                    if let Some(notes) = &d.dependency.notes {
                        println!("    {notes}");
                    }
                }
            }
            if !any {
                println!("nothing to show");
            }
        }
        Command::Ignore {
            name,
            dependency,
            undo,
        } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let m = loaded
                .find(&name)
                .with_context(|| format!("{} has no mod named `{name}`", loaded.label()))?;
            let soft = pw
                .load_lock(&profile)?
                .map(|l| l.soft_dependencies(&loaded, &mod_key(&m.source, &m.id)))
                .unwrap_or_default();
            let ident = soft
                .iter()
                .find(|d| d.ident == dependency || d.dependency.label() == dependency)
                .map(|d| d.ident.clone())
                .with_context(|| {
                    format!("{name} lists nothing called `{dependency}`; see `patchwork soft-deps {name}`")
                })?;
            pw.set_ignored(&profile, &name, &ident, !undo)?;
            match undo {
                true => println!("{name}: pointing out {dependency} again"),
                false => println!("{name}: ignoring {dependency}"),
            }
        }
        Command::Replace {
            name,
            dependencies,
            undo,
        } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let m = loaded
                .mods
                .iter()
                .find(|m| m.name == name)
                .with_context(|| format!("{} has no mod named `{name}`", loaded.label()))?;
            let lock = pw.load_lock(&profile)?;
            let key_of = |given: &String| {
                lock.iter()
                    .flat_map(|l| &l.mods)
                    .find(|m| m.name == *given)
                    .map_or_else(|| given.clone(), |m| m.key())
            };
            if dependencies.is_empty() && !undo {
                if m.replaces.is_empty() {
                    println!("{name} doesn't replace anything");
                }
                for key in &m.replaces {
                    println!("{key}");
                }
                return Ok(());
            }
            let replaces: Vec<String> = if undo {
                let undone: Vec<String> = dependencies.iter().map(key_of).collect();
                m.replaces
                    .iter()
                    .filter(|k| !dependencies.is_empty() && !undone.contains(k))
                    .cloned()
                    .collect()
            } else {
                m.replaces.iter().cloned().chain(dependencies).collect()
            };
            pw.set_replaces(&profile, &name, &replaces)?;
            let lock = pw.lock(&profile, false).await?;
            let loaded = pw.load_profile(&profile)?;
            let info = lock.dependency_info(&loaded);
            let replaces = loaded
                .mods
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.replaces.clone())
                .unwrap_or_default();
            if replaces.is_empty() {
                println!(
                    "{name} doesn't replace anything ({} mods locked)",
                    lock.mods.len()
                );
            } else {
                println!(
                    "{name} replaces {} ({} mods locked)",
                    replaces.join(", "),
                    lock.mods.len()
                );
            }
            for key in replaces {
                if !info.iter().any(|i| i.key == key && !i.needed_by.is_empty()) {
                    println!("  nothing needs {key}");
                }
            }
        }
        Command::Priority { name, priority } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            match priority {
                Some(priority) => {
                    pw.set_priority(&profile, &name, priority)?;
                    println!("{name} has priority {priority}");
                }
                None => {
                    let m = loaded
                        .mods
                        .iter()
                        .find(|m| m.name == name)
                        .with_context(|| format!("{} has no mod named `{name}`", loaded.label()))?;
                    println!("{}", m.priority);
                }
            }
        }
        Command::Order { list, action } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let id = pick_list(&pw, &profile, &loaded.game, list.as_deref())?;
            match action {
                OrderCommand::List => {
                    let list = pw.list(&profile, &id)?;
                    if list.items.is_empty() {
                        println!(
                            "none of {}'s locked mods have anything for {}; lock it to include mods added since",
                            loaded.label(),
                            list.title.to_lowercase()
                        );
                    }
                    print_list(&list);
                }
                OrderCommand::Move { name, to } => {
                    let index = pw.move_in_list(&profile, &id, &name, &to.position())?;
                    let list = pw.list(&profile, &id)?;
                    println!("moved {name} to {} of {}", index + 1, list.items.len());
                    for problem in &list.problems {
                        eprintln!("{}: {}", severity(problem.severity), problem.message);
                    }
                }
                OrderCommand::Enable { name } => {
                    pw.set_list_enabled(&profile, &id, &name, true)?;
                    println!("enabled {name}");
                }
                OrderCommand::Disable { name } => {
                    pw.set_list_enabled(&profile, &id, &name, false)?;
                    println!("disabled {name}");
                }
            }
        }
        Command::Action { id, dry_run } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let actions = pw.actions(&profile)?;
            let Some(wanted) = id else {
                if actions.is_empty() {
                    println!("{} doesn't have any actions", loaded.game);
                }
                for a in &actions {
                    println!("{}  {}", a.id, a.title);
                    if let Some(d) = &a.description {
                        println!("    {d}");
                    }
                }
                return Ok(());
            };
            let action = actions
                .iter()
                .find(|a| a.id == wanted || a.id.rsplit('.').next() == Some(wanted.as_str()))
                .with_context(|| match actions.len() {
                    0 => format!("{} doesn't have any actions", loaded.game),
                    _ => format!(
                        "{} has no action `{wanted}`; it has {}",
                        loaded.game,
                        actions
                            .iter()
                            .map(|a| a.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                })?;
            let report = pw.run_action(&profile, &action.id, !dry_run).await?;
            print_action(&report);
        }
        Command::Conflicts { name } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            if let Some(name) = &name
                && !loaded.mods.iter().any(|m| m.name == *name)
            {
                bail!("{} has no mod named `{name}`", loaded.label());
            }
            let conflicts = pw.conflicts(&profile)?;
            print_conflicts(&conflicts, name.as_deref());
        }
        Command::Sort { by, tags, dry_run } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let sorted = pw.sorted_mods(&profile, &by, &tags).await?;
            // only for showing ids, so a broken one isn't an error here
            let lock = pw.load_lock(&profile).ok().flatten();
            print_mods(sorted.iter().enumerate(), |m| {
                pw.display_id(lock.as_ref(), m)
            });
            if !dry_run {
                let names: Vec<String> = sorted.into_iter().map(|m| m.name).collect();
                pw.reorder_mods(&profile, &names)?;
            }
        }
        Command::Lock { update } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let lock = pw.lock(&profile, update).await?;
            println!("locked {} mods for {}", lock.mods.len(), loaded.label());
        }
        Command::Deploy { dry_run, verify } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let check = if verify { Check::Full } else { Check::Quick };
            let plan = pw.plan_deploy_checked(&profile, check).await?;
            finish(&pw, &plan, dry_run)?;
        }
        Command::Capture { name } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let output = match &name {
                Some(name) => pw.capture(&profile, name).await?.output,
                None => pw.tool_output(&profile)?,
            };
            print_tool_output(&output);
            let n = output.files.len();
            let files = format!("{n} file{}", if n == 1 { "" } else { "s" });
            match name {
                _ if n == 0 => println!("no files to capture for {}", loaded.label()),
                None => {
                    println!("{files} to capture; `patchwork capture <name>` keeps them as a mod")
                }
                Some(name) => {
                    println!("captured {files} into `{name}`");
                    let plan = pw.plan_deploy(&profile).await?;
                    finish(&pw, &plan, false)?;
                }
            }
        }
        Command::Tool {
            into,
            dry_run,
            exe,
            args,
        } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let exe = std::path::absolute(&exe)
                .with_context(|| format!("can't find {}", exe.display()))?;
            let into = match into {
                Some(into) => into,
                None => format!(
                    "{} Output",
                    exe.file_stem().unwrap_or_default().to_string_lossy()
                ),
            };
            if pw.deploy_status(&profile)? != DeployStatus::Deployed {
                if dry_run {
                    println!("would deploy {} first", loaded.label());
                } else {
                    let plan = pw.plan_deploy(&profile).await?;
                    finish(&pw, &plan, false)?;
                }
            }
            let tool = pw.plan_tool(&profile, &exe, args)?;
            if dry_run {
                println!(
                    "would run {} {}, in {}, then capture what it writes into `{into}`:",
                    exe.display(),
                    tool.via,
                    tool.cwd.display()
                );
                println!("{tool}");
                return Ok(());
            }
            println!("running {} {}...", exe.display(), tool.via);
            let ran = pw.run_tool(&profile, &tool, &into).await?;
            if ran.protected > 0 {
                println!(
                    "{} deployed {} linked to patchwork's store, so {} read-only while it ran",
                    ran.protected,
                    if ran.protected == 1 {
                        "file is"
                    } else {
                        "files are"
                    },
                    if ran.protected == 1 {
                        "it was"
                    } else {
                        "they were"
                    },
                );
            }
            let Some(captured) = ran.captured else {
                bail!(
                    "the tool failed ({}); what it printed is in {}, and anything it wrote is still in the game's folder, where `patchwork capture` can keep it",
                    ran.status,
                    ran.log.display()
                );
            };
            print_tool_output(&captured.output);
            let n = captured.output.files.len();
            if n == 0 {
                println!("the tool didn't write anything to capture");
            } else {
                println!(
                    "captured {n} file{} into `{into}`",
                    if n == 1 { "" } else { "s" }
                );
                let plan = pw.plan_deploy(&profile).await?;
                finish(&pw, &plan, false)?;
            }
        }
        Command::History => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let history = pw.history(&profile)?;
            if history.is_empty() {
                println!("{} has no deploys in its history yet", loaded.label());
            }
            for entry in history {
                let when = match entry.deployed_at {
                    Some(at) => at
                        .to_zoned(jiff::tz::TimeZone::system())
                        .strftime("%Y-%m-%d %H:%M")
                        .to_string(),
                    None => "?".to_owned(),
                };
                let current = if entry.current { "  (current)" } else { "" };
                println!(
                    "#{}  {when}  {} mods enabled{current}",
                    entry.number, entry.enabled
                );
                for change in &entry.changes {
                    println!("    {change}");
                }
            }
        }
        Command::Rollback { number, dry_run } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let number = match number {
                Some(number) => number,
                None => match pw.history(&profile)?.last() {
                    Some(newest) => newest.number,
                    None => bail!("{} hasn't been deployed yet", loaded.label()),
                },
            };
            let changes = pw.rollback_changes(&profile, number)?;
            if changes.is_empty() {
                println!("{} is already like deploy #{number}", loaded.label());
                return Ok(());
            }
            for change in &changes {
                println!("{change}");
            }
            if !dry_run {
                pw.rollback(&profile, number)?;
                println!(
                    "rolled {} back to deploy #{number}; run `patchwork deploy` to apply it",
                    loaded.label()
                );
            }
        }
        Command::Launch {
            option,
            runner,
            list,
            dry_run,
            no_deploy,
        } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
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
                    finish(&pw, &plan, false)?;
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
            if launch.needs_steam() && launch::steam_running() == Some(false) {
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
        }
        Command::Undeploy { game, dry_run } => {
            let target = match game {
                Some(game) => DeployTarget::Game(game),
                None => pw.deploy_target(&selected(&pw, &cli.profile)?.1)?,
            };
            let plan = pw.plan_undeploy(&target)?;
            finish(&pw, &plan, dry_run)?;
        }
        Command::Provide { name, file } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let pending = match pw.lock(&profile, false).await {
                Ok(_) => bail!("{} has nothing waiting to be downloaded", loaded.label()),
                Err(patchwork_core::Error::NeedsDownload(pending)) => pending,
                Err(e) => return Err(e.into()),
            };
            let Some(wanted) = pending
                .iter()
                .find(|p| p.name == name || p.file.key() == name)
            else {
                let names: Vec<&str> = pending.iter().map(|p| p.name.as_str()).collect();
                bail!(
                    "`{name}` isn't waiting to be downloaded; these are: {}",
                    names.join(", ")
                );
            };
            pw.provide(&wanted.file, &file)?;
            let lock = pw.lock(&profile, false).await?;
            println!("locked {} mods for {}", lock.mods.len(), loaded.label());
        }
        Command::Link { link } => {
            let Some(file) = pw.accept_link(&link).await? else {
                bail!("none of patchwork's sources recognize {link}");
            };
            println!(
                "downloaded {} for {}; it's used the next time a profile is locked",
                file.key(),
                file.game
            );
        }
        Command::Status => status(&pw, &cli.profile)?,
        Command::SelfUpdate { .. } | Command::Daemon(_) => unreachable!("handled before opening"),
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
                let hidden = if game.hidden { " (hidden)" } else { "" };
                println!(
                    "{}  {}{hidden} [{}]  {installed}",
                    game.id,
                    game.name,
                    entry.origins()
                );
            }
        }
        GameCommand::Show { id } => {
            let registry = pw.registry();
            let entry = registry.game(&id)?;
            println!(
                "# origin: {}, hash: {}",
                entry.origins(),
                entry.hash.short()
            );
            print!("{}", toml::to_string_pretty(&entry.value)?);
            if let Some(env) = pw.game_env(&DeployTarget::Game(id))? {
                println!("# installed: {env}");
            }
        }
        GameCommand::SetPath { id, path } => {
            let install = pw.set_game_path(&id, &path)?;
            println!("{id} is installed at {}", install.path.display());
        }
        GameCommand::Detect { id: Some(id), set } => {
            let registry = pw.registry();
            let def = &registry.game(&id)?.value;
            let found = Locator::scan().find(def);
            if found.is_empty() {
                bail!(
                    "couldn't find where {} is installed; run `patchwork game set-path {id} <dir>` to say",
                    def.name
                );
            }
            for found in &found {
                println!("{}  ({})", found.path.display(), found.origin);
            }
            if set {
                let [found] = found.as_slice() else {
                    bail!(
                        "{} is in more than one place; pick one with `patchwork game set-path {id} <dir>`",
                        def.name
                    );
                };
                let install = pw.set_game_path(&id, &found.path)?;
                println!("{id} is installed at {}", install.path.display());
            }
        }
        GameCommand::Detect { id: None, set } => {
            let locator = Locator::scan();
            let mut any = false;
            for entry in pw.registry().games().filter(|e| !e.value.hidden) {
                let game = &entry.value;
                let found = locator.find(game);
                if found.is_empty() {
                    continue;
                }
                any = true;
                let current = pw.game_install(&game.id)?.map(|i| i.path);
                for found in &found {
                    let mark = if current.as_ref() == Some(&found.path) {
                        "  (set up)"
                    } else {
                        ""
                    };
                    println!(
                        "{}  {}  ({}){mark}",
                        game.id,
                        found.path.display(),
                        found.origin
                    );
                }
                // never moves a game that's already set up
                if set
                    && current.is_none()
                    && let [found] = found.as_slice()
                {
                    pw.set_game_path(&game.id, &found.path)?;
                    println!("  set up {}", game.name);
                }
            }
            if !any {
                println!("no known games were found");
            }
        }
        GameCommand::Settings { id } => {
            let registry = pw.registry();
            let def = &registry.game(&id)?.value;
            let install = pw.game_install(&id)?;
            let link = install.as_ref().and_then(|i| i.link);
            let set = if link.is_some() { "" } else { " (default)" };
            let link = toml::Value::try_from(link.unwrap_or(def.link))?;
            println!("link = {link}{set}");
            for setting in pw.deployer_settings(&id)? {
                print_setting(&setting);
            }
            if let Some(launch) = install.as_ref().and_then(|i| i.launch.as_ref()) {
                println!("launch = {}", toml::Value::from(launch.as_str()));
            }
            if let Some(choices) = pw.runner_choices(&id)? {
                print_picked_runner(&choices);
                for runner in &choices.runners {
                    for setting in runner.settings.iter().filter(|s| s.origin.is_some()) {
                        print!("runner.{}.", runner.id);
                        print_setting(setting);
                    }
                }
            }
            let Some(roots) = pw.resolve_roots(&DeployTarget::Game(id.clone()))? else {
                return Ok(());
            };
            let set_prefix = install.as_ref().and_then(|i| i.prefix.as_ref());
            if let Some(prefix) = roots.prefix() {
                let how = if set_prefix.is_some() { "" } else { " (found)" };
                println!(
                    "prefix = {}{how}",
                    toml::Value::from(prefix.display().to_string())
                );
            }
            for (root, resolved) in roots.iter() {
                let allow: Vec<_> = resolved.allow.iter().map(|a| a.as_str()).collect();
                let allow = allow.join(", ");
                match &resolved.path {
                    Ok(path) => {
                        let how = match resolved.origin {
                            RootOrigin::Set => "",
                            RootOrigin::Definition => " (default)",
                            RootOrigin::Prefix => " (in the prefix)",
                        };
                        let path = toml::Value::from(path.display().to_string());
                        println!("root.{root} = {path}{how}  # for {allow}");
                    }
                    Err(why) => println!("# root.{root} isn't set: {why}"),
                }
            }
        }
        GameCommand::Runners { id } => {
            let choices = pw.runner_choices(&id)?.with_context(|| {
                format!("patchwork doesn't know where {id} is installed; run `patchwork game set-path {id} <dir>` first")
            })?;
            print_picked_runner(&choices);
            for runner in &choices.runners {
                match &runner.unavailable {
                    Some(why) => println!("{}  {}  [unavailable: {why}]", runner.id, runner.name),
                    None => println!("{}  {}", runner.id, runner.name),
                }
                for setting in &runner.settings {
                    print!("  ");
                    print_setting(setting);
                }
            }
        }
        GameCommand::Set { id, key, value } => {
            let mut install = game_install(pw, &id)?;
            if key == "link" {
                install.link = Some(toml::Value::String(value).try_into()?);
            } else if key == "prefix" {
                install.prefix = Some(PathBuf::from(value));
            } else if key == "launch" {
                install.launch = Some(value);
            } else if key == "runner" {
                install.runner = Some(value);
            } else if let Some(rest) = key.strip_prefix("runner.") {
                let (runner, setting) = runner_setting_key(rest)?;
                let choices = pw
                    .runner_choices(&id)?
                    .unwrap_or_else(|| unreachable!("game_install found that it's set up"));
                let Some(found) = choices.runners.iter().find(|r| r.id == runner) else {
                    bail!("no runner named `{runner}`; see `patchwork game runners {id}`");
                };
                let Some(spec) = found.settings.iter().find(|s| s.spec.key == setting) else {
                    let keys: Vec<_> = found.settings.iter().map(|s| s.spec.key).collect();
                    if keys.is_empty() {
                        bail!("the runner `{runner}` doesn't have any settings");
                    }
                    bail!(
                        "the runner `{runner}` has no setting `{setting}`; it has {}",
                        keys.join(", ")
                    );
                };
                install
                    .runners
                    .entry(runner.to_owned())
                    .or_default()
                    .insert(setting.to_owned(), spec.spec.parse(&value)?);
            } else if let Some(root) = key.strip_prefix("root.") {
                let root = RootId::new(root).map_err(anyhow::Error::msg)?;
                install.roots.insert(root, PathBuf::from(value));
            } else {
                let settings = pw.deployer_settings(&id)?;
                let Some(setting) = settings.iter().find(|s| s.spec.key == key) else {
                    let keys: Vec<_> = [
                        "link",
                        "prefix",
                        "root.<id>",
                        "launch",
                        "runner",
                        "runner.<runner>.<setting>",
                    ]
                    .into_iter()
                    .chain(settings.iter().map(|s| s.spec.key))
                    .collect();
                    bail!("{id} has no setting `{key}`; it has {}", keys.join(", "));
                };
                install
                    .deployer
                    .insert(key.clone(), setting.spec.parse(&value)?);
            }
            pw.set_game_install(&id, install)?;
            println!("set {key} for {id}");
        }
        GameCommand::Unset { id, key } => {
            let mut install = game_install(pw, &id)?;
            let root = key.strip_prefix("root.").and_then(|r| RootId::new(r).ok());
            if key == "link" {
                install.link = None;
            } else if key == "prefix" {
                if install.prefix.take().is_none() {
                    bail!("prefix isn't set for {id}");
                }
            } else if key == "launch" {
                if install.launch.take().is_none() {
                    bail!("launch isn't set for {id}");
                }
            } else if key == "runner" {
                if install.runner.take().is_none() {
                    bail!("runner isn't set for {id}");
                }
            } else if let Some(rest) = key.strip_prefix("runner.") {
                let (runner, setting) = runner_setting_key(rest)?;
                let set = install.runners.get_mut(runner);
                if set.and_then(|s| s.remove(setting)).is_none() {
                    bail!("{key} isn't set for {id}");
                }
                if install.runners[runner].is_empty() {
                    install.runners.remove(runner);
                }
            } else if let Some(root) = root {
                if install.roots.remove(&root).is_none() {
                    bail!("{key} isn't set for {id}");
                }
            } else if install.deployer.remove(&key).is_none() {
                bail!("{key} isn't set for {id}");
            }
            pw.set_game_install(&id, install)?;
            println!("{key} is back to {id}'s default");
        }
    }
    Ok(())
}

/// The runner and setting in what follows `runner.` in a setting's key.
fn runner_setting_key(key: &str) -> Result<(&str, &str)> {
    key.split_once('.').with_context(|| {
        format!("`runner.{key}` isn't a setting; expected `runner.<runner>.<setting>`")
    })
}

fn print_picked_runner(choices: &launch::RunnerChoices) {
    let name = |id: &str| {
        choices
            .runners
            .iter()
            .find(|r| r.id == id)
            .map_or_else(|| id.to_owned(), |r| r.name.clone())
    };
    match (&choices.picked, &choices.automatic) {
        (Some(picked), _) => println!("runner = {}", toml::Value::from(picked.as_str())),
        (None, Some(id)) => println!(
            "runner = {} (automatic: {})",
            toml::Value::from(id.as_str()),
            name(id)
        ),
        (None, None) => println!("runner = (automatic, but nothing can run it right now)"),
    }
}

fn game_install(pw: &Patchwork, id: &str) -> Result<GameInstall> {
    pw.game_install(id)?.with_context(|| {
        format!("patchwork doesn't know where {id} is installed; run `patchwork game set-path {id} <dir>` first")
    })
}

async fn config(pw: &Patchwork, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::List => {
            for (id, name) in pw.configurable_sources() {
                println!("# {name}");
                for setting in pw.source_settings(id)? {
                    print!("{id}.");
                    print_setting(&setting);
                }
            }
        }
        ConfigCommand::Set { name, value } => {
            let (source, key) = split_setting(&name)?;
            let spec = pw
                .source_settings(source)?
                .into_iter()
                .find(|s| s.spec.key == key)
                .with_context(|| format!("{source} has no setting `{key}`"))?
                .spec;
            let value = match value {
                Some(value) => value,
                None => {
                    eprint!("{}: ", spec.label);
                    std::io::stderr().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let origin = pw.set_source_setting(source, key, Some(spec.parse(&value)?))?;
            match origin {
                Some(Origin::Keychain) => println!("saved {name} in the system keychain"),
                _ => println!("saved {name}"),
            }
            if let Some(var) = spec.env.filter(|v| std::env::var_os(v).is_some()) {
                println!("note: {var} is set, and is used instead");
            }
        }
        ConfigCommand::Unset { name } => {
            let (source, key) = split_setting(&name)?;
            pw.set_source_setting(source, key, None)?;
            println!("cleared {name}");
        }
        ConfigCommand::Check { source } => match pw.check_source_settings(&source).await? {
            Some(status) => println!("{status}"),
            None => println!("{source} has nothing set to check"),
        },
    }
    Ok(())
}

/// Splits `<source>.<key>`.
fn split_setting(name: &str) -> Result<(&str, &str)> {
    name.split_once('.').with_context(|| {
        format!("settings are named <source>.<key>, like nexus.api_key, not `{name}`")
    })
}

fn print_setting(setting: &SettingState) {
    let key = setting.spec.key;
    let shown = match (&setting.value, setting.origin) {
        (_, None) => match &setting.default {
            Some(default) => format!("{default} (default)"),
            None => "(not set)".to_owned(),
        },
        (Some(value), _) => value.to_string(),
        (None, _) => "(set)".to_owned(),
    };
    let from = match setting.origin {
        Some(Origin::Env(var)) => format!(" from {var}"),
        Some(Origin::Keychain) => " in the system keychain".to_owned(),
        _ => String::new(),
    };
    println!("{key} = {shown}{from}");
}

async fn profile(pw: &Patchwork, command: ProfileCommand, explicit: &Option<String>) -> Result<()> {
    match command {
        ProfileCommand::New { profile } => {
            let Some((game, name)) = profile.split_once('/') else {
                bail!("`{profile}` isn't a valid profile; expected `<game>/<name>`");
            };
            let created = pw.create_profile(game, name)?;
            pw.use_profile(&created)?;
            println!("created {profile} and switched to it");
        }
        ProfileCommand::List => {
            let current = pw.current_profile()?;
            for (profile, loaded) in pw.profiles()? {
                let mark = if Some(&profile) == current.as_ref() {
                    '*'
                } else {
                    ' '
                };
                println!("{mark} {}", loaded.label());
            }
        }
        ProfileCommand::Use { profile } => {
            pw.use_profile(&pw.find_profile(&profile)?)?;
            println!("switched to {profile}");
        }
        ProfileCommand::Remove { profile } => {
            pw.remove_profile(&pw.find_profile(&profile)?)?;
            println!("removed {profile}");
        }
        ProfileCommand::Rename { profile, name } => {
            let found = pw.find_profile(&profile)?;
            pw.rename_profile(&found, &name)?;
            println!("renamed {profile} to {}", pw.load_profile(&found)?.label());
        }
        ProfileCommand::Copy { profile, name } => {
            let copy = pw.duplicate_profile(&pw.find_profile(&profile)?, &name)?;
            println!("copied {profile} to {}", pw.load_profile(&copy)?.label());
        }
        ProfileCommand::Export { format, output } => {
            let (profile, loaded) = selected(pw, explicit)?;
            let extension = pw.format(&format)?.extension();
            let output = output
                .or_else(|| extension.map(|ext| PathBuf::from(format!("{}.{ext}", loaded.name))));
            let bytes = pw.export_profile(&profile, &format).await?;
            match output {
                Some(path) if path != Path::new("-") => {
                    std::fs::write(&path, bytes)
                        .with_context(|| format!("writing {}", path.display()))?;
                    eprintln!("exported {} to {}", loaded.label(), path.display());
                }
                _ => std::io::stdout().write_all(&bytes)?,
            }
        }
        ProfileCommand::Import {
            input,
            format,
            game,
            name,
            replace,
        } => {
            let bytes = if input == "-" {
                let mut bytes = Vec::new();
                std::io::stdin().read_to_end(&mut bytes)?;
                bytes
            } else if Path::new(&input).is_file() {
                std::fs::read(&input).with_context(|| format!("reading {input}"))?
            } else {
                input.into_bytes()
            };
            let options = ImportOptions {
                format: format.as_deref(),
                game: game.as_deref(),
                name: name.as_deref(),
                replace,
            };
            let imported = pw.import_profile(&bytes, &options).await?;
            pw.use_profile(&imported.profile)?;
            let verb = if imported.replaced {
                "updated"
            } else {
                "imported"
            };
            println!(
                "{verb} {} ({} mods, from {}) and switched to it; run `patchwork deploy` to install them",
                imported.label, imported.mods, imported.format
            );
        }
        ProfileCommand::Formats => {
            for format in pw.formats() {
                println!("{:<18} {}", format.id(), format.description());
            }
        }
    }
    Ok(())
}

/// `explicit`, or the only source that can search for `game`'s mods.
fn search_source(pw: &Patchwork, game: &str, explicit: Option<String>) -> Result<String> {
    if let Some(source) = explicit {
        return Ok(source);
    }
    match pw.search_sources(game)?.as_slice() {
        [] => bail!("none of {game}'s sources can be searched"),
        [only] => Ok(only.to_string()),
        several => bail!(
            "{game} can be searched on {}; pick one with --source",
            several.join(", ")
        ),
    }
}

async fn search(pw: &Patchwork, game: &str, source: &str, query: &SearchQuery) -> Result<()> {
    let page = pw.search(game, source, query).await?;
    if page.items.is_empty() {
        println!("no mods found");
        return Ok(());
    }
    for item in &page.items {
        println!("{}  ({source}:{})", item.name, item.id);
        let mut facts = Vec::new();
        if let Some(author) = &item.author {
            facts.push(format!("by {author}"));
        }
        if let Some(version) = &item.latest_version {
            facts.push(format!("v{version}"));
        }
        if let Some(downloads) = item.downloads {
            facts.push(format!("{} downloads", compact(downloads)));
        }
        if let Some(updated) = item.updated {
            facts.push(format!("updated {}", updated.strftime("%Y-%m-%d")));
        }
        if !facts.is_empty() {
            println!("    {}", facts.join(" · "));
        }
        if let Some(summary) = item
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            println!("    {summary}");
        }
    }
    let shown = query.page + 1;
    let total = page
        .total
        .map(|t| format!(", {t} mods"))
        .unwrap_or_default();
    let more = if page.has_more {
        format!("; --page {} for more", shown + 1)
    } else {
        String::new()
    };
    eprintln!("page {shown}{total}, by {}{more}", page.sort);
    Ok(())
}

/// `26998136` as `27.0M`.
fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn status(pw: &Patchwork, explicit: &Option<String>) -> Result<()> {
    let profile = match explicit {
        Some(p) => Some(pw.find_profile(p)?),
        None => pw.current_profile()?,
    };
    let Some(profile) = profile else {
        println!("no profile selected");
        return Ok(());
    };
    let loaded = pw.load_profile(&profile)?;
    let enabled = loaded.mods.iter().filter(|m| m.enabled).count();
    println!(
        "profile: {} ({enabled}/{} mods enabled)",
        loaded.label(),
        loaded.mods.len()
    );
    let target = pw.deploy_target(&loaded)?;
    match pw.game_install(&loaded.game)? {
        Some(install) => {
            let env = pw.game_env(&target)?.unwrap_or_default();
            let env = if env.is_empty() {
                String::new()
            } else {
                format!(" ({env})")
            };
            println!("game:    {}{env}", install.path.display())
        }
        // Instances don't need it.
        None if matches!(target, DeployTarget::Instance { .. }) => {}
        None => println!(
            "game:    not set up; run `patchwork game set-path {} <dir>`",
            loaded.game
        ),
    }
    if let DeployTarget::Instance { profile, .. } = &target {
        println!("instance: {}", pw.instance_dir(profile).display());
    }
    let state = pw.deploy_state(&target)?;
    match &state.profile {
        Some(deployed) if !state.files.is_empty() => {
            // Recorded by id; fall back to that if the profile's gone.
            let deployed = match pw.find_profile(deployed) {
                Ok(r) => pw.load_profile(&r)?.label(),
                Err(_) => deployed.clone(),
            };
            println!("deployed: {deployed} ({} files)", state.files.len())
        }
        _ => println!("deployed: nothing"),
    }
    for issue in pw.issues(&profile)? {
        println!("{}: {}", severity(issue.severity), issue.message);
    }
    Ok(())
}

/// What dist calls this app: the package name, not the binary's.
const DIST_APP: &str = "patchwork-cli";

async fn self_update(check: bool) -> Result<()> {
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

/// Lists what tools wrote, and warns about the game's own files they
/// changed.
fn print_tool_output(output: &ToolOutput) {
    for file in &output.files {
        match &file.replaces {
            Some(m) => println!("{}  (replaces {m}'s)", file.path),
            None => println!("{}", file.path),
        }
    }
    if !output.changed.is_empty() {
        eprintln!(
            "warning: the game's own {} changed, and can't be kept as a mod: {}",
            if output.changed.len() == 1 {
                "file was"
            } else {
                "files were"
            },
            output
                .changed
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn finish(pw: &Patchwork, plan: &DeployPlan, dry_run: bool) -> Result<()> {
    for warning in &plan.warnings {
        eprintln!("warning: {warning}");
    }
    if plan.is_noop() {
        // Nothing to place, but applying a deploy records that the profile
        // is deployed as it is now, and to the game as it is now.
        if plan.profile.is_some() && !dry_run {
            pw.apply(plan)?;
        }
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

fn print_launch_options(options: &[LaunchOption]) {
    for option in options {
        let default = if option.default { " (default)" } else { "" };
        match &option.unavailable {
            Some(why) => println!("{}  {}  [unavailable: {why}]", option.id, option.name),
            None => println!("{}  {}{default}", option.id, option.name),
        }
    }
}

fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
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
        // Files no mod has, like config files from the game's own folder.
        DeployOp::Place { dst, mod_name, .. } if mod_name.is_empty() => println!("  + {dst}"),
        DeployOp::Place { dst, mod_name, .. } => println!("  + {dst}  [{mod_name}]"),
    }
}

/// Numbered, so positions can be given to `move --to`.
/// Prints mods numbered by their places in the load order, which are given
/// as indices, with `ids` saying what to show for their ids.
fn print_mods<'a>(
    mods: impl IntoIterator<Item = (usize, &'a ProfileMod)>,
    ids: impl Fn(&ProfileMod) -> String,
) {
    let mods: Vec<_> = mods.into_iter().collect();
    let width = mods
        .iter()
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    for (i, m) in mods {
        let mark = if m.enabled { 'x' } else { ' ' };
        let version = m
            .version
            .as_deref()
            .map(|v| format!(" @ {v}"))
            .unwrap_or_default();
        let priority = match m.priority {
            0 => String::new(),
            p => format!("  priority {p}"),
        };
        println!(
            "{:>width$}. [{mark}] {}  ({}:{}{version}){priority}",
            i + 1,
            m.name,
            m.source,
            ids(m)
        );
    }
}

/// The id of the choices `wanted` names (an id, or the last part of one)
/// for mod `name`, or `None` to let core take the mod's only ones.
fn pick_options(
    pw: &Patchwork,
    profile: &ProfileRef,
    name: &str,
    wanted: Option<&str>,
) -> Result<Option<String>> {
    let Some(wanted) = wanted else {
        return Ok(None);
    };
    let all = pw.mods_with_options(profile)?;
    let found = all
        .get(name)
        .into_iter()
        .flatten()
        .find(|o| o.id == wanted || o.id.rsplit('.').next() == Some(wanted));
    Ok(Some(
        found.map_or_else(|| wanted.to_owned(), |o| o.id.clone()),
    ))
}

/// The group a `--pick` is for, with what it picks, by the names the form
/// shows.
fn find_pick(view: &ModOptionsView, pick: &str) -> Result<Answer> {
    let (at, options) = pick
        .split_once('=')
        .with_context(|| format!("`{pick}` should be GROUP=OPTIONS"))?;
    let (page, group) = match at.split_once('/') {
        Some((page, group)) => (Some(page.trim()), group.trim()),
        None => (None, at.trim()),
    };
    let found: Vec<_> = view
        .form
        .pages
        .iter()
        .filter(|p| page.is_none_or(|page| p.title.eq_ignore_ascii_case(page)))
        .flat_map(|p| &p.groups)
        .filter(|g| g.pick().is_some() && g.title.eq_ignore_ascii_case(group))
        .collect();
    let g = match found.as_slice() {
        [found] => *found,
        [] => bail!("there's no group called `{at}` with the options picked so far"),
        _ => bail!("more than one page has a group called `{group}`; say which, as PAGE/{group}"),
    };
    let items = options
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(|o| {
            g.items()
                .iter()
                .find(|i| i.title.eq_ignore_ascii_case(o))
                .map(|i| i.id.clone())
                .with_context(|| {
                    let names: Vec<_> = g.items().iter().map(|i| i.title.as_str()).collect();
                    format!(
                        "`{}` has no option called `{o}`; it has {}",
                        g.title,
                        names.join(", ")
                    )
                })
        })
        .collect::<Result<_>>()?;
    Ok(Answer::picks(g.id.clone(), items))
}

/// The group and field a `--set` is for, with the value it sets, by the
/// names the form shows.
fn find_set(form: &Form, set: &str) -> Result<(String, String, Value)> {
    let (at, text) = set
        .split_once('=')
        .with_context(|| format!("`{set}` should be SETTING=VALUE"))?;
    let mut path: Vec<&str> = at.split('/').map(str::trim).collect();
    let name = path.pop().unwrap_or_default();
    let (page, group) = match path.as_slice() {
        [] => (None, None),
        [group] => (None, Some(*group)),
        [page, group] => (Some(*page), Some(*group)),
        _ => bail!("`{at}` should be SETTING, GROUP/SETTING or PAGE/GROUP/SETTING"),
    };
    let found: Vec<_> = form
        .pages
        .iter()
        .filter(|p| page.is_none_or(|page| p.title.eq_ignore_ascii_case(page)))
        .flat_map(|p| &p.groups)
        .filter(|g| group.is_none_or(|group| g.title.eq_ignore_ascii_case(group)))
        .flat_map(|g| g.fields().iter().map(move |f| (g, f)))
        .filter(|(_, f)| f.title.eq_ignore_ascii_case(name) || f.id == name)
        .collect();
    let (g, field) = match found.as_slice() {
        [found] => *found,
        [] => bail!("there's no setting called `{at}` with the options picked so far"),
        _ => bail!("more than one group has a setting called `{name}`; say which, as GROUP/{name}"),
    };
    let text = text.trim();
    let bad = || anyhow::anyhow!("`{}` should be {}", field.title, field.kind.describe());
    let value = match &field.kind {
        FieldKind::Toggle => match text.to_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Value::Bool(true),
            "false" | "off" | "no" | "0" => Value::Bool(false),
            _ => return Err(bad()),
        },
        FieldKind::Int { .. } => Value::Int(text.parse().map_err(|_| bad())?),
        FieldKind::Float { .. } => Value::Float(text.parse().map_err(|_| bad())?),
        FieldKind::Choice { choices } => {
            let choice = choices
                .iter()
                .find(|c| c.title.eq_ignore_ascii_case(text) || c.id == text)
                .with_context(|| {
                    let names: Vec<_> = choices.iter().map(|c| c.title.as_str()).collect();
                    format!("`{}` should be one of {}", field.title, names.join(", "))
                })?;
            Value::Text(choice.id.clone())
        }
        FieldKind::Text { .. } | FieldKind::Color { .. } => Value::Text(text.to_owned()),
    };
    Ok((g.id.clone(), field.id.clone(), value))
}

/// `value` as `field` shows it: a choice by its title.
fn shown(field: &Field, value: &Value) -> String {
    match (&field.kind, value) {
        (FieldKind::Choice { choices }, Value::Text(id)) => choices
            .iter()
            .find(|c| c.id == *id)
            .map_or_else(|| value.to_string(), |c| c.title.clone()),
        _ => value.to_string(),
    }
}

fn print_options(name: &str, view: &ModOptionsView) {
    let form = &view.form;
    let title = view.title.to_lowercase();
    if form.title.is_empty() || form.title == name {
        println!("{title} for {name}");
    } else {
        println!("{title} for {name} ({})", form.title);
    }
    print_pages(form);
    println!();
    if let Some(summary) = &form.summary {
        println!("{summary}");
    }
    if !view.saved {
        println!(
            "These are the defaults, which are used until you pick some with --pick GROUP=OPTION or --set SETTING=VALUE, or keep these with --save."
        );
    }
    for warning in &form.warnings {
        eprintln!("warning: {warning}");
    }
}

/// The config file in `files` that `name` names: its path, or the end of
/// one, ignoring case.
fn find_config_file(files: &[ConfigFileInfo], name: &str) -> Result<Dest> {
    let name = name.trim_matches('/').to_lowercase();
    let path = |f: &ConfigFileInfo| f.path.to_string().to_lowercase();
    if let Some(f) = files.iter().find(|f| path(f) == name) {
        return Ok(f.path.clone());
    }
    let found: Vec<_> = files
        .iter()
        .filter(|f| path(f).ends_with(&format!("/{name}")))
        .collect();
    match found.as_slice() {
        [f] => Ok(f.path.clone()),
        [] => bail!("there's no config file called `{name}`; `patchwork settings` lists them"),
        _ => {
            let paths: Vec<_> = found.iter().map(|f| f.path.to_string()).collect();
            bail!(
                "more than one config file is called `{name}`: {}",
                paths.join(", ")
            )
        }
    }
}

fn print_config_file(f: &ConfigFileInfo) {
    let mut about = Vec::new();
    if let Some(title) = &f.title {
        about.push(title.clone());
    }
    if let Some(m) = &f.from {
        about.push(format!("from {m}"));
    }
    if f.missing {
        about.push("not there, so its changes aren't applied".to_owned());
    } else if f.edited {
        about.push("changed".to_owned());
    }
    if about.is_empty() {
        println!("{}", f.path);
    } else {
        println!("{}  ({})", f.path, about.join("; "));
    }
}

fn print_config(view: &ConfigView) {
    let form = &view.form;
    // Its one page is titled with the file's path.
    match &view.from {
        Some(m) => println!("{} (from {m})", form.title),
        None => println!("{}", form.title),
    }
    print_pages(form);
    println!();
    if let Some(summary) = &form.summary {
        println!("{summary}");
    }
    if view.saved {
        println!("Deploying applies the changes; `--reset` forgets them.");
    } else {
        println!("Change settings with --set SETTING=VALUE; deploying applies them.");
    }
    for warning in &form.warnings {
        eprintln!("warning: {warning}");
    }
}

/// `form`'s pages, with their groups' items and settings.
fn print_pages(form: &Form) {
    for page in &form.pages {
        println!();
        println!("{}", page.title);
        for group in &page.groups {
            let Some(pick) = group.pick() else {
                println!("  {}:", group.title);
                for field in group.fields() {
                    let what = match &field.kind {
                        FieldKind::Choice { choices } => {
                            let names: Vec<_> = choices.iter().map(|c| c.title.as_str()).collect();
                            format!("one of {}", names.join(", "))
                        }
                        kind => kind.describe(),
                    };
                    let default = field
                        .default
                        .as_ref()
                        .map(|d| format!("; default {}", shown(field, d)))
                        .unwrap_or_default();
                    println!(
                        "    {} = {} ({what}{default})",
                        field.title,
                        shown(field, &field.value)
                    );
                    print_description(field.description.as_deref());
                }
                continue;
            };
            let (rule, single) = match pick {
                Pick::ExactlyOne => ("pick one", true),
                Pick::AtMostOne => ("pick one or none", true),
                Pick::AtLeastOne => ("pick at least one", false),
                Pick::All => ("all of these", false),
                Pick::Any => ("pick any", false),
            };
            println!("  {} ({rule}):", group.title);
            for item in group.items() {
                let mark = match (single, item.picked) {
                    (true, true) => "(*)",
                    (true, false) => "( )",
                    (false, true) => "[x]",
                    (false, false) => "[ ]",
                };
                let state = match item.state {
                    ItemState::Required => " (required)",
                    ItemState::Recommended => " (recommended)",
                    ItemState::NotUsable => " (can't be used)",
                    ItemState::CouldBeUsable => " (might not work)",
                    ItemState::Optional => "",
                };
                println!("    {mark} {}{state}", item.title);
                print_description(item.description.as_deref());
            }
        }
    }
}

/// The first line of an item's or setting's description, if it has one.
fn print_description(description: Option<&str>) {
    if let Some(line) = description
        .iter()
        .flat_map(|d| d.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
    {
        println!("        {}", truncate(line, 100));
    }
}

/// `text`, cut to about `max` characters.
fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The id of the list `wanted` names (its id or the last part of it), or
/// of the game's only list.
fn pick_list(
    pw: &Patchwork,
    profile: &ProfileRef,
    game: &str,
    wanted: Option<&str>,
) -> Result<String> {
    let lists = pw.lists(profile)?;
    let ids = || {
        lists
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match wanted {
        Some(wanted) => lists
            .iter()
            .find(|l| l.id == wanted || l.id.rsplit('.').next() == Some(wanted))
            .map(|l| l.id.clone())
            .with_context(|| match lists.len() {
                0 => format!("{game} doesn't have anything to put in order"),
                _ => format!("{game} has no list `{wanted}`; it has {}", ids()),
            }),
        None => match &lists[..] {
            [] => bail!("{game} doesn't have anything to put in order"),
            [only] => Ok(only.id.clone()),
            _ => bail!(
                "{game} has more than one list; pick one with --list: {}",
                ids()
            ),
        },
    }
}

fn print_list(list: &List) {
    if !list.fixed.is_empty() {
        let fixed: Vec<&str> = list.fixed.iter().map(|i| i.name.as_str()).collect();
        println!("always first: {}", fixed.join(", "));
    }
    let width = list.items.len().to_string().len();
    for (i, item) in list.items.iter().enumerate() {
        let mark = if item.enabled { 'x' } else { ' ' };
        let badges = if item.item.badges.is_empty() {
            String::new()
        } else {
            format!(" ({})", item.item.badges.join(", "))
        };
        // The game's own items aren't from a mod.
        let from = if item.item.mod_name.is_empty() {
            String::new()
        } else {
            format!("  ({})", item.item.mod_name)
        };
        println!(
            "{:>width$}. [{mark}] {}{badges}{from}",
            i + 1,
            item.item.name
        );
    }
    for problem in &list.problems {
        eprintln!("{}: {}", severity(problem.severity), problem.message);
    }
}

fn print_action(report: &ActionReport) {
    if let Some(message) = &report.message {
        println!("{message}");
    }
    for change in &report.lists {
        let title = change.title.to_lowercase();
        if change.moved.is_empty() {
            println!("{title}: already in order");
            continue;
        }
        let verb = if report.applied {
            "moved"
        } else {
            "would move"
        };
        let n = change.moved.len();
        println!("{title}: {verb} {n} of {}", change.after.len());
        let width = change.after.len().to_string().len();
        for m in &change.moved {
            println!(
                "  {:>width$} -> {:>width$}  {}",
                m.from + 1,
                m.to + 1,
                m.name
            );
        }
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if !report.applied && report.lists.iter().any(|c| !c.moved.is_empty()) {
        println!("nothing was saved; run it without --dry-run to");
    }
}

/// Each mod's conflicts, in load order, as how many files it shares with
/// each other mod, or every file of `only`'s.
fn print_conflicts(conflicts: &Conflicts, only: Option<&str>) {
    let files = |n: usize| {
        if n == 1 {
            "1 file".to_owned()
        } else {
            format!("{n} files")
        }
    };
    let by_mod = conflicts.by_mod();
    let shown: Vec<_> = by_mod
        .iter()
        .filter(|(name, _)| only.is_none_or(|only| only == *name))
        .collect();
    if shown.is_empty() {
        match only {
            Some(only) => println!("{only} doesn't provide any of the same files as other mods"),
            None => println!("no two mods provide the same file"),
        }
        return;
    }
    for (name, of) in &shown {
        if !of.matters() {
            continue;
        }
        let lost = if of.lost() {
            "  (other mods' copies of all of these are deployed)"
        } else {
            ""
        };
        println!("{name}{lost}");
        for (verb, list) in [
            ("overrides", &of.overrides),
            ("overridden by", &of.overridden_by),
        ] {
            for other in list {
                println!("  {verb} {} ({})", other.mod_name, files(other.files.len()));
                if only.is_some() {
                    for file in &other.files {
                        println!("    {file}");
                    }
                }
            }
        }
    }
    let identical = match shown.as_slice() {
        [(_, of)] if only.is_some() => of.identical,
        _ => conflicts.files.iter().filter(|f| f.identical).count(),
    };
    if identical > 0 {
        println!(
            "{} the same in every mod that has them, so they aren't listed",
            match identical {
                1 => "1 more file is".to_owned(),
                n => format!("{n} more files are"),
            }
        );
    }
}

async fn sync(
    pw: &Patchwork,
    command: Option<SyncCommand>,
    explicit: &Option<String>,
) -> Result<()> {
    if let Some(SyncCommand::Name { id, name }) = &command {
        let peer = id
            .parse()
            .map_err(|_| anyhow::anyhow!("`{id}` isn't a computer's id"))?;
        let name = name.join(" ");
        patchwork_sync::set_peer_name(&pw.paths().sync(), &peer, &name)?;
        if name.trim().is_empty() {
            println!("forgot the name of {id}");
        } else {
            println!("{id} is now called {}", name.trim());
        }
        return Ok(());
    }
    let node = Node::spawn(&pw.paths().sync()).await?;
    if matches!(command, Some(SyncCommand::ResetId)) {
        let old = node.id();
        let new = node.reset_id().await?;
        println!("{new}");
        eprintln!("this computer was {old}; others hear it's gone the next time it syncs");
        return Ok(());
    }
    let result = sync_with(pw, &node, command, explicit).await;
    node.shutdown().await?;
    result
}

async fn sync_with(
    pw: &Patchwork,
    node: &Node,
    command: Option<SyncCommand>,
    explicit: &Option<String>,
) -> Result<()> {
    let Some(command) = command else {
        if node.synced()?.is_empty() && !node.devices_linked()? {
            bail!(
                "no profiles are synced; link your computers with `patchwork sync link`, \
                 or share a profile with `patchwork sync share`"
            );
        }
        for outcome in node.sync_once(pw, patchwork_sync::SYNC_TIMEOUT).await? {
            print_sync(pw, &Ok(outcome), false);
        }
        return Ok(());
    };
    match command {
        SyncCommand::Link => {
            let ticket = node.link_devices(pw).await?;
            println!("{ticket}");
            eprintln!(
                "\nRun `patchwork sync join <ticket>` on your other computer with that ticket.\n\
                 It lets whoever has it change every profile here, so keep it to yourself.\n\
                 Keeping everything in step until Ctrl-C."
            );
            run_sync(pw, node).await?;
        }
        SyncCommand::Unlink => {
            node.unlink_devices().await?;
            println!(
                "unlinked; profiles synced so far stay synced (`patchwork sync leave` stops one)"
            );
        }
        SyncCommand::Share {
            read_only,
            hide_addresses,
        } => {
            let (profile, loaded) = selected(pw, explicit)?;
            let addresses = if hide_addresses {
                Addresses::Hidden
            } else {
                Addresses::Included
            };
            let ticket = node.share(pw, &profile, !read_only, addresses).await?;
            println!("{ticket}");
            let who = if read_only {
                "Whoever joins with it can follow the profile, but not change it for anyone else."
            } else {
                "Whoever joins with it can change the profile for everyone, which can't be taken back."
            };
            eprintln!(
                "\nGive that ticket to whoever's joining, to run `patchwork sync join <ticket>`.\n\
                 {who}\nKeeping {} in step until Ctrl-C.",
                loaded.label()
            );
            run_sync(pw, node).await?;
        }
        SyncCommand::Join { ticket } => {
            eprintln!("waiting for the other computer...");
            match node.join(pw, &ticket, JOIN_TIMEOUT).await? {
                Joined::Profile(outcome) => {
                    let loaded = pw.load_profile(&outcome.profile)?;
                    print_sync(pw, &Ok(outcome), false);
                    println!(
                        "joined {}; `patchwork sync` keeps it in step, and `patchwork -p {} deploy` deploys it",
                        loaded.label(),
                        loaded.label()
                    );
                }
                Joined::Devices { profiles } => {
                    eprintln!(
                        "linked; fetching {profiles} profile(s) from your other computers..."
                    );
                    for outcome in node.sync_once(pw, patchwork_sync::SYNC_TIMEOUT).await? {
                        print_sync(pw, &Ok(outcome), false);
                    }
                    println!(
                        "every profile is now kept in step with your other computers; \
                         `patchwork sync` or `patchwork sync run` keeps them so"
                    );
                }
            }
        }
        SyncCommand::List => {
            if node.devices_linked()? {
                println!("linked to your other computers: every profile is synced");
            }
            for synced in node.synced()? {
                let profile = synced.reference();
                let label = pw
                    .load_profile(&profile)
                    .map(|p| p.label())
                    .unwrap_or_else(|_| synced.profile.to_string());
                if synced.writable {
                    println!("{label}");
                    continue;
                }
                let unshared = pw.unshared_mods(&profile).unwrap_or_default();
                if unshared.is_empty() {
                    println!("{label} (following)");
                } else {
                    println!("{label} (following; only here: {})", unshared.join(", "));
                }
            }
        }
        SyncCommand::Leave => {
            let (profile, loaded) = selected(pw, explicit)?;
            node.leave(pw, &profile).await?;
            println!("stopped syncing {}", loaded.label());
        }
        SyncCommand::Peers => {
            let (profile, _) = selected(pw, explicit)?;
            let names = node.peer_names()?;
            for peer in node.peers(&profile).await? {
                match names.get(&peer.to_string()) {
                    Some(name) => println!("{peer}  {name}"),
                    None => println!("{peer}"),
                }
            }
        }
        SyncCommand::Remove { ids } => {
            let (profile, loaded) = selected(pw, explicit)?;
            let peers = ids
                .iter()
                .map(|id| {
                    id.parse()
                        .map_err(|_| anyhow::anyhow!("`{id}` isn't a computer's id"))
                })
                .collect::<Result<Vec<_>>>()?;
            eprintln!("moving everyone else to a new document...");
            let removed = node.remove_peers(pw, &profile, &peers).await?;
            println!(
                "removed {} from {}; told {} of the others",
                ids.join(", "),
                loaded.label(),
                removed.told
            );
            if removed.to_tell > 0 {
                eprintln!(
                    "{} couldn't be reached yet; they're told when they and a computer \
                     syncing {} are next online (`patchwork daemon install` keeps this one trying)",
                    removed.to_tell,
                    loaded.label()
                );
            }
        }
        SyncCommand::Run => run_sync(pw, node).await?,
        SyncCommand::Id => println!("{}", node.id()),
        SyncCommand::ResetId | SyncCommand::Name { .. } => unreachable!("handled by `sync`"),
    }
    Ok(())
}

async fn daemon(paths: Paths, command: DaemonCommand) -> Result<()> {
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

/// How long `sync join` waits for the profile to arrive.
const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

async fn run_sync(pw: &Patchwork, node: &Node) -> Result<()> {
    tokio::select! {
        result = node.run(pw, |outcome| print_sync(pw, &outcome, true)) => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

/// Says what a sync did. Running, only changes are worth a mention.
fn print_sync(pw: &Patchwork, outcome: &Result<patchwork_sync::Outcome>, running: bool) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("warning: sync failed: {e:#}");
            return;
        }
    };
    let label = pw
        .load_profile(&outcome.profile)
        .map(|p| p.label())
        .unwrap_or_else(|_| outcome.profile.to_string());
    match &outcome.applied {
        _ if outcome.moved => println!(
            "{label}: moved to a new document; another computer syncing it hands that over \
             when you're both online, unless you were removed or only follow it, \
             in which case you need a new ticket"
        ),
        Some(applied) if applied.created => println!("{label}: added"),
        Some(applied) if applied.changed => println!("{label}: updated"),
        _ if running => {}
        None => println!("{label}: nothing from other computers yet"),
        Some(_) => println!("{label}: up to date"),
    }
    if outcome.received_files > 0 {
        println!(
            "{label}: got {} local mod file(s) from other computers",
            outcome.received_files
        );
    }
    for warning in &outcome.warnings {
        eprintln!("warning: {label}: {warning}");
    }
}

fn selected(pw: &Patchwork, explicit: &Option<String>) -> Result<(ProfileRef, Profile)> {
    let profile = match explicit {
        Some(profile) => pw.find_profile(profile)?,
        None => match pw.current_profile()? {
            Some(profile) => profile,
            None => {
                bail!("no profile selected; create one with `patchwork profile new <game>/<name>`")
            }
        },
    };
    let loaded = pw.load_profile(&profile)?;
    Ok((profile, loaded))
}
