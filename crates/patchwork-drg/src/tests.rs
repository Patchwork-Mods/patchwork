use patchwork_core::ext::testing::TestGame;

use super::*;

fn game() -> TestGame {
    TestGame::builtin("deep-rock-galactic").extension(Drg)
}

#[test]
fn joins_mount_points() {
    assert_eq!(
        join_mount("../../../", "FSD/Content/A.uasset"),
        "../../../FSD/Content/A.uasset"
    );
    assert_eq!(
        join_mount("../../../FSD", "Content/A.uasset"),
        "../../../FSD/Content/A.uasset"
    );
}

#[test]
fn gives_paths_the_games_case() {
    let writer = PakWriter::new(
        Cursor::new(Vec::new()),
        &["FSD/Content/Game/BP_PlayerControllerBase.uasset".to_owned()],
    );
    assert_eq!(
        writer.game_case("fsd/content/game/bp_playercontrollerbase.uasset"),
        "FSD/Content/Game/BP_PlayerControllerBase.uasset"
    );
    assert_eq!(
        writer.game_case("fsd/content/_MyMod/InitCave.uasset"),
        "FSD/Content/_MyMod/InitCave.uasset"
    );
}

#[test]
fn packs_mints_blueprints() {
    let mut files = Vec::new();
    included_files(&INTEGRATION, &mut files);
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert!(
        paths.contains(&"FSD/Content/_AssemblyStorm/ModIntegration/MI_SpawnMods.uasset"),
        "{paths:?}"
    );
    assert!(paths.iter().all(|p| p.starts_with("FSD/Content/")));
}

#[test]
fn nothing_is_built_without_mods() {
    let plan = game().deploy().unwrap();
    assert!(plan.placed().is_empty());
}

#[test]
fn mods_without_a_pak_are_left_out_with_a_warning() {
    let plan = game()
        .mod_files("Readme only", [("readme.txt", "hi")])
        .deploy()
        .unwrap();
    assert!(plan.placed().is_empty());
    assert!(
        plan.warnings()
            .iter()
            .any(|w| w.contains("Readme only has no .pak")),
        "{:?}",
        plan.warnings()
    );
}

#[test]
fn the_games_pak_has_to_be_there() {
    let err = game()
        .mod_files("Mod", [("Mod.pak", "not really a pak")])
        .deploy()
        .unwrap_err();
    assert!(err.to_string().contains("FSD-WindowsNoEditor.pak"), "{err}");
}

/// Builds a pak from the real game's (`DRG_PAK`, the path to
/// `FSD-WindowsNoEditor.pak`) and mods (`DRG_MODS`, paths to `.pak`s
/// separated by `:`), and reads it back. Run with
/// `cargo test -p patchwork-drg -- --ignored`.
#[test]
#[ignore = "needs the game and some mods"]
fn builds_from_the_real_game() {
    let base = std::env::var("DRG_PAK").expect("DRG_PAK");
    let mods = std::env::var("DRG_MODS").expect("DRG_MODS");
    let mods: Vec<(&str, &Path)> = mods
        .split(':')
        .map(|p| {
            let path = Path::new(p);
            (path.file_stem().unwrap().to_str().unwrap(), path)
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join(BUILT);
    let warnings = integrate_mods(
        Path::new(&base),
        &mods,
        BufWriter::new(File::create(&out).unwrap()),
        &|step| eprintln!("{step}"),
    )
    .unwrap();
    eprintln!("warnings: {warnings:#?}");

    let mut reader = BufReader::new(File::open(&out).unwrap());
    let pak = repak::PakBuilder::new().reader(&mut reader).unwrap();
    let files = pak.files();
    eprintln!("{} files in {BUILT}", files.len());
    for path in [
        PLAYER_CONTROLLER,
        ESCAPE_MENU,
        MODDING_TAB,
        SERVER_LIST_ENTRY,
    ]
    .into_iter()
    .chain(PATCHED)
    {
        let raw = RawAsset {
            uasset: Some(pak.get(&format!("{path}.uasset"), &mut reader).unwrap()),
            uexp: Some(pak.get(&format!("{path}.uexp"), &mut reader).unwrap()),
        };
        raw.parse(path).unwrap();
    }
    let registry = pak.get(ASSET_REGISTRY, &mut reader).unwrap();
    AssetRegistry::read(&mut Cursor::new(registry)).unwrap();
    assert!(files.iter().any(|f| f.contains("MI_SpawnMods")));
}
