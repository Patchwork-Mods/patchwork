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
        priority: 0,
        game_versions: None,
        ext: Default::default(),
        replaces: Vec::new(),
        seen: None,
        ignores: Vec::new(),
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
            format: ArtifactFormat::Archive,
            artifact: ArtifactLocation::Url {
                url: format!("https://example.com/{id}.zip"),
            },
            dependencies: deps.iter().map(|d| format!("ts:{d}")).collect(),
            soft_dependencies: Vec::new(),
            requires: Default::default(),
            game_versions: None,
            env: Default::default(),
            kind: None,
            replaced: Default::default(),
        })
        .collect();
    Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: Default::default(),
            id: "game".to_owned(),
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

fn with_priorities(entries: &[(&str, i32)]) -> Vec<ProfileMod> {
    entries
        .iter()
        .map(|(name, priority)| ProfileMod {
            priority: *priority,
            ..profile_mod(name)
        })
        .collect()
}

#[test]
fn higher_priorities_load_later_keeping_ties_in_order() {
    let list = with_priorities(&[("a", 1), ("b", 0), ("c", -2), ("d", 0), ("e", 1)]);
    let order: Vec<&str> = load_order(&list).iter().map(|m| m.name.as_str()).collect();
    assert_eq!(order, ["c", "b", "d", "a", "e"]);
}

#[test]
fn priorities_dont_limit_moving_or_sorting() {
    let mut list = with_priorities(&[("a", 0), ("b", 1), ("c", 0)]);
    move_mod(&mut list, "p", "a", &Position::Bottom).unwrap();
    assert_eq!(names(&list), ["b", "c", "a"]);
    sort_mods(&mut list, &keys(&["name:desc"]), &[], &HashMap::new());
    assert_eq!(names(&list), ["c", "b", "a"]);
}
