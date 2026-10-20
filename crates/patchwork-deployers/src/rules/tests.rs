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
        ("BepInExPack/winhttp.dll", Some("winhttp.dll")),
        (
            "BepInExPack/BepInEx/core/BepInEx.dll",
            Some("BepInEx/core/BepInEx.dll"),
        ),
        ("manifest.json", None),
        ("icon.png", None),
        ("plugins/Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
        ("config/cool.cfg", Some("BepInEx/config/cool.cfg")),
        ("Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
        (
            "assets/thing.bundle",
            Some("BepInEx/plugins/Cool/assets/thing.bundle"),
        ),
        (
            "BepInEx/plugins/X/x.dll",
            Some("BepInEx/plugins/Cool/X/x.dll"),
        ),
        (
            "BepInEx/patchers/X/x.dll",
            Some("BepInEx/patchers/Cool/X/x.dll"),
        ),
        ("core/x.dll", Some("BepInEx/core/Cool/x.dll")),
        ("monomod/x.mm.dll", Some("BepInEx/monomod/Cool/x.mm.dll")),
        ("BepInEx/config/x.cfg", Some("BepInEx/config/x.cfg")),
    ];
    for (path, expected) in cases {
        assert_eq!(dst(&rules, "Cool", path).as_deref(), expected, "{path}");
    }
}

#[test]
fn mod_filters_and_escapes() {
    let rule = |files: &str, mods: Option<&str>, to: &str| {
        RulesDeployer
            .compile(Rule {
                files: files.to_owned(),
                mods: mods.map(str::to_owned),
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
