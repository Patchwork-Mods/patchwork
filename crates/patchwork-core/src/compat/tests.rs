use super::*;

fn versions(s: &[&str]) -> GameVersions {
    s.to_vec().try_into().unwrap()
}

#[test]
fn ranges_match_versions() {
    let v = versions(&["1.5.97", "1.6.*", ">=2.0 <2.5"]);
    assert_eq!(v.matches("1.5.97"), Some(true));
    assert_eq!(v.matches("1.5.97.0"), Some(true));
    assert_eq!(v.matches("1.5.80"), Some(false));
    assert_eq!(v.matches("1.6"), Some(true));
    assert_eq!(v.matches("1.6.1170"), Some(true));
    assert_eq!(v.matches("1.7"), Some(false));
    assert_eq!(v.matches("2.4.9"), Some(true));
    assert_eq!(v.matches("2.5"), Some(false));
    assert_eq!(versions(&["*"]).matches("anything"), Some(true));
    assert_eq!(versions(&[">1.0", "<=0.5"]).matches("0.5"), Some(true));
}

#[test]
fn versions_that_cant_be_compared_are_unknown() {
    let v = versions(&["1.20.*"]);
    assert_eq!(v.matches("23w31a"), None);
    // but a range that does match still counts
    assert_eq!(versions(&["1.20.*", "*"]).matches("23w31a"), Some(true));
}

#[test]
fn written_as_a_string_or_a_list() {
    #[derive(Deserialize, Serialize)]
    struct Doc {
        v: GameVersions,
    }
    let one: Doc = toml::from_str(r#"v = ">=1.6   <1.7""#).unwrap();
    assert_eq!(one.v.to_string(), ">=1.6 <1.7");
    let many: Doc = toml::from_str(r#"v = ["1.5.97", "=1.6.640"]"#).unwrap();
    assert_eq!(many.v.to_string(), "1.5.97 or 1.6.640");
    assert_eq!(
        toml::to_string(&many).unwrap().trim(),
        r#"v = ["1.5.97", "1.6.640"]"#
    );
    for bad in [r#"v = """#, r#"v = []"#, r#"v = ">=""#, r#"v = "1.*.2""#] {
        assert!(toml::from_str::<Doc>(bad).is_err(), "{bad}");
    }
}

fn game(versions: &str) -> GameDef {
    toml::from_str(&format!(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n[versions]\n{versions}"
    ))
    .unwrap()
}

#[test]
fn guesses_from_whole_words() {
    let g = game(r#"labels = { AE = ">=1.6", "1.5.97" = "1.5.97" }"#);
    let guess = |text: &str| guess(&g, [text]).map(|v| v.to_string());
    assert_eq!(
        guess("Engine Fixes AE-17230-7.zip").as_deref(),
        Some(">=1.6")
    );
    assert_eq!(guess("for ae").as_deref(), Some(">=1.6"));
    assert_eq!(guess("Built for 1.5.97.").as_deref(), Some("1.5.97"));
    assert_eq!(
        guess("Works on 1.5.97 and AE").as_deref(),
        Some("1.5.97 or >=1.6")
    );
    assert_eq!(guess("MAE, 1.5.970, v1.5.97"), None);
    assert_eq!(guess("nothing here"), None);
    assert_eq!(super::guess(&game(""), ["AE"]), None);
}

#[test]
fn only_some_files_can_make_a_mod_version_specific() {
    let rel = |p: &str| RelPath::new(p).unwrap();
    let g = game(r#"files = ["**/SKSE/Plugins/*.dll"]"#);
    assert!(is_version_specific(
        &g,
        &[rel("skse/plugins/EngineFixes.dll")]
    ));
    assert!(is_version_specific(&g, &[rel("Data/SKSE/Plugins/a.dll")]));
    assert!(!is_version_specific(
        &g,
        &[rel("Textures/a.dds"), rel("a.esp")]
    ));
    // every mod is when the game doesn't say
    assert!(is_version_specific(&game(""), &[rel("a.esp")]));
    let bad = "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n\
               [versions]\nfiles = [\"a[\"]";
    assert!(toml::from_str::<GameDef>(bad).is_err());
}
