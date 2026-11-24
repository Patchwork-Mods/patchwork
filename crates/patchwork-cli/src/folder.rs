//! `patchwork folder`: groups of the profile's mods.

use anyhow::Result;
use clap::Subcommand;
use patchwork_core::Patchwork;

use crate::common::selected;

#[derive(Subcommand)]
pub(crate) enum FolderCommand {
    /// List the profile's folders and what's in each.
    List,
    /// Make a folder, and put mods in it. It goes where the first of them
    /// is in the load order; a folder's mods are always kept together.
    Add {
        name: String,
        /// Mods to put in it.
        mods: Vec<String>,
    },
    /// Rename a folder.
    Rename { folder: String, name: String },
    /// Remove a folder. Its mods stay in the profile, where they are.
    Remove { folder: String },
    /// Put a mod in a folder, at its end, or with no folder take it out of
    /// the one it's in.
    Put {
        name: String,
        folder: Option<String>,
    },
    /// Enable every mod in a folder.
    On { folder: String },
    /// Disable every mod in a folder.
    Off { folder: String },
}

pub(crate) fn run(pw: &Patchwork, command: FolderCommand, explicit: &Option<String>) -> Result<()> {
    let (profile, loaded) = selected(pw, explicit)?;
    match command {
        FolderCommand::List => {
            if loaded.folders.is_empty() {
                println!("{} has no folders", loaded.label());
            }
            for f in &loaded.folders {
                let mods: Vec<_> = loaded
                    .mods
                    .iter()
                    .filter(|m| m.folder == Some(f.id))
                    .collect();
                let off = mods.iter().filter(|m| !m.enabled).count();
                let off = if off > 0 {
                    format!(", {off} disabled")
                } else {
                    String::new()
                };
                println!("{} ({} mods{off})", f.name, mods.len());
                for m in mods {
                    println!("  {}", m.name);
                }
            }
        }
        FolderCommand::Add { name, mods } => {
            let folder = pw.add_folder(&profile, &name, &mods)?;
            println!("added folder {}", folder.name);
        }
        FolderCommand::Rename { folder, name } => {
            pw.rename_folder(&profile, &folder, &name)?;
            println!("renamed {folder} to {}", name.trim());
        }
        FolderCommand::Remove { folder } => {
            pw.remove_folder(&profile, &folder)?;
            println!("removed folder {folder}; its mods are still in the profile");
        }
        FolderCommand::Put { name, folder } => {
            pw.set_mod_folder(&profile, &name, folder.as_deref())?;
            match folder {
                Some(folder) => println!("put {name} in {folder}"),
                None => println!("took {name} out of its folder"),
            }
        }
        FolderCommand::On { folder } => {
            pw.set_folder_enabled(&profile, &folder, true)?;
            println!("enabled the mods in {folder}");
        }
        FolderCommand::Off { folder } => {
            pw.set_folder_enabled(&profile, &folder, false)?;
            println!("disabled the mods in {folder}");
        }
    }
    Ok(())
}
