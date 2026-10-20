use super::*;

fn bepinex() -> Vec<CompiledRule> {
    let text = include_str!("../../../../registry/rulesets/bepinex.toml");
    let ruleset: Ruleset = toml::from_str(text).unwrap();
    ruleset
        .rules
        .into_iter()
        .map(|r| RulesDeployer.compile(r).unwrap())
        .collect()
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
fn mod_filters_and_escapes() {
    let rule = |files: &str, mods: Option<&str>, to: &str| {
        RulesDeployer
            .compile(Rule {
                files: files.to_owned(),
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
    let ruleset: Ruleset =
        toml::from_str("[[rules]]\nmods = [\"a-*\", \"B-exact\"]\nto = \"x/{file}\"\n").unwrap();
    let rules: Vec<_> = ruleset
        .rules
        .into_iter()
        .map(|r| RulesDeployer.compile(r).unwrap())
        .collect();
    assert_eq!(dst(&rules, "a-thing", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "b-EXACT", "f").as_deref(), Some("x/f"));
    assert_eq!(dst(&rules, "c", "f"), None);
}
