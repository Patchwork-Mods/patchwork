use super::*;

#[test]
fn sorts_parse_from_their_names() {
    for sort in SortBy::ALL {
        assert_eq!(sort.as_str().parse::<SortBy>().unwrap(), sort);
    }
    let err = "popular".parse::<SortBy>().unwrap_err();
    assert!(err.to_string().contains("relevance, downloads"), "{err}");
}

#[test]
fn blank_text_is_no_text() {
    let query = |text: &str| SearchQuery {
        text: Some(text.to_owned()),
        ..Default::default()
    };
    assert_eq!(query("  skyui ").text(), Some("skyui"));
    assert_eq!(query("   ").text(), None);
    assert_eq!(SearchQuery::default().text(), None);
}

#[test]
fn hidden_mods_match_ids_ignoring_case() {
    let mut registry = Registry::default();
    registry.add_data(
        HIDDEN_KIND,
        "thunderstore",
        r#"ids = ["ebkr-r2modman", "*-BepInExPack*"]"#,
    );
    let hidden = HiddenMods::of(&registry, "thunderstore").unwrap();
    assert!(hidden.hides("ebkr-r2modman"));
    assert!(hidden.hides("denikson-bepinexpack_valheim"));
    assert!(!hidden.hides("ebkr-r2modman-fork"));
    assert!(!hidden.hides("someone-BepInExConfigManager"));
    // a source without an entry hides nothing
    assert!(
        !HiddenMods::of(&registry, "nexus")
            .unwrap()
            .hides("ebkr-r2modman")
    );
}

#[test]
fn builtin_hidden_mods_load() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::load(&crate::paths::Paths::rooted(tmp.path())).unwrap();
    assert!(registry.data(HIDDEN_KIND, "thunderstore").is_some());
    for source in registry.data_ids(HIDDEN_KIND) {
        HiddenMods::of(&registry, source).unwrap();
    }
}
