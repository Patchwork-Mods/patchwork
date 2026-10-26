use super::*;

fn registry_ruleset(id: &str) -> Option<toml::Table> {
    let text = match id {
        "bepinex" => include_str!("../../../../registry/rulesets/bepinex.toml"),
        "bepinex-loader" => include_str!("../../../../registry/rulesets/bepinex-loader.toml"),
        "recursive-melonloader" => {
            include_str!("../../../../registry/rulesets/recursive-melonloader.toml")
        }
        "recursive-melonloader-loader" => {
            include_str!("../../../../registry/rulesets/recursive-melonloader-loader.toml")
        }
        "melonloader-loader" => {
            include_str!("../../../../registry/rulesets/melonloader-loader.toml")
        }
        _ => return None,
    };
    Some(toml::from_str(text).unwrap())
}

fn parse(text: &str) -> Result<Vec<CompiledRule>> {
    let ruleset: Ruleset = toml::from_str(text).unwrap();
    let tables: Vec<_> = [
        "bepinex",
        "bepinex-loader",
        "recursive-melonloader",
        "recursive-melonloader-loader",
        "melonloader-loader",
    ]
    .into_iter()
    .map(|id| (id, registry_ruleset(id).unwrap()))
    .collect();
    let lookup = |id: &str| tables.iter().find(|(t, _)| *t == id).map(|(_, t)| t);
    let mut out = Vec::new();
    RulesDeployer.expand(&lookup, ruleset.rules, &mut out, 0)?;
    Ok(out)
}

fn bepinex() -> Vec<CompiledRule> {
    parse("[[rules]]\nruleset = \"bepinex\"\n").unwrap()
}

fn dst(rules: &[CompiledRule], mod_name: &str, path: &str) -> Option<String> {
    match place(rules, mod_name, &RelPath::new(path).unwrap()).unwrap() {
        Outcome::Place(p) => Some(p.to_string()),
        _ => None,
    }
}

#[test]
fn bepinex_layout() {
    let rules = bepinex();
    let cases = [
        // Route folders keep their contents' layout, wherever they are and
        // however they're capitalized.
        ("plugins/Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
        (
            "BepInEx/plugins/X/x.dll",
            Some("BepInEx/plugins/Cool/X/x.dll"),
        ),
        (
            "Wrapper/Plugins/X/x.dll",
            Some("BepInEx/plugins/Cool/X/x.dll"),
        ),
        (
            "BepInEx/patchers/X/x.dll",
            Some("BepInEx/patchers/Cool/X/x.dll"),
        ),
        ("core/x.dll", Some("BepInEx/core/Cool/x.dll")),
        ("monomod/x.mm.dll", Some("BepInEx/monomod/Cool/x.mm.dll")),
        ("config/cool.cfg", Some("BepInEx/config/cool.cfg")),
        ("BepInEx/config/x.cfg", Some("BepInEx/config/x.cfg")),
        // The outermost route folder wins.
        ("config/plugins/x.cfg", Some("BepInEx/config/plugins/x.cfg")),
        // Everything else goes by extension, flattened.
        ("Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
        ("x.MM.dll", Some("BepInEx/monomod/Cool/x.MM.dll")),
        ("Lib/x.mm.dll", Some("BepInEx/monomod/Cool/x.mm.dll")),
        (
            "assets/thing.bundle",
            Some("BepInEx/plugins/Cool/thing.bundle"),
        ),
        ("BepInEx/other/a.txt", Some("BepInEx/plugins/Cool/a.txt")),
        ("manifest.json", Some("BepInEx/plugins/Cool/manifest.json")),
        ("icon.png", Some("BepInEx/plugins/Cool/icon.png")),
    ];
    for (path, expected) in cases {
        assert_eq!(dst(&rules, "Cool", path).as_deref(), expected, "{path}");
    }
}

#[test]
fn bepinex_loader_packs() {
    let rules = bepinex();
    let cases = [
        (
            "bbepis-BepInExPack",
            "BepInExPack/winhttp.dll",
            Some("winhttp.dll"),
        ),
        (
            "bbepis-BepInExPack",
            "BepInExPack/BepInEx/core/BepInEx.dll",
            Some("BepInEx/core/BepInEx.dll"),
        ),
        ("bbepis-BepInExPack", "manifest.json", None),
        ("BBEPIS-bepinexpack", "README.md", None),
        (
            "denikson-BepInExPack_Valheim",
            "BepInExPack_Valheim/doorstop_config.ini",
            Some("doorstop_config.ini"),
        ),
        (
            "BepInEx-BepInExPack_Core_Keeper",
            "BepInExPack_Core-Keeper/winhttp.dll",
            Some("winhttp.dll"),
        ),
        (
            "1F31A-BepInEx_Valheim_Full",
            "BepInEx_Valheim_Full/BepInEx/core/BepInEx.dll",
            Some("BepInEx/core/BepInEx.dll"),
        ),
        (
            "bbepis-BepInEx_Rogue_Tower",
            "BepInEx/core/BepInEx.dll",
            Some("BepInEx/core/BepInEx.dll"),
        ),
        (
            "bbepis-BepInEx_Rogue_Tower",
            "winhttp.dll",
            Some("winhttp.dll"),
        ),
        ("bbepis-BepInEx_Rogue_Tower", "icon.png", None),
        // Unknown packs (say, from a local zip) are recognized by folder.
        ("MyPack", "BepInExPack/winhttp.dll", Some("winhttp.dll")),
    ];
    for (mod_name, path, expected) in cases {
        assert_eq!(
            dst(&rules, mod_name, path).as_deref(),
            expected,
            "{mod_name}: {path}"
        );
    }
}

#[test]
fn recursive_melonloader_layout() {
    let rules = parse("[[rules]]\nruleset = \"recursive-melonloader\"\n").unwrap();
    let cases = [
        // Mods keep their layout in a folder of their own.
        ("Cool.dll", Some("Mods/Author-Cool/Cool.dll")),
        ("Mods/Cool.dll", Some("Mods/Author-Cool/Mods/Cool.dll")),
        ("manifest.json", Some("Mods/Author-Cool/manifest.json")),
        // Except the mod's UserData, at the top only.
        ("UserData/cool.cfg", Some("UserData/Author-Cool/cool.cfg")),
        (
            "Sub/UserData/x.cfg",
            Some("Mods/Author-Cool/Sub/UserData/x.cfg"),
        ),
    ];
    for (path, expected) in cases {
        assert_eq!(
            dst(&rules, "Author-Cool", path).as_deref(),
            expected,
            "{path}"
        );
    }

    // The loader brings only its own files.
    let cases = [
        ("version.dll", Some("version.dll")),
        ("winhttp.dll", Some("winhttp.dll")),
        (
            "MelonLoader/net6/MelonLoader.dll",
            Some("MelonLoader/net6/MelonLoader.dll"),
        ),
        ("manifest.json", None),
        ("NOTICE.txt", None),
    ];
    for (path, expected) in cases {
        assert_eq!(
            dst(&rules, "LavaGang-MelonLoader", path).as_deref(),
            expected,
            "{path}"
        );
    }
}

#[test]
fn melonloader_loader_pack() {
    let rules = parse("[[rules]]\nruleset = \"melonloader-loader\"\n").unwrap();
    let cases = [
        ("version.dll", Some("version.dll")),
        (
            "MelonLoader/MelonLoader.dll",
            Some("MelonLoader/MelonLoader.dll"),
        ),
        ("NOTICE.txt", Some("NOTICE.txt")),
        ("manifest.json", None),
        ("readme.md", None),
    ];
    for (path, expected) in cases {
        assert_eq!(
            dst(&rules, "LavaGang-MelonLoader", path).as_deref(),
            expected,
            "{path}"
        );
    }
    assert_eq!(dst(&rules, "Author-Cool", "x.dll"), None);
}

#[test]
fn mod_filters_and_escapes() {
    let rule = |files: &str, mods: Option<&str>, to: &str| {
        RulesDeployer
            .compile(Rule {
                ruleset: None,
                files: Some(files.to_owned()),
                folder: None,
                mods: mods.map(|m| Patterns::One(m.to_owned())),
                strip: 0,
                to: Some(to.to_owned()),
                skip: false,
            })
            .unwrap()
    };
    let rules = [rule("*.pak", Some("Big*"), "Paks/{file}")];
    assert_eq!(
        dst(&rules, "BigMod", "a.pak").as_deref(),
        Some("Paks/a.pak")
    );
    assert_eq!(dst(&rules, "Small", "a.pak"), None);

    let evil = [rule("**", None, "../{path}")];
    assert!(place(&evil, "m", &RelPath::new("x").unwrap()).is_err());
}

#[test]
fn mod_lists() {
    let rules = parse("[[rules]]\nmods = [\"a-*\", \"B-exact\"]\nto = \"x/{file}\"\n").unwrap();
    assert_eq!(dst(&rules, "a-thing", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "b-EXACT", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "c", "f"), None);
}

#[test]
fn includes_must_stand_alone_and_exist() {
    let mixed = parse("[[rules]]\nruleset = \"bepinex\"\nskip = true\n");
    assert!(mixed.is_err());
    let missing = parse("[[rules]]\nruleset = \"nope\"\n");
    assert!(missing.is_err());
    // Rules around an include keep their place in the order.
    let rules = parse(
        "[[rules]]\nfiles = \"first.txt\"\nto = \"a/{file}\"\n\
         [[rules]]\nruleset = \"bepinex\"\n",
    )
    .unwrap();
    assert_eq!(
        dst(&rules, "m", "first.txt").as_deref(),
        Some("a/first.txt")
    );
    assert_eq!(
        dst(&rules, "m", "other.txt").as_deref(),
        Some("BepInEx/plugins/m/other.txt")
    );
}

const SKYRIM_RULES: &str = r#"
[[rules]]
folder = "Data"
to = "Data/{path}"

[[rules]]
files = "*.exe"
to = "{path}"

[[rules]]
files = "*.dll"
to = "{path}"

[[rules]]
to = "Data/{path}"
"#;

const SKYRIM_ROOT: &str = r#"
[root]
markers = ["Data", "meshes", "textures", "scripts", "interface", "SKSE", "*.esp", "*.esm", "*.esl", "*.bsa"]
unless = ["fomod/ModuleConfig.xml"]
"#;

/// Plans one mod with `files` under `config`, giving its destinations and
/// warnings, sorted.
fn deploy(config: &str, files: &[&str]) -> Result<(Vec<String>, Vec<String>)> {
    let config: Config = toml::from_str(config).unwrap();
    let setup = RulesDeployer.setup(config, &|_| None)?;
    let m = InstalledMod {
        name: "Cool".to_owned(),
        source: "local".to_owned(),
        id: "cool".to_owned(),
        version: None,
        hash: patchwork_core::hash::ContentHash::of(b""),
        files: files
            .iter()
            .map(|f| patchwork_core::deploy::ModFile {
                rel: RelPath::new(f).unwrap(),
                src: f.into(),
            })
            .collect(),
    };
    let mut desired = DesiredState::default();
    RulesDeployer.plan_mod(&setup, &m, &mut desired)?;
    let mut dsts: Vec<_> = desired
        .placements
        .iter()
        .map(|p| match &p.src {
            patchwork_core::deploy::Src::Mod { rel, .. } => format!("{rel} -> {:?}", p.dst),
            _ => unreachable!(),
        })
        .collect();
    dsts.sort();
    desired.warnings.sort();
    Ok((dsts, desired.warnings))
}

/// Just the destinations, for mods that deploy without warnings.
fn tops(config: &str, files: &[&str]) -> Vec<String> {
    let (dsts, warnings) = deploy(config, files).unwrap();
    assert!(warnings.is_empty(), "{files:?}: {warnings:?}");
    dsts
}

#[test]
fn root_layouts() {
    let config = format!("{SKYRIM_ROOT}{SKYRIM_RULES}");
    let cases: &[(&[&str], &[&str])] = &[
        (&["meshes/a.nif"], &["Data/meshes/a.nif"]),
        (&["Data/meshes/a.nif"], &["Data/meshes/a.nif"]),
        (&["Wrapper/meshes/a.nif"], &["Data/meshes/a.nif"]),
        (&["Wrapper/Data/meshes/a.nif"], &["Data/meshes/a.nif"]),
        (&["Wrapper/Plugin.esp"], &["Data/Plugin.esp"]),
        (
            &["skse64_loader.exe", "Data/Scripts/x.pex"],
            &["Data/Scripts/x.pex", "skse64_loader.exe"],
        ),
        // Markers and the top folder ignore case.
        (&["wrapper/MESHES/a.nif"], &["Data/MESHES/a.nif"]),
        (&["Wrapper/data/meshes/a.nif"], &["Data/meshes/a.nif"]),
        (
            &["Wrapper/meshes/a.nif", "WRAPPER/textures/b.dds"],
            &["Data/meshes/a.nif", "Data/textures/b.dds"],
        ),
        // Nothing marks it, so the rules see it as packed.
        (&["Wrapper/notes/a.txt"], &["Data/Wrapper/notes/a.txt"]),
    ];
    for (files, expected) in cases {
        let mut dsts: Vec<_> = tops(&config, files)
            .into_iter()
            .map(|d| d.split_once(" -> ").unwrap().1.to_owned())
            .collect();
        dsts.sort();
        let expected: Vec<_> = expected
            .iter()
            .map(|e| format!("{:?}", Dest::game(RelPath::new(e).unwrap())))
            .collect();
        assert_eq!(dsts, expected, "{files:?}");
    }
}

#[test]
fn root_keeps_sources() {
    let config = format!("{SKYRIM_ROOT}{SKYRIM_RULES}");
    let dsts = tops(&config, &["Wrapper/meshes/a.nif"]);
    let expected = format!(
        "Wrapper/meshes/a.nif -> {:?}",
        Dest::game(RelPath::new("Data/meshes/a.nif").unwrap())
    );
    assert_eq!(dsts, [expected]);
}

#[test]
fn root_leaves_out_files_outside_the_top() {
    let config = format!("{SKYRIM_ROOT}{SKYRIM_RULES}");
    let (dsts, warnings) = deploy(
        &config,
        &["readme.txt", "screenshot.png", "Wrapper/meshes/a.nif"],
    )
    .unwrap();
    assert_eq!(dsts.len(), 1);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("2 file(s)"), "{warnings:?}");
}

#[test]
fn root_with_variants_strips_nothing() {
    let config = format!("{SKYRIM_ROOT}{SKYRIM_RULES}");
    let (dsts, warnings) =
        deploy(&config, &["Option A/meshes/a.nif", "Option B/meshes/a.nif"]).unwrap();
    assert!(dsts.iter().all(|d| d.contains("Data/Option ")), "{dsts:?}");
    assert_eq!(dsts.len(), 2);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("`option a`"), "{warnings:?}");
}

#[test]
fn root_leaves_unless_mods_alone() {
    let config = format!("{SKYRIM_ROOT}{SKYRIM_RULES}");
    let (dsts, warnings) =
        deploy(&config, &["fomod/ModuleConfig.xml", "Wrapper/meshes/a.nif"]).unwrap();
    assert!(
        dsts.iter().any(|d| d.contains("Data/Wrapper/meshes/a.nif")),
        "{dsts:?}"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn no_root_changes_nothing() {
    let files = ["Wrapper/meshes/a.nif", "readme.txt", "skse64_loader.exe"];
    let dsts = tops(SKYRIM_RULES, &files);
    let rules = parse(SKYRIM_RULES).unwrap();
    let mut expected: Vec<_> = files
        .iter()
        .map(|f| {
            let d = dst(&rules, "Cool", f).unwrap();
            format!("{f} -> {:?}", Dest::game(RelPath::new(&d).unwrap()))
        })
        .collect();
    expected.sort();
    assert_eq!(dsts, expected);
}

#[test]
fn bad_root_configs() {
    let empty = format!("[root]\nmarkers = []\n{SKYRIM_RULES}");
    assert!(deploy(&empty, &["a"]).is_err());
    let bad_marker = format!("[root]\nmarkers = [\"[\"]\n{SKYRIM_RULES}");
    assert!(deploy(&bad_marker, &["a"]).is_err());
    let bad_unless = format!("[root]\nmarkers = [\"Data\"]\nunless = [\"a{{\"]\n{SKYRIM_RULES}");
    assert!(deploy(&bad_unless, &["a"]).is_err());
    let unknown: std::result::Result<Config, _> =
        toml::from_str("[root]\nmarkers = [\"Data\"]\nother = 1\n");
    assert!(unknown.is_err());
}
