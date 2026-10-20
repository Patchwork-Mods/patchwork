//! Creating, finding, loading and removing profiles, and where their files
//! live.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Patchwork;
use crate::deploy::DeployTarget;
use crate::error::IoContext;
use crate::fsutil;
use crate::profile::{PROFILE_SCHEMA, Profile, ProfileRef, validate_name};
use crate::{Error, Result};

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_profile: Option<String>,
}

impl Patchwork {
    pub fn current_profile(&self) -> Result<Option<ProfileRef>> {
        let state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        state.current_profile.map(|p| p.parse()).transpose()
    }

    pub fn use_profile(&self, profile: &ProfileRef) -> Result<()> {
        self.load_profile(profile)?;
        let state = State {
            current_profile: Some(profile.to_string()),
        };
        fsutil::write_toml(&self.paths.state_file(), &state)
    }

    /// Creates `<game>/<name>`, which must not already exist.
    pub fn create_profile(&self, game: &str, name: &str) -> Result<ProfileRef> {
        self.registry().game(game)?;
        validate_name(name)?;
        let profile = Profile::new(game, name);
        if self
            .profiles()?
            .iter()
            .any(|(_, p)| p.label() == profile.label())
        {
            return Err(Error::invalid(format!(
                "profile {} already exists",
                profile.label()
            )));
        }
        let profile_ref = profile.reference();
        fsutil::write_toml(&self.profile_file(&profile_ref), &profile)?;
        Ok(profile_ref)
    }

    /// Removes the profile and everything kept with it: its lockfile, what
    /// was recorded at its last deploy, and local mods imported with it.
    /// Its mods have to be undeployed first, so they aren't left in the game
    /// with nothing keeping track of them. If it was the current profile,
    /// there's no current profile afterwards.
    pub fn remove_profile(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let deployed = self.deploy_state(&target)?.profile;
        if deployed.as_deref() == Some(profile.to_string().as_str()) {
            return Err(Error::invalid(format!(
                "{} is deployed; run `patchwork undeploy {}` first",
                loaded.label(),
                loaded.game
            )));
        }
        let dir = self.profile_dir(profile);
        fs::remove_dir_all(&dir).at(&dir)?;
        let mut state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        if state.current_profile.as_deref() == Some(profile.to_string().as_str()) {
            state.current_profile = None;
            fsutil::write_toml(&self.paths.state_file(), &state)?;
        }
        Ok(())
    }

    /// Every profile on this machine, sorted by game then name.
    pub fn profiles(&self) -> Result<Vec<(ProfileRef, Profile)>> {
        let mut profiles = Vec::new();
        let Ok(entries) = fs::read_dir(self.paths.profiles()) else {
            return Ok(profiles);
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(profile_ref) = entry.file_name().to_string_lossy().parse::<ProfileRef>() else {
                continue;
            };
            if self.profile_file(&profile_ref).is_file() {
                let loaded = self.load_profile(&profile_ref)?;
                profiles.push((profile_ref, loaded));
            }
        }
        profiles.sort_by(|(_, a), (_, b)| (&a.game, &a.name).cmp(&(&b.game, &b.name)));
        Ok(profiles)
    }

    /// Looks up a profile by `<game>/<name>` or by its id.
    pub fn find_profile(&self, query: &str) -> Result<ProfileRef> {
        if let Ok(profile_ref) = query.parse::<ProfileRef>()
            && self.profile_file(&profile_ref).is_file()
        {
            return Ok(profile_ref);
        }
        self.profiles()?
            .into_iter()
            .find(|(_, p)| p.label() == query)
            .map(|(r, _)| r)
            .ok_or_else(|| Error::invalid(format!("no profile named {query}")))
    }

    /// Moves profiles from `profiles/<game>/<name>/` (schema 1) to
    /// `profiles/<id>/`, giving each one an id.
    pub(super) fn migrate_profiles(&self) -> Result<()> {
        let Ok(games) = fs::read_dir(self.paths.profiles()) else {
            return Ok(());
        };
        let mut state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        let mut state_changed = false;
        for game in games.filter_map(|e| e.ok()) {
            let game_dir = game.path();
            let game_id = game.file_name().to_string_lossy().into_owned();
            if game_id.parse::<ProfileRef>().is_ok() || !game_dir.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&game_dir)
                .at(&game_dir)?
                .filter_map(|e| e.ok())
            {
                let old_dir = entry.path();
                let old_file = old_dir.join("profile.toml");
                let Some(mut table) = fsutil::read_toml::<toml::Table>(&old_file)? else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = Uuid::new_v4();
                table.insert("schema".into(), (PROFILE_SCHEMA as i64).into());
                table.insert("id".into(), id.to_string().into());
                table.insert("name".into(), name.clone().into());
                let profile_ref = ProfileRef { uuid: id };
                let new_dir = self.profile_dir(&profile_ref);
                fs::rename(&old_dir, &new_dir).at(&old_dir)?;
                fsutil::write_toml(&self.profile_file(&profile_ref), &table)?;

                let label = format!("{game_id}/{name}");
                if state.current_profile.as_deref() == Some(label.as_str()) {
                    state.current_profile = Some(profile_ref.to_string());
                    state_changed = true;
                }
                // Profiles only shared their game's target back then.
                let target = DeployTarget::Game(game_id.clone());
                let mut deployed = self.deploy_state(&target)?;
                if deployed.profile.as_deref() == Some(label.as_str()) {
                    deployed.profile = Some(profile_ref.to_string());
                    fsutil::write_json(&self.deploy_state_file(&target), &deployed)?;
                }
            }
            // Only goes if it's now empty.
            let _ = fs::remove_dir(&game_dir);
        }
        if state_changed {
            fsutil::write_toml(&self.paths.state_file(), &state)?;
        }
        Ok(())
    }

    pub fn load_profile(&self, profile: &ProfileRef) -> Result<Profile> {
        let path = self.profile_file(profile);
        let loaded: Profile = fsutil::read_toml(&path)?
            .ok_or_else(|| Error::invalid(format!("no profile with id {profile}")))?;
        if loaded.schema > PROFILE_SCHEMA {
            return Err(Error::Schema {
                kind: "profile",
                path,
                found: loaded.schema,
                supported: PROFILE_SCHEMA,
            });
        }
        Ok(loaded)
    }

    pub(super) fn save_profile(&self, profile: &ProfileRef, value: &Profile) -> Result<()> {
        fsutil::write_toml(&self.profile_file(profile), value)
    }

    pub(super) fn profile_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.paths.profiles().join(profile.uuid.to_string())
    }

    pub(super) fn profile_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("profile.toml")
    }

    pub(super) fn lock_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("profile.lock")
    }

    /// The profile's mods and lockfile as they were at its last deploy, for
    /// [`Patchwork::revert`].
    pub(super) fn deployed_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_dir(profile).join("deployed.toml")
    }
}
