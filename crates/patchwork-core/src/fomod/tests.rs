use std::path::PathBuf;

use super::*;

const CONFIG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
    <moduleName>Better Things</moduleName>
    <requiredInstallFiles>
        <folder source="00 Core" destination="" />
    </requiredInstallFiles>
    <installSteps order="Explicit">
        <installStep name="Textures">
            <optionalFileGroups order="Explicit">
                <group name="Resolution" type="SelectExactlyOne">
                    <plugins order="Explicit">
                        <plugin name="2K">
                            <description>Smaller.</description>
                            <image path="fomod\images\2k.png" />
                            <files><folder source="10 Textures\2K" destination="textures" /></files>
                            <typeDescriptor><type name="Optional" /></typeDescriptor>
                        </plugin>
                        <plugin name="4K">
                            <description><![CDATA[Bigger & <b>sharper</b>.]]></description>
                            <files><folder source="10 Textures\4K" destination="textures" /></files>
                            <conditionFlags><flag name="big">On</flag></conditionFlags>
                            <typeDescriptor><type name="Recommended" /></typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
            </optionalFileGroups>
        </installStep>
        <installStep name="Big extras">
            <visible><flagDependency flag="big" value="On" /></visible>
            <optionalFileGroups>
                <group name="Extras" type="SelectAny">
                    <plugins>
                        <plugin name="Zeta">
                            <description />
                            <files><file source="extras/zeta.esp" destination="Zeta.esp" /></files>
                            <typeDescriptor><type name="Optional" /></typeDescriptor>
                        </plugin>
                        <plugin name="Alpha">
                            <description />
                            <files><file source="extras/alpha.esp" destination="" /></files>
                            <typeDescriptor>
                                <dependencyType>
                                    <defaultType name="Optional" />
                                    <patterns>
                                        <pattern>
                                            <dependencies operator="And">
                                                <fileDependency file="SkyUI_SE.esp" state="Active" />
                                            </dependencies>
                                            <type name="Recommended" />
                                        </pattern>
                                    </patterns>
                                </dependencyType>
                            </typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
            </optionalFileGroups>
        </installStep>
    </installSteps>
    <conditionalFileInstalls>
        <patterns>
            <pattern>
                <dependencies operator="Or">
                    <flagDependency flag="big" value="On" />
                    <fileDependency file="Nothing.esp" state="Active" />
                </dependencies>
                <files><file source="extras\big.ini" destination="Big.ini" priority="1" /></files>
            </pattern>
        </patterns>
    </conditionalFileInstalls>
</config>
"#;

const FILES: &[&str] = &[
    "fomod/ModuleConfig.xml",
    "fomod/images/2K.png",
    "00 Core/Better.esp",
    "00 Core/Big.ini",
    "10 Textures/2K/a.dds",
    "10 Textures/4K/a.dds",
    "10 Textures/4K/sub/b.dds",
    "extras/zeta.esp",
    "extras/alpha.esp",
    "extras/big.ini",
];

fn rels(files: &[&str]) -> Vec<RelPath> {
    files.iter().map(|f| RelPath::new(f).unwrap()).collect()
}

fn installer() -> Installer {
    Installer::parse(CONFIG.as_bytes(), String::new()).unwrap()
}

fn missing(_: &str) -> FileState {
    FileState::Missing
}

fn resolve(saved: &[Selection], state: &dyn Fn(&str) -> FileState) -> Resolved {
    installer().resolve(&rels(FILES), saved, state)
}

fn installed(r: &Resolved) -> Vec<String> {
    r.files
        .iter()
        .map(|(dst, src)| format!("{dst} <- {src}"))
        .collect()
}

fn select(step: &str, group: &str, options: &[&str]) -> Selection {
    Selection {
        step: step.to_owned(),
        group: group.to_owned(),
        options: options.iter().map(|o| o.to_string()).collect(),
    }
}

#[test]
fn defaults_are_the_recommended_options() {
    let r = resolve(&[], &missing);
    assert!(r.defaulted());
    let steps: Vec<_> = r.steps.iter().map(|s| s.name.as_str()).collect();
    // 4K sets the flag that shows the second step
    assert_eq!(steps, ["Textures", "Big extras"]);
    let resolution = &r.steps[0].groups[0];
    assert_eq!(resolution.kind, GroupKind::ExactlyOne);
    assert!(!resolution.options[0].picked);
    assert!(resolution.options[1].picked);
    assert_eq!(
        resolution.options[1].description,
        "Bigger & <b>sharper</b>."
    );
    // case and slashes don't have to match the mod's files
    assert_eq!(
        resolution.options[0].image.as_ref().map(|i| i.as_str()),
        Some("fomod/images/2K.png")
    );
    assert_eq!(
        r.selections,
        [
            select("Textures", "Resolution", &["4K"]),
            select("Big extras", "Extras", &[]),
        ]
    );
    assert_eq!(
        installed(&r),
        [
            "Better.esp <- 00 Core/Better.esp",
            // the conditional file's higher priority wins over the core one
            "Big.ini <- extras/big.ini",
            "textures/a.dds <- 10 Textures/4K/a.dds",
            "textures/sub/b.dds <- 10 Textures/4K/sub/b.dds",
        ]
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn saved_picks_are_used_and_hide_steps() {
    let saved = [select("Textures", "Resolution", &["2K"])];
    let r = resolve(&saved, &missing);
    assert!(!r.defaulted());
    assert_eq!(r.steps.len(), 1);
    assert_eq!(
        installed(&r),
        [
            "Better.esp <- 00 Core/Better.esp",
            "Big.ini <- 00 Core/Big.ini",
            "textures/a.dds <- 10 Textures/2K/a.dds",
        ]
    );
}

#[test]
fn picks_for_hidden_steps_are_kept() {
    let saved = [
        select("Textures", "Resolution", &["2K"]),
        select("Big extras", "Extras", &["Zeta"]),
        select("Gone", "Gone", &["Gone"]),
    ];
    let r = resolve(&saved, &missing);
    assert_eq!(r.selections, saved[..2]);
}

#[test]
fn groups_without_an_order_are_sorted_by_name() {
    let saved = [select("Big extras", "Extras", &["Zeta", "Alpha"])];
    let r = resolve(&saved, &missing);
    let extras: Vec<_> = r.steps[1].groups[0]
        .options
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(extras, ["Alpha", "Zeta"]);
    let files = installed(&r);
    // an empty destination installs a file at the top, by its own name
    assert!(
        files.contains(&"alpha.esp <- extras/alpha.esp".to_owned()),
        "{files:?}"
    );
    assert!(
        files.contains(&"Zeta.esp <- extras/zeta.esp".to_owned()),
        "{files:?}"
    );
}

#[test]
fn option_kinds_can_depend_on_files() {
    let r = resolve(&[], &|f| {
        if f == "SkyUI_SE.esp" {
            FileState::Active
        } else {
            FileState::Missing
        }
    });
    let alpha = &r.steps[1].groups[0].options[0];
    assert_eq!(alpha.kind, OptionKind::Recommended);
    assert!(alpha.picked);
}

#[test]
fn picks_are_made_to_fit_their_group() {
    let saved = [select("Textures", "Resolution", &["2K", "4K"])];
    let r = resolve(&saved, &missing);
    let picked: Vec<_> = r.steps[0].groups[0]
        .options
        .iter()
        .map(|o| o.picked)
        .collect();
    assert_eq!(picked, [true, false]);
    // nothing picked in a group that needs one gets the first
    let saved = [select("Textures", "Resolution", &[])];
    let r = resolve(&saved, &missing);
    assert_eq!(r.selections[0], select("Textures", "Resolution", &["2K"]));
}

#[test]
fn installers_are_found_under_a_wrapper_folder() {
    let files: Vec<ModFile> = [
        "Mod/readme.txt",
        "Mod/FOMOD/moduleconfig.xml",
        "Mod/Data/a.esp",
    ]
    .iter()
    .map(|f| ModFile {
        rel: RelPath::new(f).unwrap(),
        src: PathBuf::from(f),
    })
    .collect();
    let (file, base) = find(&files).unwrap();
    assert_eq!(file.rel.as_str(), "Mod/FOMOD/moduleconfig.xml");
    assert_eq!(base, "mod/");

    let config = r#"<config><requiredInstallFiles>
        <file source="Data\a.esp" destination="a.esp" />
        <file source="missing.esp" />
        <folder source="..\.." destination="up" />
    </requiredInstallFiles></config>"#;
    let installer = Installer::parse(config.as_bytes(), base).unwrap();
    let rels: Vec<_> = files.iter().map(|f| f.rel.clone()).collect();
    let r = installer.resolve(&rels, &[], &missing);
    assert_eq!(installed(&r), ["a.esp <- Mod/Data/a.esp"]);
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("`missing.esp`"), "{:?}", r.warnings);
}

#[test]
fn utf16_configs_are_read() {
    let config =
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?><config><moduleName>Wide</moduleName></config>";
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(config.encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(
        Installer::parse(&bytes, String::new()).unwrap().name,
        "Wide"
    );
    // and without a byte order mark
    let bytes: Vec<u8> = config.encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(
        Installer::parse(&bytes, String::new()).unwrap().name,
        "Wide"
    );
}

#[test]
fn broken_configs_say_so() {
    let err = Installer::parse(b"<config><moduleName>", String::new()).unwrap_err();
    assert!(err.starts_with("it isn't valid XML"), "{err}");
}
