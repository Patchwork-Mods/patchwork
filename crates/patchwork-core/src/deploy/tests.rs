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
        rewritten: false,
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
        let roots = ResolvedRoots::game(&env.game);
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
        &ResolvedRoots::game(&env.game),
    )
    .unwrap_err();
    assert!(err.to_string().contains("`appdata`"), "{err}");
    let appdata = with_appdata(&env)
        .get(&RootId::new("appdata").unwrap())
        .unwrap()
        .to_owned();
    let roots =
        ResolvedRoots::game(&env.game).with(RootId::new("appdata").unwrap(), &appdata, &["*.txt"]);
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

fn root_defs(toml: &str) -> std::result::Result<crate::game::RootDefs, String> {
    #[derive(Deserialize)]
    struct Def {
        roots: crate::game::RootDefs,
    }
    toml::from_str::<Def>(toml)
        .map(|d| d.roots)
        .map_err(|e| e.to_string())
}

#[test]
fn root_definitions_need_an_allowlist_and_a_known_folder() {
    let roots = root_defs(
        r#"
        [roots.appdata]
        path = "{localappdata}/Skyrim Special Edition"
        linux = "{data}"
        allow = ["plugins.txt"]
        "#,
    )
    .unwrap();
    let appdata = roots.get(&RootId::new("appdata").unwrap()).unwrap();
    assert_eq!(
        String::from(appdata.path.clone()),
        "{localappdata}/Skyrim Special Edition"
    );
    assert_eq!(appdata.linux.as_ref().unwrap().path, None);

    for (bad, why) in [
        (r#"path = "{localappdata}/x""#, "missing field `allow`"),
        (
            r#"path = "{localappdata}/x"
            allow = []"#,
            "allow",
        ),
        (
            r#"path = "{nowhere}/x"
            allow = ["a"]"#,
            "has to start with one of",
        ),
        (
            r#"path = "/abs/x"
            allow = ["a"]"#,
            "has to start with one of",
        ),
        (
            r#"path = "{home}/../x"
            allow = ["a"]"#,
            "outside",
        ),
        (
            r#"path = "{home}x"
            allow = ["a"]"#,
            "needs a `/`",
        ),
        (
            r#"path = "{appdata}/x"
            linux = "{appdata}/x"
            allow = ["a"]"#,
            "only for Windows",
        ),
    ] {
        let err = root_defs(&format!("[roots.other]\n{bad}")).unwrap_err();
        assert!(err.contains(why), "{bad}: {err}");
    }
    let err = root_defs("[roots.game]\npath = \"{home}\"\nallow = [\"a\"]").unwrap_err();
    assert!(err.contains("game's own folder"), "{err}");
}

/// A Wine prefix like Proton's, with its user folder made.
fn wine_prefix(dir: &Path, user: &str) -> PathBuf {
    let prefix = dir.join("pfx");
    fs::create_dir_all(prefix.join(format!("drive_c/users/{user}/AppData/Local"))).unwrap();
    fs::create_dir_all(prefix.join("drive_c/users/Public")).unwrap();
    prefix
}

#[test]
fn roots_are_found_on_the_host_or_in_the_wine_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = root_defs(
        r#"
        [roots.appdata]
        path = "{localappdata}/Game"
        allow = ["plugins.txt"]
        [roots.saves]
        path = "{documents}/My Games/Game"
        linux = "{data}/game"
        allow = ["*.sav"]
        "#,
    )
    .unwrap();
    let host = Folders {
        local_appdata: Some(tmp.path().join("Local")),
        documents: Some(tmp.path().join("Documents")),
        data: Some(tmp.path().join("share")),
        ..Folders::default()
    };
    let prefix = wine_prefix(tmp.path(), "steamuser");
    let none = BTreeMap::new();
    let resolve = |os: Os, prefix: Option<&Path>, set: &BTreeMap<RootId, PathBuf>| {
        let resolved = ResolvedRoots::new(
            "game-id",
            &roots,
            &RootSetup {
                os,
                game: Path::new("/game"),
                host: &host,
                prefix,
                set,
            },
        );
        resolved
            .iter()
            .map(|(id, r)| {
                let path = r
                    .path
                    .clone()
                    .map(|p| p.strip_prefix(tmp.path()).unwrap().to_owned());
                (id.to_string(), (path, r.origin))
            })
            .collect::<BTreeMap<_, _>>()
    };
    let ok = |p: &str, origin| (Ok(PathBuf::from(p)), origin);

    let windows = resolve(Os::Windows, None, &none);
    assert_eq!(windows["appdata"], ok("Local/Game", RootOrigin::Definition));
    assert_eq!(
        windows["saves"],
        ok("Documents/My Games/Game", RootOrigin::Definition)
    );

    // a Windows game in its prefix, unless there's a native path
    let linux = resolve(Os::Linux, Some(&prefix), &none);
    assert_eq!(
        linux["appdata"],
        ok(
            "pfx/drive_c/users/steamuser/AppData/Local/Game",
            RootOrigin::Prefix
        )
    );
    assert_eq!(linux["saves"], ok("share/game", RootOrigin::Definition));

    let (Err(why), _) = &resolve(Os::Linux, None, &none)["appdata"] else {
        panic!("found without a prefix");
    };
    assert!(why.contains("set game-id prefix"), "{why}");
    let not_a_prefix = tmp.path().join("Documents");
    let (Err(why), _) = &resolve(Os::Linux, Some(&not_a_prefix), &none)["appdata"] else {
        panic!("found in something that isn't a prefix");
    };
    assert!(why.contains("doesn't look like a Wine prefix"), "{why}");
    let (Err(why), _) = &resolve(Os::Macos, None, &none)["appdata"] else {
        panic!("found on macOS without a path for it");
    };
    assert!(why.contains("root.appdata"), "{why}");

    // what the user says wins
    let set = BTreeMap::from([(RootId::new("appdata").unwrap(), tmp.path().join("mine"))]);
    assert_eq!(
        resolve(Os::Linux, None, &set)["appdata"],
        ok("mine", RootOrigin::Set)
    );

    // only roots that were found are deployed to
    let resolved = ResolvedRoots::new(
        "game-id",
        &roots,
        &RootSetup {
            os: Os::Linux,
            game: Path::new("/game"),
            host: &host,
            prefix: None,
            set: &none,
        },
    );
    let found: Vec<_> = resolved
        .roots()
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    assert_eq!(found, ["game", "saves"]);
}

#[test]
fn wine_prefixes_are_named_after_their_user() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = wine_prefix(tmp.path(), "someone-else");
    let folders = Folders::wine(&prefix).unwrap();
    assert_eq!(
        folders.local_appdata.unwrap(),
        prefix.join("drive_c/users/someone-else/AppData/Local")
    );
    // more than one user and none of them obvious
    fs::create_dir_all(prefix.join("drive_c/users/a-third")).unwrap();
    if std::env::var("USER").is_ok_and(|u| u != "someone-else" && u != "a-third") {
        assert_eq!(Folders::wine(&prefix), None);
    }
    assert_eq!(Folders::wine(tmp.path()), None);
}

#[test]
fn other_roots_only_take_allowed_files_in_folders_that_exist() {
    let env = env();
    let appdata = with_appdata(&env)
        .get(&RootId::new("appdata").unwrap())
        .unwrap()
        .to_owned();
    let id = RootId::new("appdata").unwrap();
    let roots =
        ResolvedRoots::game(&env.game).with(id.clone(), &appdata, &["plugins.txt", "Saves/*.ess"]);
    let check = |dst: &str| roots.check(&dst.parse().unwrap());

    assert!(check("anything/at/all.dll").is_ok());
    assert!(check("appdata:plugins.txt").is_ok());
    assert!(check("appdata:PLUGINS.TXT").is_ok());
    for bad in [
        "appdata:other.txt",
        "appdata:x/plugins.txt",
        "appdata:Saves/a/b.ess",
    ] {
        let (blame, why) = check(bad).unwrap_err();
        assert!(blame && why.contains("isn't allowed"), "{bad}: {why}");
    }
    let (blame, why) = check("elsewhere:plugins.txt").unwrap_err();
    assert!(blame && why.contains("doesn't have"), "{why}");

    // the folder has to be there already, since it's someone else's
    let gone = ResolvedRoots::game(&env.game).with(id, &appdata.join("gone"), &["plugins.txt"]);
    let (blame, why) = gone
        .check(&"appdata:plugins.txt".parse().unwrap())
        .unwrap_err();
    assert!(!blame && why.contains("doesn't exist"), "{why}");
}

#[test]
fn files_the_game_rewrites_are_put_back_and_restored() {
    let env = env();
    let roots = with_appdata(&env);
    let appdata = roots
        .get(&RootId::new("appdata").unwrap())
        .unwrap()
        .to_owned();
    let id = RootId::new("appdata").unwrap();
    let resolved =
        ResolvedRoots::game(&env.game).with_rewritten(id, &appdata, &["*.txt"], &["plugins.txt"]);
    assert_eq!(resolved.check(&"appdata:Plugins.txt".parse().unwrap()), Ok(true));
    assert_eq!(resolved.check(&"appdata:other.txt".parse().unwrap()), Ok(false));

    let mut state = DeployState::empty("g", &roots);
    let rewritten = |env: &Env| Resolved {
        rewritten: true,
        ..placement(env, "appdata:plugins.txt", "ours", "built")
    };
    deploy(&env, &mut state, vec![rewritten(&env)]);
    let read = |p: PathBuf| fs::read_to_string(p).unwrap();
    assert_eq!(read(appdata.join("plugins.txt")), "ours");

    // The game rewriting it doesn't make what it wrote the original, and
    // deploying again puts ours back.
    fs::write(appdata.join("plugins.txt"), "the game's").unwrap();
    let again = deploy(&env, &mut state, vec![rewritten(&env)]);
    assert_eq!(again.ops.len(), 1);
    assert_eq!(read(appdata.join("plugins.txt")), "ours");
    assert!(deploy(&env, &mut state, vec![rewritten(&env)]).is_noop());

    // Undeploying removes it even after the game's changed it.
    fs::write(appdata.join("plugins.txt"), "the game's").unwrap();
    deploy(&env, &mut state, Vec::new());
    assert_eq!(read(appdata.join("plugins.txt")), "original");
    assert!(state.files.is_empty());
}

#[cfg(unix)]
#[test]
fn other_roots_are_always_copied_to() {
    let env = env();
    let roots = with_appdata(&env);
    let appdata = roots
        .get(&RootId::new("appdata").unwrap())
        .unwrap()
        .to_owned();
    let mut state = DeployState::empty("g", &roots);
    let plan = plan(
        DeployTarget::Game("g".into()),
        &state,
        vec![
            placement(&env, "appdata:plugins.txt", "modded", "a"),
            placement(&env, "Mods/a.dll", "a", "a"),
        ],
        Vec::new(),
        None,
        LinkMethod::Symlink,
    )
    .unwrap();
    apply(&plan, &mut state, &env.backups, &Silent).unwrap();
    let is_link = |p: PathBuf| p.symlink_metadata().unwrap().file_type().is_symlink();
    assert!(is_link(env.game.join("Mods/a.dll")));
    assert!(!is_link(appdata.join("plugins.txt")));
}

#[test]
fn paths_take_the_case_already_on_disk() {
    let env = env();
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
    let placements = vec![
        placement(&env, "data/Meshes/a.nif", "a", "a"),
        placement(&env, "DATA/meshes/b.nif", "b", "b"),
        placement(&env, "data/VANILLA.txt", "modded", "a"),
    ];
    let plan = deploy(&env, &mut state, placements.clone());
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);

    let mut names: Vec<_> = state.files.keys().map(|d| d.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        ["Data/Meshes/a.nif", "Data/Meshes/b.nif", "Data/vanilla.txt"]
    );
    let read = |p: &str| fs::read_to_string(env.game.join(p)).unwrap();
    assert_eq!(read("Data/Meshes/b.nif"), "b");
    assert_eq!(read("Data/vanilla.txt"), "modded");
    let top: Vec<_> = fs::read_dir(&env.game).unwrap().collect();
    assert_eq!(top.len(), 1, "made a second Data folder");

    assert!(deploy(&env, &mut state, placements).is_noop());
    deploy(&env, &mut state, Vec::new());
    assert_eq!(read("Data/vanilla.txt"), "vanilla");
    assert!(!env.game.join("Data/Meshes").exists());
}

#[test]
fn spellings_of_one_file_conflict() {
    let env = env();
    let mut state = DeployState::empty("g", &Roots::game(&env.game));
    let plan = deploy(
        &env,
        &mut state,
        vec![
            placement(&env, "Mods/x.cfg", "first", "a"),
            placement(&env, "mods/X.CFG", "second", "b"),
        ],
    );
    assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
    assert_eq!(state.files.len(), 1);
    assert_eq!(
        fs::read_to_string(env.game.join("Mods/x.cfg")).unwrap(),
        "second"
    );
}
