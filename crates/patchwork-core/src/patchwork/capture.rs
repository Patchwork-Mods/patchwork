//! Keeping what tools wrote into the game's folder as a local mod; see
//! [`crate::capture`].

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::local::LOCAL;
use super::{Patchwork, not_installed};
use crate::capture::{self, ToolOutput};
use crate::deploy::DeployState;
use crate::error::IoContext;
use crate::event::Event;
use crate::game::CaptureDef;
use crate::launch::{self, Launch};
use crate::lock::mod_key;
use crate::profile::{ProfileMod, ProfileRef};
use crate::share::sanitize_name;
use crate::source::ModSpec;
use crate::{Error, Result};

/// What [`Patchwork::run_tool`] did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRan {
    /// Whether the tool said it succeeded.
    pub success: bool,
    /// How it exited, as the system puts it.
    pub status: String,
    /// Where what it printed went.
    pub log: PathBuf,
    /// How many store files were read-only while it ran.
    pub protected: usize,
    /// What it wrote, captured; `None` if it failed, in which case what it
    /// wrote is left in the game's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured: Option<Captured>,
}

/// What [`Patchwork::capture`] did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Captured {
    /// The mod the files went into.
    pub name: String,
    /// Whether it was added to the profile, rather than already in it.
    pub added: bool,
    /// What was captured, which has been taken out of the game's folder to
    /// be deployed again from the mod.
    pub output: ToolOutput,
}

impl Patchwork {
    /// What tools have written into the game's folder since `profile` was
    /// deployed. Errors if the profile isn't the one deployed, or its game
    /// has nowhere to capture from.
    pub fn tool_output(&self, profile: &ProfileRef) -> Result<ToolOutput> {
        let (def, state) = self.capture_setup(profile)?;
        let baseline = state.baseline.as_ref().expect("checked by capture_setup");
        capture::find(&state.root, &def, &state, baseline)
    }

    /// Moves what tools have written into the game's folder since
    /// `profile` was deployed into the mod called `name`, adding it to the
    /// end of the profile if it isn't there yet. If it is, it has to be one
    /// that was captured, and the files are added to it, replacing its own
    /// where they're the same.
    ///
    /// The files are taken out of the game's folder, so the profile needs
    /// deploying again afterwards to put them back.
    pub async fn capture(&self, profile: &ProfileRef, name: &str) -> Result<Captured> {
        self.capture_since(profile, name, None).await
    }

    /// How to run the tool at `exe` for `profile`'s game; see
    /// [`launch::plan_tool`].
    pub fn plan_tool(&self, profile: &ProfileRef, exe: &Path, args: Vec<String>) -> Result<Launch> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let def = &registry.game(&loaded.game)?.value;
        let install = self
            .game_install(&loaded.game)?
            .ok_or_else(|| not_installed(def))?;
        let state = self.deploy_state(&self.deploy_target(&loaded)?)?;
        launch::plan_tool(def, &launch::Setup::find(def, &install, &state), exe, args)
    }

    /// Runs `tool` (from [`Patchwork::plan_tool`]) for `profile`, which has
    /// to be deployed, waits for it to finish, and if it succeeded captures
    /// what it wrote into the game's folder into the mod called `into`, as
    /// [`Patchwork::capture`] does. Only what changed while it ran is
    /// captured. Store files deployed as links are read-only while it runs;
    /// see [`capture::protect`].
    pub async fn run_tool(
        &self,
        profile: &ProfileRef,
        tool: &Launch,
        into: &str,
    ) -> Result<ToolRan> {
        let (def, state) = self.capture_setup(profile)?;
        // Before the tool runs, so a name that won't do doesn't waste it.
        self.capture_target(profile, into)?;
        let before = capture::snapshot(&state.root, &def)?;
        let log = self.paths.tool_log(&self.load_profile(profile)?.game);
        let (status, protected) = {
            let protected = capture::protect(&state.root, &def, &state)?;
            (tool.run(&log)?, protected.len())
        };
        let captured = if status.success() {
            Some(self.capture_since(profile, into, Some(&before)).await?)
        } else {
            None
        };
        Ok(ToolRan {
            success: status.success(),
            status: status.to_string(),
            log,
            protected,
            captured,
        })
    }

    /// Captures into the mod called `name`, as [`Patchwork::capture`] does,
    /// only what's changed since `before` if it's given.
    async fn capture_since(
        &self,
        profile: &ProfileRef,
        name: &str,
        before: Option<&capture::Snapshot>,
    ) -> Result<Captured> {
        let (def, state) = self.capture_setup(profile)?;
        let baseline = state.baseline.as_ref().expect("checked by capture_setup");
        let mut output = capture::find(&state.root, &def, &state, baseline)?;
        if let Some(before) = before {
            output = capture::since(output, &state.root, before);
        }
        let (dir, existing) = self.capture_target(profile, name)?;
        if output.files.is_empty() {
            return Ok(Captured {
                name: name.to_owned(),
                added: false,
                output,
            });
        }

        for file in &output.files {
            let from = file.path.under(&state.root);
            let to = file.path.under(&dir);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).at(parent)?;
            }
            // Over a link, so the file it points to is what's copied.
            if to.symlink_metadata().is_ok() {
                fs::remove_file(&to).at(&to)?;
            }
            fs::copy(&from, &to).at(&from)?;
        }

        let added = match existing {
            Some(m) => {
                self.lock_updating(profile, &[mod_key(&m.source, &m.id)])
                    .await?;
                false
            }
            None => {
                let spec = ModSpec {
                    source: LOCAL.to_owned(),
                    id: dir.to_string_lossy().into_owned(),
                    version: None,
                };
                self.add_mod(profile, spec, Some(name.to_owned()), true)
                    .await?;
                true
            }
        };

        // Only now that the mod has them, so nothing's lost if that failed.
        for file in &output.files {
            let path = file.path.under(&state.root);
            if let Err(e) = fs::remove_file(&path) {
                self.reporter.report(Event::Warning(format!(
                    "couldn't take {} out of the game's folder: {e}",
                    file.path
                )));
            }
        }
        Ok(Captured {
            name: name.to_owned(),
            added,
            output,
        })
    }

    /// Where files captured into the mod called `name` go, and that mod if
    /// `profile` has it already. Errors if it has a mod by that name that
    /// wasn't captured.
    fn capture_target(
        &self,
        profile: &ProfileRef,
        name: &str,
    ) -> Result<(PathBuf, Option<ProfileMod>)> {
        let loaded = self.load_profile(profile)?;
        let dir = self.captured_dir(profile, name);
        let existing = loaded.mods.iter().find(|m| m.name == name).cloned();
        if let Some(m) = &existing
            && !self.is_captured(m, &dir)?
        {
            return Err(Error::invalid(format!(
                "{} already has a mod named `{name}` that wasn't captured; pick another name",
                loaded.label()
            )));
        }
        Ok((dir, existing))
    }

    /// The game's capture folders and the deploy state of `profile`, which
    /// has to be the one deployed and have a baseline.
    fn capture_setup(&self, profile: &ProfileRef) -> Result<(CaptureDef, DeployState)> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let Some(def) = game.capture.clone() else {
            return Err(Error::invalid(format!(
                "patchwork doesn't know where tools write files for {}",
                game.name
            )));
        };
        let state = self.deploy_state(&self.deploy_target(&loaded)?)?;
        if state.profile.as_deref() != Some(&profile.to_string()) {
            return Err(Error::invalid(format!(
                "{} isn't deployed; deploy it, run the tool, then capture what it made",
                loaded.label()
            )));
        }
        if state.baseline.is_none() {
            return Err(Error::invalid(format!(
                "patchwork doesn't know yet what was in the game's folder before; deploy {} again, then run the tool",
                loaded.label()
            )));
        }
        Ok((def, state))
    }

    /// Where the files of the mod called `name` captured into `profile`
    /// are kept.
    fn captured_dir(&self, profile: &ProfileRef, name: &str) -> PathBuf {
        self.paths
            .captured()
            .join(profile.to_string())
            .join(sanitize_name(name))
    }

    /// Whether `m` is a local mod added from `dir`.
    fn is_captured(&self, m: &ProfileMod, dir: &Path) -> Result<bool> {
        if m.source != LOCAL {
            return Ok(false);
        }
        let origin = self.local_origin(&m.id)?;
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
        Ok(origin.is_some_and(|o| o == dir))
    }

    /// What's in the game's capture folders after deploying as `state`,
    /// out of what was in `previous`; see [`capture::baseline`]. `None` if
    /// the game has none.
    pub(super) fn capture_baseline(
        &self,
        game: &str,
        state: &DeployState,
        previous: Option<&capture::Baseline>,
    ) -> Result<Option<capture::Baseline>> {
        let registry = self.registry();
        let Some(def) = &registry.game(game)?.value.capture else {
            return Ok(None);
        };
        capture::baseline(&state.root, def, state, previous).map(Some)
    }
}
