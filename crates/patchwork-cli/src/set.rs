//! `patchwork set`: shared sets of mods that several profiles include,
//! each shown in them as a linked folder. They're made by `patchwork folder
//! use`, and go once only one profile has them.

use anyhow::Result;
use clap::Subcommand;
use patchwork_core::Patchwork;

use crate::common::selected;

#[derive(Subcommand)]
pub(crate) enum SetCommand {
    /// List the game's sets, and which profiles include each.
    List,
    /// Include a set in the profile, as a linked folder. Changing what's in
    /// it changes the set, for every profile that includes it; only turning
    /// its mods on or off is the profile's own.
    Include {
        set: String,
        /// The profile's own mod it goes after; after all of them if left
        /// out.
        #[arg(long)]
        after: Option<String>,
    },
    /// Copy a set's mods into the profile as an ordinary folder of its own,
    /// rather than including it, so changing them doesn't change the set.
    Copy { set: String },
    /// Stop the profile including a set. Its mods stay as an ordinary
    /// folder of the profile's own, unless `--remove`. A set only one
    /// profile has left is that one's own folder again.
    Unlink {
        set: String,
        /// Take the set's mods out of the profile too.
        #[arg(long)]
        remove: bool,
    },
    /// Rename a set, for every profile that includes it.
    Rename { set: String, name: String },
    /// Delete a set left over from before sets went once no profile had
    /// them. No profile can still include it.
    Remove { set: String },
}

pub(crate) fn run(pw: &Patchwork, command: SetCommand, explicit: &Option<String>) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    let game = &loaded.game;
    match command {
        SetCommand::List => {
            let sets = pw.sets(game)?;
            if sets.is_empty() {
                println!("{game} has no sets");
            }
            let mut includers = pw.includers()?;
            for set in sets {
                let including: Vec<String> = includers
                    .remove(&set.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect();
                let mark = if loaded.include(set.id).is_some() {
                    '*'
                } else {
                    ' '
                };
                println!("{mark} {} ({} mods)", set.name, set.mods.len());
                for m in &set.mods {
                    println!("    {}", m.name);
                }
                if !including.is_empty() {
                    println!("    included by {}", including.join(", "));
                }
            }
        }
        SetCommand::Include { set, after } => {
            let set = pw.find_set(game, &set)?;
            pw.include_set(&profile, set.id, after.as_deref())?;
            println!("{} now includes {}", loaded.label(), set.name);
        }
        SetCommand::Copy { set } => {
            let set = pw.find_set(game, &set)?;
            pw.copy_set(&profile, set.id)?;
            println!("copied {} into {}", set.name, loaded.label());
        }
        SetCommand::Unlink { set, remove } => {
            // one that's missing goes by its id
            let missing = loaded
                .includes
                .iter()
                .find(|i| i.linked.is_none() && i.set.to_string() == set);
            if let Some(include) = missing {
                pw.unlink_set(&profile, include.set, false)?;
                println!(
                    "{} no longer includes the missing set {set}",
                    loaded.label()
                );
                return Ok(());
            }
            let set = pw.find_set(game, &set)?;
            pw.unlink_set(&profile, set.id, !remove)?;
            if remove {
                println!("took {} and its mods out of {}", set.name, loaded.label());
            } else {
                println!("{}'s mods are now {}'s own", set.name, loaded.label());
            }
        }
        SetCommand::Rename { set, name } => {
            let found = pw.find_set(game, &set)?;
            pw.rename_set(found.id, &name)?;
            println!("renamed {} to {}", found.name, name.trim());
        }
        SetCommand::Remove { set } => {
            let found = pw.find_set(game, &set)?;
            pw.remove_set(found.id)?;
            println!("removed set {}", found.name);
        }
    }
    Ok(())
}
