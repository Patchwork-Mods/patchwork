use std::fs;

use super::*;
use crate::deploy::{DeployedFile, Roots};
use crate::hash::ContentHash;

fn def(ignore: &[&str]) -> CaptureDef {
    CaptureDef {
        folders: vec![RelPath::new("Data").unwrap()],
        ignore: ignore
            .iter()
            .map(|p| p.to_string().try_into().unwrap())
            .collect(),
    }
}

fn rel(path: &str) -> RelPath {
    RelPath::new(path).unwrap()
}

/// A game folder with one file of its own in Data and one outside it, and
/// a state with `deployed` deployed from a store next to it.
fn setup(deployed: &[(&str, &str)]) -> (tempfile::TempDir, DeployState) {
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(game.join("Data")).unwrap();
    fs::write(game.join("Data/Skyrim.esm"), "master").unwrap();
    fs::write(game.join("SkyrimSE.exe"), "exe").unwrap();
    let mut state = DeployState::empty("g", &Roots::game(&game));
    let store = tmp.path().join("store");
    fs::create_dir_all(&store).unwrap();
    for (path, contents) in deployed {
        let src = store.join(path.replace('/', "_"));
        fs::write(&src, contents).unwrap();
        let dst = rel(path).under(&game);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&dst, contents).unwrap();
        state.files.insert(
            Dest::game(rel(path)),
            DeployedFile {
                mod_name: "A Mod".to_owned(),
                src,
                hash: ContentHash::of(contents.as_bytes()),
                backup: None,
                rewritten: false,
            },
        );
    }
    (tmp, state)
}

#[test]
fn baseline_leaves_out_deployed_files_and_other_folders() {
    let (_tmp, state) = setup(&[("Data/mod.esp", "mod")]);
    let baseline = baseline(&state.root, &def(&[]), &state, None).unwrap();
    assert_eq!(
        baseline.keys().collect::<Vec<_>>(),
        [&rel("Data/Skyrim.esm")]
    );
}

#[test]
fn finds_new_files_and_replaced_deployed_ones() {
    let (_tmp, state) = setup(&[("Data/meshes/body.nif", "original")]);
    let def = def(&[]);
    let base = baseline(&state.root, &def, &state, None).unwrap();
    let root = &state.root;
    fs::create_dir_all(root.join("Data/meshes/lod")).unwrap();
    fs::write(root.join("Data/meshes/lod/tree.nif"), "lod").unwrap();
    fs::write(root.join("Data/DynDOLOD.esp"), "plugin").unwrap();
    // Written over, as BodySlide does, rather than through the link.
    fs::remove_file(root.join("Data/meshes/body.nif")).unwrap();
    fs::write(root.join("Data/meshes/body.nif"), "built by a tool").unwrap();

    let output = find(root, &def, &state, &base).unwrap();
    assert_eq!(
        output.files,
        [
            OutputFile {
                path: rel("Data/DynDOLOD.esp"),
                replaces: None,
            },
            OutputFile {
                path: rel("Data/meshes/body.nif"),
                replaces: Some("A Mod".to_owned()),
            },
            OutputFile {
                path: rel("Data/meshes/lod/tree.nif"),
                replaces: None,
            },
        ]
    );
    assert!(output.changed.is_empty());
}

#[test]
fn reports_changed_game_files_without_capturing_them() {
    let (_tmp, state) = setup(&[]);
    let def = def(&[]);
    let base = baseline(&state.root, &def, &state, None).unwrap();
    fs::write(state.root.join("Data/Skyrim.esm"), "cleaned master").unwrap();

    let output = find(&state.root, &def, &state, &base).unwrap();
    assert!(output.files.is_empty());
    assert_eq!(output.changed, [rel("Data/Skyrim.esm")]);
}

#[test]
fn ignores_matching_files() {
    let (_tmp, state) = setup(&[]);
    let def = def(&["**/*.log"]);
    let base = baseline(&state.root, &def, &state, None).unwrap();
    fs::write(state.root.join("Data/tool.log"), "log").unwrap();
    fs::write(state.root.join("Data/patch.esp"), "patch").unwrap();

    let output = find(&state.root, &def, &state, &base).unwrap();
    assert_eq!(
        output.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
        [&rel("Data/patch.esp")]
    );
}

#[test]
fn deploying_again_keeps_tool_output_out_of_the_baseline() {
    let (_tmp, state) = setup(&[]);
    let def = def(&[]);
    let first = baseline(&state.root, &def, &state, None).unwrap();
    fs::write(state.root.join("Data/patch.esp"), "patch").unwrap();
    fs::remove_file(state.root.join("Data/Skyrim.esm")).unwrap();

    let again = baseline(&state.root, &def, &state, Some(&first)).unwrap();
    assert!(again.is_empty());
    let output = find(&state.root, &def, &state, &again).unwrap();
    assert_eq!(output.files.len(), 1);
}
