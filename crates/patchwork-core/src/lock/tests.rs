use super::*;
use crate::source::compare_dotted;

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
        requires: requires
            .iter()
            .map(|(k, r)| (k.to_string(), r.parse().unwrap()))
            .collect(),
        game_versions: None,
        env: Default::default(),
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
