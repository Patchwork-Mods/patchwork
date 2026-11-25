//! `patchwork folder`: groups of the profile's mods.

use anyhow::{Result, bail};
use clap::Subcommand;
use patchwork_core::Patchwork;
use patchwork_core::set::OtherFolder;

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
    /// List the folders of the game's other profiles, which `use` can add
    /// to this one.
    Others,
    /// Use a folder another profile of the game has in this one too, as the
    /// same folder: adding, removing or reordering its mods changes it in
    /// every profile that has it, while turning them on or off is each
    /// profile's own.
    Use {
        folder: String,
        /// The profile it's from, by name, if more than one has a folder
        /// called that.
        #[arg(long)]
        from: Option<String>,
        /// Copy its mods in as a folder of this profile's own instead, which
        /// changes apart from the other's.
        #[arg(long)]
        copy: bool,
    },
    /// Rename a folder. A shared one is renamed in every profile that has it.
    Rename { folder: String, name: String },
    /// Remove a folder. Its mods stay in the profile, where they are.
    Remove { folder: String },
    /// Put a mod in a folder, at its end, or with no folder take it out of
    /// the one it's in.
    Put {
        name: String,
        folder: Option<String>,
    },
    /// Keep mods of a synced profile to yourself: they go in its Personal
    /// folder, and are synced only to your own linked computers.
    Personal {
        #[arg(required = true)]
        mods: Vec<String>,
    },
    /// Move personal mods back into what's synced, for everyone.
    Publish {
        #[arg(required = true)]
        mods: Vec<String>,
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
            if loaded.shown_folders().next().is_none() {
                println!("{} has no folders", loaded.label());
            }
            for f in loaded.shown_folders() {
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
                let shared = if f.id == loaded.id {
                    format!(", only on {}", personal_place(pw))
                } else if let Some(include) = loaded.include(f.id) {
                    let others: Vec<String> = pw
                        .profiles_including(f.id)?
                        .iter()
                        .filter(|p| **p != profile)
                        .filter_map(|p| pw.load_profile(p).ok().map(|l| l.name))
                        .collect();
                    let only = if include.personal {
                        format!(", only on {}", personal_place(pw))
                    } else {
                        String::new()
                    };
                    format!(", shared with {}{only}", others.join(", "))
                } else {
                    String::new()
                };
                println!("{} ({} mods{off}{shared})", f.name, mods.len());
                for m in mods {
                    println!("  {}", m.name);
                }
            }
        }
        FolderCommand::Others => {
            let others = pw.other_folders(&profile)?;
            if others.is_empty() {
                println!("{}'s other profiles have no folders", loaded.game);
            }
            for f in others {
                println!("{} ({} mods, {})", f.name, f.mods.len(), whose(&f));
                for m in &f.mods {
                    println!("  {m}");
                }
                if !f.already.is_empty() {
                    println!(
                        "  {} has {} of its own already",
                        loaded.label(),
                        f.already.join(", ")
                    );
                }
                if let Some(why) = &f.copy_only {
                    println!("  can only be copied in: {why}");
                }
                if f.personal {
                    println!(
                        "  {} is synced, so used here it'd only be on {}; copy it in to share \
                         it with everyone",
                        loaded.label(),
                        personal_place(pw)
                    );
                }
            }
        }
        FolderCommand::Use { folder, from, copy } => {
            let found = other_folder(pw.other_folders(&profile)?, &folder, from.as_deref())?;
            pw.use_folder(&profile, &found.source, !copy)?;
            if copy {
                println!("copied {} into {}", found.name, loaded.label());
            } else {
                let only = if found.personal {
                    format!(", only on {}", personal_place(pw))
                } else {
                    String::new()
                };
                println!(
                    "{} now shares {} with {}{only}",
                    loaded.label(),
                    found.name,
                    found.profiles.join(", ")
                );
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
        FolderCommand::Personal { mods } => {
            pw.set_personal(&profile, &mods, true)?;
            println!("{} only on {} now", is_are(&mods), personal_place(pw));
        }
        FolderCommand::Publish { mods } => {
            pw.set_personal(&profile, &mods, false)?;
            println!("{} synced to everyone now", is_are(&mods));
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

/// Where a folder another profile has is, for listing it.
fn whose(f: &OtherFolder) -> String {
    match f.profiles.as_slice() {
        [] => "in no profile".to_owned(),
        [one] => format!("from {one}"),
        all => format!("shared by {}", all.join(", ")),
    }
}

/// The folder of `others` called `name`, from the profile `from` if given.
fn other_folder(others: Vec<OtherFolder>, name: &str, from: Option<&str>) -> Result<OtherFolder> {
    let mut found: Vec<OtherFolder> = others
        .into_iter()
        .filter(|f| f.name.eq_ignore_ascii_case(name))
        .filter(|f| from.is_none_or(|from| f.profiles.iter().any(|p| p.eq_ignore_ascii_case(from))))
        .collect();
    match found.len() {
        0 => match from {
            Some(from) => bail!("{from} has no folder named `{name}` to use here"),
            None => bail!("no other profile has a folder named `{name}` to use here"),
        },
        1 => Ok(found.remove(0)),
        _ => {
            let all: Vec<String> = found.iter().map(whose).collect();
            bail!(
                "more than one profile has a folder named `{name}` ({}); say which with --from",
                all.join("; ")
            )
        }
    }
}

/// `mods` and "is" or "are".
fn is_are(mods: &[String]) -> String {
    match mods {
        [one] => format!("{one} is"),
        _ => format!("{} are", mods.join(", ")),
    }
}

/// Where a synced profile's personal mods are kept: this computer, or all
/// of the person's linked ones (see `patchwork_sync`'s devices document).
fn personal_place(pw: &Patchwork) -> &'static str {
    if patchwork_sync::devices_linked(&pw.paths().sync()).unwrap_or(false) {
        "your linked computers"
    } else {
        "this computer"
    }
}
