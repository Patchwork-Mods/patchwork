//! Rearranging a profile's mods. Their order is the load order: later mods
//! win when two provide the same file. Dependencies are always deployed
//! before the mods that need them whatever the order, so no rearrangement
//! can break a mod by putting it above what it needs.
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
/// sort is stable: mods that tie on every key keep their order.
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
mod tests {
    use super::*;
    use crate::hash::ContentHash;
    use crate::lock::{LOCK_SCHEMA, LockedGame, LockedMod};
    use crate::source::{ArtifactFormat, ArtifactLocation};

    fn profile_mod(name: &str) -> ProfileMod {
        ProfileMod {
            name: name.to_owned(),
            source: "ts".to_owned(),
            id: name.to_owned(),
            version: None,
            enabled: true,
        }
    }

    fn mods(names: &[&str]) -> Vec<ProfileMod> {
        names.iter().map(|n| profile_mod(n)).collect()
    }

    fn names(mods: &[ProfileMod]) -> Vec<&str> {
        mods.iter().map(|m| m.name.as_str()).collect()
    }

    fn keys(text: &[&str]) -> Vec<SortKey> {
        text.iter().map(|k| k.parse().unwrap()).collect()
    }

    fn tagged(entries: &[(&str, &[&str])]) -> HashMap<String, ModFacts> {
        entries
            .iter()
            .map(|(id, tags)| {
                let facts = ModFacts {
                    tags: tags.iter().map(|t| t.to_string()).collect(),
                    ..Default::default()
                };
                (format!("ts:{id}"), facts)
            })
            .collect()
    }

    #[test]
    fn moves_mods() {
        let mut list = mods(&["a", "b", "c", "d"]);
        assert_eq!(move_mod(&mut list, "p", "d", &Position::Top).unwrap(), 0);
        assert_eq!(names(&list), ["d", "a", "b", "c"]);
        move_mod(&mut list, "p", "d", &Position::Bottom).unwrap();
        assert_eq!(names(&list), ["a", "b", "c", "d"]);
        move_mod(&mut list, "p", "a", &Position::After("c".to_owned())).unwrap();
        assert_eq!(names(&list), ["b", "c", "a", "d"]);
        move_mod(&mut list, "p", "d", &Position::Before("b".to_owned())).unwrap();
        assert_eq!(names(&list), ["d", "b", "c", "a"]);
        assert_eq!(
            move_mod(&mut list, "p", "d", &Position::Index(2)).unwrap(),
            2
        );
        assert_eq!(names(&list), ["b", "c", "d", "a"]);
        assert_eq!(
            move_mod(&mut list, "p", "b", &Position::Index(99)).unwrap(),
            3
        );
        assert_eq!(names(&list), ["c", "d", "a", "b"]);
        move_mod(&mut list, "p", "a", &Position::After("a".to_owned())).unwrap();
        assert_eq!(names(&list), ["c", "d", "a", "b"]);
    }

    #[test]
    fn moving_unknown_mods_fails_without_changing_anything() {
        let mut list = mods(&["a", "b"]);
        assert!(move_mod(&mut list, "p", "x", &Position::Top).is_err());
        let err = move_mod(&mut list, "p", "a", &Position::After("x".to_owned())).unwrap_err();
        assert!(err.to_string().contains("`x`"), "{err}");
        assert_eq!(names(&list), ["a", "b"]);
    }

    #[test]
    fn sort_keys_parse() {
        for field in SortField::ALL {
            let key: SortKey = field.as_str().parse().unwrap();
            assert_eq!(key.to_string(), field.as_str());
        }
        let key: SortKey = "dependents:desc".parse().unwrap();
        assert_eq!(key.field, SortField::Dependents);
        assert!(key.descending);
        assert_eq!(key.to_string(), "dependents:desc");
        assert!("size".parse::<SortKey>().is_err());
        assert!("name:up".parse::<SortKey>().is_err());
    }

    #[test]
    fn sorts_by_name_stably_and_by_later_keys() {
        let mut list = mods(&["b", "C", "a"]);
        sort_mods(&mut list, &keys(&["name"]), &[], &HashMap::new());
        assert_eq!(names(&list), ["a", "b", "C"]);
        sort_mods(&mut list, &keys(&["name:desc"]), &[], &HashMap::new());
        assert_eq!(names(&list), ["C", "b", "a"]);

        let mut list = mods(&["b", "a", "c"]);
        list[2].source = "local".to_owned();
        sort_mods(&mut list, &keys(&["source"]), &[], &HashMap::new());
        assert_eq!(names(&list), ["c", "b", "a"]);
        sort_mods(&mut list, &keys(&["source", "name"]), &[], &HashMap::new());
        assert_eq!(names(&list), ["c", "a", "b"]);
    }

    #[test]
    fn groups_mods_by_tags_with_untagged_last() {
        let facts = tagged(&[
            ("suit", &["Cosmetics", "Suits"]),
            ("moon", &["Moons"]),
            ("hat", &["cosmetics"]),
            ("tweak", &["Tweaks"]),
        ]);
        let mut list = mods(&["plain", "tweak", "suit", "moon", "hat"]);
        sort_mods(&mut list, &keys(&["tags"]), &[], &facts);
        assert_eq!(names(&list), ["hat", "suit", "moon", "tweak", "plain"]);
    }

    #[test]
    fn tag_order_picks_each_mods_most_important_tag() {
        let facts = tagged(&[
            ("suit", &["Cosmetics", "Suits"]),
            ("moon", &["Moons"]),
            ("hat", &["Cosmetics"]),
            ("tweak", &["Tweaks"]),
            ("lib", &["Libraries"]),
        ]);
        let mut list = mods(&["plain", "hat", "tweak", "suit", "moon", "lib"]);
        let order = [
            "suits".to_owned(),
            "moons".to_owned(),
            "cosmetics".to_owned(),
        ];
        sort_mods(&mut list, &keys(&["tags"]), &order, &facts);
        assert_eq!(
            names(&list),
            ["suit", "moon", "hat", "lib", "tweak", "plain"]
        );
    }

    #[test]
    fn sorts_by_when_mods_were_updated_with_unknown_last_when_descending() {
        let at = |s: &str| ModFacts {
            updated: Some(s.parse().unwrap()),
            ..Default::default()
        };
        let facts = HashMap::from([
            ("ts:old".to_owned(), at("2025-01-01T00:00:00Z")),
            ("ts:new".to_owned(), at("2026-06-01T00:00:00Z")),
            ("ts:mid".to_owned(), at("2026-01-01T00:00:00Z")),
        ]);
        let mut list = mods(&["unknown", "old", "new", "mid"]);
        sort_mods(&mut list, &keys(&["updated:desc"]), &[], &facts);
        assert_eq!(names(&list), ["new", "mid", "old", "unknown"]);
        sort_mods(&mut list, &keys(&["updated"]), &[], &facts);
        assert_eq!(names(&list), ["unknown", "old", "mid", "new"]);
    }

    fn lock(entries: &[(&str, &[&str])]) -> Lockfile {
        let mods = entries
            .iter()
            .map(|(id, deps)| LockedMod {
                name: id.to_string(),
                source: "ts".to_owned(),
                id: id.to_string(),
                requested: None,
                version: None,
                hash: ContentHash::of(id.as_bytes()),
                format: ArtifactFormat::Zip,
                artifact: ArtifactLocation::Url {
                    url: format!("https://example.com/{id}.zip"),
                },
                dependencies: deps.iter().map(|d| format!("ts:{d}")).collect(),
                requires: Default::default(),
            })
            .collect();
        Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: "game".to_owned(),
                definition: ContentHash::of(b"game"),
            },
            mods,
        }
    }

    #[test]
    fn counts_whole_dependency_trees_once() {
        // app -> {ui, net}; ui -> core; net -> core; core -> loader
        let facts = dependency_facts(&lock(&[
            ("app", &["ui", "net"]),
            ("ui", &["core"]),
            ("net", &["core"]),
            ("core", &["loader"]),
            ("loader", &[]),
        ]));
        let count = |id: &str| {
            let f = &facts[&format!("ts:{id}")];
            (f.dependencies, f.dependents)
        };
        assert_eq!(count("app"), (4, 0));
        assert_eq!(count("ui"), (2, 1));
        assert_eq!(count("core"), (1, 3));
        assert_eq!(count("loader"), (0, 4));
    }

    #[test]
    fn dependency_cycles_end() {
        let facts = dependency_facts(&lock(&[("a", &["b"]), ("b", &["a"]), ("c", &["missing"])]));
        assert_eq!(facts["ts:a"].dependencies, 1);
        assert_eq!(facts["ts:a"].dependents, 1);
        assert_eq!(facts["ts:c"].dependencies, 1);
    }

    #[test]
    fn sorts_by_dependents_with_names_breaking_ties() {
        let facts = dependency_facts(&lock(&[
            ("app", &["core"]),
            ("tool", &["core"]),
            ("core", &["loader"]),
            ("loader", &[]),
        ]));
        let mut list = mods(&["tool", "core", "app", "loader"]);
        sort_mods(&mut list, &keys(&["dependents:desc", "name"]), &[], &facts);
        assert_eq!(names(&list), ["loader", "core", "app", "tool"]);
    }
}
