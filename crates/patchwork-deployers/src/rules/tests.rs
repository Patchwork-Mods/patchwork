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
        "nexus-bepinex" => include_str!("../../../../registry/rulesets/nexus-bepinex.toml"),
        "nexus-melonloader" => {
            include_str!("../../../../registry/rulesets/nexus-melonloader.toml")
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
        "nexus-bepinex",
        "nexus-melonloader",
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
    kind_dst(rules, mod_name, None, path)
}

fn kind_dst(
    rules: &[CompiledRule],
    mod_name: &str,
    kind: Option<&str>,
    path: &str,
) -> Option<String> {
    let who = Who {
        name: mod_name,
        id: "an-id",
        kind,
        source: "local",
        has: &[],
    };
    match place(rules, &who, &RelPath::new(path).unwrap()).unwrap() {
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
                kinds: None,
                sources: None,
                has: None,
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
    let who = Who {
        name: "m",
        id: "m",
        kind: None,
        source: "local",
        has: &[],
    };
    assert!(place(&evil, &who, &RelPath::new("x").unwrap()).is_err());
}

#[test]
fn mod_lists() {
    let rules = parse("[[rules]]\nmods = [\"a-*\", \"B-exact\"]\nto = \"x/{file}\"\n").unwrap();
    assert_eq!(dst(&rules, "a-thing", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "b-EXACT", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "c", "f"), None);
}

#[test]
fn sources() {
    let rules = parse(
        r#"
[[rules]]
sources = "modio"
to = "Mods/{mod}/{path}"

[[rules]]
sources = ["thunder*", "nexus"]
to = "BepInEx/plugins/{mod}/{file}"
"#,
    )
    .unwrap();
    let dst = |source: &str, path: &str| {
        let who = Who {
            name: "m",
            id: "m",
            kind: None,
            source,
            has: &[],
        };
        match place(&rules, &who, &RelPath::new(path).unwrap()).unwrap() {
            Outcome::Place(p) => Some(p.to_string()),
            _ => None,
        }
    };
    assert_eq!(dst("modio", "a/b.json").as_deref(), Some("Mods/m/a/b.json"));
    assert_eq!(
        dst("thunderstore", "a/b.dll").as_deref(),
        Some("BepInEx/plugins/m/b.dll")
    );
    assert_eq!(
        dst("nexus", "b.dll").as_deref(),
        Some("BepInEx/plugins/m/b.dll")
    );
    assert_eq!(dst("local", "b.dll"), None);
}

#[test]
fn kinds() {
    let rules = parse(
        r#"
[[rules]]
kinds = "shader"
to = "shaderpacks/{file}"

[[rules]]
kinds = ["resourcepack", "texture*"]
to = "resourcepacks/{file}"

[[rules]]
files = "*.jar"
to = "mods/{file}"
"#,
    )
    .unwrap();
    let cases = [
        (Some("shader"), "pack.zip", Some("shaderpacks/pack.zip")),
        (Some("Shader"), "pack.zip", Some("shaderpacks/pack.zip")),
        (
            Some("resourcepack"),
            "pack.zip",
            Some("resourcepacks/pack.zip"),
        ),
        (
            Some("texturepack"),
            "pack.zip",
            Some("resourcepacks/pack.zip"),
        ),
        (Some("mod"), "sodium.jar", Some("mods/sodium.jar")),
        // A kind no rule names still goes by its files...
        (Some("mod"), "pack.zip", None),
        // ...and so does a mod whose source doesn't say.
        (None, "pack.zip", None),
        (None, "sodium.jar", Some("mods/sodium.jar")),
    ];
    for (kind, path, want) in cases {
        assert_eq!(
            kind_dst(&rules, "m", kind, path).as_deref(),
            want,
            "{kind:?} {path}"
        );
    }
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
        kind: None,
        ext: Default::default(),
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

#[test]
fn rulesets_say_which_files_are_config_through_includes() {
    let tables: Vec<(&str, toml::Table)> = vec![
        ("bepinex", registry_ruleset("bepinex").unwrap()),
        ("bepinex-loader", registry_ruleset("bepinex-loader").unwrap()),
        ("nexus-bepinex", registry_ruleset("nexus-bepinex").unwrap()),
        (
            "outer",
            toml::from_str(
                "[[rules]]\nruleset = \"bepinex\"\n\n[[config]]\nfiles = \"x/*.ini\"\nformat = \"a.ini\"\n",
            )
            .unwrap(),
        ),
    ];
    let lookup = |id: &str| tables.iter().find(|(t, _)| *t == id).map(|(_, t)| t);
    let config: Config = toml::from_str("rulesets = [\"outer\", \"bepinex\"]").unwrap();
    let defs = RulesDeployer.config_defs(&lookup, &config).unwrap();
    let formats: Vec<&str> = defs.iter().map(|d| d.format.as_str()).collect();
    // included ones first, as their rules are, and each only once
    assert_eq!(formats, ["bepinex.cfg", "a.ini"]);
    assert_eq!(defs[0].files[0].as_str(), "BepInEx/config/**/*.cfg");
}

#[test]
fn id_placeholder() {
    let rules = parse("[[rules]]\nkinds = \"map\"\nto = \"Levels/{id}/{path}\"\n").unwrap();
    let who = Who {
        name: "Song: Remix?",
        id: "25f",
        kind: Some("map"),
        source: "beatsaver",
        has: &[],
    };
    let placed = place(&rules, &who, &RelPath::new("sub/Info.dat").unwrap()).unwrap();
    assert!(matches!(placed, Outcome::Place(p) if p.to_string() == "Levels/25f/sub/Info.dat"));
    let who = Who { id: "a/b", ..who };
    let placed = place(&rules, &who, &RelPath::new("x").unwrap()).unwrap();
    assert!(matches!(placed, Outcome::Place(p) if p.to_string() == "Levels/a_b/x"));
}

#[test]
fn rules_can_place_in_other_roots() {
    let rules = parse(
        "[[rules]]\nfiles = \"**/*.pak\"\nto = \"mods:{file}\"\n\n[[rules]]\nto = \"Data/{path}\"\n",
    )
    .unwrap();
    assert_eq!(
        dst(&rules, "m", "Wrapper/Cool.pak").as_deref(),
        Some("mods:Cool.pak")
    );
    assert_eq!(
        dst(&rules, "m", "Public/x.txt").as_deref(),
        Some("Data/Public/x.txt")
    );

    let bad = parse("[[rules]]\nto = \"Not A Root:{path}\"\n").unwrap();
    let who = Who {
        name: "m",
        id: "m",
        kind: None,
        source: "local",
        has: &[],
    };
    assert!(place(&bad, &who, &RelPath::new("x").unwrap()).is_err());
}

const PAK_RULES: &str = r#"
[[rules]]
files = "**/*.pak"
to = "{file}"
"#;

#[test]
fn unmatched_files_make_one_warning() {
    let (dsts, warnings) = deploy(
        PAK_RULES,
        &["a.pak", "x/1.py", "x/2.py", "x/3.py", "x/4.py"],
    )
    .unwrap();
    assert_eq!(dsts.len(), 1);
    assert_eq!(
        warnings,
        ["no rule for 4 files in `Cool` (x/1.py, x/2.py, x/3.py and 1 more); skipped them"]
    );
    let (_, warnings) = deploy(PAK_RULES, &["a.pak", "x/1.py"]).unwrap();
    assert_eq!(warnings, ["no rule for x/1.py in `Cool`; skipped it"]);
}

#[test]
fn mod_with_nothing_for_the_game_says_so() {
    let (dsts, warnings) = deploy(PAK_RULES, &["tool/run.py", "tool/lib.py"]).unwrap();
    assert!(dsts.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with("nothing in `Cool` goes in the game"),
        "{warnings:?}"
    );
}

/// Where each of an archive's files goes, for a mod from `source`, with
/// `has` worked out from the whole archive as a deploy does.
fn archive(rules: &[CompiledRule], source: &str, files: &[&str]) -> Vec<Option<String>> {
    let rels: Vec<_> = files.iter().map(|f| RelPath::new(f).unwrap()).collect();
    let has = rules_had(rules, rels.iter());
    let who = Who {
        name: "M",
        id: "1",
        kind: None,
        source,
        has: &has,
    };
    rels.iter()
        .map(|rel| match place(rules, &who, rel).unwrap() {
            Outcome::Place(p) => Some(p.to_string()),
            _ => None,
        })
        .collect()
}

fn check_archive(rules: &[CompiledRule], source: &str, cases: &[(&str, Option<&str>)]) {
    let files: Vec<_> = cases.iter().map(|(f, _)| *f).collect();
    for ((file, expected), got) in cases.iter().zip(archive(rules, source, &files)) {
        assert_eq!(got.as_deref(), *expected, "{source}: {file}");
    }
}

#[test]
fn has_needs_a_matching_file_in_the_mod() {
    let rules = parse(
        r#"
[[rules]]
has = "*.esp"
to = "Data/{path}"

[[rules]]
to = "other/{path}"
"#,
    )
    .unwrap();
    assert_eq!(
        archive(&rules, "local", &["a.esp", "b.txt"]),
        [Some("Data/a.esp".to_owned()), Some("Data/b.txt".to_owned())]
    );
    assert_eq!(
        archive(&rules, "local", &["sub/a.esp", "b.txt"]),
        [
            Some("other/sub/a.esp".to_owned()),
            Some("other/b.txt".to_owned())
        ]
    );
}

/// Layouts of real Nexus archives for BepInEx games (Valheim, Subnautica,
/// Lethal Company), from the file listings Nexus shows.
#[test]
fn nexus_bepinex_archives() {
    let rules = bepinex();
    // Plain mods need nothing Nexus-specific.
    check_archive(
        &rules,
        "nexus",
        &[(
            "LongshipUpgrades.dll",
            Some("BepInEx/plugins/M/LongshipUpgrades.dll"),
        )],
    );
    check_archive(
        &rules,
        "nexus",
        &[(
            "BepInEx/plugins/LokiTrap/LokiTrap.dll",
            Some("BepInEx/plugins/M/LokiTrap/LokiTrap.dll"),
        )],
    );
    check_archive(
        &rules,
        "nexus",
        &[(
            "FirstPersonPerspective/BepInEx/plugins/FirstPersonPerspective.dll",
            Some("BepInEx/plugins/M/FirstPersonPerspective.dll"),
        )],
    );
    check_archive(
        &rules,
        "nexus",
        &[(
            "plugins/CoreWoodExtras.dll",
            Some("BepInEx/plugins/M/CoreWoodExtras.dll"),
        )],
    );
    check_archive(
        &rules,
        "nexus",
        &[(
            "Digitalroot.Valheim.EternalFire/Digitalroot.Valheim.EternalFire.dll",
            Some("BepInEx/plugins/M/Digitalroot.Valheim.EternalFire.dll"),
        )],
    );
    check_archive(
        &rules,
        "nexus",
        &[(
            "BepInEx/patchers/MortarBepInExBridge.Patcher.dll",
            Some("BepInEx/patchers/M/MortarBepInExBridge.Patcher.dll"),
        )],
    );
    // A Mac zip's resource forks are left out.
    check_archive(
        &rules,
        "nexus",
        &[
            (
                "Creature Outcrops/OutcropCreatures.dll",
                Some("BepInEx/plugins/M/OutcropCreatures.dll"),
            ),
            ("__MACOSX/Creature Outcrops/._OutcropCreatures.dll", None),
        ],
    );

    // BepInEx bundled with a mod (SkToolbox) goes in as packed.
    let bundled = [
        ("winhttp.dll", Some("winhttp.dll")),
        ("doorstop_config.ini", Some("doorstop_config.ini")),
        (
            "doorstop_libs/libdoorstop_x64.so",
            Some("doorstop_libs/libdoorstop_x64.so"),
        ),
        (
            "unstripped_corlib/UnityEngine.dll",
            Some("unstripped_corlib/UnityEngine.dll"),
        ),
        (
            "BepInEx/config/BepInEx.cfg",
            Some("BepInEx/config/BepInEx.cfg"),
        ),
        (
            "BepInEx/core/BepInEx.Preloader.dll",
            Some("BepInEx/core/BepInEx.Preloader.dll"),
        ),
        (
            "BepInEx/core/0Harmony.dll",
            Some("BepInEx/core/0Harmony.dll"),
        ),
        (
            "BepInEx/plugins/SkToolboxValheim.dll",
            Some("BepInEx/plugins/SkToolboxValheim.dll"),
        ),
        ("SkToolbox Readme.txt", Some("SkToolbox Readme.txt")),
    ];
    check_archive(&rules, "nexus", &bundled);
    // Packed in a folder of its own, and BepInEx 6.
    check_archive(
        &rules,
        "nexus",
        &[
            ("Valheim_Bepinex_5.4.5/winhttp.dll", Some("winhttp.dll")),
            (
                "Valheim_Bepinex_5.4.5/BepInEx/core/BepInEx.Preloader.Core.dll",
                Some("BepInEx/core/BepInEx.Preloader.Core.dll"),
            ),
            (
                "Valheim_Bepinex_5.4.5/BepInEx/XUnity.AutoTranslator/Translators/X.dll",
                Some("BepInEx/XUnity.AutoTranslator/Translators/X.dll"),
            ),
        ],
    );
    // From anywhere else, those files are a mod's as before.
    check_archive(
        &rules,
        "local",
        &[
            ("winhttp.dll", Some("BepInEx/plugins/M/winhttp.dll")),
            (
                "BepInEx/core/BepInEx.Preloader.dll",
                Some("BepInEx/core/M/BepInEx.Preloader.dll"),
            ),
        ],
    );
}

/// Layouts of real Nexus archives for MelonLoader 0.7 games (Schedule I,
/// BONELAB, Bloons TD 6), from the file listings Nexus shows.
#[test]
fn nexus_recursive_melonloader_archives() {
    let rules = parse("[[rules]]\nruleset = \"recursive-melonloader\"\n").unwrap();
    check_archive(
        &rules,
        "nexus",
        &[
            // MelonLoader only loads a Mods subfolder with a manifest.json,
            // so these can't have a folder of their own.
            ("Mods/QuickDelivery.dll", Some("Mods/QuickDelivery.dll")),
            ("mods/Rollin.dll", Some("Mods/Rollin.dll")),
            (
                "DeliveryAppPlusPlus.dll",
                Some("Mods/DeliveryAppPlusPlus.dll"),
            ),
            (
                "StackedInventory-1.1.0/StackedInventory.dll",
                Some("Mods/StackedInventory.dll"),
            ),
            (
                "DayNightCycle/DayNightCycle/Mods/DayNightCycle.dll",
                Some("Mods/DayNightCycle.dll"),
            ),
            ("Mods/OWO/HitDefault.owo", Some("Mods/OWO/HitDefault.owo")),
            (
                "UserLibs/Translators/BaiduTranslate.dll",
                Some("UserLibs/Translators/BaiduTranslate.dll"),
            ),
            ("UserLibs/OWO.dll", Some("UserLibs/OWO.dll")),
            (
                "UserData/BetterEmployees/config.json",
                Some("UserData/BetterEmployees/config.json"),
            ),
            (
                "Plugins/Tobey.BepInExMelonLoaderWizard.MLPlugin.dll",
                Some("Plugins/Tobey.BepInExMelonLoaderWizard.MLPlugin.dll"),
            ),
            ("__MACOSX/._UltimateCrosspathing.dll", None),
            // Anything else is kept, where MelonLoader won't load it.
            ("readme.txt", Some("Mods/M/readme.txt")),
        ],
    );
    // MelonLoader bundled with a mod goes in as packed, even its own
    // Plugins folder.
    check_archive(
        &rules,
        "nexus",
        &[
            ("Schedule_I_UA/version.dll", Some("version.dll")),
            (
                "Schedule_I_UA/MelonLoader/net6/MelonLoader.dll",
                Some("MelonLoader/net6/MelonLoader.dll"),
            ),
            (
                "Schedule_I_UA/MelonLoader/Dependencies/Il2CppAssemblyGenerator/Cpp2IL/Plugins/P.dll",
                Some("MelonLoader/Dependencies/Il2CppAssemblyGenerator/Cpp2IL/Plugins/P.dll"),
            ),
            (
                "Schedule_I_UA/Mods/Translation.dll",
                Some("Mods/Translation.dll"),
            ),
        ],
    );
    // Thunderstore packages still get a folder each.
    check_archive(
        &rules,
        "thunderstore",
        &[
            ("Mods/Cool.dll", Some("Mods/M/Mods/Cool.dll")),
            ("manifest.json", Some("Mods/M/manifest.json")),
        ],
    );
}

#[test]
fn nexus_old_melonloader_archives() {
    let rules = parse("[[rules]]\nruleset = \"melonloader-loader\"\n").unwrap();
    check_archive(
        &rules,
        "nexus",
        &[
            (
                "MelonLoader/MelonLoader.dll",
                Some("MelonLoader/MelonLoader.dll"),
            ),
            ("version.dll", Some("version.dll")),
        ],
    );
    check_archive(
        &rules,
        "nexus",
        &[
            ("Mods/RealView.dll", Some("Mods/RealView.dll")),
            (
                "UserData/MarrowLink/a.wav",
                Some("UserData/MarrowLink/a.wav"),
            ),
        ],
    );
}
