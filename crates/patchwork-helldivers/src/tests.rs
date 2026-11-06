use patchwork_core::ext::testing::TestGame;

use super::*;

const A: &str = "9ba626afa44a3aa3";
const B: &str = "2e24ba9dd702da5c";

fn game() -> TestGame {
    TestGame::builtin("helldivers-2").extension(Helldivers)
}

/// A mod's three files for patch `n` of `archive`, in `folder`.
fn patch(folder: &str, archive: &str, n: u32) -> Vec<(String, Vec<u8>)> {
    PARTS
        .iter()
        .map(|part| {
            let path = format!("{folder}{archive}.patch_{n}{part}");
            let contents = path.clone().into_bytes();
            (path, contents)
        })
        .collect()
}

#[test]
fn parses_patch_names() {
    let name = |archive: &str, index, part| {
        Some(PatchName {
            archive: archive.to_owned(),
            index,
            part,
        })
    };
    assert_eq!(PatchName::parse(&format!("{A}.patch_0")), name(A, 0, 0));
    assert_eq!(
        PatchName::parse(&format!("{A}.patch_12.gpu_resources")),
        name(A, 12, 1)
    );
    assert_eq!(
        PatchName::parse(&format!("{}.PATCH_3.Stream", A.to_uppercase())),
        name(A, 3, 2)
    );
    for bad in [
        format!("{A}.patch_"),
        format!("{A}.patch_x"),
        format!("{A}.patch_0.dds"),
        format!("{A}.patch_0.stream.bak"),
        format!("{A}x.patch_0"),
        A.to_owned(),
        "readme.txt".to_owned(),
        "manifest.json".to_owned(),
    ] {
        assert_eq!(PatchName::parse(&bad), None, "{bad}");
    }
}

#[test]
fn mods_patching_one_archive_are_numbered_in_load_order() {
    let t = game().mod_files("First", patch("", A, 0)).mod_files(
        "Second",
        patch("Second/", A, 0)
            .into_iter()
            .chain(patch("Second/", B, 0)),
    );
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        [
            format!("data/{B}.patch_0"),
            format!("data/{B}.patch_0.gpu_resources"),
            format!("data/{B}.patch_0.stream"),
            format!("data/{A}.patch_0"),
            format!("data/{A}.patch_0.gpu_resources"),
            format!("data/{A}.patch_0.stream"),
            format!("data/{A}.patch_1"),
            format!("data/{A}.patch_1.gpu_resources"),
            format!("data/{A}.patch_1.stream"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    assert_eq!(plan.placed_by(&format!("data/{A}.patch_0")), Some("First"));
    assert_eq!(plan.placed_by(&format!("data/{A}.patch_1")), Some("Second"));
    assert_eq!(
        t.read(&format!("data/{A}.patch_1.stream")).unwrap(),
        format!("Second/{A}.patch_0.stream")
    );
    assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
}

#[test]
fn a_mods_own_numbers_keep_their_order_without_gaps() {
    let t = game().mod_files("Gappy", patch("", A, 7).into_iter().chain(patch("", A, 2)));
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("{A}.patch_2")
    );
    assert_eq!(
        t.read(&format!("data/{A}.patch_1")).unwrap(),
        format!("{A}.patch_7")
    );
    assert!(!t.exists(&format!("data/{A}.patch_2")));
}

#[test]
fn numbering_carries_on_after_the_games_own_patches() {
    let t = game()
        .game_file(&format!("data/{A}.patch_0"), b"the game's")
        .game_file(&format!("data/{A}.patch_0.gpu_resources"), b"the game's")
        .mod_files("Mod", patch("", A, 0));
    let plan = t.deploy().unwrap();
    assert!(!plan.places(&format!("data/{A}.patch_0")));
    assert_eq!(t.read(&format!("data/{A}.patch_0")).unwrap(), "the game's");
    assert_eq!(
        t.read(&format!("data/{A}.patch_1")).unwrap(),
        format!("{A}.patch_0")
    );
}

#[test]
fn missing_parts_are_placed_empty_and_other_files_left_out() {
    let t = game().mod_files(
        "Partial",
        [
            (format!("{A}.patch_0"), b"patch".to_vec()),
            ("readme.txt".to_owned(), b"hi".to_vec()),
            ("preview.png".to_owned(), b"png".to_vec()),
        ],
    );
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        [
            format!("data/{A}.patch_0"),
            format!("data/{A}.patch_0.gpu_resources"),
            format!("data/{A}.patch_0.stream"),
        ]
    );
    assert_eq!(t.read(&format!("data/{A}.patch_0")).unwrap(), "patch");
    assert_eq!(t.read(&format!("data/{A}.patch_0.stream")).unwrap(), "");
}

#[test]
fn warns_about_mods_without_patches_and_with_variants() {
    let t = game()
        .mod_files("Empty", [("readme.txt", b"hi")])
        .mod_files(
            "Variants",
            patch("Red/", A, 0).into_iter().chain(patch("Blue/", A, 0)),
        );
    let plan = t.deploy().unwrap();
    let warnings = plan.warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("Empty has no patch files"))
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("Variants patches the same archive"))
    );
    // Blue sorts first, so Red goes over it.
    assert_eq!(
        t.read(&format!("data/{A}.patch_1")).unwrap(),
        format!("Red/{A}.patch_0")
    );
}

#[test]
fn reordering_mods_renumbers_them() {
    let t = game()
        .mod_files("First", patch("", A, 0))
        .mod_files("Second", patch("Second/", A, 0));
    t.deploy().unwrap();
    t.patchwork()
        .reorder_mods(t.profile(), &["Second".to_owned(), "First".to_owned()])
        .unwrap();
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Second/{A}.patch_0")
    );
    assert_eq!(
        t.read(&format!("data/{A}.patch_1")).unwrap(),
        format!("{A}.patch_0")
    );
}

#[test]
fn disabling_a_mod_closes_the_gap() {
    let t = game()
        .mod_files("First", patch("", A, 0))
        .mod_files("Second", patch("Second/", A, 0));
    t.deploy().unwrap();
    t.set_enabled("First", false).unwrap();
    let plan = t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Second/{A}.patch_0")
    );
    assert!(plan.removed().contains(&format!("data/{A}.patch_1")));
    assert!(!t.exists(&format!("data/{A}.patch_1")));
}
