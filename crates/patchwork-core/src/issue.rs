//! Problems with a profile that are worth showing the user. Every check
//! reports through [`Issue`], so a frontend can show them all in one place
//! without knowing about each kind.

use serde::Serialize;

use crate::check::Problem;
use crate::compat::GameVersions;
use crate::deploy::{Dest, Drift};
use crate::environment::GameEnv;
use crate::fix::Fix;
use crate::lock::UnmetRequirement;
use crate::source::VersionReq;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub severity: Severity,
    /// The profile's mods it's about, by name. Empty for issues with the
    /// whole profile.
    pub mods: Vec<String>,
    /// Says what's wrong in a sentence, for frontends that don't know the
    /// kind.
    pub message: String,
    pub kind: IssueKind,
    /// What deals with it, if something does. Goes to frontends with its
    /// `label` and `doing` alongside, for a button.
    #[serde(
        serialize_with = "crate::fix::labelled",
        skip_serializing_if = "Option::is_none"
    )]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Deploying still works, but something might not.
    Warning,
    /// Deploying won't work until it's dealt with.
    Error,
}

/// The details of an [`Issue`], for frontends that want to do more than
/// show its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum IssueKind {
    /// See [`UnmetRequirement`].
    #[serde(rename_all = "camelCase")]
    UnmetRequirement {
        dependency: String,
        needs: VersionReq,
        found: String,
        pinned: bool,
    },
    /// The game install isn't what it was when the profile was deployed,
    /// most likely because the game updated.
    GameChanged { from: GameEnv, to: GameEnv },
    /// Files patchwork deployed aren't there any more, or something else is
    /// in their place, most likely because the game updated. Deploying
    /// puts them back.
    FilesChanged { missing: usize, changed: usize },
    /// A mod says it doesn't work with the installed version of the game.
    IncompatibleGame {
        version: String,
        supports: GameVersions,
    },
    /// A mod says it doesn't work with what the profile picks for one of
    /// the game's `[env]` axes, such as its mod loader.
    IncompatibleEnv {
        axis: String,
        value: String,
        supports: Vec<String>,
    },
    /// The game turns on an extension (see [`crate::ext`]) this build of
    /// patchwork doesn't have, so whatever it adds is missing.
    MissingExtension { id: String },
    /// The game's `[ext.<extension>]` turns off something the extension
    /// doesn't have, most likely a typo.
    UnknownHook { ext: String, name: String },
    /// Something wrong with one of the game's ordered lists (see
    /// [`crate::list`]) as the profile has it, like an item that comes
    /// before one it needs.
    List { id: String },
    /// A locked mod downloads as a different file now, at the same version;
    /// see [`crate::Error::ArtifactChanged`]. Until the new file's taken,
    /// it can't be deployed.
    ArtifactChanged { key: String },
    /// Local mods added on another machine whose files haven't been synced
    /// here yet.
    OnAnotherPc,
    /// What an extension's [`Check`](crate::check::Check) found.
    Check {
        /// The check's id, `<extension>.<check>`.
        id: String,
        /// The id of the list it's about, if it's about one.
        list: Option<String>,
    },
}

impl From<UnmetRequirement> for Issue {
    fn from(unmet: UnmetRequirement) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: vec![unmet.name.clone()],
            message: unmet.to_string(),
            kind: IssueKind::UnmetRequirement {
                dependency: unmet.dependency,
                needs: unmet.req,
                found: unmet.found,
                pinned: unmet.pinned,
            },
            fix: None,
        }
    }
}

impl Issue {
    pub fn incompatible_game(
        name: &str,
        game: &str,
        version: &str,
        supports: GameVersions,
    ) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: vec![name.to_owned()],
            message: format!(
                "`{name}` is for {game} {supports}, but {version} is installed; \
                 look for a version of it that works with {version}"
            ),
            kind: IssueKind::IncompatibleGame {
                version: version.to_owned(),
                supports,
            },
            fix: None,
        }
    }

    /// `axis_name` is what the game definition calls `axis`.
    pub fn incompatible_env(
        name: &str,
        axis: &str,
        axis_name: &str,
        value: &str,
        supports: &[String],
    ) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: vec![name.to_owned()],
            message: format!(
                "`{name}` is for {axis_name} {}, but the profile uses {value}; \
                 look for a version of it that works with {value}",
                supports.join(" or ")
            ),
            kind: IssueKind::IncompatibleEnv {
                axis: axis.to_owned(),
                value: value.to_owned(),
                supports: supports.to_vec(),
            },
            fix: None,
        }
    }

    pub fn missing_extension(game: &str, id: &str) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: Vec::new(),
            message: format!(
                "{game} uses the `{id}` extension, which this build of patchwork doesn't \
                 include, so some of what it does for {game} is missing"
            ),
            kind: IssueKind::MissingExtension { id: id.to_owned() },
            fix: Some(Fix::UpdatePatchwork),
        }
    }

    pub fn unknown_hook(game: &str, ext: &str, name: &str) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: Vec::new(),
            message: format!(
                "{game} turns off `{name}` of the `{ext}` extension, which doesn't have one; \
                 check the `off` list in its `[ext.{ext}]`"
            ),
            kind: IssueKind::UnknownHook {
                ext: ext.to_owned(),
                name: name.to_owned(),
            },
            fix: None,
        }
    }

    /// A problem with list `id`.
    pub fn list(id: &str, problem: Problem) -> Self {
        Issue {
            severity: problem.severity,
            mods: problem.mods,
            message: problem.message,
            kind: IssueKind::List { id: id.to_owned() },
            fix: None,
        }
    }

    /// A problem check `id` of extension `ext` found.
    pub fn check(ext: &str, id: &str, problem: Problem) -> Self {
        Issue {
            severity: problem.severity,
            mods: problem.mods,
            message: problem.message,
            kind: IssueKind::Check {
                id: id.to_owned(),
                list: problem.list.map(|name| crate::ext::hook_id(ext, &name)),
            },
            fix: None,
        }
    }

    pub fn artifact_changed(changed: &crate::error::ArtifactChanged) -> Self {
        let name = &changed.name;
        Issue {
            severity: Severity::Error,
            mods: vec![name.clone()],
            message: format!(
                "`{name}` downloads as a different file now than the one it was locked with, \
                 most likely because it was repackaged; use the new file to deploy it"
            ),
            kind: IssueKind::ArtifactChanged {
                key: changed.key.clone(),
            },
            fix: Some(Fix::AcceptChanged {
                keys: vec![changed.key.clone()],
                names: vec![name.clone()],
            }),
        }
    }

    /// `names` are local mods whose files are on another machine.
    pub fn on_another_pc(names: Vec<String>) -> Self {
        let message = match names.as_slice() {
            [name] => format!(
                "`{name}` was added from a file on another PC that hasn't been copied here yet"
            ),
            _ => format!(
                "{} mods were added from files on another PC that haven't been copied here yet",
                names.len()
            ),
        };
        Issue {
            severity: Severity::Error,
            mods: names,
            message,
            kind: IssueKind::OnAnotherPc,
            fix: Some(Fix::Sync),
        }
    }

    pub fn game_changed(game: &str, from: GameEnv, to: GameEnv) -> Self {
        Issue {
            severity: Severity::Warning,
            mods: Vec::new(),
            message: format!(
                "{game} has changed since this profile was deployed ({from} -> {to}); \
                 deploy again, and check that its mods work with this version"
            ),
            kind: IssueKind::GameChanged { from, to },
            fix: Some(Fix::Deploy),
        }
    }

    pub fn files_changed(drifted: &[(Dest, Drift)]) -> Self {
        let missing = drifted.iter().filter(|(_, d)| *d == Drift::Missing).count();
        let changed = drifted.len() - missing;
        let files = |n: usize| {
            if n == 1 {
                "1 file".to_owned()
            } else {
                format!("{n} files")
            }
        };
        let what = match (missing, changed) {
            (0, n) => format!(
                "{} patchwork deployed {} been replaced",
                files(n),
                if n == 1 { "has" } else { "have" }
            ),
            (n, 0) => format!(
                "{} patchwork deployed {} missing",
                files(n),
                if n == 1 { "is" } else { "are" }
            ),
            (m, c) => format!(
                "{} patchwork deployed {} missing and {c} replaced",
                files(m),
                if m == 1 { "is" } else { "are" }
            ),
        };
        Issue {
            severity: Severity::Warning,
            mods: Vec::new(),
            message: format!(
                "{what}, most likely by a game update or a check of the game's files; \
                 deploy again to put them back"
            ),
            kind: IssueKind::FilesChanged { missing, changed },
            fix: Some(Fix::Deploy),
        }
    }
}
