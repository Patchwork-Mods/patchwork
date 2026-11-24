//! Folders: groups of a profile's mods (see [`Profile::folders`]).

use uuid::Uuid;

use super::Patchwork;
use crate::profile::{Folder, Profile, ProfileRef, no_such_mod};
use crate::{Error, Result};

impl Patchwork {
    /// Adds a folder called `name` and puts `mods` in it, where the first
    /// of them is. With no mods it's empty, and shown after the rest until
    /// a mod goes in.
    pub fn add_folder(&self, profile: &ProfileRef, name: &str, mods: &[String]) -> Result<Folder> {
        let mut loaded = self.load_profile(profile)?;
        let name = folder_name(&loaded, name, None)?;
        let folder = Folder::new(&name);
        for name in mods {
            let label = loaded.label();
            loaded
                .find_mut(name)
                .ok_or_else(|| no_such_mod(&label, name))?
                .folder = Some(folder.id);
        }
        loaded.folders.push(folder.clone());
        self.save_profile(profile, &loaded)?;
        Ok(folder)
    }

    /// Renames the folder `folder` names (see [`Profile::find_folder`]).
    /// A linked folder's set is renamed, for every profile that includes it.
    pub fn rename_folder(&self, profile: &ProfileRef, folder: &str, name: &str) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let id = loaded.find_folder(folder)?.id;
        if id == loaded.id {
            return Err(personal_folder(&loaded));
        }
        let name = folder_name(&loaded, name, Some(id))?;
        if loaded.include(id).is_some() {
            return self.rename_set(id, &name);
        }
        let f = loaded.folders.iter_mut().find(|f| f.id == id);
        f.expect("just found").name = name;
        self.save_profile(profile, &loaded)
    }

    /// Removes a folder. Its mods stay where they are, in no folder.
    pub fn remove_folder(&self, profile: &ProfileRef, folder: &str) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let found = loaded.find_folder(folder)?;
        let id = found.id;
        if id == loaded.id {
            return Err(personal_folder(&loaded));
        }
        if loaded.include(id).is_some() {
            return Err(Error::invalid(format!(
                "{} is a shared set; unlink it instead",
                found.name
            )));
        }
        loaded.folders.retain(|f| f.id != id);
        for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(id)) {
            m.folder = None;
        }
        self.save_profile(profile, &loaded)
    }

    /// Puts mod `name` in `folder`, at its end, or with `None` takes it
    /// out of the folder it's in, to just after it.
    pub fn set_mod_folder(
        &self,
        profile: &ProfileRef,
        name: &str,
        folder: Option<&str>,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let id = folder
            .map(|f| loaded.find_folder(f))
            .transpose()?
            .map(|f| f.id);
        if move_to_folder(&mut loaded, name, id)? {
            self.save_profile(profile, &loaded)?;
        }
        Ok(())
    }

    /// Turns all of a folder's mods on or off.
    pub fn set_folder_enabled(
        &self,
        profile: &ProfileRef,
        folder: &str,
        enabled: bool,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let id = loaded.find_folder(folder)?.id;
        for m in loaded.mods.iter_mut().filter(|m| m.folder == Some(id)) {
            m.enabled = enabled;
        }
        self.save_profile(profile, &loaded)
    }
}

/// The personal layer's folder (see [`crate::personal`]) isn't one to
/// rename or remove: it's there while it has mods.
fn personal_folder(loaded: &Profile) -> Error {
    Error::invalid(format!(
        "{}'s personal mods are always in {}; move them out to share them",
        loaded.label(),
        crate::personal::PERSONAL_NAME
    ))
}

/// Puts mod `name` in folder `id`, at its end, or with `None` takes it out
/// of the folder it's in, to just after it. Returns whether that changed
/// anything.
pub(super) fn move_to_folder(loaded: &mut Profile, name: &str, id: Option<Uuid>) -> Result<bool> {
    let from = loaded
        .mods
        .iter()
        .position(|m| m.name == name)
        .ok_or_else(|| no_such_mod(&loaded.label(), name))?;
    if loaded.mods[from].folder == id {
        return Ok(false);
    }
    let mut moving = loaded.mods.remove(from);
    // after the last mod of the folder it's going into, or taken out,
    // of the one it was in; where it was if there's none
    let after = id.or(moving.folder);
    moving.folder = id;
    let to = after
        .and_then(|f| loaded.mods.iter().rposition(|m| m.folder == Some(f)))
        .map_or(from, |last| last + 1);
    loaded.mods.insert(to, moving);
    Ok(true)
}

/// `name` trimmed, if it isn't blank and no folder of `profile` but
/// `except` has it (ignoring case).
fn folder_name(profile: &Profile, name: &str, except: Option<Uuid>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("a folder needs a name"));
    }
    let taken = profile
        .all_folders()
        .any(|f| Some(f.id) != except && f.name.eq_ignore_ascii_case(name));
    if taken {
        return Err(Error::invalid(format!(
            "{} already has a folder named `{name}`",
            profile.label()
        )));
    }
    Ok(name.to_owned())
}
