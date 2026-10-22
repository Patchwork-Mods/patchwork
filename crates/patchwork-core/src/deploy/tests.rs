use super::*;
use crate::event::Silent;

struct Env {
    _tmp: tempfile::TempDir,
    game: PathBuf,
    store: PathBuf,
    backups: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("game");
    let store = tmp.path().join("store");
    fs::create_dir_all(game.join("Data")).unwrap();
    fs::create_dir_all(&store).unwrap();
    fs::write(game.join("Data/vanilla.txt"), "vanilla").unwrap();
    Env {
        backups: tmp.path().join("backups"),
        game,
        store,
        _tmp: tmp,
    }
}

fn placement(env: &Env, dst: &str, contents: &str, mod_name: &str) -> Resolved {
    let src = env
        .store
        .join(format!("{mod_name}-{}", dst.replace('/', "_")));
    fs::write(&src, contents).unwrap();
    Resolved {
        dst: RelPath::new(dst).unwrap(),
        src,
        mod_name: mod_name.to_owned(),
    }
}

fn deploy(env: &Env, state: &mut DeployState, placements: Vec<Resolved>) -> DeployPlan {
    let plan = plan(
        DeployTarget::Game("g".into()),
        state,
        placements,
        Vec::new(),
        None,
        LinkMethod::Copy,
    );
    apply(&plan, state, &env.backups, &Silent).unwrap();
    plan
}

#[test]
fn deploy_and_undeploy_round_trip() {
    let env = env();
    let mut state = DeployState::empty("g", &env.game);
    let placements = vec![
        placement(&env, "Mods/a/a.dll", "a", "a"),
        placement(&env, "Data/vanilla.txt", "modded", "a"),
    ];
    deploy(&env, &mut state, placements.clone());

    assert_eq!(
        fs::read_to_string(env.game.join("Mods/a/a.dll")).unwrap(),
        "a"
    );
    assert_eq!(
        fs::read_to_string(env.game.join("Data/vanilla.txt")).unwrap(),
        "modded"
    );

    // Deploying the same thing again does nothing.
    let again = deploy(&env, &mut state, placements);
    assert!(again.is_noop());
    assert_eq!(again.unchanged, 2);

    deploy(&env, &mut state, Vec::new());
    assert!(state.files.is_empty() && state.created_dirs.is_empty());
    assert!(!env.game.join("Mods").exists());
    assert_eq!(
        fs::read_to_string(env.game.join("Data/vanilla.txt")).unwrap(),
        "vanilla"
    );
}

#[test]
fn later_mods_win_conflicts() {
    let env = env();
    let mut state = DeployState::empty("g", &env.game);
    let plan = deploy(
        &env,
        &mut state,
        vec![
            placement(&env, "x.cfg", "first", "a"),
            placement(&env, "x.cfg", "second", "b"),
        ],
    );
    assert_eq!(plan.warnings.len(), 1);
    assert_eq!(
        fs::read_to_string(env.game.join("x.cfg")).unwrap(),
        "second"
    );
    assert_eq!(state.files[&RelPath::new("x.cfg").unwrap()].mod_name, "b");
}

#[test]
fn modified_files_are_left_alone() {
    let env = env();
    let mut state = DeployState::empty("g", &env.game);
    deploy(&env, &mut state, vec![placement(&env, "x.cfg", "mod", "a")]);
    fs::write(env.game.join("x.cfg"), "user edits").unwrap();
    deploy(&env, &mut state, Vec::new());
    assert_eq!(
        fs::read_to_string(env.game.join("x.cfg")).unwrap(),
        "user edits"
    );
}

#[test]
fn game_files_see_the_game_without_deployed_mods() {
    let env = env();
    fs::write(env.game.join("Data/other.txt"), "other").unwrap();
    let mut state = DeployState::empty("g", &env.game);
    deploy(
        &env,
        &mut state,
        vec![
            placement(&env, "Data/vanilla.txt", "modded", "a"),
            placement(&env, "Data/added.txt", "added", "a"),
        ],
    );
    let game = GameFiles::new(&env.game, &state);
    let rel = |p: &str| RelPath::new(p).unwrap();

    let original = game.original(&rel("Data/vanilla.txt")).unwrap();
    assert_eq!(fs::read_to_string(original).unwrap(), "vanilla");
    assert_eq!(
        game.original(&rel("Data/other.txt")),
        Some(env.game.join("Data/other.txt"))
    );
    assert_eq!(game.original(&rel("Data/added.txt")), None);
    assert_eq!(game.original(&rel("Data/missing.txt")), None);

    assert_eq!(
        game.list(Some(&rel("Data"))).unwrap(),
        vec![rel("Data/other.txt"), rel("Data/vanilla.txt")]
    );
    assert!(game.list(None).unwrap().is_empty());
    assert!(game.list(Some(&rel("Missing"))).unwrap().is_empty());
}

#[test]
fn files_changed_since_deploying_become_the_new_original() {
    let env = env();
    let mut state = DeployState::empty("g", &env.game);
    deploy(
        &env,
        &mut state,
        vec![placement(&env, "Data/vanilla.txt", "modded", "a")],
    );
    // a game update puts its own file back
    fs::write(env.game.join("Data/vanilla.txt"), "updated").unwrap();
    let rel = RelPath::new("Data/vanilla.txt").unwrap();
    let original = GameFiles::new(&env.game, &state).original(&rel).unwrap();
    assert_eq!(fs::read_to_string(original).unwrap(), "updated");

    deploy(
        &env,
        &mut state,
        vec![placement(&env, "Data/vanilla.txt", "modded again", "b")],
    );
    let read = |p: &str| fs::read_to_string(env.game.join(p)).unwrap();
    assert_eq!(read("Data/vanilla.txt"), "modded again");
    deploy(&env, &mut state, Vec::new());
    assert_eq!(read("Data/vanilla.txt"), "updated");
}

#[test]
fn placements_can_only_come_from_mods_and_builds() {
    let env = env();
    let rel = |p: &str| RelPath::new(p).unwrap();
    let src = env.store.join("a.dll");
    fs::write(&src, "a").unwrap();
    let m = InstalledMod {
        name: "a".to_owned(),
        source: "local".to_owned(),
        id: "a".to_owned(),
        version: None,
        hash: ContentHash::of(b"a"),
        files: vec![ModFile {
            rel: rel("a.dll"),
            src: src.clone(),
        }],
    };
    let out = env.store.join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("built.txt"), "built").unwrap();
    let place = |src: Src| Placement {
        dst: rel("Mods/x"),
        src,
        mod_name: "a".to_owned(),
    };
    let resolve = |src: Src, out: Option<&Path>| {
        resolve("test", vec![place(src)], std::slice::from_ref(&m), out)
            .map(|resolved| resolved[0].src.clone())
    };

    assert_eq!(resolve(m.src(&m.files[0]), None).unwrap(), src);
    assert_eq!(
        resolve(Src::Built(rel("built.txt")), Some(&out)).unwrap(),
        out.join("built.txt")
    );
    // a file the mod doesn't have, or a mod that isn't being deployed
    let other = Src::Mod {
        hash: m.hash,
        rel: rel("b.dll"),
    };
    assert!(resolve(other, None).is_err());
    let other = Src::Mod {
        hash: ContentHash::of(b"b"),
        rel: rel("a.dll"),
    };
    assert!(resolve(other, None).is_err());
    // built files only from a build, and only ones it made
    assert!(resolve(Src::Built(rel("built.txt")), None).is_err());
    assert!(resolve(Src::Built(rel("missing.txt")), Some(&out)).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&src, out.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&env.store, out.join("dir")).unwrap();
        assert!(resolve(Src::Built(rel("link.txt")), Some(&out)).is_err());
        assert!(resolve(Src::Built(rel("dir/a.dll")), Some(&out)).is_err());
    }
}
