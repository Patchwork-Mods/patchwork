use super::*;
use crate::profile::Profile;

fn profile_mod() -> ProfileMod {
    let toml = r#"
        schema = 3
        id = "00000000-0000-0000-0000-000000000000"
        game = "g"
        name = "p"

        [[mod]]
        name = "cool-mod"
        source = "thunderstore"
        id = "Someone-CoolMod"
    "#;
    let profile: Profile = toml::from_str(toml).unwrap();
    profile.mods.into_iter().next().unwrap()
}

fn summary() -> ModSummary {
    ModSummary {
        source: "thunderstore".into(),
        id: "Someone-CoolMod".into(),
        name: "Cool Mod".into(),
        file: None,
        url: Some("https://example.com/cool".into()),
        author: Some("Someone".into()),
        author_url: Some("https://example.com/someone".into()),
        summary: Some("Makes things cool.".into()),
        icon_url: Some("https://example.com/cool.png".into()),
        downloads: Some(10),
        rating: None,
        updated: None,
        latest_version: None,
        categories: vec![Category {
            id: "1".into(),
            name: "Tweaks".into(),
        }],
    }
}

fn text(s: &str) -> Option<FieldValue> {
    Some(FieldValue::Text(s.into()))
}

#[test]
fn no_edits_leaves_the_summary_alone() {
    let m = profile_mod();
    assert_eq!(m.edits.apply(&m, Some(summary())), Some(summary()));
    assert_eq!(m.edits.apply(&m, None), None);
}

#[test]
fn edits_replace_what_the_source_says() {
    let mut m = profile_mod();
    m.edits.set("name", text("  Mine  ")).unwrap();
    m.edits.set("author", text("Me")).unwrap();
    m.edits
        .set(
            "tags",
            Some(FieldValue::List(vec![
                "a".into(),
                " ".into(),
                "b ".into(),
                "a".into(),
            ])),
        )
        .unwrap();
    let shown = m.edits.apply(&m, Some(summary())).unwrap();
    assert_eq!(shown.name, "Mine");
    assert_eq!(shown.author.as_deref(), Some("Me"));
    assert_eq!(shown.author_url, None);
    let tags: Vec<_> = shown.categories.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(tags, ["a", "b"]);
    // the rest is the source's
    assert_eq!(shown.summary.as_deref(), Some("Makes things cool."));
    assert_eq!(shown.downloads, Some(10));
}

#[test]
fn edits_make_a_summary_for_a_mod_without_one() {
    let mut m = profile_mod();
    m.edits.set("description", text("Mine.")).unwrap();
    let shown = m.edits.apply(&m, None).unwrap();
    assert_eq!(shown.name, "cool-mod");
    assert_eq!(shown.summary.as_deref(), Some("Mine."));
}

#[test]
fn notes_are_kept_but_not_shown() {
    let mut m = profile_mod();
    m.edits.set("notes", text("remember this")).unwrap();
    assert_eq!(m.edits.apply(&m, None), None);
    let notes = m
        .edits
        .fields(&m, None)
        .into_iter()
        .find(|f| f.id == "notes");
    assert_eq!(notes.unwrap().edited, text("remember this"));
    // with nothing from its source, its own name is what it's called
    let name = m
        .edits
        .fields(&m, None)
        .into_iter()
        .find(|f| f.id == "name");
    assert_eq!(name.unwrap().source, text("cool-mod"));
}

#[test]
fn empty_values_go_back_to_the_source() {
    let mut m = profile_mod();
    m.edits.set("name", text("Mine")).unwrap();
    m.edits.set("name", text("   ")).unwrap();
    assert!(m.edits.is_empty());
    m.edits.set("tags", Some(FieldValue::List(vec![]))).unwrap();
    assert!(m.edits.is_empty());
    m.edits.set("name", text("Mine")).unwrap();
    m.edits.set("name", None).unwrap();
    assert!(m.edits.is_empty());
}

#[test]
fn values_have_to_suit_the_field() {
    let mut m = profile_mod();
    assert!(m.edits.set("nonsense", text("x")).is_err());
    assert!(
        m.edits
            .set("name", Some(FieldValue::List(vec!["x".into()])))
            .is_err()
    );
    assert!(m.edits.set("tags", text("x")).is_err());
    assert!(m.edits.set("name", text("two\nlines")).is_err());
    m.edits
        .set("description", text("two\n\nparagraphs\n"))
        .unwrap();
    let fields = m.edits.fields(&m, Some(&summary()));
    let description = fields.iter().find(|f| f.id == "description").unwrap();
    assert_eq!(description.edited, text("two\n\nparagraphs"));
    assert_eq!(description.source, text("Makes things cool."));
}

#[test]
fn edits_round_trip_and_keep_unknown_fields() {
    let toml = r#"
        schema = 3
        id = "00000000-0000-0000-0000-000000000000"
        game = "g"
        name = "p"

        [[mod]]
        name = "cool-mod"
        source = "thunderstore"
        id = "Someone-CoolMod"

        [mod.edits]
        name = "Mine"
        tags = ["a", "b"]
        from_the_future = { x = 1 }
    "#;
    let profile: Profile = toml::from_str(toml).unwrap();
    let m = &profile.mods[0];
    let shown = m.edits.apply(m, Some(summary())).unwrap();
    assert_eq!(shown.name, "Mine");
    let again: Profile = toml::from_str(&toml::to_string(&profile).unwrap()).unwrap();
    assert_eq!(again.mods[0].edits, m.edits);
    let saved = toml::to_string(&profile).unwrap();
    assert!(saved.contains("from_the_future"), "{saved}");
}
