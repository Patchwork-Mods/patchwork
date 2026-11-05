//! Instances: folders of a profile's own that it's deployed to instead of
//! the game's folder, for games whose definition has
//! [`InstancesDef`](crate::game::InstancesDef). Whether a profile uses one
//! is up to each machine, so it's kept with the profile's local state
//! rather than in the profile.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Patchwork;
use crate::deploy::DeployTarget;
use crate::error::IoContext;
use crate::event::Event;
use crate::fsutil;
use crate::game::LaunchDefs;
use crate::launch::{Launch, LaunchOption};
use crate::profile::{Profile, ProfileRef};
use crate::{Error, Result};

/// What a profile is set to on this machine only.
#[derive(Debug, Default, Serialize, Deserialize)]
struct LocalSettings {
    /// Whether it uses its instance; the game's default if `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance: Option<bool>,
}

impl Patchwork {
    /// Whether `profile` is deployed to its own instance rather than the
    /// game's folder: what it's been set to here, or else what the game's
    /// definition says. Never for games without instances.
    pub fn uses_instance(&self, profile: &Profile) -> Result<bool> {
        let registry = self.registry();
        let Some(def) = &registry.game(&profile.game)?.value.instances else {
            return Ok(false);
        };
        let set = self.local_settings(&profile.reference())?.instance;
        Ok(set.unwrap_or(def.default))
    }

    /// Sets whether `profile` uses its own instance on this machine, or
    /// with `None` goes back to the game's default. Whatever's deployed
    /// from it has to be undeployed first, so it isn't left behind with
    /// nothing keeping track of it.
    pub fn set_instance(&self, profile: &ProfileRef, instance: Option<bool>) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        if instance == Some(true) && game.instances.is_none() {
            return Err(Error::invalid(format!(
                "{} profiles can't have instances of their own",
                game.name
            )));
        }
        let was = self.deploy_target(&loaded)?;
        let mut settings = self.local_settings(profile)?;
        settings.instance = instance;
        let now = match instance.or(game.instances.as_ref().map(|d| d.default)) {
            Some(true) => DeployTarget::Instance {
                game: loaded.game.clone(),
                profile: profile.clone(),
            },
            _ => DeployTarget::Game(loaded.game.clone()),
        };
        if was != now && self.is_deployed_to(profile, &was)? {
            return Err(Error::invalid(format!(
                "{} is deployed; run `{}` first",
                loaded.label(),
                self.undeploy_command(&was)?
            )));
        }
        fsutil::write_toml(&self.local_settings_file(profile), &settings)
    }

    /// The ways `profile`'s game can be started, the default one marked:
    /// as for the game (see [`Patchwork::launch_options`]) unless it uses
    /// an instance, which patchwork can't start games in. `None` if the
    /// game hasn't been set up.
    pub fn profile_launch_options(&self, profile: &Profile) -> Result<Option<Vec<LaunchOption>>> {
        if !self.uses_instance(profile)? {
            return self.launch_options(&profile.game);
        }
        let registry = self.registry();
        let game = &registry.game(&profile.game)?.value;
        Ok(Some(vec![LaunchOption {
            id: LaunchDefs::GAME.to_owned(),
            name: game.name.clone(),
            default: true,
            unavailable: Some("patchwork can't start it in an instance of its own".to_owned()),
        }]))
    }

    /// How to start `profile`'s game, as [`Patchwork::plan_launch`] does,
    /// unless it uses an instance. Deploying first is up to the caller.
    pub fn plan_profile_launch(
        &self,
        profile: &ProfileRef,
        option: Option<&str>,
        runner: Option<&str>,
    ) -> Result<Launch> {
        let loaded = self.load_profile(profile)?;
        if !self.uses_instance(&loaded)? {
            return self.plan_launch(&loaded.game, option, runner);
        }
        let registry = self.registry();
        Err(Error::invalid(format!(
            "patchwork can't start {} in an instance of its own; start it from the launcher the instance is for, or run `patchwork --profile {} instance off` to deploy it to the game's folder",
            registry.game(&loaded.game)?.value.name,
            loaded.label()
        )))
    }

    /// The folder `profile`'s instance is in. It's only made when
    /// something's first deployed to it.
    pub fn instance_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.paths.instances().join(profile.to_string())
    }

    /// The game's own target and every instance of its profiles that's
    /// had something deployed to it.
    pub(super) fn targets_of(&self, game: &str) -> Result<Vec<DeployTarget>> {
        let mut targets = vec![DeployTarget::Game(game.to_owned())];
        for (profile, loaded) in self.profiles()? {
            let target = DeployTarget::Instance {
                game: loaded.game,
                profile,
            };
            if target.game() == game && self.deploy_state_file(&target).exists() {
                targets.push(target);
            }
        }
        Ok(targets)
    }

    /// The command that undeploys everything from `target`.
    pub(super) fn undeploy_command(&self, target: &DeployTarget) -> Result<String> {
        Ok(match target {
            DeployTarget::Game(game) => format!("patchwork undeploy {game}"),
            DeployTarget::Instance { profile, .. } => format!(
                "patchwork --profile {} undeploy",
                self.load_profile(profile)?.label()
            ),
        })
    }

    /// Whether `target` has anything of `profile`'s that patchwork would
    /// lose track of if the profile stopped being deployed there.
    fn is_deployed_to(&self, profile: &ProfileRef, target: &DeployTarget) -> Result<bool> {
        let state = self.deploy_state(target)?;
        let placed = !state.files.is_empty() || !state.created_dirs.is_empty();
        Ok(placed && state.profile.as_deref() == Some(profile.to_string().as_str()))
    }

    /// Forgets `profile`'s instance as a target, once nothing's deployed
    /// to it. Its folder goes too if that's empty; anything else in it
    /// (the game's saves, say) is the user's, so it's left be.
    pub(super) fn remove_instance(&self, profile: &ProfileRef, game: &str) -> Result<()> {
        let target = DeployTarget::Instance {
            game: game.to_owned(),
            profile: profile.clone(),
        };
        let state = self.deploy_state_file(&target);
        if state.exists() {
            fs::remove_file(&state).at(&state)?;
        }
        for dir in [self.paths.backups(), self.paths.generated()] {
            let dir = dir.join(target.key());
            if dir.exists() {
                fs::remove_dir_all(&dir).at(&dir)?;
            }
        }
        let dir = self.instance_dir(profile);
        if dir.exists() && fs::remove_dir(&dir).is_err() {
            self.reporter.report(Event::Warning(format!(
                "left {} where it is, since there's still something in it",
                dir.display()
            )));
        }
        Ok(())
    }

    /// Gives `to` what `from` is set to on this machine, for a copy.
    pub(super) fn copy_local_settings(&self, from: &ProfileRef, to: &ProfileRef) -> Result<()> {
        let settings = self.local_settings(from)?;
        if settings.instance.is_none() {
            return Ok(());
        }
        fsutil::write_toml(&self.local_settings_file(to), &settings)
    }

    fn local_settings(&self, profile: &ProfileRef) -> Result<LocalSettings> {
        Ok(fsutil::read_toml(&self.local_settings_file(profile))?.unwrap_or_default())
    }

    fn local_settings_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("settings.toml")
    }
}
