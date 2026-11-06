//! Rearranging a profile's mods. Their order is the load order: later mods
//! win when two provide the same file. Dependencies are always deployed
//! before the mods that need them whatever the order, so no rearrangement
//! can break a mod by putting it above what it needs.
//!
//! The profile's order is the user's own arrangement, which nothing but
//! the user changes. The load order is that arrangement put in order of
//! [`ProfileMod::priority`] (see [`load_order`]), so a mod's priority decides
//! which mods it wins over wherever it's put, and its place among the mods
//! with the same priority decides the rest. With every priority 0, the two
//! are the same.
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;

use crate::Error;
use crate::lock::Lockfile;
use crate::profile::{ProfileMod, no_such_mod};

/// Where [`move_mod`] puts a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position {
    Top,
    Bottom,
    /// Zero-based; past the end means the bottom.
    Index(usize),
    Before(String),
    After(String),
}

/// `mods` in load order: lowest priority first, and mods with the same
/// priority in the order they're in. Later mods win when two provide the
/// same file.
pub fn load_order(mods: &[ProfileMod]) -> Vec<&ProfileMod> {
    let mut order: Vec<&ProfileMod> = mods.iter().collect();
    order.sort_by_key(|m| m.priority);
    order
}

/// Moves the mod called `name` to `to`, returning where it ended up.
/// `label` names the profile in errors.
pub fn move_mod(
    mods: &mut Vec<ProfileMod>,
    label: &str,
    name: &str,
    to: &Position,
) -> Result<usize, Error> {
    let find = |mods: &[ProfileMod], name: &str| {
        mods.iter()
            .position(|m| m.name == name)
            .ok_or_else(|| no_such_mod(label, name))
    };
    let from = find(mods, name)?;
    if let Position::Before(other) | Position::After(other) = to {
        find(mods, other)?;
        if other == name {
            return Ok(from);
        }
    }
    let moving = mods.remove(from);
    let index = match to {
        Position::Top => 0,
        Position::Bottom => mods.len(),
        Position::Index(i) => (*i).min(mods.len()),
        Position::Before(other) => find(mods, other)?,
        Position::After(other) => find(mods, other)? + 1,
    };
    mods.insert(index, moving);
    Ok(index)
}

/// Something mods can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortField {
    /// Alphabetically, ignoring case.
    Name,
    /// By source id, so mods from the same place end up together.
    Source,
    /// By the tags (categories) their source gives them; see [`sort_mods`].
    Tags,
    /// How many mods each one needs, directly or through other mods.
    Dependencies,
    /// How many mods need each one, directly or through other mods.
    Dependents,
    /// When their source last saw a new version, oldest first. Mods it
    /// doesn't know that for go first, or last with `:desc`.
    Updated,
}

impl SortField {
    pub const ALL: [SortField; 6] = [
        SortField::Name,
        SortField::Source,
        SortField::Tags,
        SortField::Dependencies,
        SortField::Dependents,
        SortField::Updated,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SortField::Name => "name",
            SortField::Source => "source",
            SortField::Tags => "tags",
            SortField::Dependencies => "dependencies",
            SortField::Dependents => "dependents",
            SortField::Updated => "updated",
        }
    }
}

/// A field and which way to sort it, written `tags` or `dependents:desc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SortKey {
    pub field: SortField,
    pub descending: bool,
}

impl SortKey {
    /// Whether sorting by `keys` needs each mod's summary from its source,
    /// for [`ModFacts::tags`] or [`ModFacts::updated`].
    pub fn needs_summaries(keys: &[SortKey]) -> bool {
        keys.iter()
            .any(|k| matches!(k.field, SortField::Tags | SortField::Updated))
    }

    pub fn needs_dependencies(keys: &[SortKey]) -> bool {
        keys.iter()
            .any(|k| matches!(k.field, SortField::Dependencies | SortField::Dependents))
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.field.as_str())?;
        if self.descending {
            f.write_str(":desc")?;
        }
        Ok(())
    }
}

impl FromStr for SortKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let (field, direction) = s.split_once(':').unwrap_or((s, "asc"));
        let descending = match direction {
            "asc" => false,
            "desc" => true,
            _ => {
                return Err(Error::invalid(format!(
                    "unknown direction `{direction}`; expected asc or desc"
                )));
            }
        };
        let field = SortField::ALL
            .into_iter()
            .find(|f| f.as_str() == field)
            .ok_or_else(|| {
                let all: Vec<_> = SortField::ALL.iter().map(|f| f.as_str()).collect();
                Error::invalid(format!(
                    "unknown sort `{field}`; expected one of {}",
                    all.join(", ")
                ))
            })?;
        Ok(SortKey { field, descending })
    }
}

/// What sorting knows about a mod beyond its entry in the profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModFacts {
    pub tags: Vec<String>,
    pub dependencies: usize,
    pub dependents: usize,
    pub updated: Option<Timestamp>,
}

/// Counts every mod in `lock`'s dependencies and dependents, keyed by
/// `source:id`. Each mod is counted once however many paths lead to it, and
/// cycles don't count a mod as its own dependency.
pub fn dependency_facts(lock: &Lockfile) -> HashMap<String, ModFacts> {
    let deps: HashMap<String, &[String]> = lock
        .mods
        .iter()
        .map(|m| (m.key(), m.dependencies.as_slice()))
        .collect();
    let mut facts: HashMap<String, ModFacts> = HashMap::new();
    for key in deps.keys() {
        let mut reached = HashSet::new();
        let mut stack: Vec<&String> = deps[key].iter().collect();
        while let Some(dep) = stack.pop() {
            if dep != key && reached.insert(dep) {
                stack.extend(deps.get(dep).copied().unwrap_or_default());
            }
        }
        facts.entry(key.clone()).or_default().dependencies = reached.len();
        for dep in reached {
            facts.entry(dep.clone()).or_default().dependents += 1;
        }
    }
    facts
}

/// Sorts `mods` by each key in turn, so later keys only break ties. The
/// sort is stable: mods that tie on every key keep their order. Priorities
/// don't come into it: they apply on top, in [`load_order`].
///
/// `facts` is keyed by `source:id`; mods missing from it have no tags or
/// dependencies. Tags sort by `tag_order` (matched ignoring case) if it's
/// given: each mod goes by whichever of its tags comes first in it, which
/// groups mods by their most important tag. Ties, and mods with none of
/// those tags, go by their tags alphabetically, which groups mods with the
/// same tags. Untagged mods go last.
pub fn sort_mods(
    mods: &mut [ProfileMod],
    keys: &[SortKey],
    tag_order: &[String],
    facts: &HashMap<String, ModFacts>,
) {
    let none = ModFacts::default();
    let facts_of = |m: &ProfileMod| {
        facts
            .get(&crate::lock::mod_key(&m.source, &m.id))
            .unwrap_or(&none)
    };
    let tag_order: Vec<String> = tag_order.iter().map(|t| t.to_lowercase()).collect();
    let tag_key = |m: &ProfileMod| {
        let mut tags: Vec<String> = facts_of(m).tags.iter().map(|t| t.to_lowercase()).collect();
        tags.sort();
        let rank = tags
            .iter()
            .filter_map(|t| tag_order.iter().position(|o| o == t))
            .min()
            .unwrap_or(tag_order.len());
        (rank, tags.is_empty(), tags)
    };
    mods.sort_by(|a, b| {
        keys.iter()
            .map(|key| {
                let ord = match key.field {
                    SortField::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    SortField::Source => a.source.cmp(&b.source),
                    SortField::Tags => tag_key(a).cmp(&tag_key(b)),
                    SortField::Dependencies => {
                        facts_of(a).dependencies.cmp(&facts_of(b).dependencies)
                    }
                    SortField::Dependents => facts_of(a).dependents.cmp(&facts_of(b).dependents),
                    SortField::Updated => facts_of(a).updated.cmp(&facts_of(b).updated),
                };
                if key.descending { ord.reverse() } else { ord }
            })
            .find(|ord| ord.is_ne())
            .unwrap_or(Ordering::Equal)
    });
}

#[cfg(test)]
mod tests;
