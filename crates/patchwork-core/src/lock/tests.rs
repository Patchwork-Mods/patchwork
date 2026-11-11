use super::*;
use crate::source::{Need, SoftDependency, compare_dotted};

fn locked(id: &str, version: &str, requested: bool, requires: &[(&str, &str)]) -> LockedMod {
    LockedMod {
        name: id.to_owned(),
        source: "thunderstore".to_owned(),
        id: id.to_owned(),
        requested: requested.then(|| version.to_owned()),
        version: Some(version.to_owned()),
        hash: ContentHash::of(id.as_bytes()),
        format: ArtifactFormat::Archive,
        artifact: ArtifactLocation::Url {
            url: format!("https://example.com/{id}.zip"),
        },
        dependencies: requires.iter().map(|(k, _)| k.to_string()).collect(),
        soft_dependencies: Vec::new(),
        requires: requires
            .iter()
            .map(|(k, r)| (k.to_string(), r.parse().unwrap()))
            .collect(),
        game_versions: None,
        env: Default::default(),
        kind: None,
        replaced: Default::default(),
    }
}

fn lockfile(mods: Vec<LockedMod>) -> Lockfile {
    Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: Default::default(),
            id: "lethal-company".to_owned(),
        },
        mods,
    }
}

#[test]
fn finds_unmet_requirements() {
    let lock = lockfile(vec![
        locked("Old", "1.0.0", true, &[("thunderstore:Lib", ">=1.0")]),
        locked("New", "3.0.0", false, &[("thunderstore:Lib", ">=2.0")]),
        locked("Picky", "1.0.0", false, &[("thunderstore:Lib", "=1.0")]),
        locked("Lib", "1.5", true, &[]),
    ]);
    let unmet = lock.unmet_requirements(|_, a, b| compare_dotted(a, b));
    let names: Vec<_> = unmet.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["New", "Picky"]);
    assert_eq!(unmet[0].dependency_key, "thunderstore:Lib");
    assert_eq!(
        unmet[0].to_string(),
        "`New` needs `Lib` 2.0 or newer, but the profile pins it to 1.5"
    );
}

#[test]
fn undecidable_requirements_are_assumed_met() {
    let lock = lockfile(vec![
        locked("Mod", "1.0", false, &[("thunderstore:Lib", ">=2.0")]),
        locked("Lib", "1.0beta", false, &[]),
    ]);
    assert!(
        lock.unmet_requirements(|_, a, b| compare_dotted(a, b))
            .is_empty()
    );
}

#[test]
fn requirements_round_trip_through_toml() {
    let lock = lockfile(vec![locked(
        "Mod",
        "1.0",
        false,
        &[("thunderstore:BepInEx-BepInExPack", ">=5.4.2100")],
    )]);
    let text = toml::to_string(&lock).unwrap();
    assert!(
        text.contains(r#""thunderstore:BepInEx-BepInExPack" = ">=5.4.2100""#),
        "{text}"
    );
    let back: Lockfile = toml::from_str(&text).unwrap();
    assert_eq!(back.mods[0].requires, lock.mods[0].requires);
}

#[test]
fn soft_dependencies_say_what_the_profile_has() {
    let soft = |key: &str, need| SoftDependency {
        key: Some(key.to_owned()),
        name: None,
        need,
        notes: None,
        url: None,
    };
    let mut app = locked("app", "1.0", false, &[]);
    app.soft_dependencies = vec![
        soft("thunderstore:added", Need::Unknown),
        soft("thunderstore:dep", Need::Unknown),
        soft("thunderstore:forked", Need::Optional),
        soft("thunderstore:ignored", Need::Unknown),
        soft("thunderstore:missing", Need::Unknown),
    ];
    let mut dep = locked("dep", "1.0", false, &[]);
    dep.name = "Dep".to_owned();
    let lock = lockfile(vec![app, dep]);
    let profile: crate::profile::Profile = toml::from_str(
        r#"
        schema = 3
        id = "8f0b6a1e-6a55-4d5c-9b4b-3c1f0e2d7a10"
        game = "lethal-company"
        name = "default"

        [[mod]]
        name = "app"
        source = "thunderstore"
        id = "app"
        ignores = ["thunderstore:ignored"]

        [[mod]]
        name = "Added"
        source = "thunderstore"
        id = "added"

        [[mod]]
        name = "Fork"
        source = "thunderstore"
        id = "fork"
        replaces = ["thunderstore:forked"]
        "#,
    )
    .unwrap();

    let info = lock.soft_dependencies(&profile, "thunderstore:app");
    let have: Vec<(&str, Option<&str>, bool)> = info
        .iter()
        .map(|i| (i.ident.as_str(), i.installed.as_deref(), i.missing()))
        .collect();
    assert_eq!(
        have,
        [
            ("thunderstore:added", Some("Added"), false),
            ("thunderstore:dep", Some("Dep"), false),
            ("thunderstore:forked", Some("Fork"), false),
            ("thunderstore:ignored", None, false),
            ("thunderstore:missing", None, true),
        ]
    );
    assert!(info[3].ignored);

    // and they survive a round trip
    let text = toml::to_string(&lock).unwrap();
    let back: Lockfile = toml::from_str(&text).unwrap();
    assert_eq!(
        back.mods[0].soft_dependencies,
        lock.mods[0].soft_dependencies
    );
    assert!(text.contains(r#"need = "unknown""#), "{text}");
}

#[test]
fn is_key_matches_mod_key() {
    for (source, id) in [
        ("thunderstore", "a-b"),
        ("local", ""),
        ("a:b", "c"),
        ("a", "b:c"),
    ] {
        let key = mod_key(source, id);
        assert!(is_key(source, id, &key));
    }
    assert!(!is_key("a", "b:c", "a:b:d"));
    assert!(!is_key("a:b", "c", "a:bc"));
    assert!(!is_key("nexus", "1", "nexus1"));
    assert!(!is_key("nexus", "1", "nexus:12"));
}
