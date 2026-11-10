//! Mod options, mod settings and config files.

use anyhow::{Context, Result, bail};
use patchwork_core::deploy::Dest;
use patchwork_core::options::{Answer, Field, FieldKind, Form, ItemState, Pick, Value};
use patchwork_core::profile::ProfileRef;
use patchwork_core::{ConfigFileInfo, ConfigView, ModOptionsView, Patchwork};

use crate::common::selected;

/// The id of the choices `wanted` names (an id, or the last part of one)
/// for mod `name`, or `None` to let core take the mod's only ones.
pub(crate) fn pick_options(
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
pub(crate) fn find_pick(view: &ModOptionsView, pick: &str) -> Result<Answer> {
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
pub(crate) fn find_set(form: &Form, set: &str) -> Result<(String, String, Value)> {
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
pub(crate) fn shown(field: &Field, value: &Value) -> String {
    match (&field.kind, value) {
        (FieldKind::Choice { choices }, Value::Text(id)) => choices
            .iter()
            .find(|c| c.id == *id)
            .map_or_else(|| value.to_string(), |c| c.title.clone()),
        _ => value.to_string(),
    }
}

pub(crate) fn print_options(name: &str, view: &ModOptionsView) {
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
pub(crate) fn find_config_file(files: &[ConfigFileInfo], name: &str) -> Result<Dest> {
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

pub(crate) fn print_config_file(f: &ConfigFileInfo) {
    let mut about = Vec::new();
    if let Some(title) = &f.title {
        about.push(title.clone());
    }
    if let Some(m) = &f.from {
        about.push(format!("from {m}"));
    } else if let Some(m) = &f.owner {
        about.push(format!("for {m}"));
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

pub(crate) fn print_config(view: &ConfigView) {
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

#[derive(clap::Args)]
pub(crate) struct Options {
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
}

/// `patchwork options`.
pub(crate) fn options(pw: &Patchwork, explicit: &Option<String>, args: Options) -> Result<()> {
    let Options {
        name,
        of,
        picks,
        sets,
        save,
        reset,
    } = args;
    let (profile, _) = selected(pw, explicit)?;
    let of = pick_options(pw, &profile, &name, of.as_deref())?;
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
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Settings {
    /// A config file, by its path as listed, or the end of it (like its
    /// name) if that's enough to tell which.
    file: Option<String>,
    /// Only list the config files for this mod: the ones it deploys, and
    /// ones the game made that are named like it.
    #[arg(long = "mod", value_name = "NAME", conflicts_with = "file")]
    of: Option<String>,
    /// Set a setting, as `SETTING=VALUE`, or `GROUP/SETTING=VALUE` if
    /// more than one group has it. Repeat for more; they're saved.
    #[arg(long = "set", value_name = "SETTING=VALUE", requires = "file")]
    sets: Vec<String>,
    /// Forget the settings changed in the file, so it's deployed as it is.
    #[arg(long, requires = "file", conflicts_with = "sets")]
    reset: bool,
}

/// `patchwork settings`.
pub(crate) fn settings(pw: &Patchwork, explicit: &Option<String>, args: Settings) -> Result<()> {
    let Settings {
        file,
        of,
        sets,
        reset,
    } = args;
    let (profile, _) = selected(pw, explicit)?;
    let files = pw.config_files(&profile)?;
    let Some(file) = file else {
        let shown: Vec<_> = files
            .iter()
            .filter(|f| of.as_ref().is_none_or(|m| f.owner.as_ref() == Some(m)))
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
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Env {
    /// Which axis; all of them if left out.
    axis: Option<String>,
    value: Option<String>,
    /// Go back to the game's default.
    #[arg(long, requires = "axis", conflicts_with = "value")]
    reset: bool,
}

/// `patchwork env`.
pub(crate) async fn env(pw: &Patchwork, explicit: &Option<String>, args: Env) -> Result<()> {
    let Env { axis, value, reset } = args;
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork instance`.
pub(crate) fn instance(
    pw: &Patchwork,
    explicit: &Option<String>,
    value: Option<bool>,
    reset: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}
