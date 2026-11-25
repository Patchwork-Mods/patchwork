//! What the user can do about an error or an [`Issue`](crate::issue::Issue).
//!
//! An error that has something in the way of what was asked, rather than
//! something broken, says what that is with a [`Blocker`] (see
//! [`Error::blocker`](crate::Error::blocker)): a title, a detail, and the
//! [`Fix`]es that clear it. Each case is written once, here or where the
//! error is raised; the GUI shows each fix as a button and the CLI prints
//! [`Fix::command`].
//!
//! Every error that carries a blocker has to be raised before anything was
//! changed, since frontends try what failed again once it's cleared.

use std::path::PathBuf;

use serde::{Deserialize, Serialize, Serializer};

/// Something in the way, and what clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    /// What's in the way, short enough for a dialog's heading.
    pub title: String,
    /// Why it's in the way, and what the fixes do.
    pub detail: String,
    /// What clears it, the one to suggest first. Empty when nothing
    /// patchwork can do will: the user deals with it themselves (the detail
    /// says how) and tries again. Goes to frontends with each fix's `label`
    /// and `doing` alongside, for its button.
    #[serde(serialize_with = "labelled_all")]
    pub fixes: Vec<Fix>,
}

impl Blocker {
    /// A blocker cleared by `fix`.
    pub fn new(title: impl Into<String>, detail: impl Into<String>, fix: Fix) -> Self {
        Self::with_fixes(title, detail, vec![fix])
    }

    /// A blocker cleared by any of `fixes`, the first being the one to
    /// suggest.
    pub fn with_fixes(
        title: impl Into<String>,
        detail: impl Into<String>,
        fixes: Vec<Fix>,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            fixes,
        }
    }

    /// A blocker only the user can clear, outside patchwork, after which
    /// what failed can be tried again; `detail` says how.
    pub fn try_again(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::with_fixes(title, detail, Vec::new())
    }

    /// The fix to suggest, if there is one.
    pub fn fix(&self) -> Option<&Fix> {
        self.fixes.first()
    }
}

/// A fix with its [`Fix::label`] and [`Fix::doing`] alongside, for
/// frontends that show it as a button. Reading it back as a [`Fix`] ignores
/// them.
#[derive(Serialize)]
struct Labelled<'a> {
    #[serde(flatten)]
    fix: &'a Fix,
    label: String,
    doing: String,
}

impl<'a> From<&'a Fix> for Labelled<'a> {
    fn from(fix: &'a Fix) -> Self {
        Labelled {
            fix,
            label: fix.label(),
            doing: fix.doing(),
        }
    }
}

/// Serializes a fix as [`Labelled`].
pub(crate) fn labelled<S: Serializer>(fix: &Option<Fix>, s: S) -> Result<S::Ok, S::Error> {
    fix.as_ref().map(Labelled::from).serialize(s)
}

/// Serializes fixes as [`Labelled`].
pub(crate) fn labelled_all<S: Serializer>(fixes: &[Fix], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(fixes.iter().map(Labelled::from))
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

/// What going back to a deploy (a revert or a rollback) does with the
/// shared sets that have changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetsThen {
    /// Put them back as they were, for every profile that includes them.
    Restore,
    /// Copy them in as they were, as ordinary folders of the profile's own.
    Copy,
    /// Keep them as they are, going back for the rest.
    Keep,
}

impl SetsThen {
    /// As the CLI's `--sets` takes it.
    pub fn as_str(self) -> &'static str {
        match self {
            SetsThen::Restore => "restore",
            SetsThen::Copy => "copy",
            SetsThen::Keep => "keep",
        }
    }
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
    /// Move the mods in `keys` (as `<source>:<id>`, named `names`) to their
    /// newest versions; see
    /// [`Patchwork::lock_updating`](crate::Patchwork::lock_updating).
    UpdateMods {
        keys: Vec<String>,
        names: Vec<String>,
    },
    /// Take the mods named `names` out of the profile.
    RemoveMods { names: Vec<String> },
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
    /// Open Settings at `source`'s settings (its API key, say), named
    /// `name`, to fill in something it needs. Only a frontend can do this.
    SourceSettings { source: String, name: String },
    /// Save the profile's folder `folder` (by id, named `name`), which a
    /// friend changed, to the set it came from and include that again, so
    /// it's the same in every profile that has it; see
    /// [`Patchwork::use_folder_everywhere`](crate::Patchwork::use_folder_everywhere).
    UseEverywhere { folder: String, name: String },
    /// Keep the profile's folder `folder` (by id, named `name`), which a
    /// friend changed, as its own, leaving the set it came from as it is
    /// for the other profiles; see
    /// [`Patchwork::keep_folder_here`](crate::Patchwork::keep_folder_here).
    KeepHere { folder: String, name: String },
    /// Go back to the profile's last deploy (`number` `None`) or deploy
    /// `number` of its history, doing as `sets` says with the shared sets
    /// that have changed since; see
    /// [`Patchwork::go_back`](crate::Patchwork::go_back). Frontends show
    /// the profile it's gone back to rather than trying again.
    GoBack { number: Option<u32>, sets: SetsThen },
    /// Stop the profile including the set `set` (by id), which can't be
    /// found; see [`IssueKind::MissingSet`](crate::issue::IssueKind::MissingSet).
    ForgetSet { set: String },
}

impl Fix {
    /// What a button that applies it says.
    pub fn label(&self) -> String {
        match self {
            Fix::AcceptChanged { names, .. } if names.len() == 1 => "Use the new file".to_owned(),
            Fix::AcceptChanged { .. } => "Use the new files".to_owned(),
            Fix::UpdateMods { .. } => "Update to latest".to_owned(),
            Fix::RemoveMods { names } if names.len() == 1 => "Remove it".to_owned(),
            Fix::RemoveMods { .. } => "Remove them".to_owned(),
            Fix::Undeploy { .. } => "Undeploy and continue".to_owned(),
            Fix::Deploy => "Deploy".to_owned(),
            Fix::SetGamePath { .. } => "Use this folder".to_owned(),
            Fix::ChooseGamePath { .. } => "Choose folder…".to_owned(),
            Fix::Lock => "Download".to_owned(),
            Fix::Sync => "Get it from my other PC".to_owned(),
            Fix::UpdatePatchwork => "Update Patchwork".to_owned(),
            Fix::ProfileSettings => "Open profile settings".to_owned(),
            Fix::SourceSettings { .. } => "Open Settings".to_owned(),
            Fix::UseEverywhere { .. } => "Use in every profile".to_owned(),
            Fix::KeepHere { .. } => "Keep it only in this profile".to_owned(),
            Fix::GoBack { sets, .. } => match sets {
                SetsThen::Restore => "Put them back as they were",
                SetsThen::Copy => "Copy them in as they were",
                SetsThen::Keep => "Keep them as they are",
            }
            .to_owned(),
            Fix::ForgetSet { .. } => "Stop including it".to_owned(),
        }
    }

    /// What to say while it runs.
    pub fn doing(&self) -> String {
        match self {
            Fix::AcceptChanged { names, .. } => {
                format!("Using the new file for {}…", names.join(", "))
            }
            Fix::UpdateMods { names, .. } => format!("Updating {}…", names.join(", ")),
            Fix::RemoveMods { names } => format!("Removing {}…", names.join(", ")),
            Fix::Undeploy { .. } => "Undeploying…".to_owned(),
            Fix::Deploy => "Deploying…".to_owned(),
            Fix::SetGamePath { .. } | Fix::ChooseGamePath { .. } => {
                "Setting the game's folder…".to_owned()
            }
            Fix::Lock => "Downloading…".to_owned(),
            Fix::Sync => "Waiting for your other PC…".to_owned(),
            Fix::UpdatePatchwork => "Updating Patchwork…".to_owned(),
            Fix::ProfileSettings => "Opening profile settings…".to_owned(),
            Fix::SourceSettings { .. } => "Opening Settings…".to_owned(),
            Fix::UseEverywhere { name, .. } => format!("Saving {name} for every profile…"),
            Fix::KeepHere { name, .. } => format!("Keeping {name} only in this profile…"),
            Fix::GoBack { number: None, .. } => "Reverting…".to_owned(),
            Fix::GoBack { .. } => "Rolling back…".to_owned(),
            Fix::ForgetSet { .. } => "Removing the missing set…".to_owned(),
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
            Fix::UpdateMods { keys, .. } => {
                let updates: Vec<_> = keys.iter().map(|k| format!("--update-mod {k}")).collect();
                on(format!("lock {}", updates.join(" ")))
            }
            Fix::RemoveMods { names } => {
                let removes: Vec<_> = names
                    .iter()
                    .map(|n| on(format!("remove {}", quoted(n))))
                    .collect();
                removes.join(" && ")
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
            Fix::SourceSettings { source, .. } => {
                format!("patchwork config set {source}.api_key <key>")
            }
            Fix::UseEverywhere { name, .. } => on(format!("folder everywhere {}", quoted(name))),
            Fix::KeepHere { name, .. } => on(format!("folder keep {}", quoted(name))),
            Fix::GoBack { number, sets } => {
                let number = number.map(|n| format!(" {n}")).unwrap_or_default();
                on(format!("rollback{number} --sets {}", sets.as_str()))
            }
            Fix::ForgetSet { set } => on(format!("set unlink {set}")),
        }
    }
}

/// `arg` as one shell word.
fn quoted(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/+@".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
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
        let remove = Fix::RemoveMods {
            names: vec!["A".to_owned(), "It's B".to_owned()],
        };
        assert_eq!(
            remove.command(None),
            r"patchwork remove A && patchwork remove 'It'\''s B'"
        );
    }

    #[test]
    fn blockers_go_to_frontends_with_labelled_fixes() {
        let blocker = Blocker::new("t", "d", Fix::Lock);
        let value = serde_json::to_value(&blocker).unwrap();
        assert_eq!(value["fixes"][0]["type"], "lock");
        assert_eq!(value["fixes"][0]["label"], "Download");
        assert_eq!(serde_json::from_value::<Blocker>(value).unwrap(), blocker);
        let retry = serde_json::to_value(Blocker::try_again("t", "d")).unwrap();
        assert_eq!(retry["fixes"], serde_json::json!([]));
    }
}
