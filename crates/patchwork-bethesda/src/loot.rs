//! Sorting the load order with LOOT's masterlist: the `loot` extension's
//! `sort` action. The masterlist (YAML, from LOOT's repositories) puts
//! plugins in groups that load in a set order, and says which plugins load
//! after which. This reads it ourselves rather than using libloot, which is
//! GPL.
//!
//! ```toml
//! [ext.loot.sort]
//! masterlist = "https://raw.githubusercontent.com/loot/skyrimse/v0.29/masterlist.yaml"
//! ```
//!
//! What it does, in order of what wins:
//! 1. Plugins load after their masters, and after what the masterlist says
//!    they load after or need (`after`, `req`). Rules with a `condition`
//!    are skipped, since conditions aren't understood yet.
//! 2. Plugins load in their groups' order; ones the masterlist doesn't put
//!    in a group are in `default`.
//! 3. Otherwise the order stays as it was.
//!
//! Unlike LOOT itself, it doesn't look at which records plugins change, and
//! there's no userlist; the user moves plugins by hand after sorting.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use async_trait::async_trait;
use patchwork_core::action::{Action, ActionContext, Outcome, StateChange};
use patchwork_core::list::List;
use patchwork_core::{Error, Result};
use regex::Regex;
use serde::Deserialize;

use crate::plugins::{EXTENSION as BETHESDA, LIST};

pub const LOOT_EXTENSION: &str = "loot";
pub const SORT: &str = "sort";
/// What plugins the masterlist doesn't put in a group are in.
const DEFAULT_GROUP: &str = "default";

/// The `loot` extension's `sort` action.
pub struct LootSort;

fn error(message: String) -> Error {
    Error::Extension {
        id: LOOT_EXTENSION.to_owned(),
        message,
    }
}

/// The id of the load order, which this sorts.
pub fn load_order() -> String {
    format!("{BETHESDA}.{LIST}")
}

#[async_trait]
impl Action for LootSort {
    fn title(&self) -> &str {
        "Sort with LOOT"
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Puts plugins after their masters and in the order LOOT's masterlist gives (its \
             groups, and which plugins load after which), otherwise keeping your order. It \
             doesn't look at what plugins change, as LOOT itself does.",
        )
    }

    fn shown_with(&self) -> Option<&str> {
        Some("bethesda.plugins")
    }

    async fn run(&self, ctx: &ActionContext<'_>) -> Result<Outcome> {
        let url = ctx
            .config()
            .get("masterlist")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                error(format!(
                    "{} has no `masterlist` in `[ext.{LOOT_EXTENSION}.{SORT}]`",
                    ctx.game().name
                ))
            })?;
        let yaml = ctx.fetch(url).await?;
        let masterlist = Masterlist::parse(&yaml)?;
        let id = load_order();
        let list = ctx.list(&id)?;
        let sorted = sort(list, &masterlist);
        Ok(Outcome {
            message: Some(format!(
                "Sorted {} plugins with LOOT's masterlist, which has rules for {} of them",
                list.items.len(),
                sorted.known
            )),
            changes: vec![StateChange::List {
                id,
                order: sorted.order,
            }],
            warnings: sorted.warnings,
        })
    }
}

/// What the masterlist has to say about load order.
#[derive(Debug, Default)]
pub(crate) struct Masterlist {
    /// Each group's place, lowest first, by name.
    ranks: HashMap<String, usize>,
    /// By lowercased name.
    exact: HashMap<String, Vec<Entry>>,
    regexes: Vec<(Regex, Entry)>,
}

/// One plugin's entry, or a pattern's.
#[derive(Debug, Clone, Default)]
struct Entry {
    group: Option<String>,
    /// Names of plugins it loads after, from `after` and `req`.
    after: Vec<String>,
}

#[derive(Deserialize)]
struct RawMasterlist {
    #[serde(default)]
    groups: Vec<RawGroup>,
    #[serde(default)]
    plugins: Vec<RawPlugin>,
}

#[derive(Deserialize)]
struct RawGroup {
    name: String,
    #[serde(default)]
    after: Vec<String>,
}

#[derive(Deserialize)]
struct RawPlugin {
    name: String,
    group: Option<String>,
    #[serde(default)]
    after: Vec<RawFile>,
    #[serde(default)]
    req: Vec<RawFile>,
}

/// A file named in `after` or `req`: just its name, or with more about it.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawFile {
    Name(String),
    Full {
        name: String,
        condition: Option<String>,
    },
}

impl RawFile {
    /// Its name, unless it has a condition.
    fn unconditional(self) -> Option<String> {
        match self {
            RawFile::Name(name) => Some(name),
            RawFile::Full {
                name,
                condition: None,
            } => Some(name),
            RawFile::Full { .. } => None,
        }
    }
}

/// LOOT treats a name with any of these in it as a regular expression.
fn is_regex(name: &str) -> bool {
    name.contains([':', '\\', '*', '?', '|'])
}

impl Masterlist {
    pub(crate) fn parse(yaml: &[u8]) -> Result<Self> {
        let bad = |e: serde_norway::Error| error(format!("couldn't read LOOT's masterlist: {e}"));
        // Entries are often built from others with merge keys (`<<: *x`).
        let mut value: serde_norway::Value = serde_norway::from_slice(yaml).map_err(bad)?;
        value.apply_merge().map_err(bad)?;
        let raw: RawMasterlist = serde_norway::from_value(value).map_err(bad)?;
        let mut masterlist = Masterlist {
            ranks: group_ranks(&raw.groups),
            ..Default::default()
        };
        for plugin in raw.plugins {
            let after = plugin
                .after
                .into_iter()
                .chain(plugin.req)
                .filter_map(RawFile::unconditional)
                .collect();
            let entry = Entry {
                group: plugin.group,
                after,
            };
            if is_regex(&plugin.name) {
                // ECMAScript syntax, which Rust's mostly shares; patterns it
                // doesn't are left out.
                if let Ok(re) = Regex::new(&format!("(?i)^(?:{})$", plugin.name)) {
                    masterlist.regexes.push((re, entry));
                }
            } else {
                masterlist
                    .exact
                    .entry(plugin.name.to_lowercase())
                    .or_default()
                    .push(entry);
            }
        }
        Ok(masterlist)
    }

    /// Everything the masterlist says about `name`, merged: its group is
    /// the first one given, by its own entries before patterns'.
    fn entry(&self, name: &str) -> Option<Entry> {
        let lower = name.to_lowercase();
        let found: Vec<&Entry> = self
            .exact
            .get(&lower)
            .into_iter()
            .flatten()
            .chain(
                self.regexes
                    .iter()
                    .filter(|(re, _)| re.is_match(name))
                    .map(|(_, e)| e),
            )
            .collect();
        if found.is_empty() {
            return None;
        }
        Some(Entry {
            group: found.iter().find_map(|e| e.group.clone()),
            after: found.iter().flat_map(|e| e.after.clone()).collect(),
        })
    }

    fn rank(&self, group: Option<&str>) -> usize {
        let default = self.ranks.get(DEFAULT_GROUP).copied().unwrap_or(0);
        group
            .and_then(|g| self.ranks.get(g).copied())
            .unwrap_or(default)
    }
}

/// Each group's place: groups come after those they're `after`, and
/// otherwise in the order they're defined. A cycle (which LOOT wouldn't
/// accept) is broken by definition order.
fn group_ranks(groups: &[RawGroup]) -> HashMap<String, usize> {
    let at: HashMap<&str, usize> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.as_str(), i))
        .collect();
    let deps: Vec<Vec<usize>> = groups
        .iter()
        .map(|g| {
            g.after
                .iter()
                .filter_map(|a| at.get(a.as_str()).copied())
                .collect()
        })
        .collect();
    let order = stable_topo(groups.len(), &deps, |i| i).0;
    order
        .into_iter()
        .enumerate()
        .map(|(rank, i)| (groups[i].name.clone(), rank))
        .collect()
}

/// A topological order of `0..n`, where `deps[i]` must come before `i`,
/// taking whichever ready node has the lowest `key` each time. Also returns
/// the nodes that were put in place before something they need, to break a
/// cycle.
fn stable_topo<K: Ord>(
    n: usize,
    deps: &[Vec<usize>],
    key: impl Fn(usize) -> K,
) -> (Vec<usize>, Vec<usize>) {
    let mut waiting: Vec<usize> = deps.iter().map(Vec::len).collect();
    let mut dependents = vec![Vec::new(); n];
    for (i, ds) in deps.iter().enumerate() {
        for &d in ds {
            dependents[d].push(i);
        }
    }
    let mut ready: BinaryHeap<Reverse<(K, usize)>> = (0..n)
        .filter(|&i| waiting[i] == 0)
        .map(|i| Reverse((key(i), i)))
        .collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut forced = Vec::new();
    while order.len() < n {
        let i = match ready.pop() {
            Some(Reverse((_, i))) => i,
            None => {
                // A cycle: go on with whichever's left that comes first.
                let i = (0..n)
                    .filter(|&i| !done[i])
                    .min_by_key(|&i| key(i))
                    .expect("something's left");
                forced.push(i);
                i
            }
        };
        if done[i] {
            continue;
        }
        done[i] = true;
        order.push(i);
        for &d in &dependents[i] {
            waiting[d] = waiting[d].saturating_sub(1);
            if waiting[d] == 0 && !done[d] {
                ready.push(Reverse((key(d), d)));
            }
        }
    }
    (order, forced)
}

pub(crate) struct Sorted {
    pub order: Vec<String>,
    /// How many plugins the masterlist has something on.
    pub known: usize,
    pub warnings: Vec<String>,
}

/// `list`'s items sorted by `masterlist` (see the module docs), each tier
/// on its own since tiers can't be mixed.
pub(crate) fn sort(list: &List, masterlist: &Masterlist) -> Sorted {
    let mut order = Vec::with_capacity(list.items.len());
    let mut known = 0;
    let mut warnings = Vec::new();
    let mut tiers: Vec<u8> = list.items.iter().map(|i| i.item.tier).collect();
    tiers.sort_unstable();
    tiers.dedup();
    for tier in tiers {
        let items: Vec<_> = list
            .items
            .iter()
            .filter(|i| i.item.tier == tier)
            .map(|i| &i.item)
            .collect();
        let at: HashMap<String, usize> = items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.name.to_lowercase(), i))
            .collect();
        let entries: Vec<Option<Entry>> = items.iter().map(|i| masterlist.entry(&i.name)).collect();
        known += entries.iter().filter(|e| e.is_some()).count();
        let deps: Vec<Vec<usize>> = items
            .iter()
            .zip(&entries)
            .enumerate()
            .map(|(me, (item, entry))| {
                let mut deps: Vec<usize> = item
                    .after
                    .iter()
                    .chain(entry.iter().flat_map(|e| &e.after))
                    .filter_map(|name| at.get(&name.to_lowercase()).copied())
                    .filter(|&d| d != me)
                    .collect();
                deps.sort_unstable();
                deps.dedup();
                deps
            })
            .collect();
        let ranks: Vec<usize> = entries
            .iter()
            .map(|e| masterlist.rank(e.as_ref().and_then(|e| e.group.as_deref())))
            .collect();
        let (sorted, forced) = stable_topo(items.len(), &deps, |i| (ranks[i], i));
        for i in forced {
            warnings.push(format!(
                "{} is in a loop of plugins that each have to load after another, so it was put \
                 before some of them",
                items[i].name
            ));
        }
        order.extend(sorted.into_iter().map(|i| items[i].name.clone()));
    }
    Sorted {
        order,
        known,
        warnings,
    }
}

#[cfg(test)]
mod tests;
