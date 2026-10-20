use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use axoupdater::AxoUpdater;
use clap::{Parser, Subcommand};
use patchwork_core::deploy::{DeployOp, DeployPlan, DeployTarget};
use patchwork_core::event::{Event, Reporter};
use patchwork_core::order::{Position, SortKey};
use patchwork_core::paths::Paths;
use patchwork_core::profile::{Profile, ProfileMod, ProfileRef};
use patchwork_core::search::{SearchQuery, SortBy};
use patchwork_core::{ImportOptions, Patchwork};
use patchwork_deployers::RulesDeployer;
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
    // Doesn't need the registry or profiles, so don't let problems there get
    // in the way of fixing them with an update.
    if let Command::SelfUpdate { check } = cli.command {
        return self_update(check).await;
    }

    let pw = Patchwork::builder(Paths::from_env()?)
        .source(LocalSource)
        .source(Thunderstore)
        .source(Nexus::from_env())
        .source(UrlSource)
        .deployer(RulesDeployer)
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
        }
        GameCommand::SetPath { id, path } => {
            let install = pw.set_game_path(&id, &path)?;
            println!("{id} is installed at {}", install.path.display());
        }
    }
    Ok(())
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
    match pw.game_install(&loaded.game)? {
        Some(install) => println!("game:    {}", install.path.display()),
        None => println!(
            "game:    not set up; run `patchwork game set-path {} <dir>`",
            loaded.game
        ),
    }
    let state = pw.deploy_state(&pw.deploy_target(&loaded)?)?;
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
    Ok(())
}

/// What dist calls this app: the package name, not the binary's.
const DIST_APP: &str = "patchwork-cli";

async fn self_update(check: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let mut updater = AxoUpdater::new_for(DIST_APP);
    // Without a receipt from our installer we didn't put this binary here
    // (Nix, cargo install, a dev build, ...), so replacing it isn't ours to do.
    let ours = updater.load_receipt().is_ok() && updater.check_receipt_is_for_this_executable()?;
    if !ours {
        bail!(
            "this copy of patchwork ({current}) wasn't installed by its installer; \
             update it the way you installed it"
        );
    }
    if check {
        if !updater.is_update_needed().await? {
            println!("patchwork {current} is up to date");
        } else if let Some(new) = updater.query_new_version().await? {
            println!(
                "patchwork {new} is available (you have {current}); run `patchwork self-update`"
            );
        }
        return Ok(());
    }
    match updater.run().await? {
        Some(result) => println!("updated patchwork {current} -> {}", result.new_version),
        None => println!("patchwork {current} is up to date"),
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
