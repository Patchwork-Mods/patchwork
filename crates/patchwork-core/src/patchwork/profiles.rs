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
use crate::lock::Lockfile;
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
        let profile = Profile::new(game, name);
        self.check_new_name(&profile)?;
        let profile_ref = profile.reference();
        self.save_profile(&profile_ref, &profile)?;
        Ok(profile_ref)
    }

    /// Renames a profile within its game. Everything else refers to it by
    /// id, so nothing else has to change.
    pub fn rename_profile(&self, profile: &ProfileRef, name: &str) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        if loaded.name == name {
            return Ok(());
        }
        loaded.name = name.to_owned();
        self.check_new_name(&loaded)?;
        self.save_profile(profile, &loaded)
    }

    /// Copies a profile to `<game>/<name>` with a new id: its mods, its
    /// lockfile and whether it uses an instance, but not what's in it. The copy hasn't been deployed, so it has nothing to revert
    /// to.
    pub fn duplicate_profile(&self, profile: &ProfileRef, name: &str) -> Result<ProfileRef> {
        let mut copy = self.load_profile(profile)?;
        copy.id = Uuid::new_v4();
        copy.name = name.to_owned();
        self.check_new_name(&copy)?;
        let copy_ref = copy.reference();
        let lock = self.load_lock(profile)?;
        self.save_profile(&copy_ref, &copy)?;
        if let Some(lock) = lock {
            self.save_lock(&copy_ref, &lock)?;
        }
        self.copy_local_settings(profile, &copy_ref)?;
        Ok(copy_ref)
    }

    /// Fails unless `profile`'s name is valid and no other profile for its
    /// game already has it.
    pub(super) fn check_new_name(&self, profile: &Profile) -> Result<()> {
        validate_name(&profile.name)?;
        if self
            .profiles()?
            .iter()
            .any(|(r, p)| r.uuid != profile.id && p.label() == profile.label())
        {
            return Err(Error::invalid(format!(
                "profile {} already exists",
                profile.label()
            )));
        }
        Ok(())
    }

    /// Removes the profile and everything kept with it: its lockfile and
    /// what was recorded when it was deployed.
    /// Its mods have to be undeployed first, so they aren't left in the game
    /// with nothing keeping track of them. If it was the current profile,
    /// there's no current profile afterwards.
    pub fn remove_profile(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let deployed = self.deploy_state(&target)?.profile;
        if deployed.as_deref() == Some(profile.to_string().as_str()) {
            return Err(Error::invalid(format!(
                "{} is deployed; run `{}` first",
                loaded.label(),
                self.undeploy_command(&target)?
            )));
        }
        self.remove_instance(profile, &loaded.game)?;
        let dir = self.profile_dir(profile);
        fs::remove_dir_all(&dir).at(&dir)?;
        let state = self.profile_state_dir(profile);
        if state.exists() {
            fs::remove_dir_all(&state).at(&state)?;
        }
        let mut state: State = fsutil::read_toml(&self.paths.state_file())?.unwrap_or_default();
        if state.current_profile.as_deref() == Some(profile.to_string().as_str()) {
            state.current_profile = None;
            fsutil::write_toml(&self.paths.state_file(), &state)?;
        }
        // so that it stops being synced, rather than coming back
        self.profile_changed(profile, false);
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

    /// Moves what only this machine knows about each profile out of the
    /// profile's folder, and gives local mods ids that work anywhere; both
    /// came about so profiles could be kept in step between machines.
    pub(super) fn migrate_profile_state(&self) -> Result<()> {
        let Ok(entries) = fs::read_dir(self.paths.profiles()) else {
            return Ok(());
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(profile) = entry.file_name().to_string_lossy().parse::<ProfileRef>() else {
                continue;
            };
            if !self.profile_file(&profile).is_file() {
                continue;
            }
            let old_dir = self.profile_dir(&profile);
            let new_dir = self.profile_state_dir(&profile);
            let mut moved = false;
            for name in ["deployed.toml", "history"] {
                let old = old_dir.join(name);
                if old.exists() {
                    let new = new_dir.join(name);
                    fs::create_dir_all(&new_dir).at(&new_dir)?;
                    fs::rename(&old, &new).at(&old)?;
                    moved = true;
                }
            }
            if moved {
                self.migrate_history_local_mods(&profile)?;
            }
            self.migrate_local_mods(&profile)?;
        }
        Ok(())
    }

    pub fn load_profile(&self, profile: &ProfileRef) -> Result<Profile> {
        let path = self.profile_file(profile);
        let mut loaded: Profile = fsutil::read_toml(&path)?
            .ok_or_else(|| Error::invalid(format!("no profile with id {profile}")))?;
        if loaded.schema > PROFILE_SCHEMA {
            return Err(Error::Schema {
                kind: "profile",
                path,
                found: loaded.schema,
                supported: PROFILE_SCHEMA,
            });
        }
        // Whatever an older schema had has been moved where it is now, so
        // it's saved as the current one, which older builds won't open.
        loaded.schema = PROFILE_SCHEMA;
        Ok(loaded)
    }

    pub(super) fn save_profile(&self, profile: &ProfileRef, value: &Profile) -> Result<()> {
        fsutil::write_toml(&self.profile_file(profile), value)?;
        self.profile_changed(profile, false);
        Ok(())
    }

    pub(super) fn save_lock(&self, profile: &ProfileRef, lock: &Lockfile) -> Result<()> {
        fsutil::write_toml(&self.lock_file(profile), lock)?;
        self.profile_changed(profile, false);
        Ok(())
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

    /// Where what only this machine knows about the profile is kept.
    pub(super) fn profile_state_dir(&self, profile: &ProfileRef) -> PathBuf {
        self.paths.profile_state().join(profile.uuid.to_string())
    }

    /// The profile's mods and lockfile as they were at its last deploy, for
    /// [`Patchwork::revert`].
    pub(super) fn deployed_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("deployed.toml")
    }
}
