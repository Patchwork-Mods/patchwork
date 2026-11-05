//! The Bethesda and FOMOD extensions end to end, on Skyrim SE as the
//! registry defines it, through `patchwork_core::ext::testing`.

use patchwork_bethesda::{Bethesda, Fomod};
use patchwork_core::ext::testing::TestGame;
use patchwork_core::options::Answer;

/// A plugin's bytes: a TES4 record with `flags` and `masters`.
fn plugin(flags: u32, masters: &[&str]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut sub = |kind: &[u8], field: &[u8]| {
        data.extend_from_slice(kind);
        data.extend_from_slice(&(field.len() as u16).to_le_bytes());
        data.extend_from_slice(field);
    };
    sub(b"HEDR", &[0; 12]);
    for master in masters {
        sub(b"MAST", format!("{master}\0").as_bytes());
        sub(b"DATA", &[0; 8]);
    }
    let mut bytes = b"TES4".to_vec();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&data);
    bytes
}

const MASTER: u32 = 0x1;

fn skyrim() -> TestGame {
    TestGame::builtin("skyrim-se")
        .extension(Bethesda)
        .extension(Fomod)
        .game_file("SkyrimSE.exe", b"")
        .game_file("Data/Skyrim.esm", plugin(MASTER, &[]))
}

#[test]
fn plugins_are_ordered_and_written_to_plugins_txt() {
    let t = skyrim()
        .mod_files("Base", [("Base.esm", plugin(MASTER, &["Skyrim.esm"]))])
        .mod_files(
            "Patch",
            [
                ("Data/Patch.esp", plugin(0, &["Base.esm"])),
                ("Data/textures/a.dds", Vec::new()),
            ],
        )
        .mod_files("Other", [("Other.esp", plugin(0, &[]))]);

    assert_eq!(
        t.order("bethesda.plugins").unwrap(),
        ["Base.esm", "Patch.esp", "Other.esp"]
    );
    t.set_order("bethesda.plugins", &["Other.esp", "Patch.esp"])
        .unwrap();
    assert_eq!(
        t.order("bethesda.plugins").unwrap(),
        ["Base.esm", "Other.esp", "Patch.esp"]
    );

    let plan = t.deploy().unwrap();
    assert!(plan.places("Data/Patch.esp"));
    assert!(plan.places("Data/textures/a.dds"));
    assert_eq!(plan.placed_by("Data/Base.esm"), Some("Base"));
    assert!(plan.places("appdata:plugins.txt"));
    let written = t.read("appdata:plugins.txt").unwrap();
    let lines: Vec<&str> = written.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines, ["*Base.esm", "*Other.esp", "*Patch.esp"]);

    // a master that's missing is an issue
    t.set_enabled("Base", false).unwrap();
    let issues = t.issues().unwrap();
    assert!(
        issues.iter().any(|i| i.message.contains("Base.esm")),
        "{issues:?}"
    );
}

const FOMOD: &str = r#"<config>
    <moduleName>Pick One</moduleName>
    <installSteps>
        <installStep name="Main">
            <optionalFileGroups>
                <group name="Look" type="SelectExactlyOne">
                    <plugins>
                        <plugin name="Light">
                            <description />
                            <files><file source="light/Look.esp" destination="Look.esp" /></files>
                            <typeDescriptor><type name="Recommended" /></typeDescriptor>
                        </plugin>
                        <plugin name="Dark">
                            <description />
                            <files><file source="dark/Look.esp" destination="Look.esp" /></files>
                            <typeDescriptor><type name="Optional" /></typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
            </optionalFileGroups>
        </installStep>
    </installSteps>
</config>"#;

#[test]
fn fomod_picks_say_what_is_deployed() {
    let t = skyrim().mod_files(
        "Looks",
        [
            ("fomod/ModuleConfig.xml", FOMOD.as_bytes().to_vec()),
            ("light/Look.esp", plugin(0, &["Light.esm"])),
            ("dark/Look.esp", plugin(0, &["Dark.esm"])),
        ],
    );

    let view = t.options("Looks").unwrap();
    assert!(!view.saved);
    let group = &view.form.pages[0].groups[0];
    assert_eq!(group.id, "Main/Look");
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        ["Data/Look.esp", "appdata:plugins.txt"],
        "only the picked files are installed"
    );
    let light = t.read("Data/Look.esp").unwrap();
    assert!(light.contains("Light.esm"));

    t.answer_options(
        "Looks",
        &[Answer {
            group: "Main/Look".to_owned(),
            items: vec!["Dark".to_owned()],
        }],
    )
    .unwrap();
    assert!(t.options("Looks").unwrap().saved);
    let plan = t.deploy().unwrap();
    assert!(plan.places("Data/Look.esp"));
    assert!(t.read("Data/Look.esp").unwrap().contains("Dark.esm"));
}
