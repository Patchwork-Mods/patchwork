//! `patchwork profile`: making, picking, importing and exporting profiles.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use patchwork_core::{ImportOptions, Patchwork};

use crate::common::selected;

#[derive(Subcommand)]
pub(crate) enum ProfileCommand {
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

pub(crate) async fn run(
    pw: &Patchwork,
    command: ProfileCommand,
    explicit: &Option<String>,
) -> Result<()> {
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
