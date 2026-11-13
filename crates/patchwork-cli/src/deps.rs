//! Dependencies, replacements, compatibility and conflicts between mods.

use anyhow::{Context, Result, bail};
use patchwork_core::Patchwork;
use patchwork_core::compat::GameVersions;
use patchwork_core::conflicts::Conflicts;
use patchwork_core::lock::mod_key;
use patchwork_core::source::Need;

use crate::common::selected;

/// Each mod's conflicts, in load order, as how many files it shares with
/// each other mod, or every file of `only`'s.
pub(crate) fn print_conflicts(conflicts: &Conflicts, only: Option<&str>) {
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
            "  (every one of its files is overwritten by later mods)"
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

#[derive(clap::Args)]
pub(crate) struct Supports {
    name: String,
    /// Version ranges, any of which will do: `1.5.97`, `1.6.*`,
    /// `">=1.6.640 <1.7"`.
    versions: Vec<String>,
    /// Go back to what the mod's source says.
    #[arg(long, conflicts_with = "versions")]
    reset: bool,
}

/// `patchwork supports`.
pub(crate) fn supports(pw: &Patchwork, explicit: &Option<String>, args: Supports) -> Result<()> {
    let Supports {
        name,
        versions,
        reset,
    } = args;
    let (profile, mut loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork deps`.
pub(crate) fn deps(pw: &Patchwork, explicit: &Option<String>, all: bool) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork soft-deps`.
pub(crate) fn soft_deps(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: Option<String>,
    missing: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let Some(lock) = pw.load_lock(&profile)? else {
        println!("{} isn't locked yet; run `patchwork lock`", loaded.label());
        return Ok(());
    };
    if let Some(name) = &name
        && loaded.find(name).is_none()
    {
        bail!("{} has no mod named `{name}`", loaded.label());
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
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Ignore {
    name: String,
    dependency: String,
    /// Point it out again.
    #[arg(long)]
    undo: bool,
}

/// `patchwork ignore`.
pub(crate) fn ignore(pw: &Patchwork, explicit: &Option<String>, args: Ignore) -> Result<()> {
    let Ignore {
        name,
        dependency,
        undo,
    } = args;
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Replace {
    name: String,
    dependencies: Vec<String>,
    /// Stop it standing in for the dependencies given, or for all of
    /// them if none are.
    #[arg(long)]
    undo: bool,
}

/// `patchwork replace`.
pub(crate) async fn replace(
    pw: &Patchwork,
    explicit: &Option<String>,
    args: Replace,
) -> Result<()> {
    let Replace {
        name,
        dependencies,
        undo,
    } = args;
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork conflicts`.
pub(crate) fn conflicts(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: Option<String>,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    if let Some(name) = &name
        && !loaded.mods.iter().any(|m| m.name == *name)
    {
        bail!("{} has no mod named `{name}`", loaded.label());
    }
    let conflicts = pw.conflicts(&profile)?;
    print_conflicts(&conflicts, name.as_deref());
    Ok(())
}
