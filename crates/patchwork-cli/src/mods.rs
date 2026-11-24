//! Adding, finding and managing a profile's mods.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use patchwork_core::Patchwork;
use patchwork_core::edits;
use patchwork_core::follow::{Follows, set_followed};
use patchwork_core::lock::mod_key;
use patchwork_core::order::{self, SortKey};
use patchwork_core::profile::{Profile, ProfileMod};
use patchwork_core::search::{SearchQuery, SortBy};
use patchwork_core::source::{FileChoice, ModSpec};

use crate::common::{MoveTo, field_changes, print_fields, selected};

/// `explicit`, or the only source that can search for `game`'s mods.
pub(crate) fn search_source(
    pw: &Patchwork,
    game: &str,
    explicit: Option<String>,
) -> Result<String> {
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

pub(crate) async fn print_search(
    pw: &Patchwork,
    game: &str,
    source: &str,
    query: &SearchQuery,
) -> Result<()> {
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

/// Numbered, so positions can be given to `move --to`.
/// Prints mods numbered by their places in the load order, which are given
/// as indices, with `ids` saying what to show for their ids, and the folder
/// each is in from `profile`.
pub(crate) fn print_mods<'a>(
    mods: impl IntoIterator<Item = (usize, &'a ProfileMod)>,
    profile: &Profile,
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
        let folder = profile
            .folder_of(m)
            .map(|f| match profile.include(f.id) {
                Some(_) => format!("  in {} (shared)", f.name),
                None => format!("  in {}", f.name),
            })
            .unwrap_or_default();
        println!(
            "{:>width$}. [{mark}] {}  ({}:{}{version}){priority}{folder}",
            i + 1,
            m.name,
            m.source,
            ids(m)
        );
    }
}

/// Asks which of a mod's files to add, if there's more than one; see
/// `Patchwork::file_choices`. `None` means the mod's id is fine as it is.
/// A folder can be picked too, which the caller then asks about in turn.
pub(crate) fn choose_file<'a>(
    source: &str,
    choices: &'a [FileChoice],
) -> Result<Option<&'a FileChoice>> {
    match choices {
        [] => return Ok(None),
        [only] => return Ok(Some(only)),
        _ => {}
    }
    if !std::io::stdin().is_terminal() {
        let ids: Vec<String> = choices
            .iter()
            .map(|c| {
                let spec = match c.id.contains(' ') {
                    true => format!("'{source}:{}'", c.id),
                    false => format!("{source}:{}", c.id),
                };
                let slash = if c.folder { "/" } else { "" };
                format!("\n  {}{slash}: {spec}", c.name)
            })
            .collect();
        bail!(
            "it has more than one file; add one of these instead:{}",
            ids.concat()
        );
    }
    eprintln!("which file?");
    for (i, choice) in choices.iter().enumerate() {
        let size = choice
            .size
            .map(|s| format!(" ({:.1} MB)", s as f64 / 1_000_000.0))
            .unwrap_or_default();
        let (slash, note) = match (choice.folder, choice.not_mod) {
            (true, _) => ("/", " (folder)"),
            (false, true) => ("", " (probably not a mod)"),
            (false, false) => ("", ""),
        };
        eprintln!("  {}. {}{slash}{size}{note}", i + 1, choice.name);
        if let Some(description) = &choice.description {
            eprintln!("     {description}");
        }
    }
    loop {
        eprint!("pick 1-{}: ", choices.len());
        std::io::stderr().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no file picked");
        }
        match line.trim().parse::<usize>() {
            Ok(n) if (1..=choices.len()).contains(&n) => {
                return Ok(Some(&choices[n - 1]));
            }
            _ => eprintln!("that isn't one of them"),
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct Add {
    spec: String,
    /// What to call the mod in this profile.
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    version: Option<String>,
    /// Add it without enabling it.
    #[arg(long)]
    disabled: bool,
}

/// `patchwork add`.
pub(crate) async fn add(pw: &Patchwork, explicit: &Option<String>, args: Add) -> Result<()> {
    let Add {
        spec,
        name,
        version,
        disabled,
    } = args;
    let (profile, loaded) = selected(pw, explicit)?;
    let mut spec = match spec.starts_with("https://") || spec.starts_with("http://") {
        true => ModSpec {
            version,
            ..pw.parse_link(&loaded.game, &spec)?
        },
        false => pw.parse_spec(&spec, version)?,
    };
    // a folder's files are chosen between in turn
    loop {
        let choices = pw.file_choices(&profile, &spec).await?;
        let Some(choice) = choose_file(&spec.source, &choices)? else {
            break;
        };
        spec.id = choice.id.clone();
        if !choice.folder {
            break;
        }
    }
    let added = pw.add_mod(&profile, spec, name, !disabled).await?;
    println!("added {} to {}", added.name, loaded.label());
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Search {
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
}

/// `patchwork search`.
pub(crate) async fn search(pw: &Patchwork, explicit: &Option<String>, args: Search) -> Result<()> {
    let Search {
        text,
        source,
        game,
        sort,
        categories,
        excluded_categories,
        page,
        list_categories,
    } = args;
    let game = match game {
        Some(game) => game,
        None => selected(pw, explicit)?.1.game,
    };
    let source = search_source(pw, &game, source)?;
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
    print_search(pw, &game, &source, &query).await?;
    Ok(())
}

/// `patchwork remove`.
pub(crate) fn remove(pw: &Patchwork, explicit: &Option<String>, name: String) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    pw.remove_mod(&profile, &name)?;
    println!("removed {name} from {}", loaded.label());
    Ok(())
}

/// `patchwork enable`.
pub(crate) fn enable(pw: &Patchwork, explicit: &Option<String>, name: String) -> Result<()> {
    pw.set_enabled(&selected(pw, explicit)?.0, &name, true)?;
    println!("enabled {name}");
    Ok(())
}

/// `patchwork disable`.
pub(crate) fn disable(pw: &Patchwork, explicit: &Option<String>, name: String) -> Result<()> {
    pw.set_enabled(&selected(pw, explicit)?.0, &name, false)?;
    println!("disabled {name}");
    Ok(())
}

/// `patchwork pin`.
pub(crate) async fn pin(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    version: String,
) -> Result<()> {
    let (profile, _) = selected(pw, explicit)?;
    pw.set_version(&profile, &name, Some(version.clone()))?;
    let lock = pw.lock(&profile, false).await?;
    println!(
        "pinned {name} to {version} ({} mods locked)",
        lock.mods.len()
    );
    Ok(())
}

/// `patchwork unpin`.
pub(crate) async fn unpin(pw: &Patchwork, explicit: &Option<String>, name: String) -> Result<()> {
    let (profile, _) = selected(pw, explicit)?;
    pw.set_version(&profile, &name, None)?;
    let lock = pw.lock(&profile, false).await?;
    let version = lock
        .mods
        .iter()
        .find(|m| m.name == name)
        .and_then(|m| m.version.as_deref())
        .unwrap_or("its newest version");
    println!("unpinned {name}; it's now at {version}");
    Ok(())
}

/// `patchwork follow` and `patchwork unfollow`.
pub(crate) fn follow(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: Option<String>,
    follow: bool,
) -> Result<()> {
    let Some(name) = name else {
        let follows = Follows::load(pw.paths())?;
        if follows.mods.is_empty() {
            println!("you don't follow any mods; `patchwork follow <mod>` does");
        }
        for key in &follows.mods {
            println!("{key}");
        }
        return Ok(());
    };
    let (_, loaded) = selected(pw, explicit)?;
    let m = loaded
        .mods
        .iter()
        .find(|m| m.name == name)
        .with_context(|| format!("{} has no mod named `{name}`", loaded.label()))?;
    set_followed(pw.paths(), &m.source, &m.id, follow)?;
    if !follow {
        println!("no longer following {name}");
        return Ok(());
    }
    println!("following {name} in every profile that has it");
    let settings = patchwork_daemon::DaemonSettings::load(pw.paths())?;
    let updates = settings.task("updates");
    if patchwork_daemon::service::installed()?.is_none() {
        println!(
            "the daemon isn't installed, so nothing will tell you; `patchwork daemon install` sets it up"
        );
    } else if !settings.on() || updates.enabled == Some(false) {
        println!(
            "the daemon's update check is off, so nothing will tell you; `patchwork daemon enable updates` turns it on"
        );
    } else if !updates.notifies() {
        println!("its notifications are off; `patchwork daemon notify updates on` turns them on");
    }
    Ok(())
}

/// `patchwork versions`.
pub(crate) async fn versions(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
) -> Result<()> {
    let (_, loaded) = selected(pw, explicit)?;
    let m = loaded
        .mods
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| anyhow::anyhow!("{} has no mod named `{name}`", loaded.label()))?;
    for v in pw.versions(&pw.lookup(&loaded)?, &m.source, &m.id).await? {
        let mark = if m.version.as_ref() == Some(&v) {
            " (pinned)"
        } else {
            ""
        };
        println!("{v}{mark}");
    }
    Ok(())
}

/// `patchwork changelog`.
pub(crate) async fn changelog(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    new: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let m = loaded
        .mods
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| anyhow::anyhow!("{} has no mod named `{name}`", loaded.label()))?;
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
    Ok(())
}

/// `patchwork list`.
pub(crate) fn list(
    pw: &Patchwork,
    explicit: &Option<String>,
    query: Vec<String>,
    load_order: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    // only for showing ids, so a broken one isn't an error here
    let lock = pw.load_lock(&profile).ok().flatten();
    let ids = |m: &ProfileMod| pw.display_id(lock.as_ref(), m);
    if loaded.mods.is_empty() {
        println!("{} has no mods", loaded.label());
    } else if load_order {
        let order = order::load_order(&loaded.mods);
        print_mods(order.into_iter().enumerate(), &loaded, ids);
    } else if query.is_empty() {
        print_mods(loaded.mods.iter().enumerate(), &loaded, ids);
    } else {
        let query = query.join(" ");
        let found = pw.find_mods(&profile, &query)?;
        if found.is_empty() {
            println!("no mods in {} match \"{query}\"", loaded.label());
        }
        print_mods(found.iter().map(|(i, m)| (*i, m)), &loaded, ids);
    }
    Ok(())
}

/// `patchwork move`.
pub(crate) fn move_mod(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    to: MoveTo,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let index = pw.move_mod(&profile, &name, &to.position())?;
    println!("moved {name} to {} of {}", index + 1, loaded.mods.len());
    Ok(())
}

/// `patchwork edit`.
pub(crate) fn edit(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    set: Vec<String>,
    reset: Vec<String>,
) -> Result<()> {
    let (profile, _) = selected(pw, explicit)?;
    let changes = field_changes(set, reset, |id| edits::mod_field(id).map(|f| f.kind))?;
    let changed = !changes.is_empty();
    if changed {
        pw.set_mod_edits(&profile, &name, changes)?;
    }
    let fields = pw.mod_fields(&profile, &name)?;
    let ids: Vec<_> = edits::MOD_FIELDS.iter().map(|f| f.id).collect();
    print_fields(&fields, changed, &ids);
    Ok(())
}

/// `patchwork priority`.
pub(crate) fn priority(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    priority: Option<i32>,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Sort {
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
}

/// `patchwork sort`.
pub(crate) async fn sort(pw: &Patchwork, explicit: &Option<String>, args: Sort) -> Result<()> {
    let Sort { by, tags, dry_run } = args;
    let (profile, loaded) = selected(pw, explicit)?;
    let sorted = pw.sorted_mods(&profile, &by, &tags).await?;
    // only for showing ids, so a broken one isn't an error here
    let lock = pw.load_lock(&profile).ok().flatten();
    print_mods(sorted.iter().enumerate(), &loaded, |m| {
        pw.display_id(lock.as_ref(), m)
    });
    if !dry_run {
        let names: Vec<String> = sorted.into_iter().map(|m| m.name).collect();
        pw.reorder_mods(&profile, &names)?;
    }
    Ok(())
}

/// `patchwork provide`.
pub(crate) async fn provide(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: String,
    file: PathBuf,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork link`.
pub(crate) async fn link(pw: &Patchwork, link: String) -> Result<()> {
    let Some(file) = pw.accept_link(&link).await? else {
        bail!("none of patchwork's sources recognize {link}");
    };
    println!(
        "downloaded {} for {}; it's used the next time a profile is locked",
        file.key(),
        file.game
    );
    Ok(())
}
