//! Ordered lists: things in mods that the user puts in an order of their
//! own, apart from the mods' order, like Bethesda games' plugins. An
//! extension says what can go in a list with an [`OrderedList`]; core keeps
//! the order the user picks, merges it with what's found, saves and syncs
//! it, and hands it to the deployer through
//! [`BuildContext::list`](crate::deploy::BuildContext::list).
//!
//! The profile only keeps the items the user has placed (in the
//! extension's state, under the list's name). Any others are added below
//! them in the order of their mods, so a new mod's items go last, and items
//! of mods that are gone are just skipped. Items are matched by name,
//! ignoring ASCII case, since they're usually file names on Windows.
//!
//! Core checks that each item comes after the ones it needs
//! ([`ListItem::after`]). Anything else wrong with an arrangement, like too
//! many plugins, is for a [`Check`](crate::check::Check) that says its
//! problem is about the list.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::check::Problem;
use crate::deploy::{DeployContext, GameFiles, InstalledMod};
use crate::game::GameDef;
use crate::order::Position;
use crate::{Error, Result};

/// Something in mods the user puts in order, registered with
/// [`Registrar::ordered_list`](crate::ext::Registrar::ordered_list).
pub trait OrderedList: Send + Sync {
    /// Shown as the tab or the list's name, like "Plugins".
    fn title(&self) -> &str;

    /// A sentence or two on what the order does, for frontends to show
    /// above the list.
    fn description(&self) -> Option<&str> {
        None
    }

    /// What can go in the list for `mods`. Unlike
    /// [`Deployer::plan`](crate::deploy::Deployer::plan), this may read
    /// mods' and the game's files, but not write anything.
    fn items(&self, ctx: &ListContext<'_>, mods: &[InstalledMod]) -> Result<ListItems>;
}

/// What an [`OrderedList`] gets to work with.
pub struct ListContext<'a> {
    deploy: &'a DeployContext<'a>,
    files: &'a GameFiles<'a>,
    config: &'a toml::Table,
}

impl<'a> ListContext<'a> {
    /// Core makes these; this is for testing lists on their own.
    pub fn new(
        deploy: &'a DeployContext<'a>,
        files: &'a GameFiles<'a>,
        config: &'a toml::Table,
    ) -> Self {
        Self {
            deploy,
            files,
            config,
        }
    }

    pub fn game(&self) -> &GameDef {
        self.deploy.game
    }

    /// What the game's deployer gets, for lists that need to know where
    /// mods' files will go.
    pub fn deploy(&self) -> &DeployContext<'a> {
        self.deploy
    }

    /// The game's own files, as they are without anything patchwork
    /// deployed.
    pub fn files(&self) -> &GameFiles<'a> {
        self.files
    }

    /// The list's `[ext.<extension>.<list>]` table in the game definition.
    pub fn config(&self) -> &toml::Table {
        self.config
    }
}

/// What an [`OrderedList`] found.
#[derive(Debug, Clone, Default)]
pub struct ListItems {
    /// In their mods' load order. If two have the same name, the later
    /// one's is the one that counts, as it is for files.
    pub items: Vec<ListItem>,
    /// Always first and not movable, like the game's own masters. Items
    /// with the same names are left out of `items`.
    pub fixed: Vec<ListItem>,
    /// What each tier is called, for frontends that show tiers apart, like
    /// `["Masters", "Plugins"]`.
    pub tiers: Vec<String>,
    /// Problems found along the way, like files that couldn't be read.
    pub warnings: Vec<String>,
}

/// Something that can go in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub name: String,
    /// The mod it's from, or empty for the game's own.
    pub mod_name: String,
    /// Items with a lower tier always come before higher ones, whatever
    /// the user does: masters are tier 0, other plugins 1.
    pub tier: u8,
    /// Short labels a frontend shows next to it, like "master", "light".
    pub badges: Vec<String>,
    /// Names of items it needs before it, which core checks.
    pub after: Vec<String>,
    /// More about it, for a frontend to show on hover.
    pub detail: Option<String>,
    pub can_disable: bool,
}

/// An item in a list as a profile saves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// An item where it is in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderedItem {
    #[serde(flatten)]
    pub item: ListItem,
    pub enabled: bool,
}

/// A list as it is for a profile: what the user saved, merged with what's
/// in its mods.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct List {
    /// `<extension>.<list>`.
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub tiers: Vec<String>,
    /// Lower tiers first, each in the user's order.
    pub items: Vec<OrderedItem>,
    pub fixed: Vec<ListItem>,
    /// Problems with it as it is, like an item before one it needs: what
    /// [`ListItems::warnings`] has, what core finds, and (from
    /// [`Patchwork::list`](crate::Patchwork::list)) what the extension's
    /// checks say about it.
    pub problems: Vec<Problem>,
}

impl List {
    /// Merges what `found` has into the order `saved`. Core makes these;
    /// this is for testing lists and deployers on their own.
    pub fn new(id: String, list: &dyn OrderedList, saved: &[ListEntry], found: ListItems) -> Self {
        let key = |name: &str| name.to_ascii_lowercase();
        let fixed: HashSet<String> = found.fixed.iter().map(|i| key(&i.name)).collect();
        let mut items: Vec<ListItem> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        for item in found.items {
            if fixed.contains(&key(&item.name)) {
                continue;
            }
            match at.get(&key(&item.name)) {
                Some(&i) => items[i] = item,
                None => {
                    at.insert(key(&item.name), items.len());
                    items.push(item);
                }
            }
        }
        let mut taken = vec![false; items.len()];
        let mut ordered = Vec::new();
        for entry in saved {
            if let Some(&i) = at.get(&key(&entry.name))
                && !taken[i]
            {
                taken[i] = true;
                ordered.push(OrderedItem {
                    enabled: entry.enabled || !items[i].can_disable,
                    item: items[i].clone(),
                });
            }
        }
        for (i, item) in items.into_iter().enumerate() {
            if !taken[i] {
                ordered.push(OrderedItem {
                    item,
                    enabled: true,
                });
            }
        }
        // Stable, so each tier keeps the user's order.
        ordered.sort_by_key(|i| i.item.tier);

        let mut list = Self {
            id,
            title: list.title().to_owned(),
            description: list.description().map(str::to_owned),
            tiers: found.tiers,
            items: ordered,
            fixed: found.fixed,
            problems: found.warnings.into_iter().map(Problem::warning).collect(),
        };
        let problems = list.check_after();
        list.problems.extend(problems);
        list
    }

    /// The order as a profile saves it.
    pub fn entries(&self) -> Vec<ListEntry> {
        self.items
            .iter()
            .map(|i| ListEntry {
                name: i.item.name.clone(),
                enabled: i.enabled,
            })
            .collect()
    }

    fn find(&self, name: &str) -> Result<usize> {
        self.items
            .iter()
            .position(|i| i.item.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "there's nothing named `{name}` in {}",
                    self.title.to_lowercase()
                ))
            })
    }

    /// Moves the item called `name` to `to`, returning where it ended up.
    /// It stays among the items of its own tier, since it'd only be put
    /// back there.
    pub fn move_item(&mut self, name: &str, to: &Position) -> Result<usize> {
        let from = self.find(name)?;
        let tier = self.items[from].item.tier;
        if let Position::Before(other) | Position::After(other) = to {
            let other = self.find(other)?;
            if other == from {
                return Ok(from);
            }
        }
        let moving = self.items.remove(from);
        let start = self.items.iter().filter(|i| i.item.tier < tier).count();
        let end = start + self.items.iter().filter(|i| i.item.tier == tier).count();
        let index = match to {
            Position::Top => 0,
            Position::Bottom => self.items.len(),
            Position::Index(i) => *i,
            Position::Before(other) => self.find(other)?,
            Position::After(other) => self.find(other)? + 1,
        };
        let index = index.clamp(start, end);
        self.items.insert(index, moving);
        Ok(index)
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        let i = self.find(name)?;
        if !enabled && !self.items[i].item.can_disable {
            return Err(Error::invalid(format!("{name} can't be turned off")));
        }
        self.items[i].enabled = enabled;
        Ok(())
    }

    /// Enabled items that need something that comes after them, is
    /// disabled or isn't there.
    fn check_after(&self) -> Vec<Problem> {
        let key = |name: &str| name.to_ascii_lowercase();
        let fixed: HashSet<String> = self.fixed.iter().map(|i| key(&i.name)).collect();
        let enabled: HashMap<String, usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.enabled)
            .map(|(at, i)| (key(&i.item.name), at))
            .collect();
        let all: HashSet<String> = self.items.iter().map(|i| key(&i.item.name)).collect();
        let mut problems = Vec::new();
        for (at, item) in self.items.iter().enumerate().filter(|(_, i)| i.enabled) {
            for needs in &item.item.after {
                if fixed.contains(&key(needs)) {
                    continue;
                }
                let problem = match enabled.get(&key(needs)) {
                    Some(&there) if there < at => continue,
                    Some(_) => "comes after it",
                    None if all.contains(&key(needs)) => "is disabled",
                    None => "isn't in any of the mods",
                };
                problems.push(
                    Problem::warning(format!("{} needs {needs}, which {problem}", item.item.name))
                        .about_mod(&item.item.mod_name),
                );
            }
        }
        problems
    }
}

/// `new`, with the entries from `old` that it leaves out put back after
/// whatever came before them in `old`, so the items of a mod that's
/// disabled for now keep their place for when it's back.
pub fn keep_missing(old: &[ListEntry], mut new: Vec<ListEntry>) -> Vec<ListEntry> {
    let find = |list: &[ListEntry], name: &str| {
        list.iter().position(|e| e.name.eq_ignore_ascii_case(name))
    };
    let mut before: Option<&str> = None;
    for entry in old {
        if find(&new, &entry.name).is_none() {
            let at = before.and_then(|b| find(&new, b)).map_or(0, |i| i + 1);
            new.insert(at, entry.clone());
        }
        before = Some(&entry.name);
    }
    new
}

#[cfg(test)]
mod tests;
