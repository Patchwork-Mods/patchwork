use patchwork_core::ext::testing::TestGame;

use super::*;
use pak::write::{How, pak, split};
use patchwork_bethesda::Fomod;
use patchwork_core::DeployStatus;
use patchwork_core::options::Answer;

const ID: &str = "bg3.mods";

fn game() -> TestGame {
    TestGame::builtin("baldurs-gate-3").extension(Bg3)
}

/// A `meta.lsx` for module `name` (folder and UUID made from it), needing
/// the modules with `deps`' UUIDs.
fn meta_lsx(name: &str, deps: &[(&str, &str)]) -> String {
    let deps: String = deps
        .iter()
        .map(|(uuid, name)| {
            format!(
                r#"<node id="ModuleShortDesc">
                    <attribute id="Folder" type="LSString" value="{name}"/>
                    <attribute id="Name" type="LSString" value="{name}"/>
                    <attribute id="UUID" type="FixedString" value="{uuid}"/>
                    <attribute id="Version64" type="int64" value="36028797018963968"/>
                </node>"#
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<save>
    <version major="4" minor="0" revision="9" build="331"/>
    <region id="Config">
        <node id="root">
            <children>
                <node id="Dependencies"><children>{deps}</children></node>
                <node id="ModuleInfo">
                    <attribute id="Author" type="LSString" value="someone"/>
                    <attribute id="Folder" type="LSString" value="{name}Folder"/>
                    <attribute id="Name" type="LSString" value="{name}"/>
                    <attribute id="UUID" type="FixedString" value="{}"/>
                    <attribute id="Version64" type="int64" value="36028797018963969"/>
                </node>
            </children>
        </node>
    </region>
</save>"#,
        uuid(name)
    )
}

/// A made-up UUID for module `name`.
fn uuid(name: &str) -> String {
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    format!("{hash:08x}-0000-0000-0000-000000000000")
}

/// A package with module `name`.
fn module_pak(name: &str, deps: &[(&str, &str)]) -> Vec<u8> {
    let meta = meta_lsx(name, deps);
    pak(&[
        (
            &format!("Mods/{name}Folder/meta.lsx"),
            meta.as_bytes(),
            How::Lz4,
        ),
        ("Public/x.txt", b"data", How::Stored),
    ])
}

/// Module names as listed in `modsettings.lsx`, in order.
fn listed(text: &str) -> Vec<String> {
    let doc = roxmltree::Document::parse(text).unwrap();
    doc.descendants()
        .filter(|n| n.attribute("id") == Some("ModuleShortDesc"))
        .map(|n| {
            n.children()
                .find(|c| c.attribute("id") == Some("Name"))
                .and_then(|c| c.attribute("value"))
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn reads_files_from_packages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.pak");
    let big: Vec<u8> = (0..5000u32).flat_map(|i| (i % 7).to_le_bytes()).collect();
    std::fs::write(
        &path,
        pak(&[
            ("stored.txt", b"plain", How::Stored),
            ("Folder\\zlib.txt", &big, How::Zlib),
            ("lz4.txt", &big, How::Lz4),
            ("zstd.txt", &big, How::Zstd),
        ]),
    )
    .unwrap();
    let mut p = Pak::open(&path).unwrap();
    let names: Vec<_> = p.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(
        names,
        ["stored.txt", "Folder/zlib.txt", "lz4.txt", "zstd.txt"]
    );
    let entries = p.entries.clone();
    assert_eq!(p.read(&entries[0], "a.pak", &|_| None).unwrap(), b"plain");
    assert_eq!(p.read(&entries[1], "a.pak", &|_| None).unwrap(), big);
    assert_eq!(p.read(&entries[2], "a.pak", &|_| None).unwrap(), big);
    assert_eq!(p.read(&entries[3], "a.pak", &|_| None).unwrap(), big);

    std::fs::write(&path, b"PK\x03\x04 not a package").unwrap();
    assert!(Pak::open(&path).unwrap_err().contains("LSPK"));
    let mut v16 = pak(&[]);
    v16[4] = 16;
    std::fs::write(&path, v16).unwrap();
    assert!(Pak::open(&path).unwrap_err().contains("version 16"));
}

#[test]
fn parses_meta_lsx() {
    let text = format!("\u{feff}{}", meta_lsx("Cool", &[("abc", "Lib")]));
    let m = meta::parse(&text).unwrap();
    assert_eq!(m.name, "Cool");
    assert_eq!(m.folder, "CoolFolder");
    assert_eq!(m.uuid, uuid("Cool"));
    assert_eq!(m.version64, 36028797018963969);
    assert_eq!(meta::version(m.version64), "1.0.0.1");
    assert_eq!(
        m.dependencies,
        [meta::Dependency {
            uuid: "abc".to_owned(),
            name: "Lib".to_owned()
        }]
    );

    // Older mods have a 32-bit version: 1.2.3.4.
    let old = meta_lsx("Old", &[]).replace(
        r#"<attribute id="Version64" type="int64" value="36028797018963969"/>"#,
        &format!(
            r#"<attribute id="Version" type="int32" value="{}"/>"#,
            (1u32 << 28) | (2 << 24) | (3 << 16) | 4
        ),
    );
    assert_eq!(
        meta::version(meta::parse(&old).unwrap().version64),
        "1.2.3.4"
    );

    assert!(meta::parse("<save/>").unwrap_err().contains("ModuleInfo"));
}

#[test]
fn modsettings_lists_modules_in_order_and_escapes() {
    let module = |name: &str| Module {
        uuid: uuid(name),
        folder: name.to_owned(),
        name: name.to_owned(),
        version64: 1,
        publish_handle: 0,
        md5: String::new(),
        dependencies: Vec::new(),
    };
    let (a, b) = (module("GustavX"), module("Tom & \"Jerry\" <3"));
    let text = meta::modsettings(&[&a, &b]);
    assert_eq!(listed(&text), ["GustavX", "Tom & \"Jerry\" <3"]);
    assert!(!text.contains("ModOrder"));
}

#[test]
fn deploys_packages_to_mods_and_writes_the_load_order() {
    let t = game()
        .mod_files(
            "Cool",
            [
                ("Cool Mod/Cool.pak", module_pak("Cool", &[])),
                ("Cool Mod/info.json", b"{}".to_vec()),
                ("readme.txt", b"hi".to_vec()),
            ],
        )
        .mod_files(
            "Loader",
            [
                ("bin/DWrite.dll", b"dll".to_vec()),
                ("NativeMods/Fix.dll", b"dll".to_vec()),
            ],
        );
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        [
            "bin/DWrite.dll",
            "bin/NativeMods/Fix.dll",
            "mods:Cool.pak",
            "profile:modsettings.lsx",
        ]
    );
    assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
    let settings = t.read("profile:modsettings.lsx").unwrap();
    assert_eq!(listed(&settings), ["GustavX", "Cool"]);
    assert!(settings.contains("cb555efe-2d9e-131f-8195-a89329d218ea"));
    assert!(settings.contains(r#"value="CoolFolder""#));
}

#[test]
fn the_load_order_follows_the_list() {
    let t = game()
        .mod_files("A", [("A.pak", module_pak("Alpha", &[]))])
        .mod_files("B", [("B.pak", module_pak("Beta", &[]))])
        .mod_files("C", [("C.pak", module_pak("Gamma", &[]))]);
    assert_eq!(t.order(ID).unwrap(), ["Alpha", "Beta", "Gamma"]);
    t.set_order(ID, &["Gamma", "Alpha", "Beta"]).unwrap();
    t.set_list_enabled(ID, "Alpha", false).unwrap();
    t.deploy().unwrap();
    assert_eq!(
        listed(&t.read("profile:modsettings.lsx").unwrap()),
        ["GustavX", "Gamma", "Beta"]
    );
}

#[test]
fn dependencies_are_checked_but_not_the_games_own() {
    let gustav = ("991c9c7a-fb80-40cb-8f0d-b92d4e80e9b1", "Gustav");
    let t = game()
        .mod_files(
            "Needs",
            [(
                "Needs.pak",
                module_pak("Needs", &[(&uuid("Lib"), "Lib"), gustav]),
            )],
        )
        .mod_files("Lib", [("Lib.pak", module_pak("Lib", &[]))])
        .mod_files(
            "Orphan",
            [(
                "Orphan.pak",
                module_pak("Orphan", &[("0000", "Missing Lib"), gustav]),
            )],
        );
    let list = t.list(ID).unwrap();
    let needs = list.items.iter().find(|i| i.item.name == "Needs").unwrap();
    assert_eq!(needs.item.after, ["Lib"]);
    let messages: Vec<_> = list.problems.iter().map(|p| p.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Needs") && m.contains("Lib")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("Missing Lib")),
        "{messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("Gustav")),
        "{messages:?}"
    );
}

#[test]
fn overrides_are_deployed_but_not_listed() {
    let t = game().mod_files(
        "Override",
        [(
            "Override.pak",
            pak(&[("Public/Shared/x.lsf", b"x", How::Stored)]),
        )],
    );
    assert!(t.order(ID).unwrap().is_empty());
    let plan = t.deploy().unwrap();
    assert!(plan.places("mods:Override.pak"));
    assert_eq!(
        listed(&t.read("profile:modsettings.lsx").unwrap()),
        ["GustavX"]
    );
}

#[test]
fn unreadable_packages_are_warned_about() {
    let t = game().mod_files("Broken", [("Broken.pak", b"not a package".to_vec())]);
    let list = t.list(ID).unwrap();
    assert!(
        list.problems
            .iter()
            .any(|p| p.message.contains("couldn't read Broken.pak")),
        "{:?}",
        list.problems
    );
}

#[test]
fn a_later_copy_of_a_module_wins() {
    let t = game()
        .mod_files("Old", [("Cool_v1.pak", module_pak("Cool", &[]))])
        .mod_files("New", [("Cool_v2.pak", module_pak("Cool", &[]))]);
    let list = t.list(ID).unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].item.mod_name, "New");
    assert!(
        list.problems
            .iter()
            .any(|p| p.message.contains("Cool_v1.pak") && p.message.contains("Cool_v2.pak")),
        "{:?}",
        list.problems
    );
}

#[test]
fn later_parts_of_a_split_package_are_skipped() {
    let all = ["Big.pak", "Big_1.pak", "Other_2.pak"];
    assert!(is_later_part("Big_1.pak", all.into_iter()));
    assert!(!is_later_part("Big.pak", all.into_iter()));
    assert!(!is_later_part("Other_2.pak", all.into_iter()));
    assert!(!is_later_part("No_Digits.pak", all.into_iter()));
}

#[test]
fn reads_files_from_later_parts() {
    let dir = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..5000u32).flat_map(|i| (i % 7).to_le_bytes()).collect();
    let parts = split(&[
        ("first.txt", b"one", How::Stored, 0),
        ("second.txt", &big, How::Lz4, 1),
        ("third.txt", b"three", How::Zlib, 2),
    ]);
    assert_eq!(parts.len(), 3);
    for (i, bytes) in parts.iter().enumerate() {
        let name = if i == 0 {
            "Big.pak".to_owned()
        } else {
            format!("Big_{i}.pak")
        };
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    let mut p = Pak::open(&dir.path().join("Big.pak")).unwrap();
    let entries = p.entries.clone();
    let part = |name: &str| Some(dir.path().join(name));
    assert_eq!(p.read(&entries[0], "Big.pak", &part).unwrap(), b"one");
    assert_eq!(p.read(&entries[1], "Big.pak", &part).unwrap(), big);
    assert_eq!(p.read(&entries[2], "Big.pak", &part).unwrap(), b"three");

    let err = p.read(&entries[1], "Big.pak", &|_| None).unwrap_err();
    assert!(err.contains("Big_1.pak"), "{err}");
    std::fs::remove_file(dir.path().join("Big_2.pak")).unwrap();
    let err = p.read(&entries[2], "Big.pak", &part).unwrap_err();
    assert!(err.contains("Big_2.pak"), "{err}");
}

#[test]
fn part_names_follow_the_package() {
    assert_eq!(pak::part_name("Big.pak", 1), "Big_1.pak");
    assert_eq!(pak::part_name("Big.PAK", 2), "Big_2.pak");
    assert_eq!(pak::part_name("Odd", 3), "Odd_3.pak");
}

#[test]
fn modules_in_a_later_part_are_listed() {
    let meta = meta_lsx("Huge", &[]);
    let parts = split(&[
        ("Public/x.txt", b"data", How::Stored, 0),
        ("Mods/HugeFolder/meta.lsx", meta.as_bytes(), How::Lz4, 1),
    ]);
    let t = game().mod_files(
        "Huge",
        [
            ("Huge/Huge.pak", parts[0].clone()),
            ("Huge/Huge_1.pak", parts[1].clone()),
        ],
    );
    let plan = t.deploy().unwrap();
    assert!(plan.places("mods:Huge.pak"));
    assert!(plan.places("mods:Huge_1.pak"));
    assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
    let settings = t.read("profile:modsettings.lsx").unwrap();
    assert_eq!(listed(&settings), ["GustavX", "Huge"]);
}

/// An installer with a choice of two packages, put in different folders as
/// BG3 installers often do.
const FOMOD: &str = r#"<config>
    <moduleName>Pick One</moduleName>
    <installSteps>
        <installStep name="Main">
            <optionalFileGroups>
                <group name="Look" type="SelectExactlyOne">
                    <plugins>
                        <plugin name="Light">
                            <description />
                            <files><file source="light/Light.pak" destination="Light.pak" /></files>
                            <typeDescriptor><type name="Recommended" /></typeDescriptor>
                        </plugin>
                        <plugin name="Dark">
                            <description />
                            <files><file source="dark/Dark.pak" destination="PAK_FILES/Dark.pak" /></files>
                            <typeDescriptor><type name="Optional" /></typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
            </optionalFileGroups>
        </installStep>
    </installSteps>
</config>"#;

#[test]
fn fomod_installers_pick_which_packages_are_deployed() {
    let t = game().extension(Fomod).mod_files(
        "Looks",
        [
            ("fomod/ModuleConfig.xml", FOMOD.as_bytes().to_vec()),
            ("light/Light.pak", module_pak("Light", &[])),
            ("dark/Dark.pak", module_pak("Dark", &[])),
        ],
    );
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        ["mods:Light.pak", "profile:modsettings.lsx"],
        "only the picked package is deployed"
    );
    let settings = t.read("profile:modsettings.lsx").unwrap();
    assert_eq!(listed(&settings), ["GustavX", "Light"]);

    t.answer_options(
        "Looks",
        &[Answer::picks(
            "Main/Look".to_owned(),
            vec!["Dark".to_owned()],
        )],
    )
    .unwrap();
    let plan = t.deploy().unwrap();
    assert_eq!(plan.placed(), ["mods:Dark.pak", "profile:modsettings.lsx"]);
    let settings = t.read("profile:modsettings.lsx").unwrap();
    assert_eq!(listed(&settings), ["GustavX", "Dark"]);
}

#[test]
fn warns_when_script_extender_is_missing() {
    let extender_pak = || {
        let meta = meta_lsx("Scripted", &[]);
        pak(&[
            ("Mods/ScriptedFolder/meta.lsx", meta.as_bytes(), How::Zlib),
            (
                "Mods/ScriptedFolder/ScriptExtender/Config.json",
                b"{}",
                How::Stored,
            ),
        ])
    };
    let without = game().mod_files("Scripted", [("Scripted.pak", extender_pak())]);
    let list = without.list(ID).unwrap();
    assert_eq!(list.items[0].item.badges, [NEEDS_EXTENDER]);
    assert!(
        list.problems
            .iter()
            .any(|p| p.message.contains("Scripted needs Script Extender")),
        "{:?}",
        list.problems
    );
    without.set_list_enabled(ID, "Scripted", false).unwrap();
    assert!(
        !without
            .list(ID)
            .unwrap()
            .problems
            .iter()
            .any(|p| p.message.contains("Script Extender"))
    );

    let with = game()
        .mod_files("Scripted", [("Scripted.pak", extender_pak())])
        .mod_files("BG3SE", [("DWrite.dll", b"dll".to_vec())]);
    assert!(
        !with
            .list(ID)
            .unwrap()
            .problems
            .iter()
            .any(|p| p.message.contains("Script Extender"))
    );
}

/// How `modsettings.lsx` lists module `name` (as [`module_pak`] makes it).
fn short(name: &str) -> Module {
    Module {
        uuid: uuid(name),
        folder: format!("{name}Folder"),
        name: name.to_owned(),
        version64: 36028797018963969,
        publish_handle: 7,
        md5: format!("md5-of-{name}"),
        dependencies: Vec::new(),
    }
}

/// The game's `modsettings.lsx` loading `names` after the campaign.
fn game_settings(names: &[&str]) -> String {
    let base = Module {
        uuid: "cb555efe-2d9e-131f-8195-a89329d218ea".to_owned(),
        folder: "GustavX".to_owned(),
        name: "GustavX".to_owned(),
        version64: 145241946983074840,
        publish_handle: 0,
        md5: String::new(),
        dependencies: Vec::new(),
    };
    let modules: Vec<Module> = names.iter().map(|n| short(n)).collect();
    let all: Vec<&Module> = std::iter::once(&base).chain(&modules).collect();
    meta::modsettings(&all)
}

#[test]
fn reads_modsettings_back() {
    let text = game_settings(&["A & B", "C"]);
    let modules = meta::parse_modsettings(&text).unwrap();
    assert_eq!(modules.len(), 3);
    assert_eq!(modules[1], short("A & B"));
    assert_eq!(modules[2].md5, "md5-of-C");
    assert!(meta::parse_modsettings("<save/>").unwrap().is_empty());
    assert!(meta::parse_modsettings("not xml").is_err());
}

#[test]
fn mods_from_the_in_game_mod_manager_stay_loaded() {
    let t = game()
        // From the in-game mod manager: its package and its place in the
        // game's load order. `Gone` was uninstalled but is still listed,
        // `Off` is installed but turned off.
        .game_file("mods:InGame.pak", module_pak("InGame", &[]))
        .game_file("mods:Off.pak", module_pak("Off", &[]))
        .game_file(
            "profile:modsettings.lsx",
            game_settings(&["Gone", "InGame", "Cool"]),
        )
        .mod_files("Cool", [("Cool.pak", module_pak("Cool", &[]))]);

    let list = t.list(ID).unwrap();
    let fixed: Vec<_> = list.fixed.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(fixed, ["GustavX", "InGame"]);
    assert_eq!(list.fixed[1].badges, [NOT_IN_PROFILE]);
    assert_eq!(t.order(ID).unwrap(), ["Cool"]);

    t.deploy().unwrap();
    let written = t.read("profile:modsettings.lsx").unwrap();
    assert_eq!(listed(&written), ["GustavX", "InGame", "Cool"]);
    // Kept as the game had it, so the in-game mod manager knows it.
    assert!(written.contains("md5-of-InGame"));
    assert!(t.exists("mods:InGame.pak"));
    let status = || t.patchwork().deploy_status(t.profile()).unwrap();
    assert_eq!(status(), DeployStatus::Deployed);

    // The game rewrites it as it starts, which changes nothing.
    std::fs::write(
        t.path("profile:modsettings.lsx"),
        game_settings(&["InGame", "Cool"]),
    )
    .unwrap();
    assert_eq!(status(), DeployStatus::Deployed);

    // Turning it off in the game does.
    std::fs::write(t.path("profile:modsettings.lsx"), game_settings(&["Cool"])).unwrap();
    assert_eq!(status(), DeployStatus::Stale);
    t.deploy().unwrap();
    assert_eq!(
        listed(&t.read("profile:modsettings.lsx").unwrap()),
        ["GustavX", "Cool"]
    );
    assert_eq!(status(), DeployStatus::Deployed);
}

#[test]
fn the_profiles_copy_of_a_module_wins() {
    let t = game()
        .game_file("mods:Cool_by_hand.pak", module_pak("Cool", &[]))
        .game_file("profile:modsettings.lsx", game_settings(&["Cool"]))
        .mod_files("Cool", [("Cool.pak", module_pak("Cool", &[]))]);
    let list = t.list(ID).unwrap();
    assert_eq!(list.fixed.len(), 1);
    assert_eq!(t.order(ID).unwrap(), ["Cool"]);
    t.deploy().unwrap();
    assert_eq!(
        listed(&t.read("profile:modsettings.lsx").unwrap()),
        ["GustavX", "Cool"]
    );
}
