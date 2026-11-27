//! Dependencies a game doesn't need installed: ones its deployer or
//! extensions do the job of (the game's `provided`), and ones some other mod
//! in the profile already does the job of (`alike` groups in the
//! [`HIDDEN_KIND`] entries of the game's sources).
//!
//! HITMAN's deployer does what Simple Mod Framework does, so mods that list
//! SMF don't get it. Every BepInEx game has its own BepInEx pack on
//! Thunderstore, and a mod published for several games may list another
//! game's; a profile with any pack has what it needs.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::Error;
use crate::game::GameDef;
use crate::lock::mod_key;
use crate::registry::Registry;
use crate::search::HIDDEN_KIND;
use crate::source::{Dependency, SoftDependency};

/// What a game doesn't need installed; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct Provided {
    /// The game's `provided`.
    by_game: GlobSet,
    /// `alike` groups: a source, and globs over its ids.
    alike: Vec<(String, GlobSet)>,
}

fn glob(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).case_insensitive(true).build()
}

impl Provided {
    /// What `game` doesn't need installed.
    pub fn of(registry: &Registry, game: &GameDef) -> Result<Self, Error> {
        let bad = |what: String, why: String| Error::invalid(format!("{what}: {why}"));
        let mut by_game = GlobSetBuilder::new();
        for pattern in &game.provided {
            let what = format!("game {}'s `provided`", game.id);
            by_game.add(glob(pattern).map_err(|e| bad(what, format!("`{pattern}`: {e}")))?);
        }
        let by_game = by_game
            .build()
            .map_err(|e| bad(format!("game {}'s `provided`", game.id), e.to_string()))?;
        let mut alike = Vec::new();
        for source in game.sources.keys() {
            alike.extend(alike_of(registry, source)?);
        }
        Ok(Self { by_game, alike })
    }

    /// Whether the game does the job of the mod `key` (`source:id`) itself.
    pub fn provides(&self, key: &str) -> bool {
        self.by_game.is_match(key)
    }

    /// Whether the game does the job of `dep` itself, going by its key or,
    /// for ones without a usable key, its name.
    pub fn provides_soft(&self, dep: &SoftDependency) -> bool {
        dep.key.as_deref().is_some_and(|k| self.provides(k))
            || dep
                .name
                .as_deref()
                .is_some_and(|n| self.by_game.is_match(n))
    }

    /// Leaves out what the game provides from a mod's dependencies.
    pub fn drop_provided(&self, deps: &mut Vec<Dependency>, soft: &mut Vec<SoftDependency>) {
        deps.retain(|d| !self.provides(&mod_key(&d.source, &d.id)));
        soft.retain(|d| !self.provides_soft(d));
    }

    /// Of `keys` (each `source:id`), the first that does the same job as
    /// `dep`, going by its key or, for ones without a key, its name taken
    /// as an id.
    pub fn alike_in<'a>(
        &self,
        dep: &SoftDependency,
        mut keys: impl Iterator<Item = &'a str>,
    ) -> Option<&'a str> {
        let (source, ids) = self.alike.iter().find(|(source, ids)| match &dep.key {
            Some(key) => key
                .split_once(':')
                .is_some_and(|(s, id)| s == source && ids.is_match(id)),
            None => dep.name.as_deref().is_some_and(|n| ids.is_match(n)),
        })?;
        keys.find(|k| {
            Some(*k) != dep.key.as_deref()
                && k.split_once(':')
                    .is_some_and(|(s, id)| s == source && ids.is_match(id))
        })
    }
}

/// The `alike` groups of `source`'s [`HIDDEN_KIND`] entry: lists of id
/// globs, each a group of mods that do the same job.
fn alike_of(registry: &Registry, source: &str) -> Result<Vec<(String, GlobSet)>, Error> {
    let Some(entry) = registry.data(HIDDEN_KIND, source) else {
        return Ok(Vec::new());
    };
    let bad = |why: String| Error::invalid(format!("{HIDDEN_KIND}/{source}.toml: {why}"));
    let shape = || bad("`alike` must be a list of lists of patterns".into());
    let groups = match entry.value.get("alike") {
        None => return Ok(Vec::new()),
        Some(toml::Value::Array(groups)) => groups,
        Some(_) => return Err(shape()),
    };
    groups
        .iter()
        .map(|group| {
            let toml::Value::Array(ids) = group else {
                return Err(shape());
            };
            let mut set = GlobSetBuilder::new();
            for id in ids {
                let id = id.as_str().ok_or_else(shape)?;
                set.add(glob(id).map_err(|e| bad(format!("`{id}`: {e}")))?);
            }
            let set = set.build().map_err(|e| bad(e.to_string()))?;
            Ok((source.to_owned(), set))
        })
        .collect()
}

#[cfg(test)]
mod tests;
