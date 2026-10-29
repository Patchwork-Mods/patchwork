//! Which extensions are on for a game, and the state they keep in profiles;
//! see [`crate::ext`].

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Patchwork;
use crate::ext::{self, ExtTables, Extension, StateOf};
use crate::game::GameDef;
use crate::profile::{Profile, ProfileRef, no_such_mod};
use crate::{Error, Result};

impl Patchwork {
    /// The ids of the extensions this build has, in order.
    pub fn extensions(&self) -> impl Iterator<Item = &str> {
        self.extensions.keys().map(String::as_str)
    }

    /// The extensions `def` turns on that this build has, in id order.
    /// Extensions' hooks are only ever consulted for games they're on for.
    pub fn extensions_on<'a>(&'a self, def: &GameDef) -> Vec<&'a str> {
        self.extensions
            .keys()
            .filter(|id| def.ext.contains_key(*id))
            .map(String::as_str)
            .collect()
    }

    /// The extensions `def` turns on that this build doesn't have.
    pub fn missing_extensions<'a>(&self, def: &'a GameDef) -> Vec<&'a str> {
        def.ext
            .keys()
            .filter(|id| !self.extensions.contains_key(*id))
            .map(String::as_str)
            .collect()
    }

    /// What extension `id` keeps for `profile` or one of its mods, or the
    /// default if it hasn't kept anything.
    pub fn ext_state<T: DeserializeOwned + Default>(
        &self,
        profile: &ProfileRef,
        id: &str,
        of: StateOf<'_>,
    ) -> Result<T> {
        let loaded = self.load_profile(profile)?;
        let extension = self.extension_on(&loaded, id)?;
        ext::read_state(extension, tables(&loaded, of)?, &self.list_names(id))
    }

    /// Saves `value` as what extension `id` keeps for `profile` or one of
    /// its mods, replacing what it kept before. Like any other change to
    /// the profile, it's synced and makes the profile's deploy out of date.
    pub fn set_ext_state<T: Serialize>(
        &self,
        profile: &ProfileRef,
        id: &str,
        of: StateOf<'_>,
        value: &T,
    ) -> Result<()> {
        let mut loaded = self.load_profile(profile)?;
        let extension = self.extension_on(&loaded, id)?;
        let label = loaded.label();
        let tables = match of {
            StateOf::Profile => &mut loaded.ext,
            StateOf::Mod(name) => {
                &mut loaded
                    .find_mut(name)
                    .ok_or_else(|| no_such_mod(&label, name))?
                    .ext
            }
        };
        ext::write_state(extension, tables, value, &self.list_names(id))?;
        self.save_profile(profile, &loaded)
    }

    /// Extension `id`, which has to be one this build has, as the id of
    /// something it registered is.
    pub(super) fn extension(&self, id: &str) -> &dyn Extension {
        self.extensions[id].as_ref()
    }

    /// Extension `id`, if it's on for `loaded`'s game.
    fn extension_on(&self, loaded: &Profile, id: &str) -> Result<&dyn Extension> {
        let Some(extension) = self.extensions.get(id) else {
            return Err(Error::invalid(format!(
                "this build of patchwork doesn't include the `{id}` extension"
            )));
        };
        let registry = self.registry();
        let def = &registry.game(&loaded.game)?.value;
        if !def.ext.contains_key(id) {
            return Err(Error::invalid(format!(
                "the `{id}` extension isn't on for {}; its definition needs an `[ext.{id}]` table",
                def.name
            )));
        }
        Ok(extension.as_ref())
    }
}

fn tables<'a>(loaded: &'a Profile, of: StateOf<'_>) -> Result<&'a ExtTables> {
    match of {
        StateOf::Profile => Ok(&loaded.ext),
        StateOf::Mod(name) => loaded
            .mods
            .iter()
            .find(|m| m.name == name)
            .map(|m| &m.ext)
            .ok_or_else(|| no_such_mod(&loaded.label(), name)),
    }
}
