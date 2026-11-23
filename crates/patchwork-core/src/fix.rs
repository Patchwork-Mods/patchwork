//! What the user can do about an error or an [`Issue`](crate::issue::Issue).
//!
//! An error that has something in the way of what was asked, rather than
//! something broken, says what that is with a [`Blocker`] (see
//! [`Error::blocker`](crate::Error::blocker)): a title, a detail, and the
//! [`Fix`] that clears it. Each case is written once, here or where the
//! error is raised; the GUI shows it as a button and the CLI prints
//! [`Fix::command`].
//!
//! Every error that carries a fix has to be raised before anything was
//! changed, since frontends try what failed again once the fix has run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Something in the way, and what clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    /// What's in the way, short enough for a dialog's heading.
    pub title: String,
    /// Why it's in the way, and what the fix does.
    pub detail: String,
    pub fix: Fix,
}

/// A profile a [`Fix`] is about, by id and by the name users know it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileName {
    /// Its [`ProfileRef`](crate::profile::ProfileRef).
    pub id: String,
    /// `<game>/<name>`.
    pub label: String,
}

/// Something that clears a [`Blocker`] or deals with an issue. Fixes about
/// "the profile" are about the one the command that failed was working on;
/// the rest say what they're about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Fix {
    /// Lock the mods in `keys` (as `<source>:<id>`, named `names`) to the
    /// files they're downloaded as now; see
    /// [`Patchwork::accept_changed`](crate::Patchwork::accept_changed).
    AcceptChanged {
        keys: Vec<String>,
        names: Vec<String>,
    },
    /// Take every file patchwork put in `game`'s folder back out, or in
    /// `instance`'s folder for a profile with an instance of its own.
    /// Frontends confirm it first, since it changes the game.
    Undeploy {
        game: String,
        instance: Option<ProfileName>,
    },
    /// Deploy the profile again.
    Deploy,
    /// Use `path` as where `game` is installed.
    SetGamePath { game: String, path: PathBuf },
    /// Ask the user where `game` is installed. Only a frontend can do this;
    /// it ends in [`Fix::SetGamePath`].
    ChooseGamePath { game: String },
    /// Lock the profile, downloading what it needs.
    Lock,
    /// Get local mods' files from another of the user's machines, which
    /// syncing does as soon as one that has them is online.
    Sync,
    /// Update patchwork itself.
    UpdatePatchwork,
    /// Open the profile's settings, to pick something it needs. Only a
    /// frontend can do this.
    ProfileSettings,
}

impl Fix {
    /// What a button that applies it says.
    pub fn label(&self) -> String {
        match self {
            Fix::AcceptChanged { names, .. } if names.len() == 1 => "Use the new file".to_owned(),
            Fix::AcceptChanged { .. } => "Use the new files".to_owned(),
            Fix::Undeploy { .. } => "Undeploy and continue".to_owned(),
            Fix::Deploy => "Deploy".to_owned(),
            Fix::SetGamePath { .. } => "Use this folder".to_owned(),
            Fix::ChooseGamePath { .. } => "Choose folder…".to_owned(),
            Fix::Lock => "Download".to_owned(),
            Fix::Sync => "Get it from my other PC".to_owned(),
            Fix::UpdatePatchwork => "Update Patchwork".to_owned(),
            Fix::ProfileSettings => "Open profile settings".to_owned(),
        }
    }

    /// What to say while it runs.
    pub fn doing(&self) -> String {
        match self {
            Fix::AcceptChanged { names, .. } => {
                format!("Using the new file for {}…", names.join(", "))
            }
            Fix::Undeploy { .. } => "Undeploying…".to_owned(),
            Fix::Deploy => "Deploying…".to_owned(),
            Fix::SetGamePath { .. } | Fix::ChooseGamePath { .. } => {
                "Setting the game's folder…".to_owned()
            }
            Fix::Lock => "Downloading…".to_owned(),
            Fix::Sync => "Waiting for your other PC…".to_owned(),
            Fix::UpdatePatchwork => "Updating Patchwork…".to_owned(),
            Fix::ProfileSettings => "Opening profile settings…".to_owned(),
        }
    }

    /// The CLI command that does the same, run on `profile` (as given to
    /// `--profile`; `None` for the current one).
    pub fn command(&self, profile: Option<&str>) -> String {
        let on = |rest: String| match profile {
            Some(p) => format!("patchwork --profile {p} {rest}"),
            None => format!("patchwork {rest}"),
        };
        match self {
            Fix::AcceptChanged { keys, .. } => {
                let accepts: Vec<_> = keys.iter().map(|k| format!("--accept {k}")).collect();
                on(format!("lock {}", accepts.join(" ")))
            }
            Fix::Undeploy {
                instance: Some(p), ..
            } => format!("patchwork --profile {} undeploy", p.label),
            Fix::Undeploy { game, .. } => format!("patchwork undeploy {game}"),
            Fix::Deploy => on("deploy".to_owned()),
            Fix::SetGamePath { game, path } => {
                format!("patchwork game set-path {game} {}", path.display())
            }
            Fix::ChooseGamePath { game } => format!("patchwork game set-path {game} <dir>"),
            Fix::Lock => on("lock".to_owned()),
            Fix::Sync => "patchwork sync".to_owned(),
            Fix::UpdatePatchwork => "patchwork self-update".to_owned(),
            Fix::ProfileSettings => on("env".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_go_to_frontends_tagged_by_type() {
        let fix = Fix::SetGamePath {
            game: "g".to_owned(),
            path: "/games/g".into(),
        };
        let value = serde_json::to_value(&fix).unwrap();
        assert_eq!(value["type"], "setGamePath");
        assert_eq!(value["path"], "/games/g");
        assert_eq!(serde_json::from_value::<Fix>(value).unwrap(), fix);
        let undeploy = Fix::Undeploy {
            game: "g".to_owned(),
            instance: None,
        };
        assert_eq!(serde_json::to_value(&undeploy).unwrap()["type"], "undeploy");
    }

    #[test]
    fn commands_run_on_the_profile_given() {
        let accept = Fix::AcceptChanged {
            keys: vec!["nexus:1".to_owned()],
            names: vec!["A".to_owned()],
        };
        assert_eq!(accept.command(None), "patchwork lock --accept nexus:1");
        assert_eq!(
            accept.command(Some("g/p")),
            "patchwork --profile g/p lock --accept nexus:1"
        );
        let undeploy = Fix::Undeploy {
            game: "g".to_owned(),
            instance: None,
        };
        assert_eq!(undeploy.command(Some("g/p")), "patchwork undeploy g");
    }
}
