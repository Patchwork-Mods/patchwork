//! Helpers that most commands share: the selected profile, game installs, and
//! arguments used by more than one command.

use anyhow::{Context, Result, bail};
use patchwork_core::issue::Severity;
use patchwork_core::order::Position;
use patchwork_core::profile::{Profile, ProfileRef};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::{GameInstall, Patchwork};

#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub(crate) struct MoveTo {
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
    pub(crate) fn position(self) -> Position {
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

pub(crate) fn game_install(pw: &Patchwork, id: &str) -> Result<GameInstall> {
    pw.game_install(id)?.with_context(|| {
        format!("patchwork doesn't know where {id} is installed; run `patchwork game set-path {id} <dir>` first")
    })
}

pub(crate) fn print_setting(setting: &SettingState) {
    let key = setting.spec.key;
    let shown = match (&setting.value, setting.origin) {
        (_, None) => match &setting.default {
            Some(default) => format!("{default} (default)"),
            None => "(not set)".to_owned(),
        },
        (Some(value), _) => value.to_string(),
        (None, _) => "(set)".to_owned(),
    };
    let from = match setting.origin {
        Some(Origin::Env(var)) => format!(" from {var}"),
        Some(Origin::Keychain) => " in the system keychain".to_owned(),
        _ => String::new(),
    };
    println!("{key} = {shown}{from}");
}

pub(crate) fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

pub(crate) fn selected(pw: &Patchwork, explicit: &Option<String>) -> Result<(ProfileRef, Profile)> {
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
