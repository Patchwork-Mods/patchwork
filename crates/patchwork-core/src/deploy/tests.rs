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

fn placement(env: &Env, dst: &str, contents: &str, mod_name: &str) -> Placement {
    let src = env
        .store
        .join(format!("{mod_name}-{}", dst.replace('/', "_")));
    fs::write(&src, contents).unwrap();
    Placement {
        dst: RelPath::new(dst).unwrap(),
        src,
        mod_name: mod_name.to_owned(),
    }
}

fn deploy(env: &Env, state: &mut DeployState, placements: Vec<Placement>) -> DeployPlan {
    let desired = DesiredState {
        placements,
        warnings: Vec::new(),
    };
    let plan = plan(
        DeployTarget::Game("g".into()),
        state,
        desired,
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
