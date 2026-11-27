use super::*;
use crate::source::{Need, VersionReq};

fn game(provided: &[&str]) -> GameDef {
    let mut game: GameDef = toml::from_str(
        r#"
        schema = 1
        id = "test"
        name = "Test"
        [sources.thunderstore]
        community = "test"
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap();
    game.provided = provided.iter().map(|p| p.to_string()).collect();
    game
}

fn soft(key: Option<&str>, name: Option<&str>) -> SoftDependency {
    SoftDependency {
        key: key.map(str::to_owned),
        name: name.map(str::to_owned),
        need: Need::Unknown,
        notes: None,
        url: None,
    }
}

#[test]
fn drops_what_the_game_provides() {
    let registry = Registry::default();
    let provided = Provided::of(&registry, &game(&["nexus:200", "simple mod framework*"])).unwrap();
    let dep = |source: &str, id: &str| Dependency {
        source: source.to_owned(),
        id: id.to_owned(),
        req: VersionReq::Any,
    };
    let mut deps = vec![dep("nexus", "200"), dep("nexus", "2000")];
    let mut softs = vec![
        soft(Some("nexus:200"), None),
        soft(None, Some("Simple Mod Framework 2")),
        soft(Some("nexus:12"), Some("ZHMModSDK")),
    ];
    provided.drop_provided(&mut deps, &mut softs);
    assert_eq!(deps, vec![dep("nexus", "2000")]);
    assert_eq!(softs, vec![soft(Some("nexus:12"), Some("ZHMModSDK"))]);
    assert!(
        Provided::default()
            .alike_in(&softs[0], ["nexus:12"].into_iter())
            .is_none()
    );
}

#[test]
fn alike_mods_stand_in_for_each_other() {
    let mut registry = Registry::default();
    registry.add_data(
        HIDDEN_KIND,
        "thunderstore",
        r#"alike = [["*-BepInExPack", "*-BepInExPack_*"]]"#,
    );
    let provided = Provided::of(&registry, &game(&[])).unwrap();
    let keys = [
        "thunderstore:someone-Mod",
        "thunderstore:bbepis-BepInExPack",
    ];
    let other = soft(Some("thunderstore:BepInEx-BepInExPack"), None);
    assert_eq!(
        provided.alike_in(&other, keys.into_iter()),
        Some("thunderstore:bbepis-BepInExPack")
    );
    // by name, for ones the source can't get
    let named = soft(None, Some("BepInEx-BepInExPack_PEAK"));
    assert_eq!(
        provided.alike_in(&named, keys.into_iter()),
        Some("thunderstore:bbepis-BepInExPack")
    );
    // not itself, nor anything outside the group, nor another source
    let itself = soft(Some("thunderstore:bbepis-BepInExPack"), None);
    assert_eq!(provided.alike_in(&itself, keys.into_iter()), None);
    let unrelated = soft(Some("thunderstore:someone-Other"), None);
    assert_eq!(provided.alike_in(&unrelated, keys.into_iter()), None);
    let elsewhere = soft(Some("nexus:BepInEx-BepInExPack"), None);
    assert_eq!(provided.alike_in(&elsewhere, keys.into_iter()), None);
}

#[test]
fn builtin_alike_groups_load() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::load(&crate::paths::Paths::rooted(tmp.path())).unwrap();
    for source in registry.data_ids(HIDDEN_KIND) {
        alike_of(&registry, source).unwrap();
    }
    let games: Vec<GameDef> = registry.games().map(|g| g.value.clone()).collect();
    for game in &games {
        Provided::of(&registry, game).unwrap();
    }
}
