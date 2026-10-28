//! Problems with a profile that are worth showing the user. Every check
//! reports through [`Issue`], so a frontend can show them all in one place
//! without knowing about each kind.

use serde::Serialize;

use crate::compat::GameVersions;
use crate::deploy::{Dest, Drift};
use crate::environment::GameEnv;
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
        }
    }
}
