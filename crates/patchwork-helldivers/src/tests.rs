use patchwork_core::ext::testing::TestGame;

use patchwork_core::options::Answer;

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
    let variants = "Variants has more than one version of the same patches, in “Blue” and “Red”, which usually means you're meant to pick one. It doesn't say how to pick, so all of them are deployed, and “Red” wins where they overlap.";
    assert!(warnings.iter().any(|w| w == variants), "{warnings:?}");
    // It's an issue with the mod too, so it's shown with the profile.
    let issues = t.issues().unwrap();
    let issue = issues.iter().find(|i| i.message == variants).unwrap();
    assert_eq!(issue.mods, ["Variants"]);
    // Blue sorts first, so Red goes over it.
    assert_eq!(
        t.read(&format!("data/{A}.patch_1")).unwrap(),
        format!("Red/{A}.patch_0")
    );
}

/// A mod made for the community's mod manager, as the Remastered mods are
/// packed: two options, the first with two mixes.
fn with_manifest() -> Vec<(String, Vec<u8>)> {
    let manifest = r#"{
        "Version": 1,
        "Guid": "45e0d8ff-7a17-449a-b7d8-8f8e86b19350",
        "Name": "Units Remastered",
        "Options": [
            {
                "Name": "Bots",
                "Description": "Louder bots.",
                "Image": "botlogo.png",
                "Include": ["Bots"],
                "SubOptions": [
                    { "Name": "Standard Mix", "Include": ["Bots/Standard Mix"] },
                    { "Name": "TF2 Mix", "Include": ["Bots\\TF2 Mix"] }
                ]
            },
            { "Name": "Bugs", "Include": ["Bugs"] }
        ]
    }"#;
    [
        ("manifest.json".to_owned(), manifest.as_bytes().to_vec()),
        ("BotLogo.png".to_owned(), b"png".to_vec()),
    ]
    .into_iter()
    .chain(patch("Bots/Standard Mix/", A, 0))
    .chain(patch("Bots/TF2 Mix/", A, 0))
    .chain(patch("Bugs/", A, 0))
    .chain(patch("Bugs/", B, 0))
    .collect()
}

#[test]
fn a_manifest_picks_the_first_sub_option_of_every_option() {
    let t = game().mod_files("Units", with_manifest());
    let plan = t.deploy().unwrap();
    assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
    assert!(t.issues().unwrap().is_empty());
    // In the manifest's order: Bots' mix, then Bugs.
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Bots/Standard Mix/{A}.patch_0")
    );
    assert_eq!(
        t.read(&format!("data/{A}.patch_1.stream")).unwrap(),
        format!("Bugs/{A}.patch_0.stream")
    );
    assert_eq!(
        t.read(&format!("data/{B}.patch_0")).unwrap(),
        format!("Bugs/{B}.patch_0")
    );
    assert!(!t.exists(&format!("data/{A}.patch_2")));
}

#[test]
fn a_manifests_options_can_be_picked() {
    let t = game().mod_files("Units", with_manifest());
    let view = t.options("Units").unwrap();
    assert_eq!(view.form.title, "Units Remastered");
    let groups = &view.form.pages[0].groups;
    assert_eq!(
        groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        ["options", "option:Bots"]
    );
    let bots = &groups[0].items()[0];
    assert!(bots.picked);
    assert_eq!(bots.description.as_deref(), Some("Louder bots."));
    // The manifest gets the picture's case wrong.
    assert_eq!(bots.image.as_ref().map(|i| i.as_str()), Some("BotLogo.png"));
    assert_eq!(view.form.summary.as_deref(), Some("Deploys 3 patches."));

    t.answer_options(
        "Units",
        &[
            Answer::picks("options", vec!["Bots".to_owned()]),
            Answer::picks("option:Bots", vec!["TF2 Mix".to_owned()]),
        ],
    )
    .unwrap();
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Bots/TF2 Mix/{A}.patch_0")
    );
    assert!(!t.exists(&format!("data/{A}.patch_1")));
    assert!(!t.exists(&format!("data/{B}.patch_0")));

    // Turning Bots off hides its mixes but keeps what was picked.
    let view = t
        .options_for(
            "Units",
            &[Answer::picks("options", vec!["Bugs".to_owned()])],
        )
        .unwrap();
    let groups = &view.form.pages[0].groups;
    assert_eq!(groups.len(), 1);
    assert!(
        view.form
            .answers
            .iter()
            .any(|a| a.group == "option:Bots" && a.items == ["TF2 Mix"])
    );
}

#[test]
fn an_older_manifest_picks_one_folder() {
    let t = game().mod_files(
        "Old",
        [(
            "Old/manifest.json".to_owned(),
            br#"{ "Guid": "x", "Name": "Old", "Options": ["Loud", "Quiet"] }"#.to_vec(),
        )]
        .into_iter()
        .chain(patch("Old/Loud/", A, 0))
        .chain(patch("Old/Quiet/", A, 0)),
    );
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Old/Loud/{A}.patch_0")
    );
    assert!(!t.exists(&format!("data/{A}.patch_1")));
    t.answer_options("Old", &[Answer::picks("variant", vec!["Quiet".to_owned()])])
        .unwrap();
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("Old/Quiet/{A}.patch_0")
    );
}

#[test]
fn a_manifest_without_options_deploys_what_is_next_to_it() {
    let t = game().mod_files(
        "Plain",
        [(
            "manifest.json".to_owned(),
            br#"{ "Version": 1, "Name": "Plain" }"#.to_vec(),
        )]
        .into_iter()
        .chain(patch("", A, 0))
        .chain(patch("Extras/", A, 0)),
    );
    t.deploy().unwrap();
    assert_eq!(
        t.read(&format!("data/{A}.patch_0")).unwrap(),
        format!("{A}.patch_0")
    );
    assert!(!t.exists(&format!("data/{A}.patch_1")));
    assert!(t.options("Plain").is_err());
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
