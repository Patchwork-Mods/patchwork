//! Actions: things the user runs once, like "sort plugins with LOOT". An
//! [`Action`] looks at the profile and says what it wants changed, as
//! [`StateChange`]s; core shows or applies those, so a profile is never left
//! half changed and every change is saved, synced and kept in history like
//! any other edit.
//!
//! An action can change its own extension's lists, and the lists of other
//! extensions it declares with
//! [`Registrar::uses`](crate::ext::Registrar::uses). Changes go through the
//! generic list machinery, so the owner's rules (tiers, say) still hold.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::Serialize;

use crate::deploy::{GameFiles, InstalledMod};
use crate::game::GameDef;
use crate::list::{List, ListEntry};
use crate::{Error, Result};

/// Something to run once, registered with
/// [`Registrar::action`](crate::ext::Registrar::action).
#[async_trait]
pub trait Action: Send + Sync {
    /// What a button or menu item says, like "Sort with LOOT".
    fn title(&self) -> &str;

    /// A sentence or two on what it does, for frontends to show on hover or
    /// in help.
    fn description(&self) -> Option<&str> {
        None
    }

    /// The id of the list it's about (`bethesda.plugins`), so frontends
    /// offer it with that list; `None` offers it with the profile.
    fn shown_with(&self) -> Option<&str> {
        None
    }

    /// What it wants done to the profile. It may read mods' and the game's
    /// files and fetch things through [`ActionContext::fetch`], but changes
    /// nothing itself.
    async fn run(&self, ctx: &ActionContext<'_>) -> Result<Outcome>;
}

/// What an [`Action`] wants done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// A sentence saying what it did, like "Sorted 212 plugins".
    pub message: Option<String>,
    pub changes: Vec<StateChange>,
    /// Things the user should know, that didn't stop it.
    pub warnings: Vec<String>,
}

/// A change an [`Action`] wants made. More kinds will be added, so match
/// with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateChange {
    /// Puts list `id` (`<extension>.<list>`) in this order, by name. Items
    /// it leaves out go after the ones it names, in the order they were.
    /// Lower tiers still come first, and enabled items stay enabled.
    List { id: String, order: Vec<String> },
}

/// What an [`Action`] gets to work with.
pub struct ActionContext<'a> {
    game: &'a GameDef,
    config: &'a toml::Table,
    ext: &'a str,
    uses: &'a [&'static str],
    files: &'a GameFiles<'a>,
    lists: &'a [List],
    mods: &'a [InstalledMod],
    fetcher: &'a (dyn Fetch + 'a),
    warnings: Mutex<Vec<String>>,
}

/// How an [`ActionContext`] fetches things; core's caches what it fetches.
#[async_trait]
pub trait Fetch: Send + Sync {
    /// `url`'s contents, and a warning if they're an old copy.
    async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>)>;
}

impl<'a> ActionContext<'a> {
    /// Core makes these; this is for testing actions on their own. `lists`
    /// are every list that's on for the game, as they are for the profile;
    /// the action sees its extension's own, and those it `uses`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game: &'a GameDef,
        config: &'a toml::Table,
        ext: &'a str,
        uses: &'a [&'static str],
        files: &'a GameFiles<'a>,
        lists: &'a [List],
        mods: &'a [InstalledMod],
        fetcher: &'a (dyn Fetch + 'a),
    ) -> Self {
        Self {
            game,
            config,
            ext,
            uses,
            files,
            lists,
            mods,
            fetcher,
            warnings: Mutex::new(Vec::new()),
        }
    }

    pub fn game(&self) -> &GameDef {
        self.game
    }

    /// The action's `[ext.<extension>.<action>]` table in the game
    /// definition.
    pub fn config(&self) -> &toml::Table {
        self.config
    }

    /// The game's own files, as they are without anything patchwork
    /// deployed. Empty if the game isn't set up.
    pub fn files(&self) -> &GameFiles<'a> {
        self.files
    }

    /// The profile's enabled mods, in load order, with their files as
    /// transforms leave them.
    pub fn mods(&self) -> &[InstalledMod] {
        self.mods
    }

    /// List `id` (`<extension>.<list>`) as the user has it. Only the
    /// extension's own lists and those it [`uses`](crate::ext::Registrar::uses)
    /// can be read.
    pub fn list(&self, id: &str) -> Result<&List> {
        if !may_use(self.ext, self.uses, id) {
            return Err(not_declared(self.ext, id));
        }
        self.lists
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| Error::Extension {
                id: self.ext.to_owned(),
                message: format!("{} doesn't have the `{id}` list on", self.game.name),
            })
    }

    /// What's at `url`. Core keeps a copy, so if it can't be fetched again
    /// the last copy is used, with a warning in the outcome.
    pub async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let (bytes, warning) = self.fetcher.fetch(url).await?;
        if let Some(warning) = warning {
            self.warnings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(warning);
        }
        Ok(bytes)
    }

    /// The warnings fetching gave, for core to add to the outcome.
    pub(crate) fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut self.warnings.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Whether extension `ext`, which declared `uses`, may read or change hook
/// `id`.
pub(crate) fn may_use(ext: &str, uses: &[&str], id: &str) -> bool {
    id.split_once('.').is_some_and(|(owner, _)| owner == ext) || uses.contains(&id)
}

pub(crate) fn not_declared(ext: &str, id: &str) -> Error {
    Error::Extension {
        id: ext.to_owned(),
        message: format!(
            "it uses `{id}` without declaring it; add `ext.uses(\"{id}\")` to its `register`"
        ),
    }
}

/// An action that's on for a profile's game, from
/// [`Patchwork::actions`](crate::Patchwork::actions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionInfo {
    /// `<extension>.<action>`.
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    /// The list it's about, if it's about one.
    pub shown_with: Option<String>,
}

/// What running an action did, or would do, from
/// [`Patchwork::run_action`](crate::Patchwork::run_action).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReport {
    pub id: String,
    pub title: String,
    pub message: Option<String>,
    pub warnings: Vec<String>,
    /// Each list it changes, and how.
    pub lists: Vec<ListChange>,
    /// Whether the changes were saved, rather than only worked out.
    pub applied: bool,
}

/// How an action changes a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChange {
    pub id: String,
    pub title: String,
    /// The order before, for undoing it.
    pub before: Vec<ListEntry>,
    pub after: Vec<ListEntry>,
    /// The fewest items that, moved, turn `before` into `after`, with where
    /// they were and end up (from 0). The others stay in the same order.
    pub moved: Vec<Moved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    pub name: String,
    pub from: usize,
    pub to: usize,
}

/// `list`'s entries put in `order`: the items it names first (each once,
/// ignoring ASCII case), then the rest as they were, lower tiers first.
/// Also returns the names in `order` that aren't in the list.
pub(crate) fn reorder(list: &List, order: &[String]) -> (Vec<ListEntry>, Vec<String>) {
    let mut taken = vec![false; list.items.len()];
    let mut picked = Vec::new();
    let mut unknown = Vec::new();
    for name in order {
        match list
            .items
            .iter()
            .position(|i| i.item.name.eq_ignore_ascii_case(name))
        {
            Some(i) if !taken[i] => {
                taken[i] = true;
                picked.push(i);
            }
            Some(_) => {}
            None => unknown.push(name.clone()),
        }
    }
    picked.extend((0..list.items.len()).filter(|&i| !taken[i]));
    // Stable, as `List::new`'s is, so this is what it'll be shown as.
    picked.sort_by_key(|&i| list.items[i].item.tier);
    let entries = picked
        .into_iter()
        .map(|i| ListEntry {
            name: list.items[i].item.name.clone(),
            enabled: list.items[i].enabled,
        })
        .collect();
    (entries, unknown)
}

/// The fewest items to move to turn `before` into `after` (the same names
/// in another order): those outside a longest run that keeps its order.
pub(crate) fn moved(before: &[ListEntry], after: &[ListEntry]) -> Vec<Moved> {
    let from: Vec<usize> = after
        .iter()
        .map(|a| {
            before
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(&a.name))
                .unwrap_or(usize::MAX)
        })
        .collect();
    let kept = longest_increasing(&from);
    from.iter()
        .enumerate()
        .filter(|(to, _)| !kept[*to])
        .map(|(to, &from)| Moved {
            name: after[to].name.clone(),
            from,
            to,
        })
        .collect()
}

/// Which of `xs` are in a longest strictly increasing subsequence.
fn longest_increasing(xs: &[usize]) -> Vec<bool> {
    // `tails[k]` is the index of the smallest last element of a run of
    // length `k + 1`; `prev` links each element to the one before it.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; xs.len()];
    for (i, &x) in xs.iter().enumerate() {
        let k = tails.partition_point(|&t| xs[t] < x);
        if k > 0 {
            prev[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut kept = vec![false; xs.len()];
    let mut at = tails.last().copied();
    while let Some(i) = at {
        kept[i] = true;
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    kept
}

#[cfg(test)]
mod tests;
