//! Plugins: files in mods that a game loads one after another in an order
//! the user picks, separately from which mod's files win (Bethesda games'
//! `.esp`, `.esm` and `.esl` files). Deployers that know about them say which
//! ones the mods have (see [`Deployer::plugins`](crate::deploy::Deployer::plugins));
//! the order the user wants them in is kept in the profile.
//!
//! The profile only keeps the plugins the user has placed. Any others are
//! added below them in the order of their mods, so a new mod's plugins go
//! last, and plugins of mods that are gone are just skipped.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::order::Position;

/// A plugin in the profile's load order, as saved there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginEntry {
    /// The plugin's file name, matched ignoring case.
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// A plugin one of the mods being deployed has, as its deployer found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    /// Its file name.
    pub name: String,
    /// The mod it's from.
    pub mod_name: String,
    /// Loaded before every plugin that isn't one, wherever it's put.
    pub master: bool,
    /// Doesn't take up one of the game's numbered slots.
    pub light: bool,
    /// The plugins it needs loaded before it.
    pub masters: Vec<String>,
}

/// What a deployer found about a deploy's plugins.
#[derive(Debug, Clone, Default)]
pub struct Plugins {
    /// The mods' plugins, in their mods' load order.
    pub found: Vec<Plugin>,
    /// Plugins the game always loads first, before any in the load order
    /// (its own masters and DLC), and so never has to be told about.
    pub implicit: Vec<String>,
    /// The most plugins that aren't light the game can load, counting
    /// `implicit`.
    pub limit: Option<usize>,
    /// Problems found along the way, like plugins that couldn't be read.
    pub warnings: Vec<String>,
}

/// A plugin where it is in the load order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderedPlugin {
    #[serde(flatten)]
    pub plugin: Plugin,
    pub enabled: bool,
}

/// The order the game will load a profile's plugins in.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadOrder {
    /// Masters first, since the game puts them there anyway.
    pub plugins: Vec<OrderedPlugin>,
    pub implicit: Vec<String>,
    /// Problems the game would have with it, like a plugin whose master
    /// isn't loaded.
    pub warnings: Vec<String>,
}

impl LoadOrder {
    /// Works out the load order from what the profile has `saved` and the
    /// plugins its mods have.
    pub fn new(saved: &[PluginEntry], plugins: Plugins) -> Self {
        // The same plugin from two mods is one file once deployed: the
        // later mod's.
        let mut found: Vec<Plugin> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        for plugin in plugins.found {
            match at.get(&plugin.name.to_lowercase()) {
                Some(&i) => found[i] = plugin,
                None => {
                    at.insert(plugin.name.to_lowercase(), found.len());
                    found.push(plugin);
                }
            }
        }
        let implicit: HashSet<String> = plugins.implicit.iter().map(|p| p.to_lowercase()).collect();
        let mut taken = vec![false; found.len()];
        let mut ordered = Vec::new();
        for entry in saved {
            if let Some(&i) = at.get(&entry.name.to_lowercase())
                && !taken[i]
            {
                taken[i] = true;
                ordered.push(OrderedPlugin {
                    plugin: found[i].clone(),
                    enabled: entry.enabled,
                });
            }
        }
        for (i, plugin) in found.into_iter().enumerate() {
            if !taken[i] {
                ordered.push(OrderedPlugin {
                    plugin,
                    enabled: true,
                });
            }
        }
        ordered.retain(|p| !implicit.contains(&p.plugin.name.to_lowercase()));
        // Stable, so each group keeps the user's order.
        ordered.sort_by_key(|p| !p.plugin.master);

        let mut warnings = plugins.warnings;
        warnings.extend(check(&ordered, &implicit, plugins.implicit.len(), plugins.limit));
        Self {
            plugins: ordered,
            implicit: plugins.implicit,
            warnings,
        }
    }

    /// The order as a profile saves it.
    pub fn entries(&self) -> Vec<PluginEntry> {
        self.plugins
            .iter()
            .map(|p| PluginEntry {
                name: p.plugin.name.clone(),
                enabled: p.enabled,
            })
            .collect()
    }

    fn find(&self, name: &str) -> Result<usize, Error> {
        self.plugins
            .iter()
            .position(|p| p.plugin.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::invalid(format!("none of the mods have a plugin named `{name}`")))
    }

    /// Moves the plugin called `name` to `to`, returning where it ended up.
    /// Masters can't go below other plugins, nor others above masters,
    /// since the game would only put them back.
    pub fn move_plugin(&mut self, name: &str, to: &Position) -> Result<usize, Error> {
        let from = self.find(name)?;
        let master = self.plugins[from].plugin.master;
        if let Position::Before(other) | Position::After(other) = to {
            let other = self.find(other)?;
            if other == from {
                return Ok(from);
            }
        }
        let moving = self.plugins.remove(from);
        // Where its group is.
        let masters = self.plugins.iter().filter(|p| p.plugin.master).count();
        let (start, end) = if master {
            (0, masters)
        } else {
            (masters, self.plugins.len())
        };
        let index = match to {
            Position::Top => 0,
            Position::Bottom => self.plugins.len(),
            Position::Index(i) => *i,
            Position::Before(other) => self.find(other)?,
            Position::After(other) => self.find(other)? + 1,
        };
        let index = index.clamp(start, end);
        self.plugins.insert(index, moving);
        Ok(index)
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), Error> {
        let i = self.find(name)?;
        self.plugins[i].enabled = enabled;
        Ok(())
    }
}

/// `new`, with the entries from `old` that it leaves out put back after
/// whatever came before them in `old`, so the plugins of a mod that's
/// disabled for now keep their place for when it's back.
pub fn keep_missing(old: &[PluginEntry], mut new: Vec<PluginEntry>) -> Vec<PluginEntry> {
    let find = |list: &[PluginEntry], name: &str| {
        list.iter().position(|p| p.name.eq_ignore_ascii_case(name))
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

/// What's wrong with `plugins` as a load order.
fn check(
    plugins: &[OrderedPlugin],
    implicit: &HashSet<String>,
    implicit_count: usize,
    limit: Option<usize>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let enabled: HashMap<String, usize> = plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.enabled)
        .map(|(i, p)| (p.plugin.name.to_lowercase(), i))
        .collect();
    let all: HashSet<String> = plugins
        .iter()
        .map(|p| p.plugin.name.to_lowercase())
        .collect();
    for (i, p) in plugins.iter().enumerate().filter(|(_, p)| p.enabled) {
        for master in &p.plugin.masters {
            let key = master.to_lowercase();
            if implicit.contains(&key) {
                continue;
            }
            let problem = match enabled.get(&key) {
                Some(&at) if at < i => continue,
                Some(_) => "loads after it",
                None if all.contains(&key) => "is disabled",
                None => "isn't in any of the mods",
            };
            warnings.push(format!(
                "{} needs {master}, which {problem}; the game will crash or leave it out",
                p.plugin.name
            ));
        }
    }
    if let Some(limit) = limit {
        let full = implicit_count
            + plugins
                .iter()
                .filter(|p| p.enabled && !p.plugin.light)
                .count();
        if full > limit {
            warnings.push(format!(
                "{full} plugins that aren't light are enabled, counting the game's own, but the game can only load {limit}"
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests;
