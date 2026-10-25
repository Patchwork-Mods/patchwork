use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use patchwork_core::compat::GameVersions;
use patchwork_core::deploy::{DeployOp, DeployPlan, DeployTarget};
use patchwork_core::event::{Event, Reporter};
use patchwork_core::issue::Severity;
use patchwork_core::lock::mod_key;
use patchwork_core::order::{Position, SortKey};
use patchwork_core::paths::Paths;
use patchwork_core::profile::{Profile, ProfileMod, ProfileRef};
use patchwork_core::search::{SearchQuery, SortBy};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::update;
use patchwork_core::{GameInstall, ImportOptions, Patchwork};
use patchwork_deployers::{RulesDeployer, SmfDeployer};
use patchwork_formats::{Archive, R2modman, ThunderstoreCode, UrlList};
use patchwork_sources::{LocalSource, Nexus, Thunderstore, UrlSource};

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
    /// Show a mod's changelog, newest first.
    Changelog {
        name: String,
        /// Only show what's changed since the version it's locked at.
        #[arg(long)]
        new: bool,
    },
    /// List the mods in the profile, in load order.
    List {
        /// Only list mods matching this, best match first. Mods are found by
        /// name, and by author and description once their details have been
        /// fetched (by sorting by tags, say). Close misspellings still match.
        query: Vec<String>,
    },
    /// Move a mod in the load order. Later mods win when two provide the
    /// same file; a mod's dependencies always load before it regardless.
    Move {
        name: String,
        #[command(flatten)]
        to: MoveTo,
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
    },
    /// Remove everything patchwork deployed to a game.
    Undeploy {
        /// Defaults to the profile's game.
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
enum GameCommand {
    /// List known games.
    List,
    /// Show a game's definition.
    Show { id: String },
    /// Tell patchwork where a game is installed.
    SetPath { id: String, path: PathBuf },
    /// List a game's settings: how mods are linked into it, and its
    /// deployer's settings.
    Settings { id: String },
    /// Change one of a game's settings (see `game settings`).
    Set {
        id: String,
        key: String,
        value: String,
    },
    /// Go back to what the game's definition says for a setting.
    Unset { id: String, key: String },
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
    /// Delete a profile, along with its lockfile and any local mods imported
    /// with it. A deployed profile has to be undeployed first.
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

async fn run(cli: Cli) -> Result<()> {
    // Doesn't need the registry or profiles, so don't let problems there get
    // in the way of fixing them with an update.
    if let Command::SelfUpdate { check } = cli.command {
        return self_update(check).await;
    }

    let pw = Patchwork::builder(Paths::from_env()?)
        .source(LocalSource)
        .source(Thunderstore)
        .source(Nexus::default())
        .source(UrlSource)
        .deployer(RulesDeployer)
        .deployer(SmfDeployer)
        .format(Archive)
        .format(UrlList)
        .format(R2modman)
        .format(ThunderstoreCode::default())
        .reporter(Printer)
        .open()?;
    for problem in &pw.registry().problems {
        eprintln!("warning: skipped registry entry: {problem}");
    }

    match cli.command {
        Command::Game(command) => game(&pw, command)?,
        Command::Config(command) => config(&pw, command).await?,
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
        Command::Versions { name } => {
            let (_, loaded) = selected(&pw, &cli.profile)?;
            let m =
                loaded.mods.iter().find(|m| m.name == name).ok_or_else(|| {
                    anyhow::anyhow!("{} has no mod named `{name}`", loaded.label())
                })?;
            for v in pw.versions(&loaded.game, &m.source, &m.id).await? {
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
                    &loaded.game,
                    &m.source,
                    &m.id,
                    Some(&locked),
                    m.version.as_deref(),
                )
                .await?
            } else {
                pw.changelog(&loaded.game, &m.source, &m.id, None).await?
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
        Command::List { query } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            if loaded.mods.is_empty() {
                println!("{} has no mods", loaded.label());
            } else if query.is_empty() {
                print_mods(loaded.mods.iter().enumerate());
            } else {
                let query = query.join(" ");
                let found = pw.find_mods(&profile, &query)?;
                if found.is_empty() {
                    println!("no mods in {} match \"{query}\"", loaded.label());
                }
                print_mods(found.iter().map(|(i, m)| (*i, m)));
            }
        }
        Command::Move { name, to } => {
            let (profile, loaded) = selected(&pw, &cli.profile)?;
            let index = pw.move_mod(&profile, &name, &to.position())?;
            println!("moved {name} to {} of {}", index + 1, loaded.mods.len());
        }
        Command::Sort { by, tags, dry_run } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let sorted = pw.sorted_mods(&profile, &by, &tags).await?;
            print_mods(sorted.iter().enumerate());
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
        Command::Deploy { dry_run } => {
            let (profile, _) = selected(&pw, &cli.profile)?;
            let plan = pw.plan_deploy(&profile).await?;
            finish(&pw, &plan, dry_run)?;
        }
        Command::Undeploy { game, dry_run } => {
            let game = match game {
                Some(game) => game,
                None => selected(&pw, &cli.profile)?.1.game,
            };
            let plan = pw.plan_undeploy(&DeployTarget::Game(game))?;
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
        Command::SelfUpdate { .. } => unreachable!("handled before opening"),
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
        }
        GameCommand::Set { id, key, value } => {
            let mut install = game_install(pw, &id)?;
            if key == "link" {
                install.link = Some(toml::Value::String(value).try_into()?);
            } else {
                let settings = pw.deployer_settings(&id)?;
                let Some(setting) = settings.iter().find(|s| s.spec.key == key) else {
                    let keys: Vec<_> = std::iter::once("link")
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
            if key == "link" {
                install.link = None;
            } else if install.deployer.remove(&key).is_none() {
                bail!("{key} isn't set for {id}");
            }
            pw.set_game_install(&id, install)?;
            println!("{key} is back to {id}'s default");
        }
    }
    Ok(())
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
        None => println!(
            "game:    not set up; run `patchwork game set-path {} <dir>`",
            loaded.game
        ),
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
    match update::install(DIST_APP, true).await? {
        Some(new) => println!("updated patchwork {current} -> {new}"),
        None => println!("patchwork {current} is up to date"),
    }
    Ok(())
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
        DeployOp::Place { dst, mod_name, .. } => println!("  + {dst}  [{mod_name}]"),
    }
}

/// Numbered, so positions can be given to `move --to`.
/// Prints mods numbered by their places in the load order, which are given
/// as indices.
fn print_mods<'a>(mods: impl IntoIterator<Item = (usize, &'a ProfileMod)>) {
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
        println!(
            "{:>width$}. [{mark}] {}  ({}:{}{version})",
            i + 1,
            m.name,
            m.source,
            m.id
        );
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
