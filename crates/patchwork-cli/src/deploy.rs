//! Locking, deploying and undeploying, history, ordered lists, actions, tools
//! and `status`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use patchwork_core::action::ActionReport;
use patchwork_core::cancel::Cancel;
use patchwork_core::capture::ToolOutput;
use patchwork_core::deploy::{Check, DeployOp, DeployPlan, DeployTarget};
use patchwork_core::list::List;
use patchwork_core::profile::ProfileRef;
use patchwork_core::{DeployStatus, Patchwork};

use crate::common::{MoveTo, selected, severity};

#[derive(Subcommand)]
pub(crate) enum OrderCommand {
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

pub(crate) fn status(pw: &Patchwork, explicit: &Option<String>) -> Result<()> {
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

/// Lists what tools wrote, and warns about the game's own files they
/// changed.
pub(crate) fn print_tool_output(output: &ToolOutput) {
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

pub(crate) fn finish(pw: &Patchwork, plan: &DeployPlan, dry_run: bool) -> Result<()> {
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

pub(crate) fn changes(n: usize) -> String {
    format!("{n} change{}", if n == 1 { "" } else { "s" })
}

fn in_place(n: usize) -> String {
    format!("{n} file{} already in place", if n == 1 { "" } else { "s" })
}

pub(crate) fn print_op(op: &DeployOp) {
    match op {
        DeployOp::Remove { dst } => println!("  - {dst}"),
        DeployOp::Backup { dst } => println!("  ~ {dst} (backing up original)"),
        // Files no mod has, like config files from the game's own folder.
        DeployOp::Place { dst, mod_name, .. } if mod_name.is_empty() => println!("  + {dst}"),
        DeployOp::Place { dst, mod_name, .. } => println!("  + {dst}  [{mod_name}]"),
    }
}

/// The id of the list `wanted` names (its id or the last part of it), or
/// of the game's only list.
pub(crate) fn pick_list(
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

pub(crate) fn print_list(list: &List) {
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

pub(crate) fn print_action(report: &ActionReport) {
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

/// `patchwork order`.
pub(crate) fn order(
    pw: &Patchwork,
    explicit: &Option<String>,
    list: Option<String>,
    action: OrderCommand,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let id = pick_list(pw, &profile, &loaded.game, list.as_deref())?;
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
    Ok(())
}

/// `patchwork action`.
pub(crate) async fn action(
    pw: &Patchwork,
    explicit: &Option<String>,
    id: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork lock`.
pub(crate) async fn lock(pw: &Patchwork, explicit: &Option<String>, update: bool) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let lock = pw.lock(&profile, update).await?;
    println!("locked {} mods for {}", lock.mods.len(), loaded.label());
    Ok(())
}

/// `patchwork deploy`.
pub(crate) async fn deploy(
    pw: &Patchwork,
    explicit: &Option<String>,
    dry_run: bool,
    verify: bool,
) -> Result<()> {
    let (profile, _) = selected(pw, explicit)?;
    let check = if verify { Check::Full } else { Check::Quick };
    let plan = pw
        .plan_deploy_checked(&profile, check, &Cancel::never())
        .await?;
    finish(pw, &plan, dry_run)?;
    Ok(())
}

/// `patchwork capture`.
pub(crate) async fn capture(
    pw: &Patchwork,
    explicit: &Option<String>,
    name: Option<String>,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
            finish(pw, &plan, false)?;
        }
    }
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct Tool {
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
}

/// `patchwork tool`.
pub(crate) async fn tool(pw: &Patchwork, explicit: &Option<String>, args: Tool) -> Result<()> {
    let Tool {
        into,
        dry_run,
        exe,
        args,
    } = args;
    let (profile, loaded) = selected(pw, explicit)?;
    let exe = std::path::absolute(&exe).with_context(|| format!("can't find {}", exe.display()))?;
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
            finish(pw, &plan, false)?;
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
        finish(pw, &plan, false)?;
    }
    Ok(())
}

/// `patchwork history`.
pub(crate) fn history(pw: &Patchwork, explicit: &Option<String>) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork rollback`.
pub(crate) fn rollback(
    pw: &Patchwork,
    explicit: &Option<String>,
    number: Option<u32>,
    dry_run: bool,
) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
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
    Ok(())
}

/// `patchwork undeploy`.
pub(crate) fn undeploy(
    pw: &Patchwork,
    explicit: &Option<String>,
    game: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let target = match game {
        Some(game) => DeployTarget::Game(game),
        None => pw.deploy_target(&selected(pw, explicit)?.1)?,
    };
    let plan = pw.plan_undeploy(&target)?;
    finish(pw, &plan, dry_run)?;
    Ok(())
}
