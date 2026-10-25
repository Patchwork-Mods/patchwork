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
        dst: dst.parse().unwrap(),
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
    )
    .unwrap();
    apply(&plan, state, &env.backups, &Silent).unwrap();
    plan
}

#[test]
fn deploy_and_undeploy_round_trip() {
    let env = env();
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
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
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
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
    assert_eq!(state.files[&"x.cfg".parse::<Dest>().unwrap()].mod_name, "b");
}

#[test]
fn modified_files_are_left_alone() {
    let env = env();
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
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
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
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
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
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
        dst: Dest::game(rel("Mods/x")),
        src,
        mod_name: "a".to_owned(),
    };
    let resolve = |src: Src, out: Option<&Path>| {
        let roots = Roots::game(&env.game);
        resolve(
            "test",
            vec![place(src)],
            std::slice::from_ref(&m),
            out,
            &roots,
        )
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

#[test]
fn dests_name_a_root_unless_its_the_game() {
    let dest = |s: &str| {
        s.parse::<Dest>()
            .map(|d| (d.root.to_string(), d.to_string()))
    };
    assert_eq!(dest("Data/a.esp"), Ok(("game".into(), "Data/a.esp".into())));
    assert_eq!(
        dest("appdata:plugins.txt"),
        Ok(("appdata".into(), "appdata:plugins.txt".into()))
    );
    assert_eq!(
        dest("game:Data/a.esp"),
        Ok(("game".into(), "Data/a.esp".into()))
    );
    for bad in [
        "AppData:x",
        ":x",
        "appdata:",
        "appdata:../x",
        "a:b:c",
        "/abs",
    ] {
        assert!(dest(bad).is_err(), "{bad}");
    }
}

/// The game, plus an AppData folder that already has the game's own
/// `plugins.txt` in it.
fn with_appdata(env: &Env) -> Roots {
    let appdata = env.store.parent().unwrap().join("appdata");
    fs::create_dir_all(&appdata).unwrap();
    fs::write(appdata.join("plugins.txt"), "original").unwrap();
    Roots::game(&env.game).with(RootId::new("appdata").unwrap(), &appdata)
}

#[test]
fn files_can_go_in_other_roots() {
    let env = env();
    let roots = with_appdata(&env);
    let appdata = roots
        .get(&RootId::new("appdata").unwrap())
        .unwrap()
        .to_owned();
    let mut state = DeployState::empty("g", &roots);
    let plan = deploy(
        &env,
        &mut state,
        vec![
            placement(&env, "appdata:plugins.txt", "modded", "a"),
            placement(&env, "appdata:New/x.txt", "x", "a"),
            placement(&env, "Data/vanilla.txt", "modded", "a"),
        ],
    );
    let ops: Vec<String> = plan
        .ops
        .iter()
        .map(|op| match op {
            DeployOp::Backup { dst } => format!("~ {dst}"),
            DeployOp::Place { dst, .. } => format!("+ {dst}"),
            DeployOp::Remove { dst } => format!("- {dst}"),
        })
        .collect();
    assert_eq!(
        ops,
        [
            "+ appdata:New/x.txt",
            "~ appdata:plugins.txt",
            "+ appdata:plugins.txt",
            "~ Data/vanilla.txt",
            "+ Data/vanilla.txt",
        ]
    );
    let read = |p: PathBuf| fs::read_to_string(p).unwrap();
    assert_eq!(read(appdata.join("plugins.txt")), "modded");
    assert_eq!(read(appdata.join("New/x.txt")), "x");
    // each root's backups are kept apart
    assert_eq!(read(env.backups.join("appdata/plugins.txt")), "original");
    assert_eq!(read(env.backups.join("game/Data/vanilla.txt")), "vanilla");

    // the state says which root each file is in, and where the roots are
    let json = serde_json::to_value(&state).unwrap();
    assert!(json["files"]["appdata:plugins.txt"].is_object());
    assert_eq!(json["roots"]["appdata"], appdata.to_str().unwrap());
    let state_again: DeployState = serde_json::from_value(json).unwrap();
    assert_eq!(state_again.roots(), roots);

    deploy(&env, &mut state, Vec::new());
    assert_eq!(read(appdata.join("plugins.txt")), "original");
    assert!(!appdata.join("New").exists());
    assert!(state.files.is_empty() && state.created_dirs.is_empty());
}

#[test]
fn placements_need_a_root_the_target_has() {
    let env = env();
    let src = env.store.join("a.txt");
    fs::write(&src, "a").unwrap();
    let rel = RelPath::new("a.txt").unwrap();
    let m = InstalledMod {
        name: "a".to_owned(),
        source: "local".to_owned(),
        id: "a".to_owned(),
        version: None,
        hash: ContentHash::of(b"a"),
        files: vec![ModFile {
            rel: rel.clone(),
            src,
        }],
    };
    let place = Placement {
        dst: "appdata:a.txt".parse().unwrap(),
        src: m.src(&m.files[0]),
        mod_name: "a".to_owned(),
    };
    let err = resolve(
        "test",
        vec![place.clone()],
        std::slice::from_ref(&m),
        None,
        &Roots::game(&env.game),
    )
    .unwrap_err();
    assert!(err.to_string().contains("`appdata`"), "{err}");
    let roots = with_appdata(&env);
    assert!(resolve("test", vec![place], std::slice::from_ref(&m), None, &roots).is_ok());
}

#[test]
fn roots_with_files_deployed_cant_move() {
    let env = env();
    let roots = with_appdata(&env);
    let mut state = DeployState::empty("g", &roots);
    deploy(
        &env,
        &mut state,
        vec![placement(&env, "appdata:plugins.txt", "m", "a")],
    );
    assert_eq!(state.moved_root(&roots), None);
    let appdata = RootId::new("appdata").unwrap();
    let elsewhere = roots.clone().with(appdata.clone(), &env.store);
    assert_eq!(
        state.moved_root(&elsewhere).map(|(root, _)| root),
        Some(appdata.clone())
    );
    // dropping the root entirely counts too, but the game root moving
    // doesn't when nothing's deployed there
    assert!(state.moved_root(&Roots::game(&env.game)).is_some());
    let game_moved = roots.clone().with(RootId::game(), &env.store);
    assert_eq!(state.moved_root(&game_moved), None);

    let mut plan = plan(
        DeployTarget::Game("g".into()),
        &state,
        Vec::new(),
        Vec::new(),
        None,
        LinkMethod::Copy,
    )
    .unwrap();
    plan.roots = elsewhere;
    assert!(apply(&plan, &mut state, &env.backups, &Silent).is_err());
}

#[test]
fn states_from_before_roots_still_read() {
    let json = r#"{
        "schema": 1,
        "game": "g",
        "profile": "p",
        "root": "/games/g",
        "files": {
            "Data/a.esp": {"mod_name": "a", "src": "/store/a.esp", "hash": "HASH"}
        },
        "created_dirs": ["Data"]
    }"#
    .replace("HASH", &ContentHash::of(b"a").to_string());
    let state: DeployState = serde_json::from_str(&json).unwrap();
    assert_eq!(state.roots(), Roots::game(Path::new("/games/g")));
    let key: Dest = "Data/a.esp".parse().unwrap();
    assert_eq!(key.root, RootId::game());
    assert_eq!(state.files[&key].mod_name, "a");
    assert!(state.created_dirs.contains(&"Data".parse().unwrap()));
    // and write back without any roots of their own
    let back = serde_json::to_value(&state).unwrap();
    assert!(back.get("roots").is_none());
}
