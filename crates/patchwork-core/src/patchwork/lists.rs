//! The ordered lists that are on for a profile's game, and the order the
//! user puts them in; see [`crate::list`].

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use super::{Patchwork, deploy_order};
use crate::deploy::{DeployContext, GameFiles, InstalledMod};
use crate::ext::{self, Hook};
use crate::game::GameDef;
use crate::list::{self, List, ListContext, ListEntry, OrderedList};
use crate::order::Position;
use crate::profile::{Profile, ProfileRef};
use crate::{Error, Result};

/// An ordered list that's on for a profile's game, from
/// [`Patchwork::lists`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListInfo {
    /// `<extension>.<list>`.
    pub id: String,
    pub title: String,
}

impl Patchwork {
    /// The lists that are on for `def`, in id order.
    pub(super) fn lists_on(&self, def: &GameDef) -> Vec<(&str, &Hook<dyn OrderedList>)> {
        self.lists
            .iter()
            .filter(|(_, hook)| self.hook_on(def, hook.ext, hook.name))
            .map(|(id, hook)| (id.as_str(), hook))
            .collect()
    }

    /// Every list's title, by `(extension, list)`.
    pub(super) fn list_titles(&self) -> BTreeMap<(String, String), String> {
        self.lists
            .values()
            .map(|h| {
                let key = (h.ext.to_owned(), h.name.to_owned());
                (key, h.hook.title().to_owned())
            })
            .collect()
    }

    /// The names of the lists extension `ext` has, which are keys of its
    /// state that core keeps.
    pub(super) fn list_names(&self, ext: &str) -> Vec<&'static str> {
        self.lists
            .values()
            .filter(|h| h.ext == ext)
            .map(|h| h.name)
            .collect()
    }

    /// The ordered lists that are on for `profile`'s game. Cheap: nothing's
    /// worked out until [`Patchwork::list`].
    pub fn lists(&self, profile: &ProfileRef) -> Result<Vec<ListInfo>> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let def = &registry.game(&loaded.game)?.value;
        Ok(self
            .lists_on(def)
            .into_iter()
            .map(|(id, hook)| ListInfo {
                id: id.to_owned(),
                title: hook.hook.title().to_owned(),
            })
            .collect())
    }

    /// List `id` as it is for `profile`, with what its extension's checks
    /// say about it. Only mods that have been locked count, without locking
    /// again, so this never touches the network.
    pub fn list(&self, profile: &ProfileRef, id: &str) -> Result<List> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let hook = self.list_on(game, id)?;
        let (config, _, _) = self.deploy_config(game)?;
        let ctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let lock = self.load_lock(profile)?;
        let order = lock
            .as_ref()
            .map(|lock| deploy_order(&loaded, lock))
            .unwrap_or_default();
        let root = self.target_root(&target)?.unwrap_or_default();
        let state = self.deploy_state(&target)?;
        let files = GameFiles::new(&root, &state);
        let (mods, _) = self.installed(&loaded, game, Some(&files), &order)?;
        let mut list = self.make_list(&loaded, id, hook, &ctx, &files, &mods)?;
        let problems = self.list_problems(game, hook.ext, &files, &list, &mods);
        list.problems.extend(problems);
        Ok(list)
    }

    /// Every list that's on for `loaded`'s game, for a deploy of `mods`.
    pub(super) fn make_lists(
        &self,
        loaded: &Profile,
        ctx: &DeployContext<'_>,
        files: &GameFiles<'_>,
        mods: &[InstalledMod],
    ) -> Result<Vec<List>> {
        self.lists_on(ctx.game)
            .into_iter()
            .map(|(id, hook)| self.make_list(loaded, id, hook, ctx, files, mods))
            .collect()
    }

    pub(super) fn make_list(
        &self,
        loaded: &Profile,
        id: &str,
        hook: &Hook<dyn OrderedList>,
        ctx: &DeployContext<'_>,
        files: &GameFiles<'_>,
        mods: &[InstalledMod],
    ) -> Result<List> {
        let saved: Vec<ListEntry> =
            ext::read_key(self.extension(hook.ext), &loaded.ext, hook.name)?;
        let empty = toml::Table::new();
        let config = ctx.game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
        let lctx = ListContext::new(ctx, files, config);
        let found = hook.hook.items(&lctx, mods)?;
        Ok(List::new(id.to_owned(), hook.hook.as_ref(), &saved, found))
    }

    /// Saves `entries` as the order of `profile`'s list `id`. This is how a
    /// frontend saves a list the user has rearranged. Lower tiers are put
    /// first when it's next worked out, wherever they are here. Items that
    /// were in the saved order but aren't in `entries` (because their mods
    /// are disabled, say) keep their places.
    pub fn set_list(&self, profile: &ProfileRef, id: &str, entries: Vec<ListEntry>) -> Result<()> {
        let mut seen = HashSet::new();
        if let Some(twice) = entries
            .iter()
            .find(|e| !seen.insert(e.name.to_ascii_lowercase()))
        {
            return Err(Error::invalid(format!(
                "`{}` is in the list twice",
                twice.name
            )));
        }
        let mut loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let hook = self.list_on(&registry.game(&loaded.game)?.value, id)?;
        let extension = self.extension(hook.ext);
        let old: Vec<ListEntry> = ext::read_key(extension, &loaded.ext, hook.name)?;
        let new = list::keep_missing(&old, entries);
        ext::write_key(extension, &mut loaded.ext, hook.name, &new)?;
        self.save_profile(profile, &loaded)
    }

    /// Moves an item within `profile`'s list `id`, returning its new
    /// (zero-based) position. It stays among items of its own tier.
    pub fn move_in_list(
        &self,
        profile: &ProfileRef,
        id: &str,
        name: &str,
        to: &Position,
    ) -> Result<usize> {
        self.change_list(profile, id, |list| list.move_item(name, to))
    }

    pub fn set_list_enabled(
        &self,
        profile: &ProfileRef,
        id: &str,
        name: &str,
        enabled: bool,
    ) -> Result<()> {
        self.change_list(profile, id, |list| list.set_enabled(name, enabled))
    }

    fn change_list<T>(
        &self,
        profile: &ProfileRef,
        id: &str,
        change: impl FnOnce(&mut List) -> Result<T>,
    ) -> Result<T> {
        let mut list = self.list(profile, id)?;
        let changed = change(&mut list)?;
        self.set_list(profile, id, list.entries())?;
        Ok(changed)
    }

    /// List `id`, if it's on for `def`.
    fn list_on(&self, def: &GameDef, id: &str) -> Result<&Hook<dyn OrderedList>> {
        let Some(hook) = self.lists.get(id) else {
            return Err(Error::invalid(format!("there's no list `{id}`")));
        };
        if !self.hook_on(def, hook.ext, hook.name) {
            return Err(Error::invalid(format!(
                "{} doesn't use the `{id}` list",
                def.name
            )));
        }
        Ok(hook)
    }

    /// The names in `def`'s `off` lists that aren't hooks of their
    /// extensions, as `(extension, name)`.
    pub(super) fn unknown_hooks<'a>(&self, def: &'a GameDef) -> Vec<(&'a str, &'a str)> {
        def.ext
            .keys()
            .filter(|ext| self.extensions.contains_key(*ext))
            .flat_map(|ext| {
                let names = self.hook_names(ext);
                def.hooks_off(ext)
                    .into_iter()
                    .filter(move |off| !names.contains(off))
                    .map(move |off| (ext.as_str(), off))
            })
            .collect()
    }
}
