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
    assert_eq!(lc.origin, Origin::Builtin);
    assert_eq!(lc.overlays, [Origin::Dir(dev)]);
    assert_eq!(lc.value.name, "Dev");
    let new_game = registry.game("new-game").unwrap();
    assert_eq!(new_game.overlays, [Origin::User]);
    assert_eq!(new_game.value.name, "Mine");
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
    assert_eq!(lc.origins(), "builtin + user");
    assert_eq!(lc.value.name, "Mine");
    assert_eq!(registry.problems.len(), 2);
    assert!(matches!(
        registry.problems[0],
        Error::Schema { found: 99, .. }
    ));
}

fn user_file(paths: &Paths, kind: &str, id: &str, text: &str) -> std::path::PathBuf {
    let dir = paths.user_registry().join(kind);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.toml"));
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn overlays_change_only_what_they_set() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    let builtin = Registry::load(&paths).unwrap();
    let original = builtin.game("lethal-company").unwrap();

    user_file(
        &paths,
        "games",
        "lethal-company",
        "description = \"Mine\"\nimage = \"art/lc.png\"\n",
    );
    let registry = Registry::load(&paths).unwrap();
    assert!(registry.problems.is_empty(), "{:?}", registry.problems);
    let lc = registry.game("lethal-company").unwrap();
    assert_eq!(lc.value.description.as_deref(), Some("Mine"));
    assert_eq!(lc.value.name, original.value.name);
    assert_eq!(lc.value.sources, original.value.sources);
    assert_eq!(lc.value.deployer.config, original.value.deployer.config);
    let image = paths.user_registry().join("games/art/lc.png");
    assert_eq!(lc.value.image.as_deref(), image.to_str());
    // Display-only changes don't make locked profiles look out of date.
    assert_eq!(lc.hash, original.hash);

    user_file(
        &paths,
        "games",
        "lethal-company",
        "[sources.thunderstore]\ncommunity = \"elsewhere\"\n",
    );
    let registry = Registry::load(&paths).unwrap();
    let lc = registry.game("lethal-company").unwrap();
    assert_eq!(
        lc.value.sources["thunderstore"]["community"].as_str(),
        Some("elsewhere")
    );
    assert_eq!(lc.value.deployer.kind, original.value.deployer.kind);
    assert_ne!(lc.hash, original.hash);
}

#[test]
fn broken_overlays_are_reported_and_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    user_file(&paths, "games", "lethal-company", "link = \"teleport\"\n");
    // Not an overlay: nothing defines this game, so it has to be complete.
    user_file(&paths, "games", "partial", "description = \"?\"\n");

    let registry = Registry::load(&paths).unwrap();
    assert_eq!(registry.problems.len(), 2, "{:?}", registry.problems);
    let lc = registry.game("lethal-company").unwrap();
    assert_eq!(lc.origin, Origin::Builtin);
    assert!(lc.overlays.is_empty());
    assert!(registry.game("partial").is_err());
}

#[test]
fn data_entries_merge_too() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    let before = Registry::load(&paths).unwrap();
    let before = before.data("rulesets", "bepinex").unwrap();

    user_file(&paths, "rulesets", "bepinex", "note = \"mine\"\n");
    let registry = Registry::load(&paths).unwrap();
    let after = registry.data("rulesets", "bepinex").unwrap();
    assert_eq!(after.value["rules"], before.value["rules"]);
    assert_eq!(after.value["note"].as_str(), Some("mine"));
    assert_eq!(after.overlays, [Origin::User]);
    assert_ne!(after.hash, before.hash);
}

#[test]
fn the_old_plugins_table_is_the_bethesda_load_orders() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    // the builtin one has moved; an overlay in the old place still counts
    user_file(
        &paths,
        "games",
        "skyrim-se",
        "[deployer.plugins]\nlimit = 10\n",
    );
    let registry = Registry::load(&paths).unwrap();
    assert!(registry.problems.is_empty(), "{:?}", registry.problems);
    let def = &registry.game("skyrim-se").unwrap().value;
    assert!(def.deployer.config.get("plugins").is_none());
    let plugins = def.hook_config("bethesda", "plugins").unwrap();
    assert_eq!(plugins["limit"].as_integer(), Some(10));
    assert_eq!(plugins["file"].as_str(), Some("appdata:plugins.txt"));
    assert!(def.hook_on("bethesda", "plugins"));
}

#[test]
fn the_old_fomod_table_is_the_fomod_installers() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    user_file(
        &paths,
        "games",
        "skyrim-se",
        "[fomod]\nfolder = \"Data/x\"\n",
    );
    let registry = Registry::load(&paths).unwrap();
    assert!(registry.problems.is_empty(), "{:?}", registry.problems);
    let def = &registry.game("skyrim-se").unwrap().value;
    let installer = def.hook_config("fomod", "installer").unwrap();
    assert_eq!(installer["folder"].as_str(), Some("Data/x"));
    assert!(def.hook_on("fomod", "installer"));
}
