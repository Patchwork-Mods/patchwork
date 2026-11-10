//! `patchwork config`: settings for sources, such as API keys.

use std::io::Write;

use anyhow::{Context, Result};
use clap::Subcommand;
use patchwork_core::Patchwork;
use patchwork_core::settings::Origin;

use crate::common::print_setting;

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
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

pub(crate) async fn run(pw: &Patchwork, command: ConfigCommand) -> Result<()> {
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
