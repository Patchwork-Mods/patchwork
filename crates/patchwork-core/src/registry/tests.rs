use super::*;

#[test]
fn builtins_load_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::load(&Paths::rooted(tmp.path())).unwrap();
    assert!(registry.problems.is_empty(), "{:?}", registry.problems);
    assert_eq!(
        registry.game("lethal-company").unwrap().origin,
        Origin::Builtin
    );
    assert!(registry.data("rulesets", "bepinex").is_some());
}

#[test]
fn extra_registry_dirs_override_builtins_but_not_user() {
    let tmp = tempfile::tempdir().unwrap();
    let mut paths = Paths::rooted(tmp.path());
    let dev = tmp.path().join("dev");
    let missing = tmp.path().join("missing");
    paths.extra_registries = vec![dev.clone(), missing];

    let def = |name: &str, id: &str| {
        format!("schema = 1\nid = \"{id}\"\nname = \"{name}\"\n[deployer]\nkind = \"rules\"\n")
    };
    fs::create_dir_all(dev.join("games")).unwrap();
    fs::write(
        dev.join("games/lethal-company.toml"),
        def("Dev", "lethal-company"),
    )
    .unwrap();
    fs::write(dev.join("games/new-game.toml"), def("New", "new-game")).unwrap();
    let user_games = paths.user_registry().join("games");
    fs::create_dir_all(&user_games).unwrap();
    fs::write(user_games.join("new-game.toml"), def("Mine", "new-game")).unwrap();

    let registry = Registry::load(&paths).unwrap();
    let lc = registry.game("lethal-company").unwrap();
    assert_eq!(lc.origin, Origin::Dir(dev));
    assert_eq!(lc.value.name, "Dev");
    assert_eq!(registry.game("new-game").unwrap().origin, Origin::User);
    assert_eq!(registry.problems.len(), 1, "{:?}", registry.problems);
}

#[test]
fn layers_override_and_problems_are_collected() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    let games = paths.user_registry().join("games");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("lethal-company.toml"),
        "schema = 1\nid = \"lethal-company\"\nname = \"Mine\"\n[deployer]\nkind = \"rules\"\n",
    )
    .unwrap();
    fs::write(games.join("future.toml"), "schema = 99\nid = \"future\"\n").unwrap();
    fs::write(games.join("typo.toml"), "schema = 1\nid = \"other\"\n").unwrap();

    let registry = Registry::load(&paths).unwrap();
    let lc = registry.game("lethal-company").unwrap();
    assert_eq!(lc.origin, Origin::User);
    assert_eq!(lc.value.name, "Mine");
    assert_eq!(registry.problems.len(), 2);
    assert!(matches!(
        registry.problems[0],
        Error::Schema { found: 99, .. }
    ));
}
